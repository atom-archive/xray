use crate::buffer::{self, Change, Point, Text};
use crate::epoch::{self, Cursor, DirEntry, Epoch, FileId, FileType};
use crate::{time, Error, Oid, ReplicaId};
use futures::{future, stream, Async, Future, Poll, Stream};
use std::cell::{Ref, RefCell, RefMut};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io;
use std::mem;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub trait GitProvider {
    fn base_entries(&self, oid: Oid) -> Box<Stream<Item = DirEntry, Error = io::Error>>;
    fn base_text(&self, oid: Oid, path: &Path) -> Box<Future<Item = String, Error = io::Error>>;
}

pub trait ChangeObserver {
    fn text_changed(&self, buffer_id: BufferId, changes: Box<Iterator<Item = Change>>);
}

#[derive(Clone)]
pub struct WorkTree {
    epoch: Option<Rc<RefCell<Epoch>>>,
    buffers: Rc<RefCell<HashMap<BufferId, FileId>>>,
    next_buffer_id: Rc<RefCell<BufferId>>,
    deferred_ops: Rc<RefCell<HashMap<epoch::Id, Vec<epoch::Operation>>>>,
    lamport_clock: Rc<RefCell<time::Lamport>>,
    git: Rc<GitProvider>,
    observer: Option<Rc<ChangeObserver>>,
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

#[derive(Copy, Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct BufferId(u32);

enum MaybeDone<F: Future> {
    Pending(F),
    Done(Result<F::Item, F::Error>),
}

struct BaseTextRequest {
    future: MaybeDone<Box<Future<Item = String, Error = io::Error>>>,
    path: PathBuf,
}

struct SwitchEpoch {
    to_assign: Rc<RefCell<Epoch>>,
    fixup_ops: Vec<Operation>,
    cur_epoch: Rc<RefCell<Epoch>>,
    last_seen: epoch::Id,
    base_text_requests: HashMap<BufferId, Option<BaseTextRequest>>,
    buffers: Rc<RefCell<HashMap<BufferId, FileId>>>,
    deferred_ops: Rc<RefCell<HashMap<epoch::Id, Vec<epoch::Operation>>>>,
    lamport_clock: Rc<RefCell<time::Lamport>>,
    git: Rc<GitProvider>,
}

impl WorkTree {
    pub fn new<I>(
        replica_id: ReplicaId,
        base: Option<Oid>,
        ops: I,
        git: Rc<GitProvider>,
        observer: Option<Rc<ChangeObserver>>,
    ) -> Result<(WorkTree, Box<Stream<Item = Operation, Error = Error>>), Error>
    where
        I: 'static + IntoIterator<Item = Operation>,
    {
        let mut ops = ops.into_iter().peekable();
        let mut tree = WorkTree {
            epoch: None,
            buffers: Rc::new(RefCell::new(HashMap::new())),
            next_buffer_id: Rc::new(RefCell::new(BufferId(0))),
            deferred_ops: Rc::new(RefCell::new(HashMap::new())),
            lamport_clock: Rc::new(RefCell::new(time::Lamport::new(replica_id))),
            git,
            observer,
        };

        let ops = if ops.peek().is_none() {
            if let Some(base) = base {
                Box::new(tree.reset(base)) as Box<Stream<Item = Operation, Error = Error>>
            } else {
                let epoch_id = tree.lamport_clock.borrow_mut().tick();
                tree.epoch = Some(Rc::new(RefCell::new(Epoch::new(
                    replica_id, epoch_id, None,
                ))));
                Box::new(stream::empty()) as Box<Stream<Item = Operation, Error = Error>>
            }
        } else {
            Box::new(tree.apply_ops(ops)?) as Box<Stream<Item = Operation, Error = Error>>
        };

        Ok((tree, ops))
    }

