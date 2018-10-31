use crate::buffer::{self, Change, Point, Text};
use crate::epoch::{self, Cursor, DirEntry, Epoch, FileId, FileType};
use crate::{time, Error, Oid, ReplicaId};
use futures::{future, stream, Async, Future, Poll, Stream};
use serde_derive::{Deserialize, Serialize};
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
        head: Option<Oid>,
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
    cur_epoch: Rc<RefCell<Epoch>>,
    last_seen: epoch::Id,
    base_text_requests: HashMap<BufferId, Option<BaseTextRequest>>,
    buffers: Rc<RefCell<HashMap<BufferId, FileId>>>,
    deferred_ops: Rc<RefCell<HashMap<epoch::Id, Vec<epoch::Operation>>>>,
    lamport_clock: Rc<RefCell<time::Lamport>>,
    git: Rc<GitProvider>,
    observer: Option<Rc<ChangeObserver>>,
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
            Box::new(tree.reset(base)) as Box<Stream<Item = Operation, Error = Error>>
        } else {
            Box::new(tree.apply_ops(ops)?) as Box<Stream<Item = Operation, Error = Error>>
        };

        Ok((tree, ops))
    }

    pub fn reset(&mut self, head: Option<Oid>) -> impl Stream<Item = Operation, Error = Error> {
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

        if let Some(epoch_ref) = self.epoch.clone() {
            let mut epoch = epoch_ref.borrow_mut();

            let mut prev_versions = HashMap::new();
            for file_id in self.buffers.borrow().values() {
                prev_versions.insert(*file_id, epoch.buffer_version(*file_id));
            }

            let fixup_ops = epoch.apply_ops(cur_epoch_ops, &mut self.lamport_clock.borrow_mut())?;
            for (buffer_id, file_id) in self.buffers.borrow().iter() {
                let mut changes = epoch
                    .changes_since(*file_id, prev_versions.remove(file_id).unwrap().unwrap())?
                    .peekable();
                if changes.peek().is_some() {
                    if let Some(observer) = self.observer.as_ref() {
                        // Temporarily drop outstanding borrow to allow for re-entrant calls from
                        // the observer.
                        drop(epoch);
                        observer.text_changed(*buffer_id, Box::new(changes));
                        epoch = epoch_ref.borrow_mut();
                    }
                }
            }

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
        new_head: Option<Oid>,
    ) -> Box<Stream<Item = Operation, Error = Error>> {
        if self
            .epoch
            .as_ref()
            .map_or(true, |e| new_epoch_id > e.borrow().id)
        {
            let new_epoch = Rc::new(RefCell::new(Epoch::new(
                self.replica_id(),
                new_epoch_id,
                new_head,
            )));

            let lamport_clock = self.lamport_clock.clone();
            let new_epoch_clone = new_epoch.clone();
            let load_base_entries = if let Some(new_head) = new_head {
                Box::new(
                    self.git
                        .base_entries(new_head)
                        .map_err(|err| Error::IoError(err))
                        .chunks(500)
                        .and_then(move |base_entries| {
                            let fixup_ops = new_epoch_clone.borrow_mut().append_base_entries(
                                base_entries,
                                &mut lamport_clock.borrow_mut(),
                            )?;
                            Ok(stream::iter_ok(Operation::stamp(new_epoch_id, fixup_ops)))
                        })
                        .flatten(),
                ) as Box<Stream<Item = Operation, Error = Error>>
            } else {
                Box::new(stream::empty())
            };

            if let Some(cur_epoch) = self.epoch.clone() {
                let switch_epoch = SwitchEpoch::new(
                    new_epoch,
                    cur_epoch,
                    self.buffers.clone(),
                    self.deferred_ops.clone(),
                    self.lamport_clock.clone(),
                    self.git.clone(),
                    self.observer.clone(),
                )
                .then(|fixup_ops| Ok(stream::iter_ok(fixup_ops?)))
                .flatten_stream();
                Box::new(load_base_entries.chain(switch_epoch))
            } else {
                self.epoch = Some(new_epoch.clone());
                load_base_entries
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

    pub fn open_text_file<P>(&self, path: P) -> Box<Future<Item = BufferId, Error = Error>>
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
        I: IntoIterator<Item = Range<u64>>,
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

    pub fn epoch_id(&self) -> epoch::Id {
        match self {
            Operation::StartEpoch { epoch_id, .. } => *epoch_id,
            Operation::EpochOperation { epoch_id, .. } => *epoch_id,
        }
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
        observer: Option<Rc<ChangeObserver>>,
    ) -> Self {
        let last_seen = cur_epoch.borrow().id;
        Self {
            to_assign,
            cur_epoch,
            last_seen,
            base_text_requests: HashMap::new(),
            buffers,
            deferred_ops,
            lamport_clock,
            git,
            observer,
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
                let request_is_outdated =
                    if let Some(request) = self.base_text_requests.get(&buffer_id) {
                        path.as_ref() != request.as_ref().map(|r| &r.path)
                    } else {
                        true
                    };

                if request_is_outdated {
                    let will_be_untitled = path.as_ref().map_or(true, |path| {
                        if let Ok(file_id) = to_assign.file_id(path) {
                            to_assign.file_type(file_id).unwrap() != FileType::Text
                        } else {
                            true
                        }
                    });

                    if will_be_untitled {
                        self.base_text_requests.insert(*buffer_id, None);
                    } else {
                        let path = path.unwrap();
                        let head = to_assign
                            .head
                            .expect("If we found a path, destination epoch must have a head");
                        self.base_text_requests.insert(
                            *buffer_id,
                            Some(BaseTextRequest {
                                future: MaybeDone::Pending(self.git.base_text(head, &path)),
                                path,
                            }),
                        );
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
                let mut fixup_ops = Vec::new();

                let mut buffer_mappings = Vec::with_capacity(self.base_text_requests.len());
                for (buffer_id, request) in self.base_text_requests.drain() {
                    if let Some(request) = request {
                        let base_text = request.future.take_result().unwrap()?;
                        let new_file_id = to_assign.file_id(request.path).unwrap();
                        to_assign.open_text_file(new_file_id, base_text, &mut lamport_clock)?;
                        buffer_mappings.push((buffer_id, new_file_id));
                    } else {
                        // TODO: This may be okay for now, but I think we should take a smarter
                        // approach, where the site which initiates the reset transmits a mapping
                        // of previous file ids to new file ids. Then, when receiving a new epoch,
                        // we will check if we can map the open buffer to a file id and, only if we
                        // can't, we will resort to path-based mapping or to creating a completely
                        // new file id for untitled buffers.
                        let (new_file_id, operation) = to_assign.new_text_file(&mut lamport_clock);
                        fixup_ops.push(Operation::EpochOperation {
                            epoch_id: to_assign.id,
                            operation,
                        });
                        to_assign.open_text_file(new_file_id, "", &mut lamport_clock)?;
                        let operation = to_assign.edit(
                            new_file_id,
                            Some(0..0),
                            cur_epoch.text(buffers[&buffer_id])?.into_string().as_str(),
                            &mut lamport_clock,
                        )?;
                        fixup_ops.push(Operation::EpochOperation {
                            epoch_id: to_assign.id,
                            operation,
                        });
                        buffer_mappings.push((buffer_id, new_file_id));
                    }
                }

                if let Some(ops) = deferred_ops.remove(&to_assign.id) {
                    fixup_ops.extend(Operation::stamp(
                        to_assign.id,
                        to_assign.apply_ops(ops, &mut lamport_clock)?,
                    ));
                }
                deferred_ops.retain(|id, _| *id > to_assign.id);

                let mut buffer_changes = Vec::new();
                for (buffer_id, new_file_id) in buffer_mappings {
                    let old_text = cur_epoch.text(buffers[&buffer_id])?.into_string();
                    let new_text = to_assign.text(new_file_id)?.into_string();
                    let mut changes = buffer::diff(&old_text, &new_text).peekable();
                    if changes.peek().is_some() {
                        buffer_changes.push((buffer_id, changes));
                    }
                    buffers.insert(buffer_id, new_file_id);
                }

                mem::swap(&mut *cur_epoch, &mut *to_assign);

                if let Some(observer) = self.observer.as_ref() {
                    // Drop outstanding borrows to allow for re-entrant calls from the observer.
                    drop(buffers);
                    drop(cur_epoch);
                    drop(to_assign);
                    drop(deferred_ops);
                    drop(lamport_clock);
                    for (buffer_id, changes) in buffer_changes {
                        observer.text_changed(buffer_id, Box::new(changes));
                    }
                }

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
    use rand::{Rng, SeedableRng, StdRng};
    use uuid::Uuid;

    #[test]
    fn test_random() {
        use crate::tests::Network;

        const PEERS: usize = 5;

        for seed in 0..100 {
            println!("SEED: {:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);
            let git = Rc::new(TestGitProvider::new());

            let mut commits = vec![None];
            let base_tree = WorkTree::empty();
            for _ in 0..rng.gen_range(1, 10) {
                base_tree.mutate(&mut rng, 5);
                commits.push(Some(git.commit(&base_tree)));
            }

            let mut observers = Vec::new();
            let mut trees = Vec::new();
            let mut network = Network::new();
            for i in 0..PEERS {
                let observer = Rc::new(TestChangeObserver::new());
                observers.push(observer.clone());
                let (tree, ops) = WorkTree::new(
                    Uuid::from_u128((i + 1) as u128),
                    *rng.choose(&commits).unwrap(),
                    None,
                    git.clone(),
                    Some(observer),
                )
                .unwrap();
                network.add_peer(tree.replica_id());
                network.broadcast(tree.replica_id(), ops.collect().wait().unwrap(), &mut rng);
                trees.push(tree);
            }

            for _ in 0..5 {
                let replica_index = rng.gen_range(0, PEERS);
                let tree = &mut trees[replica_index];
                let replica_id = tree.replica_id();
                let observer = &mut observers[replica_index];
                let k = rng.gen_range(0, 4);

                if k == 0 {
                    let ops = tree.mutate(&mut rng, 5);
                    network.broadcast(replica_id, ops, &mut rng);
                } else if k == 1 {
                    let head = *rng.choose(&commits).unwrap();
                    let ops = tree.reset(head).collect().wait().unwrap();
                    network.broadcast(replica_id, ops, &mut rng);
                } else if k == 2 {
                    let fixup_ops = tree
                        .apply_ops(network.receive(replica_id, &mut rng))
                        .unwrap();
                    network.broadcast(replica_id, fixup_ops.collect().wait().unwrap(), &mut rng);
                } else if k == 3 {
                    let buffer_id = if tree.open_buffers().is_empty() || rng.gen() {
                        tree.select_path(FileType::Text, &mut rng).map(|path| {
                            let id = tree.open_text_file(path).wait().unwrap();
                            observer.opened_buffer(id, tree);
                            id
                        })
                    } else {
                        rng.choose(&tree.open_buffers()).cloned()
                    };

                    if let Some(buffer_id) = buffer_id {
                        let end =
                            rng.gen_range(0, (tree.text(buffer_id).unwrap().count() + 1) as u64);
                        let start = rng.gen_range(0, end + 1);
                        let text = gen_text(&mut rng);
                        observer.edit(buffer_id, start..end, text.as_str());
                        let op = tree.edit(buffer_id, Some(start..end), text).unwrap();
                        network.broadcast(replica_id, vec![op], &mut rng);
                    }
                }
            }

            while !network.is_idle() {
                for replica_index in 0..PEERS {
                    let tree = &mut trees[replica_index];
                    let replica_id = tree.replica_id();
                    let fixup_ops = tree
                        .apply_ops(network.receive(replica_id, &mut rng))
                        .unwrap();
                    network.broadcast(replica_id, fixup_ops.collect().wait().unwrap(), &mut rng);
                }
            }

            for replica_index in 0..PEERS - 1 {
                let tree_1 = &trees[replica_index];
                let tree_2 = &trees[replica_index + 1];
                assert_eq!(tree_1.cur_epoch().id, tree_2.cur_epoch().id);
                assert_eq!(tree_1.cur_epoch().head, tree_2.cur_epoch().head);
                assert_eq!(tree_1.entries(), tree_2.entries());
            }

            for replica_index in 0..PEERS {
                let tree = &trees[replica_index];
                let observer = &observers[replica_index];
                for buffer_id in tree.open_buffers() {
                    assert_eq!(
                        observer.text(buffer_id),
                        tree.text(buffer_id).unwrap().into_string()
                    );
                }
            }
        }
    }

    #[test]
    fn test_reset() {
        let git = Rc::new(TestGitProvider::new());
        let base_tree = WorkTree::empty();
        base_tree.create_file("a", FileType::Text).unwrap();
        let a_base = base_tree.open_text_file("a").wait().unwrap();
        base_tree.edit(a_base, Some(0..0), "abc").unwrap();
        let commit_0 = git.commit(&base_tree);

        base_tree.edit(a_base, Some(1..2), "def").unwrap();
        base_tree.create_file("b", FileType::Directory).unwrap();
        let commit_1 = git.commit(&base_tree);

        base_tree.edit(a_base, Some(2..3), "ghi").unwrap();
        base_tree.create_file("b/c", FileType::Text).unwrap();
        let commit_2 = git.commit(&base_tree);

        let observer_1 = Rc::new(TestChangeObserver::new());
        let observer_2 = Rc::new(TestChangeObserver::new());
        let (mut tree_1, ops_1) = WorkTree::new(
            Uuid::from_u128(1),
            Some(commit_0),
            vec![],
            git.clone(),
            Some(observer_1.clone()),
        )
        .unwrap();
        let (mut tree_2, ops_2) = WorkTree::new(
            Uuid::from_u128(2),
            Some(commit_0),
            ops_1.collect().wait().unwrap(),
            git.clone(),
            Some(observer_2.clone()),
        )
        .unwrap();
        assert!(ops_2.wait().next().is_none());

        assert_eq!(tree_1.dir_entries(), git.tree(commit_0).dir_entries());
        assert_eq!(tree_2.dir_entries(), git.tree(commit_0).dir_entries());

        let a_1 = tree_1.open_text_file("a").wait().unwrap();
        let a_2 = tree_2.open_text_file("a").wait().unwrap();
        observer_1.opened_buffer(a_1, &tree_1);
        observer_2.opened_buffer(a_2, &tree_2);
        assert_eq!(tree_1.text_str(a_1), git.tree(commit_0).text_str(a_base));
        assert_eq!(tree_2.text_str(a_2), git.tree(commit_0).text_str(a_base));

        let ops_1 = tree_1.reset(Some(commit_1)).collect().wait().unwrap();
        assert_eq!(tree_1.dir_entries(), git.tree(commit_1).dir_entries());
        assert_eq!(tree_1.text_str(a_1), git.tree(commit_1).text_str(a_1));
        assert_eq!(observer_1.text(a_1), tree_1.text_str(a_1));

        let ops_2 = tree_2.reset(Some(commit_2)).collect().wait().unwrap();
        assert_eq!(tree_2.dir_entries(), git.tree(commit_2).dir_entries());
        assert_eq!(tree_2.text_str(a_2), git.tree(commit_2).text_str(a_2));
        assert_eq!(observer_2.text(a_2), tree_2.text_str(a_2));

        let fixup_ops_1 = tree_1.apply_ops(ops_2).unwrap().collect().wait().unwrap();
        let fixup_ops_2 = tree_2.apply_ops(ops_1).unwrap().collect().wait().unwrap();
        assert!(fixup_ops_1.is_empty());
        assert!(fixup_ops_2.is_empty());
        assert_eq!(tree_1.entries(), tree_2.entries());
        assert_eq!(tree_1.dir_entries(), git.tree(commit_1).dir_entries());
        assert_eq!(tree_1.text_str(a_1), git.tree(commit_1).text_str(a_1));
        assert_eq!(observer_1.text(a_1), tree_1.text_str(a_1));
        assert_eq!(tree_2.text_str(a_2), git.tree(commit_1).text_str(a_2));
        assert_eq!(observer_2.text(a_2), tree_2.text_str(a_2));
    }

    #[test]
    fn test_reentrant_observer() {
        struct ReentrantChangeObserver(Rc<RefCell<WorkTree>>);

        impl ChangeObserver for ReentrantChangeObserver {
            fn text_changed(&self, buffer_id: BufferId, _: Box<Iterator<Item = Change>>) {
                // Assume that users of WorkTree can always acquire a mutable reference to it.
                let tree = unsafe { self.0.as_ptr().as_mut().unwrap() };
                tree.edit(buffer_id, Some(0..0), "!").unwrap();
            }
        }

        let git = Rc::new(TestGitProvider::new());
        let base_tree = WorkTree::empty();
        base_tree.create_file("a", FileType::Text).unwrap();
        let a_base = base_tree.open_text_file("a").wait().unwrap();

        base_tree.edit(a_base, Some(0..0), "abc").unwrap();
        let commit_0 = git.commit(&base_tree);

        base_tree.edit(a_base, Some(3..3), "def").unwrap();
        let commit_1 = git.commit(&base_tree);

        let (tree_1, ops_1) = WorkTree::new(
            Uuid::from_u128(1),
            Some(commit_0),
            vec![],
            git.clone(),
            None,
        )
        .unwrap();
        let tree_1 = Rc::new(RefCell::new(tree_1));
        let observer = Rc::new(ReentrantChangeObserver(tree_1.clone()));
        tree_1.borrow_mut().observer = Some(observer.clone());

        let (tree_2, ops_2) = WorkTree::new(
            Uuid::from_u128(1),
            Some(commit_0),
            ops_1.collect().wait().unwrap(),
            git.clone(),
            None,
        )
        .unwrap();
        assert!(ops_2.collect().wait().unwrap().is_empty());

        tree_1.borrow().open_text_file("a").wait().unwrap();
        let buffer_id_2 = tree_2.open_text_file("a").wait().unwrap();

        // Synchronous re-entrant calls from the observer don't throw errors.
        let edit_op = tree_2.edit(buffer_id_2, Some(0..0), "x").unwrap();
        tree_1
            .borrow_mut()
            .apply_ops(Some(edit_op))
            .unwrap()
            .collect()
            .wait()
            .unwrap();

        // Asynchronous re-entrant calls from the observer don't throw errors.
        tree_1
            .borrow_mut()
            .reset(Some(commit_1))
            .collect()
            .wait()
            .unwrap();
    }

    impl WorkTree {
        fn empty() -> Self {
            let (tree, _) = Self::new(
                Uuid::from_u128(999 as u128),
                None,
                Vec::new(),
                Rc::new(TestGitProvider::new()),
                None,
            )
            .unwrap();
            tree
        }

        fn entries(&self) -> Vec<CursorEntry> {
            self.cur_epoch().entries()
        }

        fn dir_entries(&self) -> Vec<DirEntry> {
            self.cur_epoch().dir_entries()
        }

        fn open_buffers(&self) -> Vec<BufferId> {
            self.buffers.borrow().keys().cloned().collect()
        }

        fn text_str(&self, buffer_id: BufferId) -> String {
            self.text(buffer_id).unwrap().into_string()
        }

        fn mutate<T: Rng>(&self, rng: &mut T, count: usize) -> Vec<Operation> {
            let mut epoch = self.cur_epoch_mut();
            Operation::stamp(
                epoch.id,
                epoch.mutate(rng, &mut self.lamport_clock.borrow_mut(), count),
            )
            .collect()
        }

        fn select_path<T: Rng>(&self, file_type: FileType, rng: &mut T) -> Option<PathBuf> {
            let mut visible_paths = Vec::new();
            self.with_cursor(|cursor| loop {
                let entry = cursor.entry().unwrap();
                let advanced = if entry.visible {
                    if file_type == entry.file_type {
                        visible_paths.push(cursor.path().unwrap().to_path_buf());
                    }
                    cursor.next(true)
                } else {
                    cursor.next(false)
                };

                if !advanced {
                    break;
                }
            });

            if visible_paths.is_empty() {
                None
            } else {
                Some(visible_paths.swap_remove(rng.gen_range(0, visible_paths.len())))
            }
        }
    }

    struct TestGitProvider {
        commits: RefCell<HashMap<Oid, WorkTree>>,
        next_oid: RefCell<u64>,
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
                next_oid: RefCell::new(0),
            }
        }

        fn commit(&self, tree: &WorkTree) -> Oid {
            let mut tree_clone = WorkTree::empty();
            tree_clone.epoch = tree
                .epoch
                .as_ref()
                .map(|e| Rc::new(RefCell::new(e.borrow().clone())));
            tree_clone.buffers = Rc::new(RefCell::new(tree.buffers.borrow().clone()));

            let oid = self.gen_oid();
            self.commits.borrow_mut().insert(oid, tree_clone);
            oid
        }

        fn tree(&self, oid: Oid) -> Ref<WorkTree> {
            Ref::map(self.commits.borrow(), |commits| commits.get(&oid).unwrap())
        }

        fn gen_oid(&self) -> Oid {
            let mut next_oid = self.next_oid.borrow_mut();
            let mut oid = [0; 20];
            oid[0] = (*next_oid >> 0) as u8;
            oid[1] = (*next_oid >> 8) as u8;
            oid[2] = (*next_oid >> 16) as u8;
            oid[3] = (*next_oid >> 24) as u8;
            oid[4] = (*next_oid >> 32) as u8;
            oid[5] = (*next_oid >> 40) as u8;
            oid[6] = (*next_oid >> 48) as u8;
            oid[7] = (*next_oid >> 56) as u8;
            *next_oid += 1;
            oid
        }
    }

    impl GitProvider for TestGitProvider {
        fn base_entries(&self, oid: Oid) -> Box<Stream<Item = DirEntry, Error = io::Error>> {
            match self.commits.borrow().get(&oid) {
                Some(tree) => Box::new(stream::iter_ok(tree.dir_entries().into_iter())),
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

        fn edit<T>(&self, buffer_id: BufferId, range: Range<u64>, text: T)
        where
            T: Into<Text>,
        {
            let mut buffers = self.buffers.borrow_mut();
            buffers.get_mut(&buffer_id).unwrap().edit(
                Some(range),
                text,
                &mut self.local_clock.borrow_mut(),
                &mut self.lamport_clock.borrow_mut(),
            );
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

    fn gen_text<T: Rng>(rng: &mut T) -> String {
        let text_len = rng.gen_range(0, 50);
        let mut text: String = rng.gen_ascii_chars().take(text_len).collect();
        for _ in 0..rng.gen_range(0, 5) {
            let index = rng.gen_range(0, text.len() + 1);
            text.insert(index, '\n');
        }
        text
    }
}
