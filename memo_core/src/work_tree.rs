use crate::buffer::{self, Point, Text};
use crate::epoch::{self, Cursor, DirEntry, Epoch, FileId};
use crate::time;
use crate::Error;
use crate::Oid;
use crate::ReplicaId;
use futures::prelude::*;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::marker::Unpin;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub trait GitProvider {
    type BaseEntriesStream: Stream<Item = Result<DirEntry, io::Error>> + Unpin;
    type BaseTextFuture: Future<Output = Result<String, io::Error>>;

    fn base_entries(&self, oid: Oid) -> Self::BaseEntriesStream;
    fn base_text(&self, oid: Oid, path: &Path) -> Self::BaseTextFuture;
}

pub struct WorkTree<G>(Rc<RefCell<WorkTreeState<G>>>);

struct WorkTreeState<G> {
    epoch: Option<Epoch>,
    buffers: HashMap<BufferId, FileId>,
    next_buffer_id: BufferId,
    deferred_ops: HashMap<epoch::Id, Vec<epoch::Operation>>,
    lamport_clock: time::Lamport,
    git: Rc<G>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Operation {
    StartEpoch {
        epoch_id: epoch::Id,
        head: Oid,
    },
    EpochOperation {
        epoch_id: epoch::Id,
        operation: epoch::Operation,
    },
}

#[derive(Copy, Clone, Eq, Hash, PartialEq, Serialize)]
pub struct BufferId(u32);

impl<G: 'static + GitProvider> WorkTree<G> {
    pub fn new<I>(
        replica_id: ReplicaId,
        base: Oid,
        ops: I,
        git: Rc<G>,
    ) -> (
        WorkTree<G>,
        impl Future<Output = Result<Vec<Operation>, Error>>,
    )
    where
        I: 'static + IntoIterator<Item = Operation>,
    {
        let tree = WorkTree(Rc::new(RefCell::new(WorkTreeState {
            epoch: None,
            buffers: HashMap::new(),
            next_buffer_id: BufferId(0),
            deferred_ops: HashMap::new(),
            lamport_clock: time::Lamport::new(replica_id),
            git,
        })));
        let ops = tree.init(base, ops);
        (tree, ops)
    }

    fn init<I>(&self, base: Oid, ops: I) -> impl Future<Output = Result<Vec<Operation>, Error>>
    where
        I: 'static + IntoIterator<Item = Operation>,
    {
        let mut ops = ops.into_iter().peekable();
        let this = self.clone();
        async move {
            if ops.peek().is_none() {
                await!(this.reset(base))
            } else {
                await!(this.apply_ops(ops))
            }
        }
    }

    pub fn reset(&self, head: Oid) -> impl Future<Output = Result<Vec<Operation>, Error>> {
        let epoch_id = self.0.borrow_mut().lamport_clock.tick();
        let mut ops = Vec::new();
        ops.push(Operation::StartEpoch { epoch_id, head });

        let this = self.clone();
        async move {
            ops.extend(await!(this.start_epoch(epoch_id, head))?);
            Ok(ops)
        }
    }

    pub fn apply_ops<I>(&self, ops: I) -> impl Future<Output = Result<Vec<Operation>, Error>>
    where
        I: 'static + IntoIterator<Item = Operation>,
    {
        let mut cur_epoch_ops = Vec::new();
        let mut epoch_futures = Vec::new();

        for op in ops {
            match op {
                Operation::StartEpoch { epoch_id, head } => {
                    epoch_futures.push(self.start_epoch(epoch_id, head));
                }
                Operation::EpochOperation {
                    epoch_id,
                    operation,
                } => {
                    let state = self.0.borrow();
                    if let Some(epoch) = state.epoch.as_ref() {
                        match epoch_id.cmp(&epoch.id) {
                            Ordering::Less => {}
                            Ordering::Equal => cur_epoch_ops.push(operation),
                            Ordering::Greater => {
                                drop(state);
                                self.defer_epoch_op(epoch_id, operation);
                            }
                        }
                    } else {
                        drop(state);
                        self.defer_epoch_op(epoch_id, operation);
                    }
                }
            }
        }

        let state = self.0.clone();
        async move {
            let mut fixup_ops = Vec::new();
            {
                let state = &mut *state.borrow_mut();
                if let Some(epoch) = state.epoch.as_mut() {
                    fixup_ops.extend(Operation::stamp(
                        epoch.id,
                        epoch.apply_ops(cur_epoch_ops, &mut state.lamport_clock)?,
                    ));
                } else {
                    return Err(Error::InvalidOperations);
                }
            }

            for epoch_future in epoch_futures {
                fixup_ops.extend(await!(epoch_future)?);
            }
            Ok(fixup_ops)
        }
    }