    pub fn reset(&mut self, head: Oid) -> impl Stream<Item = Operation, Error = Error> {
        let epoch_id = self.lamport_clock.borrow_mut().tick();
        stream::once(Ok(Operation::StartEpoch { epoch_id, head }))
            .chain(self.start_epoch(epoch_id, head))
    }

    pub fn apply_ops<I>(
        &mut self,
        ops: I,
    ) -> Result<impl Stream<Item = Operation, Error = Error>, Error>
    where
        I: IntoIterator<Item = Operation>,
    {
        let mut cur_epoch_ops = Vec::new();
        let mut epoch_streams = Vec::new();

        for op in ops {
            match op {
                Operation::StartEpoch { epoch_id, head } => {
                    epoch_streams.push(self.start_epoch(epoch_id, head));
                }
                Operation::EpochOperation {
                    epoch_id,
                    operation,
                } => {
                    if let Some(epoch) = self.epoch.clone() {
                        match epoch_id.cmp(&epoch.borrow().id) {
                            Ordering::Less => {}
                            Ordering::Equal => cur_epoch_ops.push(operation),
                            Ordering::Greater => self.defer_epoch_op(epoch_id, operation),
                        }
                    } else {
                        self.defer_epoch_op(epoch_id, operation);
                    }
                }
            }
        }

        if let Some(epoch) = self.epoch.clone() {
            let mut epoch = epoch.borrow_mut();
            let fixup_ops = epoch.apply_ops(cur_epoch_ops, &mut self.lamport_clock.borrow_mut())?;
            let fixup_ops_stream = Box::new(stream::iter_ok(Operation::stamp(epoch.id, fixup_ops)));
            Ok(epoch_streams.into_iter().fold(
                fixup_ops_stream as Box<Stream<Item = Operation, Error = Error>>,
                |acc, stream| Box::new(acc.chain(stream)),
            ))
        } else {
            Err(Error::InvalidOperations)
        }
    }

    fn start_epoch(
        &mut self,
        new_epoch_id: epoch::Id,
        new_head: Oid,
    ) -> Box<Stream<Item = Operation, Error = Error>> {
        if self
            .epoch
            .as_ref()
            .map_or(true, |e| new_epoch_id > e.borrow().id)
        {
            let new_epoch = Rc::new(RefCell::new(Epoch::new(
                self.replica_id(),
                new_epoch_id,
                Some(new_head),
            )));

            let lamport_clock = self.lamport_clock.clone();
            let new_epoch_clone = new_epoch.clone();
            let load_base_entries = self
                .git
                .base_entries(new_head)
                .map_err(|err| Error::IoError(err))
                .chunks(500)
                .and_then(move |base_entries| {
                    let fixup_ops = new_epoch_clone
                        .borrow_mut()
                        .append_base_entries(base_entries, &mut lamport_clock.borrow_mut())?;
                    Ok(stream::iter_ok(Operation::stamp(new_epoch_id, fixup_ops)))
                })
                .flatten();

            if let Some(cur_epoch) = self.epoch.clone() {
                let switch_epoch = SwitchEpoch::new(
                    new_epoch,
                    cur_epoch,
                    self.buffers.clone(),
                    self.deferred_ops.clone(),
                    self.lamport_clock.clone(),
                    self.git.clone(),
                )
                .then(|fixup_ops| Ok(stream::iter_ok(fixup_ops?)))
                .flatten_stream();
                Box::new(load_base_entries.chain(switch_epoch))
            } else {
                self.epoch = Some(new_epoch.clone());
                Box::new(load_base_entries)
            }
        } else {
            Box::new(stream::empty())
        }
    }

    pub fn version(&self) -> time::Global {
        self.cur_epoch().version()
    }

    pub fn with_cursor<F>(&self, mut f: F)
    where
        F: FnMut(&mut Cursor),
    {
        if let Some(mut cursor) = self.cur_epoch().cursor() {
            f(&mut cursor);
        }
    }

    pub fn create_file<P>(&self, path: P, file_type: FileType) -> Result<Operation, Error>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        let name = path
            .file_name()
            .ok_or(Error::InvalidPath("path has no file name".into()))?;
        let mut cur_epoch = self.cur_epoch_mut();
        let parent_id = if let Some(parent_path) = path.parent() {
            cur_epoch.file_id(parent_path)?
        } else {
            epoch::ROOT_FILE_ID
        };
        let epoch_id = cur_epoch.id;
        let operation = cur_epoch.create_file(
            parent_id,
            name,
            file_type,
            &mut self.lamport_clock.borrow_mut(),
        )?;
        Ok(Operation::EpochOperation {
            epoch_id,
            operation,
        })
    }

    pub fn rename<P1, P2>(&self, old_path: P1, new_path: P2) -> Result<Operation, Error>
    where
        P1: AsRef<Path>,
        P2: AsRef<Path>,
    {
        let old_path = old_path.as_ref();
        let new_path = new_path.as_ref();

        let mut cur_epoch = self.cur_epoch_mut();
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
        let operation = cur_epoch.rename(
            file_id,
            new_parent_id,
            new_name,
            &mut self.lamport_clock.borrow_mut(),
        )?;
        Ok(Operation::EpochOperation {
            epoch_id,
            operation,
        })
    }

    pub fn remove<P>(&self, path: P) -> Result<Operation, Error>
    where
        P: AsRef<Path>,
    {
        let mut cur_epoch = self.cur_epoch_mut();
        let file_id = cur_epoch.file_id(path.as_ref())?;
        let epoch_id = cur_epoch.id;
        let operation = cur_epoch.remove(file_id, &mut self.lamport_clock.borrow_mut())?;

        Ok(Operation::EpochOperation {
            epoch_id,
            operation,
        })
    }

    pub fn open_text_file<P>(&mut self, path: P) -> Box<Future<Item = BufferId, Error = Error>>
    where
        P: Into<PathBuf>,
    {
        Self::open_text_file_internal(
            path.into(),
            self.epoch.clone().unwrap(),
            self.git.clone(),
            self.buffers.clone(),
            self.next_buffer_id.clone(),
            self.lamport_clock.clone(),
        )
    }

    fn open_text_file_internal(
        path: PathBuf,
        epoch: Rc<RefCell<Epoch>>,
        git: Rc<GitProvider>,
        buffers: Rc<RefCell<HashMap<BufferId, FileId>>>,
        next_buffer_id: Rc<RefCell<BufferId>>,
        lamport_clock: Rc<RefCell<time::Lamport>>,
    ) -> Box<Future<Item = BufferId, Error = Error>> {
        if let Some(buffer_id) = Self::existing_buffer(&epoch, &buffers, &path) {
            Box::new(future::ok(buffer_id))
        } else {
            let epoch_id = epoch.borrow().id;
            Box::new(
                Self::base_text(&path, epoch.as_ref(), git.as_ref()).and_then(
                    move |(file_id, base_text)| {
                        if let Some(buffer_id) = Self::existing_buffer(&epoch, &buffers, &path) {
                            Box::new(future::ok(buffer_id))
                        } else if epoch.borrow().id == epoch_id {
                            match epoch.borrow_mut().open_text_file(
                                file_id,
                                base_text,
                                &mut lamport_clock.borrow_mut(),
                            ) {
                                Ok(()) => {
                                    let buffer_id = *next_buffer_id.borrow();
                                    next_buffer_id.borrow_mut().0 += 1;
                                    buffers.borrow_mut().insert(buffer_id, file_id);
                                    Box::new(future::ok(buffer_id))
                                }
                                Err(error) => Box::new(future::err(error)),
                            }
                        } else {
                            Self::open_text_file_internal(
                                path,
                                epoch,
                                git,
                                buffers,
                                next_buffer_id,
                                lamport_clock,
                            )
                        }
                    },
                ),
            )
        }
    }