    fn start_epoch(
        &self,
        epoch_id: epoch::Id,
        head: Oid,
    ) -> impl Future<Output = Result<Vec<Operation>, Error>> {
        let mut epoch_to_start = Some(Epoch::new(self.replica_id(), epoch_id, head));
        {
            let mut state = self.0.borrow_mut();
            if state.epoch.is_none() {
                state.epoch = epoch_to_start.take();
            }
        }

        let state = self.0.clone();
        async move {
            let mut fixup_ops = Vec::new();
            if epoch_id >= state.borrow().epoch.as_ref().unwrap().id {
                let mut pending_base_entries = state.borrow().git.base_entries(head).chunks(500);
                loop {
                    let (base_entries, next_pending_base_entries) =
                        await!(pending_base_entries.into_future());
                    pending_base_entries = next_pending_base_entries;

                    if let Some(base_entries) = base_entries {
                        let mut unwrapped_entries = Vec::with_capacity(base_entries.len());
                        for base_entry in base_entries {
                            match base_entry {
                                Ok(base_entry) => unwrapped_entries.push(base_entry),
                                Err(error) => return Err(error.into()),
                            }
                        }

                        let state = &mut *state.borrow_mut();
                        if let Some(epoch_to_start) = epoch_to_start.as_mut() {
                            epoch_to_start
                                .append_base_entries(unwrapped_entries, &mut state.lamport_clock)?;
                        } else {
                            let epoch = state.epoch.as_mut().unwrap();
                            if epoch_id == epoch.id {
                                let epoch_fixup_ops = epoch.append_base_entries(
                                    unwrapped_entries,
                                    &mut state.lamport_clock,
                                )?;
                                fixup_ops.extend(Operation::stamp(epoch_id, epoch_fixup_ops));
                            }
                        }
                    } else {
                        break;
                    }
                }

                let state = &mut *state.borrow_mut();
                let cur_epoch = state.epoch.as_mut().unwrap();
                if epoch_id > cur_epoch.id {
                    *cur_epoch = epoch_to_start.unwrap();
                    if let Some(ops) = state.deferred_ops.remove(&epoch_id) {
                        let epoch_fixup_ops = cur_epoch.apply_ops(ops, &mut state.lamport_clock)?;
                        fixup_ops.extend(Operation::stamp(epoch_id, epoch_fixup_ops));
                    }
                    state.deferred_ops.retain(|id, _| *id > epoch_id);
                }
            }

            Ok(fixup_ops)
        }
    }

    pub fn version(&self) -> time::Global {
        self.0.borrow().epoch.as_ref().unwrap().version()
    }

    pub fn with_cursor<F>(&self, mut f: F)
    where
        F: FnMut(&mut Cursor),
    {
        let state = self.0.borrow();
        let cur_epoch = state.epoch.as_ref().unwrap();
        if let Some(mut cursor) = cur_epoch.cursor() {
            f(&mut cursor);
        }
    }

    pub fn new_text_file(&self) -> (FileId, Operation) {
        let state = &mut *self.0.borrow_mut();
        let cur_epoch = state.epoch.as_mut().unwrap();
        let (file_id, operation) = cur_epoch.new_text_file(&mut state.lamport_clock);
        (
            file_id,
            Operation::EpochOperation {
                epoch_id: cur_epoch.id,
                operation,
            },
        )
    }

    pub fn create_dir<N>(&self, path: &Path) -> Result<(FileId, Operation), Error>
    where
        N: AsRef<OsStr>,
    {
        let name = path
            .file_name()
            .ok_or(Error::InvalidPath("path has no file name".into()))?;
        let state = &mut *self.0.borrow_mut();
        let cur_epoch = state.epoch.as_mut().unwrap();
        let parent_id = if let Some(parent_path) = path.parent() {
            cur_epoch.file_id(parent_path)?
        } else {
            epoch::ROOT_FILE_ID
        };
        let epoch_id = cur_epoch.id;
        let (file_id, operation) =
            cur_epoch.create_dir(parent_id, name, &mut state.lamport_clock)?;
        Ok((
            file_id,
            Operation::EpochOperation {
                epoch_id,
                operation,
            },
        ))
    }

    pub fn open_text_file(
        &mut self,
        path: PathBuf,
    ) -> impl Future<Output = Result<BufferId, Error>> {
        let this = self.clone();
        async move {
            loop {
                if let Some(buffer_id) = this.existing_buffer(&path) {
                    return Ok(buffer_id);
                } else {
                    let epoch_id;
                    let epoch_head;
                    let file_id;
                    let base_path;
                    {
                        let state = this.0.borrow();
                        let cur_epoch = state.epoch.as_ref().unwrap();
                        epoch_id = cur_epoch.id;
                        epoch_head = cur_epoch.head;
                        file_id = cur_epoch.file_id(&path)?;
                        base_path = cur_epoch.base_path(file_id);
                    }

                    let base_text = if let Some(base_path) = base_path {
                        await!(this.0.borrow().git.base_text(epoch_head, &base_path))?
                    } else {
                        String::new()
                    };

                    if let Some(buffer_id) = this.existing_buffer(&path) {
                        return Ok(buffer_id);
                    } else if epoch_id == this.0.borrow().epoch.as_ref().unwrap().id {
                        let state = &mut *this.0.borrow_mut();
                        let epoch = state.epoch.as_mut().unwrap();
                        epoch.open_text_file(
                            file_id,
                            base_text.as_str(),
                            &mut state.lamport_clock,
                        )?;

                        let buffer_id = state.next_buffer_id;
                        state.next_buffer_id.0 += 1;
                        state.buffers.insert(buffer_id, file_id);
                        return Ok(buffer_id);
                    }
                }
            }
        }
    }

    fn existing_buffer(&self, path: &Path) -> Option<BufferId> {
        let state = self.0.borrow();
        let cur_epoch = state.epoch.as_ref().unwrap();
        for (buffer_id, file_id) in &state.buffers {
            if let Some(existing_path) = cur_epoch.path(*file_id) {
                if path == existing_path {
                    return Some(*buffer_id);
                }
            }
        }
        None
    }

    pub fn rename<N>(&self, old_path: &Path, new_path: &Path) -> Result<Operation, Error>
    where
        N: AsRef<OsStr>,
    {
        let state = &mut *self.0.borrow_mut();
        let cur_epoch = state.epoch.as_mut().unwrap();

        let file_id = cur_epoch.file_id(old_path)?;
        let new_name = new_path
            .file_name()
            .ok_or(Error::InvalidPath("new path has no file name".into()))?;
        let new_parent_id = if let Some(parent_path) = new_path.parent() {
            cur_epoch.file_id(parent_path)?
        } else {
            epoch::ROOT_FILE_ID
        };

        let epoch_id = cur_epoch.id;
        let operation =
            cur_epoch.rename(file_id, new_parent_id, new_name, &mut state.lamport_clock)?;
        Ok(Operation::EpochOperation {
            epoch_id,
            operation,
        })
    }

    pub fn remove(&self, path: &Path) -> Result<Operation, Error> {
        let state = &mut *self.0.borrow_mut();
        let cur_epoch = state.epoch.as_mut().unwrap();

        let file_id = cur_epoch.file_id(path)?;
        let epoch_id = cur_epoch.id;
        let operation = cur_epoch.remove(file_id, &mut state.lamport_clock)?;

        Ok(Operation::EpochOperation {
            epoch_id,
            operation,
        })
    }

    pub fn edit<I, T>(
        &self,
        buffer_id: BufferId,
        old_ranges: I,
        new_text: T,
    ) -> Result<Operation, Error>
    where
        I: IntoIterator<Item = Range<usize>>,
        T: Into<Text>,
    {
        let file_id = self.buffer_file_id(buffer_id)?;
        let state = &mut *self.0.borrow_mut();
        let cur_epoch = state.epoch.as_mut().unwrap();
        let epoch_id = cur_epoch.id;
        let operation = cur_epoch
            .edit(file_id, old_ranges, new_text, &mut state.lamport_clock)
            .unwrap();

        Ok(Operation::EpochOperation {
            epoch_id,
            operation,
        })
    }

    pub fn edit_2d<I, T>(
        &self,
        buffer_id: BufferId,
        old_ranges: I,
        new_text: T,
    ) -> Result<Operation, Error>
    where
        I: IntoIterator<Item = Range<Point>>,
        T: Into<Text>,
    {
        let file_id = self.buffer_file_id(buffer_id)?;
        let state = &mut *self.0.borrow_mut();
        let cur_epoch = state.epoch.as_mut().unwrap();
        let epoch_id = cur_epoch.id;
        let operation = cur_epoch
            .edit_2d(file_id, old_ranges, new_text, &mut state.lamport_clock)
            .unwrap();

        Ok(Operation::EpochOperation {
            epoch_id,
            operation,
        })
    }

    pub fn path(&self, buffer_id: BufferId) -> Option<PathBuf> {
        let state = self.0.borrow();
        let cur_epoch = state.epoch.as_ref().unwrap();
        state
            .buffers
            .get(&buffer_id)
            .and_then(|file_id| cur_epoch.path(*file_id))
    }

    pub fn text(&self, buffer_id: BufferId) -> Result<buffer::Iter, Error> {
        let file_id = self.buffer_file_id(buffer_id)?;
        let state = self.0.borrow();
        let cur_epoch = state.epoch.as_ref().unwrap();
        cur_epoch.text(file_id)
    }

    pub fn changes_since(
        &self,
        buffer_id: BufferId,
        version: time::Global,
    ) -> Result<impl Iterator<Item = buffer::Change>, Error> {
        let file_id = self.buffer_file_id(buffer_id)?;
        let state = self.0.borrow();
        let cur_epoch = state.epoch.as_ref().unwrap();
        cur_epoch.changes_since(file_id, version)
    }