    fn existing_buffer(
        epoch: &Rc<RefCell<Epoch>>,
        buffers: &Rc<RefCell<HashMap<BufferId, FileId>>>,
        path: &Path,
    ) -> Option<BufferId> {
        let epoch = epoch.borrow();
        for (buffer_id, file_id) in buffers.borrow().iter() {
            if let Some(existing_path) = epoch.path(*file_id) {
                if path == existing_path {
                    return Some(*buffer_id);
                }
            }
        }
        None
    }

    fn base_text(
        path: &Path,
        epoch: &RefCell<Epoch>,
        git: &GitProvider,
    ) -> Box<Future<Item = (FileId, String), Error = Error>> {
        let epoch = epoch.borrow();
        match epoch.file_id(&path) {
            Ok(file_id) => {
                if let (Some(head), Some(base_path)) = (epoch.head, epoch.base_path(file_id)) {
                    Box::new(
                        git.base_text(head, &base_path)
                            .map_err(|err| Error::IoError(err))
                            .map(move |text| (file_id, text)),
                    )
                } else {
                    Box::new(future::ok((file_id, String::new())))
                }
            }
            Err(error) => Box::new(future::err(error)),
        }
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
        let mut cur_epoch = self.cur_epoch_mut();
        let epoch_id = cur_epoch.id;
        let operation = cur_epoch
            .edit(
                file_id,
                old_ranges,
                new_text,
                &mut self.lamport_clock.borrow_mut(),
            )
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
        let mut cur_epoch = self.cur_epoch_mut();
        let epoch_id = cur_epoch.id;
        let operation = cur_epoch
            .edit_2d(
                file_id,
                old_ranges,
                new_text,
                &mut self.lamport_clock.borrow_mut(),
            )
            .unwrap();

        Ok(Operation::EpochOperation {
            epoch_id,
            operation,
        })
    }

    pub fn path(&self, buffer_id: BufferId) -> Option<PathBuf> {
        self.buffers
            .borrow()
            .get(&buffer_id)
            .and_then(|file_id| self.cur_epoch().path(*file_id))
    }

    pub fn text(&self, buffer_id: BufferId) -> Result<buffer::Iter, Error> {
        let file_id = self.buffer_file_id(buffer_id)?;
        self.cur_epoch().text(file_id)
    }

    pub fn changes_since(
        &self,
        buffer_id: BufferId,
        version: time::Global,
    ) -> Result<impl Iterator<Item = buffer::Change>, Error> {
        let file_id = self.buffer_file_id(buffer_id)?;
        self.cur_epoch().changes_since(file_id, version)
    }

    fn cur_epoch(&self) -> Ref<Epoch> {
        self.epoch.as_ref().unwrap().borrow()
    }

    fn cur_epoch_mut(&self) -> RefMut<Epoch> {
        self.epoch.as_ref().unwrap().borrow_mut()
    }

    fn defer_epoch_op(&self, epoch_id: epoch::Id, operation: epoch::Operation) {
        self.deferred_ops
            .borrow_mut()
            .entry(epoch_id)
            .or_insert(Vec::new())
            .push(operation);
    }

    fn replica_id(&self) -> ReplicaId {
        self.lamport_clock.borrow().replica_id
    }