    fn defer_epoch_op(&self, epoch_id: epoch::Id, operation: epoch::Operation) {
        self.0
            .borrow_mut()
            .deferred_ops
            .entry(epoch_id)
            .or_insert(Vec::new())
            .push(operation);
    }

    fn replica_id(&self) -> ReplicaId {
        self.0.borrow().lamport_clock.replica_id
    }

    fn buffer_file_id(&self, buffer_id: BufferId) -> Result<FileId, Error> {
        self.0
            .borrow()
            .buffers
            .get(&buffer_id)
            .cloned()
            .ok_or(Error::InvalidBufferId)
    }
}

impl<G> Clone for WorkTree<G> {
    fn clone(&self) -> Self {
        WorkTree(self.0.clone())
    }
}

impl Operation {
    fn stamp<T>(epoch_id: epoch::Id, operations: T) -> impl Iterator<Item = Operation>
    where
        T: IntoIterator<Item = epoch::Operation>,
    {
        operations
            .into_iter()
            .map(move |operation| Operation::EpochOperation {
                epoch_id,
                operation,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::CursorEntry;
    use futures::executor::block_on;
    use rand::{SeedableRng, StdRng};
    use std::vec;

    #[test]
    fn test_reset() {
        let mut rng = StdRng::from_seed(&[1]);
        let mut base_tree_clock = &mut time::Lamport::new(999);

        let mut base_tree = Epoch::with_replica_id(999);
        base_tree.mutate(&mut rng, &mut base_tree_clock, 5);

        let mut git = TestGitProvider::new();
        git.commit([0; 20], base_tree.clone());

        base_tree.mutate(&mut rng, &mut base_tree_clock, 5);
        git.commit([1; 20], base_tree.clone());

        base_tree.mutate(&mut rng, &mut base_tree_clock, 5);
        git.commit([2; 20], base_tree.clone());

        let git = Rc::new(git);
        let (tree_1, ops_1) = WorkTree::new(1, [0; 20], Vec::new(), git.clone());
        let (tree_2, ops_2) = WorkTree::new(2, [0; 20], block_on(ops_1).unwrap(), git.clone());
        assert!(block_on(ops_2).unwrap().is_empty());

        assert_eq!(tree_1.dir_entries(), git.tree([0; 20]).dir_entries());
        assert_eq!(tree_2.dir_entries(), git.tree([0; 20]).dir_entries());

        let ops_1 = block_on(tree_1.reset([1; 20])).unwrap();
        assert_eq!(tree_1.dir_entries(), git.tree([1; 20]).dir_entries());

        let ops_2 = block_on(tree_2.reset([2; 20])).unwrap();
        assert_eq!(tree_2.dir_entries(), git.tree([2; 20]).dir_entries());

        let fixup_ops_1 = block_on(tree_1.apply_ops(ops_2)).unwrap();
        let fixup_ops_2 = block_on(tree_2.apply_ops(ops_1)).unwrap();
        assert!(fixup_ops_1.is_empty());
        assert!(fixup_ops_2.is_empty());
        assert_eq!(tree_1.entries(), tree_2.entries());
    }

    impl<G: GitProvider> WorkTree<G> {
        fn entries(&self) -> Vec<CursorEntry> {
            self.0.borrow().epoch.as_ref().unwrap().entries()
        }

        fn dir_entries(&self) -> Vec<DirEntry> {
            self.0.borrow().epoch.as_ref().unwrap().dir_entries()
        }
    }

    struct TestGitProvider {
        commits: HashMap<Oid, Epoch>,
    }

    impl TestGitProvider {
        fn new() -> Self {
            TestGitProvider {
                commits: HashMap::new(),
            }
        }

        fn commit(&mut self, oid: Oid, tree: Epoch) {
            self.commits.insert(oid, tree);
        }

        fn tree(&self, oid: Oid) -> &Epoch {
            self.commits.get(&oid).unwrap()
        }
    }

    impl GitProvider for TestGitProvider {
        type BaseEntriesStream = stream::Iter<vec::IntoIter<Result<DirEntry, io::Error>>>;
        type BaseTextFuture = future::Ready<Result<String, io::Error>>;

        fn base_entries(&self, oid: Oid) -> Self::BaseEntriesStream {
            stream::iter(
                self.commits
                    .get(&oid)
                    .unwrap()
                    .entries()
                    .into_iter()
                    .map(|entry| Ok(entry.into()))
                    .collect::<Vec<_>>()
                    .into_iter(),
            )
        }

        fn base_text(&self, _oid: Oid, _path: &Path) -> Self::BaseTextFuture {
            unimplemented!()
        }
    }
}