    fn buffer_file_id(&self, buffer_id: BufferId) -> Result<FileId, Error> {
        self.buffers
            .borrow()
            .get(&buffer_id)
            .cloned()
            .ok_or(Error::InvalidBufferId)
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

impl SwitchEpoch {
    fn new(
        to_assign: Rc<RefCell<Epoch>>,
        cur_epoch: Rc<RefCell<Epoch>>,
        buffers: Rc<RefCell<HashMap<BufferId, FileId>>>,
        deferred_ops: Rc<RefCell<HashMap<epoch::Id, Vec<epoch::Operation>>>>,
        lamport_clock: Rc<RefCell<time::Lamport>>,
        git: Rc<GitProvider>,
    ) -> Self {
        let last_seen = cur_epoch.borrow().id;
        Self {
            to_assign,
            fixup_ops: Vec::new(),
            cur_epoch,
            last_seen,
            base_text_requests: HashMap::new(),
            buffers,
            deferred_ops,
            lamport_clock,
            git,
        }
    }
}

impl Future for SwitchEpoch {
    type Item = Vec<Operation>;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let mut buffers = self.buffers.borrow_mut();
        let mut cur_epoch = self.cur_epoch.borrow_mut();
        let mut to_assign = self.to_assign.borrow_mut();
        let mut deferred_ops = self.deferred_ops.borrow_mut();
        let mut lamport_clock = self.lamport_clock.borrow_mut();

        if to_assign.id > cur_epoch.id {
            if self.last_seen != cur_epoch.id {
                self.last_seen = cur_epoch.id;
                self.base_text_requests.clear();
            }

            for (buffer_id, file_id) in buffers.iter() {
                let path = cur_epoch.path(*file_id);
                let should_load = if let Some(request) = self.base_text_requests.get(&buffer_id) {
                    path.as_ref() == request.as_ref().map(|r| &r.path)
                } else {
                    true
                };

                if should_load {
                    if path
                        .as_ref()
                        .map_or(false, |path| to_assign.file_id(path).is_ok())
                    {
                        let path = path.unwrap();
                        let base_text = self.git.base_text(to_assign.head.unwrap(), &path);
                        self.base_text_requests.insert(
                            *buffer_id,
                            Some(BaseTextRequest {
                                future: MaybeDone::Pending(base_text),
                                path,
                            }),
                        );
                    } else {
                        self.base_text_requests.insert(*buffer_id, None);
                    }
                }
            }

            let mut is_done = true;
            for request in self.base_text_requests.values_mut() {
                if let Some(request) = request {
                    request.future.poll();
                    is_done = is_done && request.future.is_done();
                }
            }

            if is_done {
                for (buffer_id, request) in self.base_text_requests.drain() {
                    let file_id;
                    let base_text;
                    if let Some(request) = request {
                        base_text = request.future.take_result().unwrap()?;
                        file_id = to_assign.file_id(request.path).unwrap();
                    } else {
                        // TODO: This may be okay for now, but I think we should take a smarter
                        // approach, where the site which initiates the reset transmits a mapping of
                        // previous file ids to new file ids. Then, when receiving a new epoch, we will
                        // check if we can map the open buffer to a file id and, only if we can't, we
                        // will resort to path-based mapping or to creating a completely new file id
                        // for untitled buffers.
                        let (new_file_id, operation) = to_assign.new_text_file(&mut lamport_clock);
                        file_id = new_file_id;
                        base_text = String::new();
                        self.fixup_ops.push(Operation::EpochOperation {
                            epoch_id: to_assign.id,
                            operation,
                        });
                    }

                    to_assign.open_text_file(file_id, base_text, &mut lamport_clock)?;

                    // Okay now we perform the diff.
                    buffers.insert(buffer_id, file_id);
                }

                let mut fixup_ops = Vec::new();
                if let Some(ops) = deferred_ops.remove(&to_assign.id) {
                    fixup_ops.extend(Operation::stamp(
                        to_assign.id,
                        to_assign.apply_ops(ops, &mut lamport_clock)?,
                    ));
                }
                deferred_ops.retain(|id, _| *id > to_assign.id);

                mem::swap(&mut *cur_epoch, &mut *to_assign);
                Ok(Async::Ready(fixup_ops))
            } else {
                Ok(Async::NotReady)
            }
        } else {
            // Cancel future prematurely if the current epoch is newer than the one we wanted to
            // assign.
            Ok(Async::Ready(Vec::new()))
        }
    }
}

impl<F: Future> MaybeDone<F> {
    fn is_done(&self) -> bool {
        match self {
            MaybeDone::Pending(_) => false,
            MaybeDone::Done(_) => true,
        }
    }

    fn poll(&mut self) {
        match self {
            MaybeDone::Pending(f) => match f.poll() {
                Ok(Async::Ready(value)) => *self = MaybeDone::Done(Ok(value)),
                Ok(Async::NotReady) => {}
                Err(error) => *self = MaybeDone::Done(Err(error)),
            },
            MaybeDone::Done(_) => {}
        }
    }

    fn take_result(self) -> Option<Result<F::Item, F::Error>> {
        match self {
            MaybeDone::Pending(_) => None,
            MaybeDone::Done(result) => Some(result),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::CursorEntry;

    #[test]
    fn test_reset() {
        const COMMIT_0: [u8; 20] = [0; 20];
        const COMMIT_1: [u8; 20] = [1; 20];
        const COMMIT_2: [u8; 20] = [2; 20];

        let git = Rc::new(TestGitProvider::new());
        let mut base_tree = WorkTree::empty();
        base_tree.create_file("a", FileType::Text).unwrap();
        let a_base = base_tree.open_text_file("a").wait().unwrap();
        base_tree.edit(a_base, Some(0..0), "abc").unwrap();

        git.commit(COMMIT_0, base_tree.clone());

        base_tree.edit(a_base, Some(1..2), "def").unwrap();
        base_tree.create_file("b", FileType::Directory).unwrap();
        git.commit(COMMIT_1, base_tree.clone());

        base_tree.edit(a_base, Some(2..3), "ghi").unwrap();
        base_tree.create_file("b/c", FileType::Text).unwrap();
        git.commit(COMMIT_2, base_tree.clone());

        let observer_1 = Rc::new(TestChangeObserver::new());
        let observer_2 = Rc::new(TestChangeObserver::new());
        let (mut tree_1, ops_1) = WorkTree::new(
            1,
            Some(COMMIT_0),
            vec![],
            git.clone(),
            Some(observer_1.clone()),
        )
        .unwrap();
        let (mut tree_2, ops_2) = WorkTree::new(
            2,
            Some(COMMIT_0),
            ops_1.collect().wait().unwrap(),
            git.clone(),
            Some(observer_2.clone()),
        )
        .unwrap();
        assert!(ops_2.wait().next().is_none());

        assert_eq!(tree_1.dir_entries(), git.tree(COMMIT_0).dir_entries());
        assert_eq!(tree_2.dir_entries(), git.tree(COMMIT_0).dir_entries());

        let a_1 = tree_1.open_text_file("a").wait().unwrap();
        let a_2 = tree_2.open_text_file("a").wait().unwrap();
        observer_1.opened_buffer(a_1, &tree_1);
        observer_2.opened_buffer(a_2, &tree_2);
        assert_eq!(
            tree_1.text_as_string(a_1),
            git.tree(COMMIT_0).text_as_string(a_base)
        );
        assert_eq!(
            tree_2.text_as_string(a_2),
            git.tree(COMMIT_0).text_as_string(a_base)
        );

        let ops_1 = tree_1.reset(COMMIT_1).collect().wait().unwrap();
        assert_eq!(tree_1.dir_entries(), git.tree(COMMIT_1).dir_entries());

        let ops_2 = tree_2.reset(COMMIT_2).collect().wait().unwrap();
        assert_eq!(tree_2.dir_entries(), git.tree(COMMIT_2).dir_entries());

        let fixup_ops_1 = tree_1.apply_ops(ops_2).unwrap().collect().wait().unwrap();
        let fixup_ops_2 = tree_2.apply_ops(ops_1).unwrap().collect().wait().unwrap();
        assert!(fixup_ops_1.is_empty());
        assert!(fixup_ops_2.is_empty());
        assert_eq!(tree_1.entries(), tree_2.entries());
        assert_eq!(tree_1.dir_entries(), git.tree(COMMIT_1).dir_entries());
        assert_eq!(
            tree_2.text_as_string(a_2),
            git.tree(COMMIT_1).text_as_string(a_base)
        );
        assert_eq!(
            tree_1.text_as_string(a_1),
            git.tree(COMMIT_1).text_as_string(a_base)
        );
    }

    impl WorkTree {
        fn empty() -> Self {
            let (tree, _) =
                Self::new(999, None, Vec::new(), Rc::new(TestGitProvider::new()), None).unwrap();
            tree
        }

        fn entries(&self) -> Vec<CursorEntry> {
            self.epoch.as_ref().unwrap().borrow().entries()
        }

        fn dir_entries(&self) -> Vec<DirEntry> {
            self.epoch.as_ref().unwrap().borrow().dir_entries()
        }

        fn text_as_string(&self, buffer_id: BufferId) -> String {
            self.text(buffer_id).unwrap().into_string()
        }
    }

    struct TestGitProvider {
        commits: RefCell<HashMap<Oid, WorkTree>>,
    }

    struct TestChangeObserver {
        buffers: RefCell<HashMap<BufferId, buffer::Buffer>>,
        local_clock: RefCell<time::Local>,
        lamport_clock: RefCell<time::Lamport>,
    }

    impl TestGitProvider {
        fn new() -> Self {
            TestGitProvider {
                commits: RefCell::new(HashMap::new()),
            }
        }

        fn commit(&self, oid: Oid, tree: WorkTree) {
            self.commits.borrow_mut().insert(oid, tree);
        }

        fn tree(&self, oid: Oid) -> Ref<WorkTree> {
            Ref::map(self.commits.borrow(), |commits| commits.get(&oid).unwrap())
        }
    }

    impl GitProvider for TestGitProvider {
        fn base_entries(&self, oid: Oid) -> Box<Stream<Item = DirEntry, Error = io::Error>> {
            match self.commits.borrow().get(&oid) {
                Some(tree) => Box::new(stream::iter_ok(
                    tree.entries().into_iter().map(|entry| entry.into()),
                )),
                None => Box::new(stream::once(Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Commit does not exist",
                )))),
            }
        }

        fn base_text(
            &self,
            oid: Oid,
            path: &Path,
        ) -> Box<Future<Item = String, Error = io::Error>> {
            use futures::IntoFuture;

            Box::new(
                self.commits
                    .borrow_mut()
                    .get_mut(&oid)
                    .ok_or(io::Error::new(
                        io::ErrorKind::Other,
                        "Commit does not exist",
                    ))
                    .and_then(|tree| {
                        tree.open_text_file(path)
                            .wait()
                            .map_err(|_| {
                                io::Error::new(io::ErrorKind::Other, "Path does not exist")
                            })
                            .map(|buffer_id| tree.text(buffer_id).unwrap().into_string())
                    })
                    .into_future(),
            )
        }
    }

    impl TestChangeObserver {
        fn new() -> Self {
            Self {
                buffers: RefCell::new(HashMap::new()),
                local_clock: RefCell::new(time::Local::default()),
                lamport_clock: RefCell::new(time::Lamport::default()),
            }
        }

        fn opened_buffer(&self, buffer_id: BufferId, tree: &WorkTree) {
            let text = tree.text(buffer_id).unwrap().collect::<Vec<u16>>();
            self.buffers
                .borrow_mut()
                .insert(buffer_id, buffer::Buffer::new(text));
        }

        fn text(&self, buffer_id: BufferId) -> String {
            self.buffers.borrow().get(&buffer_id).unwrap().to_string()
        }
    }

    impl ChangeObserver for TestChangeObserver {
        fn text_changed(&self, buffer_id: BufferId, changes: Box<Iterator<Item = Change>>) {
            if let Some(buffer) = self.buffers.borrow_mut().get_mut(&buffer_id) {
                for change in changes {
                    buffer.edit_2d(
                        Some(change.range),
                        change.code_units,
                        &mut self.local_clock.borrow_mut(),
                        &mut self.lamport_clock.borrow_mut(),
                    );
                }
            }
        }
    }
}
