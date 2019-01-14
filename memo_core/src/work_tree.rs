use crate::buffer::{self, Change, Point, Text};
use crate::epoch::{self, Cursor, DirEntry, Epoch, FileId, FileType};
use crate::serialization;
use crate::{time, Error, Oid, ReplicaId};
use flatbuffers::{FlatBufferBuilder, WIPOffset};
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
    fn changed(&self, buffer_id: BufferId, changes: Vec<Change>, selections: BufferSelectionRanges);
}

pub struct WorkTree {
    epoch: Option<Rc<RefCell<Epoch>>>,
    buffers: Rc<RefCell<HashMap<BufferId, FileId>>>,
    next_buffer_id: Rc<RefCell<BufferId>>,
    local_selection_sets:
        Rc<RefCell<HashMap<BufferId, HashMap<LocalSelectionSetId, buffer::SelectionSetId>>>>,
    next_local_selection_set_id: Rc<RefCell<LocalSelectionSetId>>,
    deferred_ops: Rc<RefCell<HashMap<epoch::Id, Vec<epoch::Operation>>>>,
    lamport_clock: Rc<RefCell<time::Lamport>>,
    git: Rc<GitProvider>,
    observer: Option<Rc<ChangeObserver>>,
}

#[derive(Serialize, Deserialize)]
pub struct Version {
    epoch_id: epoch::Id,
    epoch_version: time::Global,
}

pub struct OperationEnvelope {
    pub epoch_head: Option<Oid>,
    pub operation: Operation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct BufferId(u32);

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct LocalSelectionSetId(u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BufferSelectionRanges {
    pub local: HashMap<LocalSelectionSetId, Vec<Range<Point>>>,
    pub remote: HashMap<ReplicaId, Vec<Vec<Range<Point>>>>,
}

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
    local_selection_sets:
        Rc<RefCell<HashMap<BufferId, HashMap<LocalSelectionSetId, buffer::SelectionSetId>>>>,
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
    ) -> Result<
        (
            WorkTree,
            Box<Stream<Item = OperationEnvelope, Error = Error>>,
        ),
        Error,
    >
    where
        I: 'static + IntoIterator<Item = Operation>,
    {
        let mut ops = ops.into_iter().peekable();
        let mut tree = WorkTree {
            epoch: None,
            buffers: Rc::new(RefCell::new(HashMap::new())),
            next_buffer_id: Rc::new(RefCell::new(BufferId(0))),
            local_selection_sets: Rc::new(RefCell::new(HashMap::new())),
            next_local_selection_set_id: Rc::new(RefCell::new(LocalSelectionSetId(0))),
            deferred_ops: Rc::new(RefCell::new(HashMap::new())),
            lamport_clock: Rc::new(RefCell::new(time::Lamport::new(replica_id))),
            git,
            observer,
        };

        let ops = if ops.peek().is_none() {
            Box::new(tree.reset(base)) as Box<Stream<Item = OperationEnvelope, Error = Error>>
        } else {
            Box::new(tree.apply_ops(ops)?) as Box<Stream<Item = OperationEnvelope, Error = Error>>
        };

        Ok((tree, ops))
    }

    pub fn head(&self) -> Option<Oid> {
        self.epoch.as_ref().and_then(|e| e.borrow().head)
    }

    pub fn reset(
        &mut self,
        head: Option<Oid>,
    ) -> impl Stream<Item = OperationEnvelope, Error = Error> {
        let epoch_id = self.lamport_clock.borrow_mut().tick();
        stream::once(Ok(OperationEnvelope {
            epoch_head: head,
            operation: Operation::StartEpoch { epoch_id, head },
        }))
        .chain(self.start_epoch(epoch_id, head))
    }

    pub fn apply_ops<I>(
        &mut self,
        ops: I,
    ) -> Result<impl Stream<Item = OperationEnvelope, Error = Error>, Error>
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
                let edit_version = epoch.buffer_version(*file_id).unwrap();
                let selections_last_update = epoch.buffer_selections_last_update(*file_id).unwrap();
                prev_versions.insert(*file_id, (edit_version, selections_last_update));
            }

            let fixup_ops = epoch.apply_ops(cur_epoch_ops, &mut self.lamport_clock.borrow_mut())?;

            if let Some(observer) = self.observer.as_ref() {
                for (buffer_id, file_id) in self.buffers.borrow().iter() {
                    let (edit_version, selections_last_update) =
                        prev_versions.remove(file_id).unwrap();
                    let changes: Vec<_> = epoch.changes_since(*file_id, &edit_version)?.collect();
                    if !changes.is_empty()
                        || epoch.selections_changed_since(*file_id, selections_last_update)?
                    {
                        observer.changed(
                            *buffer_id,
                            changes,
                            Self::selection_ranges_internal(
                                &self.local_selection_sets.borrow(),
                                &self.buffers.borrow(),
                                &epoch,
                                *buffer_id,
                            )?,
                        );
                    }
                }
            }

            let fixup_ops_stream = Box::new(stream::iter_ok(OperationEnvelope::wrap_many(
                epoch.id, epoch.head, fixup_ops,
            )));
            Ok(epoch_streams.into_iter().fold(
                fixup_ops_stream as Box<Stream<Item = OperationEnvelope, Error = Error>>,
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
    ) -> Box<Stream<Item = OperationEnvelope, Error = Error>> {
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
                            Ok(stream::iter_ok(OperationEnvelope::wrap_many(
                                new_epoch_id,
                                Some(new_head),
                                fixup_ops,
                            )))
                        })
                        .flatten(),
                ) as Box<Stream<Item = OperationEnvelope, Error = Error>>
            } else {
                Box::new(stream::empty())
            };

            if let Some(cur_epoch) = self.epoch.clone() {
                let switch_epoch = SwitchEpoch::new(
                    new_epoch,
                    cur_epoch,
                    self.buffers.clone(),
                    self.local_selection_sets.clone(),
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

    pub fn observed(&self, other: Version) -> bool {
        let version = self.version();
        match version.epoch_id.cmp(&other.epoch_id) {
            Ordering::Less => false,
            Ordering::Equal => other.epoch_version <= version.epoch_version,
            Ordering::Greater => true,
        }
    }

    pub fn version(&self) -> Version {
        let epoch = self.cur_epoch();
        Version {
            epoch_id: epoch.id,
            epoch_version: epoch.version(),
        }
    }

    pub fn with_cursor<F>(&self, mut f: F)
    where
        F: FnMut(&mut Cursor),
    {
        if let Some(mut cursor) = self.cur_epoch().cursor() {
            f(&mut cursor);
        }
    }

    pub fn create_file<P>(&self, path: P, file_type: FileType) -> Result<OperationEnvelope, Error>
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
        let operation = cur_epoch.create_file(
            parent_id,
            name,
            file_type,
            &mut self.lamport_clock.borrow_mut(),
        )?;

        Ok(OperationEnvelope::wrap(
            cur_epoch.id,
            cur_epoch.head,
            operation,
        ))
    }

    pub fn rename<P1, P2>(&self, old_path: P1, new_path: P2) -> Result<OperationEnvelope, Error>
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

        let operation = cur_epoch.rename(
            file_id,
            new_parent_id,
            new_name,
            &mut self.lamport_clock.borrow_mut(),
        )?;

        Ok(OperationEnvelope::wrap(
            cur_epoch.id,
            cur_epoch.head,
            operation,
        ))
    }

    pub fn remove<P>(&self, path: P) -> Result<OperationEnvelope, Error>
    where
        P: AsRef<Path>,
    {
        let mut cur_epoch = self.cur_epoch_mut();
        let file_id = cur_epoch.file_id(path.as_ref())?;
        let operation = cur_epoch.remove(file_id, &mut self.lamport_clock.borrow_mut())?;

        Ok(OperationEnvelope::wrap(
            cur_epoch.id,
            cur_epoch.head,
            operation,
        ))
    }

    pub fn exists<P>(&self, path: P) -> bool
    where
        P: AsRef<Path>,
    {
        self.cur_epoch().file_id(path).is_ok()
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
    ) -> Result<OperationEnvelope, Error>
    where
        I: IntoIterator<Item = Range<usize>>,
        T: Into<Text>,
    {
        let file_id = self.buffer_file_id(buffer_id)?;
        let mut cur_epoch = self.cur_epoch_mut();
        let operation = cur_epoch
            .edit(
                file_id,
                old_ranges,
                new_text,
                &mut self.lamport_clock.borrow_mut(),
            )
            .unwrap();

        Ok(OperationEnvelope::wrap(
            cur_epoch.id,
            cur_epoch.head,
            operation,
        ))
    }

    pub fn edit_2d<I, T>(
        &self,
        buffer_id: BufferId,
        old_ranges: I,
        new_text: T,
    ) -> Result<OperationEnvelope, Error>
    where
        I: IntoIterator<Item = Range<Point>>,
        T: Into<Text>,
    {
        let file_id = self.buffer_file_id(buffer_id)?;
        let mut cur_epoch = self.cur_epoch_mut();
        let operation = cur_epoch
            .edit_2d(
                file_id,
                old_ranges,
                new_text,
                &mut self.lamport_clock.borrow_mut(),
            )
            .unwrap();

        Ok(OperationEnvelope::wrap(
            cur_epoch.id,
            cur_epoch.head,
            operation,
        ))
    }

    pub fn add_selection_set<I>(
        &self,
        buffer_id: BufferId,
        ranges: I,
    ) -> Result<(LocalSelectionSetId, OperationEnvelope), Error>
    where
        I: IntoIterator<Item = Range<Point>>,
    {
        let file_id = self.buffer_file_id(buffer_id)?;
        let mut cur_epoch = self.cur_epoch_mut();
        let (remote_set_id, operation) =
            cur_epoch.add_selection_set(file_id, ranges, &mut self.lamport_clock.borrow_mut())?;

        let local_set_id = self.gen_local_set_id();
        let mut local_selection_sets = self.local_selection_sets.borrow_mut();
        let buffer_sets = local_selection_sets
            .entry(buffer_id)
            .or_insert(HashMap::new());
        buffer_sets.insert(local_set_id, remote_set_id);

        Ok((
            local_set_id,
            OperationEnvelope::wrap(cur_epoch.id, cur_epoch.head, operation),
        ))
    }

    pub fn replace_selection_set<I>(
        &self,
        buffer_id: BufferId,
        local_set_id: LocalSelectionSetId,
        ranges: I,
    ) -> Result<OperationEnvelope, Error>
    where
        I: IntoIterator<Item = Range<Point>>,
    {
        let file_id = self.buffer_file_id(buffer_id)?;
        let set_id = self.selection_set_id(buffer_id, local_set_id)?;
        let mut cur_epoch = self.cur_epoch_mut();
        let operation = cur_epoch.replace_selection_set(
            file_id,
            set_id,
            ranges,
            &mut self.lamport_clock.borrow_mut(),
        )?;
        Ok(OperationEnvelope::wrap(
            cur_epoch.id,
            cur_epoch.head,
            operation,
        ))
    }

    pub fn remove_selection_set(
        &self,
        buffer_id: BufferId,
        local_set_id: LocalSelectionSetId,
    ) -> Result<OperationEnvelope, Error> {
        let file_id = self.buffer_file_id(buffer_id)?;
        let set_id = self.selection_set_id(buffer_id, local_set_id)?;
        let mut cur_epoch = self.cur_epoch_mut();
        let operation = cur_epoch.remove_selection_set(
            file_id,
            set_id,
            &mut self.lamport_clock.borrow_mut(),
        )?;
        self.local_selection_sets
            .borrow_mut()
            .get_mut(&buffer_id)
            .unwrap()
            .remove(&local_set_id);
        Ok(OperationEnvelope::wrap(
            cur_epoch.id,
            cur_epoch.head,
            operation,
        ))
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

    pub fn selection_ranges(&self, buffer_id: BufferId) -> Result<BufferSelectionRanges, Error> {
        Self::selection_ranges_internal(
            &self.local_selection_sets.borrow(),
            &self.buffers.borrow(),
            &self.cur_epoch(),
            buffer_id,
        )
    }

    fn selection_ranges_internal(
        local_selection_sets: &HashMap<
            BufferId,
            HashMap<LocalSelectionSetId, buffer::SelectionSetId>,
        >,
        buffers: &HashMap<BufferId, FileId>,
        epoch: &Epoch,
        buffer_id: BufferId,
    ) -> Result<BufferSelectionRanges, Error> {
        let file_id = buffers
            .get(&buffer_id)
            .cloned()
            .ok_or(Error::InvalidBufferId)?;

        let mut set_ids_to_local_set_ids = HashMap::new();
        if let Some(buffer_sets) = local_selection_sets.get(&buffer_id) {
            for (local_set_id, set_id) in buffer_sets {
                set_ids_to_local_set_ids.insert(*set_id, *local_set_id);
            }
        }

        let mut selections = BufferSelectionRanges {
            local: HashMap::new(),
            remote: HashMap::new(),
        };
        for (set_id, ranges) in epoch.all_selection_ranges(file_id)? {
            if let Some(local_set_id) = set_ids_to_local_set_ids.get(&set_id) {
                selections.local.insert(*local_set_id, ranges);
            } else {
                selections
                    .remote
                    .entry(set_id.replica_id)
                    .or_insert(Vec::new())
                    .push(ranges);
            }
        }

        Ok(selections)
    }

    pub fn changes_since(
        &self,
        buffer_id: BufferId,
        version: &time::Global,
    ) -> Result<impl Iterator<Item = buffer::Change>, Error> {
        let file_id = self.buffer_file_id(buffer_id)?;
        self.cur_epoch().changes_since(file_id, version)
    }

    pub fn buffer_deferred_ops_len(&self, buffer_id: BufferId) -> Result<usize, Error> {
        let file_id = self.buffer_file_id(buffer_id)?;
        self.cur_epoch().buffer_deferred_ops_len(file_id)
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

    fn gen_local_set_id(&self) -> LocalSelectionSetId {
        let local_set_id = *self.next_local_selection_set_id.borrow();
        self.next_local_selection_set_id.borrow_mut().0 += 1;
        local_set_id
    }

    fn selection_set_id(
        &self,
        buffer_id: BufferId,
        set_id: LocalSelectionSetId,
    ) -> Result<buffer::SelectionSetId, Error> {
        self.local_selection_sets
            .borrow()
            .get(&buffer_id)
            .ok_or(Error::InvalidLocalSelectionSet(set_id))?
            .get(&set_id)
            .cloned()
            .ok_or(Error::InvalidLocalSelectionSet(set_id))
    }
}

impl OperationEnvelope {
    fn wrap(epoch_id: epoch::Id, epoch_head: Option<Oid>, operation: epoch::Operation) -> Self {
        OperationEnvelope {
            epoch_head,
            operation: Operation::EpochOperation {
                epoch_id,
                operation,
            },
        }
    }

    fn wrap_many<T>(epoch_id: epoch::Id, epoch_head: Option<Oid>, operations: T) -> Vec<Self>
    where
        T: IntoIterator<Item = epoch::Operation>,
    {
        operations
            .into_iter()
            .map(move |operation| OperationEnvelope {
                epoch_head,
                operation: Operation::EpochOperation {
                    epoch_id,
                    operation,
                },
            })
            .collect()
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

    pub fn is_selection_update(&self) -> bool {
        match self {
            Operation::EpochOperation { operation, .. } => match operation {
                epoch::Operation::BufferOperation { operations, .. } => {
                    operations.iter().all(|buffer_op| match buffer_op {
                        buffer::Operation::UpdateSelections { .. } => true,
                        _ => false,
                    })
                }
                _ => false,
            },
            _ => false,
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut builder = FlatBufferBuilder::new();
        let root = self.to_flatbuf(&mut builder);
        builder.finish(root, None);
        let (mut bytes, first_valid_byte_index) = builder.collapse();
        bytes.drain(0..first_valid_byte_index);
        bytes
    }

    pub fn deserialize<'a>(buffer: &'a [u8]) -> Result<Option<Self>, Error> {
        use crate::serialization::worktree::Operation;
        let root = flatbuffers::get_root::<Operation<'a>>(buffer);
        Self::from_flatbuf(root)
    }

    pub fn to_flatbuf<'fbb>(
        &self,
        builder: &mut FlatBufferBuilder<'fbb>,
    ) -> WIPOffset<serialization::worktree::Operation<'fbb>> {
        use crate::serialization::worktree::{
            EpochOperation, EpochOperationArgs, Operation as OperationFlatbuf, OperationArgs,
            OperationVariant, StartEpoch, StartEpochArgs,
        };

        let variant_type;
        let variant;

        match self {
            Operation::StartEpoch { epoch_id, head } => {
                variant_type = OperationVariant::StartEpoch;
                let head = head.map(|head| builder.create_vector(&head));
                variant = StartEpoch::create(
                    builder,
                    &StartEpochArgs {
                        epoch_id: Some(&epoch_id.to_flatbuf()),
                        head,
                    },
                )
                .as_union_value();
            }
            Operation::EpochOperation {
                epoch_id,
                operation,
            } => {
                variant_type = OperationVariant::EpochOperation;
                let (epoch_operation_type, epoch_operation_table) = operation.to_flatbuf(builder);
                variant = EpochOperation::create(
                    builder,
                    &EpochOperationArgs {
                        epoch_id: Some(&epoch_id.to_flatbuf()),
                        operation_type: epoch_operation_type,
                        operation: Some(epoch_operation_table),
                    },
                )
                .as_union_value();
            }
        }

        OperationFlatbuf::create(
            builder,
            &OperationArgs {
                variant_type,
                variant: Some(variant),
            },
        )
    }

    pub fn from_flatbuf<'fbb>(
        message: serialization::worktree::Operation<'fbb>,
    ) -> Result<Option<Self>, Error> {
        use crate::serialization::worktree::{EpochOperation, OperationVariant, StartEpoch};

        let variant = message.variant().ok_or(Error::DeserializeError)?;
        match message.variant_type() {
            OperationVariant::StartEpoch => {
                let message = StartEpoch::init_from_table(variant);
                let epoch_id = message.epoch_id().ok_or(Error::DeserializeError)?;
                Ok(Some(Operation::StartEpoch {
                    epoch_id: time::Lamport::from_flatbuf(epoch_id),
                    head: message.head().map(|head| {
                        let mut oid = [0; 20];
                        oid.copy_from_slice(head);
                        oid
                    }),
                }))
            }
            OperationVariant::EpochOperation => {
                let message = EpochOperation::init_from_table(variant);
                let operation = message.operation().ok_or(Error::DeserializeError)?;
                let epoch_id = message.epoch_id().ok_or(Error::DeserializeError)?;
                if let Some(epoch_op) =
                    epoch::Operation::from_flatbuf(message.operation_type(), operation)?
                {
                    Ok(Some(Operation::EpochOperation {
                        epoch_id: time::Lamport::from_flatbuf(epoch_id),
                        operation: epoch_op,
                    }))
                } else {
                    Ok(None)
                }
            }
            OperationVariant::NONE => Ok(None),
        }
    }
}

impl SwitchEpoch {
    fn new(
        to_assign: Rc<RefCell<Epoch>>,
        cur_epoch: Rc<RefCell<Epoch>>,
        buffers: Rc<RefCell<HashMap<BufferId, FileId>>>,
        local_selection_sets: Rc<
            RefCell<HashMap<BufferId, HashMap<LocalSelectionSetId, buffer::SelectionSetId>>>,
        >,
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
            local_selection_sets,
            deferred_ops,
            lamport_clock,
            git,
            observer,
        }
    }
}

impl Future for SwitchEpoch {
    type Item = Vec<OperationEnvelope>;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let mut buffers = self.buffers.borrow_mut();
        let mut cur_epoch = self.cur_epoch.borrow_mut();
        let mut to_assign = self.to_assign.borrow_mut();
        let mut deferred_ops = self.deferred_ops.borrow_mut();
        let mut lamport_clock = self.lamport_clock.borrow_mut();
        let mut local_selection_sets = self.local_selection_sets.borrow_mut();

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
                        fixup_ops.push(OperationEnvelope::wrap(
                            to_assign.id,
                            to_assign.head,
                            operation,
                        ));
                        to_assign.open_text_file(new_file_id, "", &mut lamport_clock)?;
                        let operation = to_assign.edit(
                            new_file_id,
                            Some(0..0),
                            cur_epoch.text(buffers[&buffer_id])?.into_string().as_str(),
                            &mut lamport_clock,
                        )?;
                        fixup_ops.push(OperationEnvelope::wrap(
                            to_assign.id,
                            to_assign.head,
                            operation,
                        ));
                        buffer_mappings.push((buffer_id, new_file_id));
                    }
                }

                if let Some(ops) = deferred_ops.remove(&to_assign.id) {
                    fixup_ops.extend(OperationEnvelope::wrap_many(
                        to_assign.id,
                        to_assign.head,
                        to_assign.apply_ops(ops, &mut lamport_clock)?,
                    ));
                }
                deferred_ops.retain(|id, _| *id > to_assign.id);

                let mut buffer_changes = Vec::new();
                for (buffer_id, new_file_id) in buffer_mappings {
                    let old_file_id = buffers[&buffer_id];
                    let old_text = cur_epoch.text(old_file_id)?.into_string();
                    let new_text = to_assign.text(new_file_id)?.into_string();
                    let changes = buffer::diff(&old_text, &new_text).collect::<Vec<_>>();

                    // TODO: This is inefficient and somewhat inelegant. We should transform
                    // selections using only spatial coordinates, as opposed to editing the
                    // previous buffer's text.
                    let mut tmp_lamport_clock = lamport_clock.clone();
                    for change in &changes {
                        cur_epoch.edit_2d(
                            old_file_id,
                            Some(change.range.clone()),
                            change.code_units.clone(),
                            &mut tmp_lamport_clock,
                        )?;
                    }

                    if let Some(buffer_sets) = local_selection_sets.get_mut(&buffer_id) {
                        for set_id in buffer_sets.values_mut() {
                            let new_ranges =
                                cur_epoch.selection_ranges(old_file_id, *set_id).unwrap();
                            let (new_set_id, op) = to_assign
                                .add_selection_set(new_file_id, new_ranges, &mut lamport_clock)
                                .unwrap();
                            fixup_ops.push(OperationEnvelope::wrap(
                                to_assign.id,
                                to_assign.head,
                                op,
                            ));
                            *set_id = new_set_id;
                        }
                    }

                    buffer_changes.push((buffer_id, changes));
                    buffers.insert(buffer_id, new_file_id);
                }

                mem::swap(&mut *cur_epoch, &mut *to_assign);

                if let Some(observer) = self.observer.as_ref() {
                    for (buffer_id, changes) in buffer_changes {
                        observer.changed(
                            buffer_id,
                            changes,
                            WorkTree::selection_ranges_internal(
                                &local_selection_sets,
                                &buffers,
                                &cur_epoch,
                                buffer_id,
                            )?,
                        );
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
            for _ in 0..10 {
                for path in base_tree.visible_paths(FileType::Text) {
                    base_tree.open_text_file(&path).wait().unwrap();
                }
                base_tree.randomly_mutate(&mut rng, 5);
                commits.push(Some(git.commit(&base_tree)));
            }

            let mut observers = Vec::new();
            let mut trees = Vec::new();
            let mut network = Network::new();
            for i in 0..PEERS {
                let observer = Rc::new(TestChangeObserver::new());
                let (tree, ops) = WorkTree::new(
                    Uuid::from_u128((i + 1) as u128),
                    *rng.choose(&commits).unwrap(),
                    None,
                    git.clone(),
                    Some(observer.clone()),
                )
                .unwrap();
                network.add_peer(tree.replica_id());
                network.broadcast(
                    tree.replica_id(),
                    serialize_ops(open_envelopes(ops.collect().wait().unwrap())),
                    &mut rng,
                );
                observers.push(observer);
                trees.push(tree);
            }

            for _ in 0..10 {
                let replica_index = rng.gen_range(0, PEERS);
                let tree = &mut trees[replica_index];
                let observer = &observers[replica_index];
                let replica_id = tree.replica_id();
                let k = rng.gen_range(0, 4);

                if k == 0 {
                    tree.open_random_buffers(&mut rng, observer, 5);
                } else if k == 1 {
                    let head = *rng.choose(&commits).unwrap();
                    let ops = open_envelopes(tree.reset(head).collect().wait().unwrap());
                    network.broadcast(replica_id, serialize_ops(ops), &mut rng);
                } else if k == 2 && network.has_unreceived(replica_id) {
                    let received_ops = network.receive(replica_id, &mut rng);
                    let fixup_ops = open_envelopes(
                        tree.apply_ops(deserialize_ops(received_ops))
                            .unwrap()
                            .collect()
                            .wait()
                            .unwrap(),
                    );
                    network.broadcast(replica_id, serialize_ops(fixup_ops), &mut rng);
                } else {
                    let ops = tree.randomly_mutate(&mut rng, 5);
                    network.broadcast(replica_id, serialize_ops(open_envelopes(ops)), &mut rng);
                }
            }

            while !network.is_idle() {
                for replica_index in 0..PEERS {
                    let tree = &mut trees[replica_index];
                    let replica_id = tree.replica_id();
                    let received_ops = network.receive(replica_id, &mut rng);
                    let fixup_ops = tree.apply_ops(deserialize_ops(received_ops)).unwrap();
                    network.broadcast(
                        replica_id,
                        serialize_ops(open_envelopes(fixup_ops.collect().wait().unwrap())),
                        &mut rng,
                    );
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
                    assert_eq!(
                        observer.selection_ranges(buffer_id),
                        tree.selection_ranges(buffer_id).unwrap()
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
            open_envelopes(ops_1.collect().wait().unwrap()),
            git.clone(),
            Some(observer_2.clone()),
        )
        .unwrap();

        assert!(ops_2.wait().next().is_none());

        assert_eq!(tree_1.head(), Some(commit_0));
        assert_eq!(tree_1.dir_entries(), git.tree(commit_0).dir_entries());
        assert_eq!(tree_2.head(), Some(commit_0));
        assert_eq!(tree_2.dir_entries(), git.tree(commit_0).dir_entries());

        let a_1 = tree_1.open_text_file("a").wait().unwrap();
        let a_2 = tree_2.open_text_file("a").wait().unwrap();
        observer_1.opened_buffer(a_1, &tree_1);
        observer_2.opened_buffer(a_2, &tree_2);
        assert_eq!(tree_1.text_str(a_1), git.tree(commit_0).text_str(a_base));
        assert_eq!(tree_2.text_str(a_2), git.tree(commit_0).text_str(a_base));

        let ops_1 = open_envelopes(tree_1.reset(Some(commit_1)).collect().wait().unwrap());
        assert_eq!(tree_1.head(), Some(commit_1));
        assert_eq!(tree_1.dir_entries(), git.tree(commit_1).dir_entries());
        assert_eq!(tree_1.text_str(a_1), git.tree(commit_1).text_str(a_1));
        assert_eq!(observer_1.text(a_1), tree_1.text_str(a_1));

        let ops_2 = open_envelopes(tree_2.reset(Some(commit_2)).collect().wait().unwrap());
        assert_eq!(tree_2.head(), Some(commit_2));
        assert_eq!(tree_2.dir_entries(), git.tree(commit_2).dir_entries());
        assert_eq!(tree_2.text_str(a_2), git.tree(commit_2).text_str(a_2));
        assert_eq!(observer_2.text(a_2), tree_2.text_str(a_2));

        let fixup_ops_1 = tree_1.apply_ops(ops_2).unwrap().collect().wait().unwrap();
        let fixup_ops_2 = tree_2.apply_ops(ops_1).unwrap().collect().wait().unwrap();
        assert!(fixup_ops_1.is_empty());
        assert!(fixup_ops_2.is_empty());
        assert_eq!(tree_1.head(), Some(commit_1));
        assert_eq!(tree_2.head(), Some(commit_1));
        assert_eq!(tree_1.entries(), tree_2.entries());
        assert_eq!(tree_1.dir_entries(), git.tree(commit_1).dir_entries());
        assert_eq!(tree_1.text_str(a_1), git.tree(commit_1).text_str(a_1));
        assert_eq!(observer_1.text(a_1), tree_1.text_str(a_1));
        assert_eq!(tree_2.text_str(a_2), git.tree(commit_1).text_str(a_2));
        assert_eq!(observer_2.text(a_2), tree_2.text_str(a_2));
    }

    #[test]
    fn test_selections_across_resets() {
        let git = Rc::new(TestGitProvider::new());
        let base_tree = WorkTree::empty();
        base_tree.create_file("a", FileType::Text).unwrap();
        let a_base = base_tree.open_text_file("a").wait().unwrap();
        base_tree.edit(a_base, Some(0..0), "def\njkl").unwrap();
        let commit_0 = git.commit(&base_tree);

        base_tree.edit(a_base, Some(0..0), "abc\n").unwrap();
        base_tree.edit(a_base, Some(8..8), "ghi\n").unwrap();
        let commit_1 = git.commit(&base_tree);

        let (mut tree_1, ops_1) = WorkTree::new(
            Uuid::from_u128(1),
            Some(commit_0),
            vec![],
            git.clone(),
            None,
        )
        .unwrap();
        let (mut tree_2, ops_2) = WorkTree::new(
            Uuid::from_u128(2),
            Some(commit_0),
            open_envelopes(ops_1.collect().wait().unwrap()),
            git.clone(),
            None,
        )
        .unwrap();
        assert!(ops_2.wait().next().is_none());

        let a_1 = tree_1.open_text_file("a").wait().unwrap();
        let (a_1_set, a_1_set_op) = tree_1
            .add_selection_set(a_1, vec![Point::new(1, 1)..Point::new(1, 1)])
            .unwrap();

        let a_2 = tree_2.open_text_file("a").wait().unwrap();
        let (a_2_set, a_2_set_op) = tree_2
            .add_selection_set(a_2, vec![Point::new(0, 0)..Point::new(0, 0)])
            .unwrap();

        tree_1
            .apply_ops(Some(a_2_set_op.operation))
            .unwrap()
            .collect()
            .wait()
            .unwrap();
        let tree_1_selections = tree_1.selection_ranges(a_1).unwrap();
        assert_eq!(
            tree_1_selections.local.into_iter().collect::<Vec<_>>(),
            vec![(a_1_set, vec![Point::new(1, 1)..Point::new(1, 1)])]
        );
        assert_eq!(
            tree_1_selections.remote.into_iter().collect::<Vec<_>>(),
            vec![(
                tree_2.replica_id(),
                vec![vec![Point::new(0, 0)..Point::new(0, 0)]]
            )]
        );

        tree_2
            .apply_ops(Some(a_1_set_op.operation))
            .unwrap()
            .collect()
            .wait()
            .unwrap();
        let tree_2_selections = tree_2.selection_ranges(a_2).unwrap();
        assert_eq!(
            tree_2_selections.local.into_iter().collect::<Vec<_>>(),
            vec![(a_2_set, vec![Point::new(0, 0)..Point::new(0, 0)])]
        );
        assert_eq!(
            tree_2_selections.remote.into_iter().collect::<Vec<_>>(),
            vec![(
                tree_1.replica_id(),
                vec![vec![Point::new(1, 1)..Point::new(1, 1)]]
            )]
        );

        let fixup_ops_1 = tree_1.reset(Some(commit_1)).collect().wait().unwrap();
        let tree_1_selections = tree_1.selection_ranges(a_1).unwrap();
        assert_eq!(
            tree_1_selections.local.into_iter().collect::<Vec<_>>(),
            vec![(a_1_set, vec![Point::new(3, 1)..Point::new(3, 1)])]
        );
        assert_eq!(
            tree_1_selections.remote.into_iter().collect::<Vec<_>>(),
            vec![]
        );

        let fixup_ops_2 = tree_2
            .apply_ops(open_envelopes(fixup_ops_1))
            .unwrap()
            .collect()
            .wait()
            .unwrap();
        let tree_2_selections = tree_2.selection_ranges(a_2).unwrap();
        assert_eq!(
            tree_2_selections.local.into_iter().collect::<Vec<_>>(),
            vec![(a_2_set, vec![Point::new(0, 0)..Point::new(0, 0)])]
        );
        assert_eq!(
            tree_2_selections.remote.into_iter().collect::<Vec<_>>(),
            vec![(
                tree_1.replica_id(),
                vec![vec![Point::new(3, 1)..Point::new(3, 1)]]
            )]
        );

        tree_1
            .apply_ops(open_envelopes(fixup_ops_2))
            .unwrap()
            .collect()
            .wait()
            .unwrap();
        let tree_1_selections = tree_1.selection_ranges(a_1).unwrap();
        assert_eq!(
            tree_1_selections.local.into_iter().collect::<Vec<_>>(),
            vec![(a_1_set, vec![Point::new(3, 1)..Point::new(3, 1)])]
        );
        assert_eq!(
            tree_1_selections.remote.into_iter().collect::<Vec<_>>(),
            vec![(
                tree_2.replica_id(),
                vec![vec![Point::new(0, 0)..Point::new(0, 0)]]
            )]
        );
    }

    #[test]
    fn test_exists() {
        let git = Rc::new(TestGitProvider::new());
        let commit = git.commit(&WorkTree::empty());
        let (tree, ops) =
            WorkTree::new(Uuid::from_u128(1), Some(commit), vec![], git.clone(), None).unwrap();
        ops.collect().wait().unwrap();

        tree.create_file("a", FileType::Directory).unwrap();
        tree.create_file("a/b", FileType::Directory).unwrap();
        tree.create_file("a/b/c", FileType::Text).unwrap();
        tree.create_file("a/b/d", FileType::Text).unwrap();
        tree.remove("a/b/d").unwrap();
        assert!(tree.exists("a"));
        assert!(tree.exists("a/b"));
        assert!(tree.exists("a/b/c"));
        assert!(!tree.exists("a/b/d"));
        assert!(!tree.exists("non-existent-path"));
        assert!(!tree.exists("invalid-path-;.'"));
    }

    #[test]
    fn test_version() {
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

        let (mut tree_1, ops_1) = WorkTree::new(
            Uuid::from_u128(1),
            Some(commit_0),
            vec![],
            git.clone(),
            None,
        )
        .unwrap();
        let (mut tree_2, ops_2) = WorkTree::new(
            Uuid::from_u128(2),
            Some(commit_0),
            open_envelopes(ops_1.collect().wait().unwrap()),
            git.clone(),
            None,
        )
        .unwrap();
        assert!(ops_2.wait().next().is_none());

        let ops_1 = open_envelopes(tree_1.create_file("x.txt", FileType::Text));
        let ops_2 = open_envelopes(tree_2.create_file("y.txt", FileType::Text));
        assert!(!tree_1.observed(tree_2.version()));
        assert!(!tree_2.observed(tree_1.version()));

        tree_1.apply_ops(ops_2).unwrap().collect().wait().unwrap();
        assert!(tree_1.observed(tree_2.version()));
        tree_2.apply_ops(ops_1).unwrap().collect().wait().unwrap();
        assert!(tree_2.observed(tree_1.version()));

        let ops_1 = open_envelopes(tree_1.reset(Some(commit_1)).collect().wait().unwrap());
        let ops_2 = open_envelopes(tree_2.reset(Some(commit_2)).collect().wait().unwrap());
        // Even though the two sites haven't exchanged operations yet, it's as if tree_1 has
        // already observed tree_2's state, since it won't ever go back to an epoch whose Lamport
        // timestamp is smaller.
        assert!(tree_1.observed(tree_2.version()));
        assert!(!tree_2.observed(tree_1.version()));

        tree_1.apply_ops(ops_2).unwrap().collect().wait().unwrap();
        assert!(tree_1.observed(tree_2.version()));
        tree_2.apply_ops(ops_1).unwrap().collect().wait().unwrap();
        assert!(tree_2.observed(tree_1.version()));
    }

    fn open_envelopes<I: IntoIterator<Item = OperationEnvelope>>(envelopes: I) -> Vec<Operation> {
        envelopes.into_iter().map(|e| e.operation).collect()
    }

    fn serialize_ops<I: IntoIterator<Item = Operation>>(ops: I) -> Vec<Vec<u8>> {
        ops.into_iter().map(|op| op.serialize()).collect()
    }

    fn deserialize_ops<I: IntoIterator<Item = Vec<u8>>>(ops: I) -> Vec<Operation> {
        ops.into_iter()
            .map(|op| Operation::deserialize(&op).unwrap().unwrap())
            .collect()
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct BufferSelections {
        local: HashMap<LocalSelectionSetId, Vec<buffer::Selection>>,
        remote: HashMap<ReplicaId, Vec<Vec<buffer::Selection>>>,
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

        fn randomly_mutate<T: Rng>(&self, rng: &mut T, count: usize) -> Vec<OperationEnvelope> {
            // Store version for all open buffers so that we can keep the observer up to date.
            let mut buffer_versions = Vec::new();
            let buffers = self.buffers.borrow();
            for (buffer_id, file_id) in buffers.iter() {
                let version = self.cur_epoch().buffer_version(*file_id).unwrap();
                buffer_versions.push((*buffer_id, *file_id, version));
            }

            let operations = self.cur_epoch_mut().randomly_mutate(
                rng,
                &mut self.lamport_clock.borrow_mut(),
                count,
            );
            self.update_local_selection_sets();

            // Apply the random changes to the observer as well so that it matches what's in the tree.
            if let Some(observer) = self.observer.as_ref() {
                for (buffer_id, file_id, version) in buffer_versions {
                    let text_changes = self
                        .cur_epoch()
                        .changes_since(file_id, &version)
                        .unwrap()
                        .collect();
                    observer.changed(
                        buffer_id,
                        text_changes,
                        self.selection_ranges(buffer_id).unwrap(),
                    );
                }
            }

            OperationEnvelope::wrap_many(self.cur_epoch().id, self.cur_epoch().head, operations)
        }

        fn open_random_buffers<T: Rng>(
            &mut self,
            rng: &mut T,
            observer: &TestChangeObserver,
            count: usize,
        ) {
            for _ in 0..rng.gen_range(0, count) {
                if let Some(path) = self.select_path(rng, FileType::Text) {
                    let buffer_id = self.open_text_file(path).wait().unwrap();
                    self.update_local_selection_sets();
                    observer.opened_buffer(buffer_id, self);
                }
            }
        }

        fn visible_paths(&self, file_type: FileType) -> Vec<PathBuf> {
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
            visible_paths
        }

        fn select_path<T: Rng>(&self, rng: &mut T, file_type: FileType) -> Option<PathBuf> {
            let mut visible_paths = self.visible_paths(file_type);
            if visible_paths.is_empty() {
                None
            } else {
                Some(visible_paths.swap_remove(rng.gen_range(0, visible_paths.len())))
            }
        }

        fn update_local_selection_sets(&self) {
            use std::collections::HashSet;

            let mut local_selection_sets = self.local_selection_sets.borrow_mut();

            for (buffer_id, file_id) in self.buffers.borrow().iter() {
                let buffer_sets = local_selection_sets
                    .entry(*buffer_id)
                    .or_insert(HashMap::new());

                for local_set_id in buffer_sets.keys().cloned().collect::<Vec<_>>() {
                    let set_id = buffer_sets[&local_set_id];
                    match self.cur_epoch().selection_ranges(*file_id, set_id) {
                        Ok(_) => {}
                        Err(Error::InvalidSelectionSet(_)) => {
                            buffer_sets.remove(&local_set_id);
                        }
                        Err(error) => panic!("{:?}", error),
                    }
                }

                let buffer_set_ids = buffer_sets.values().cloned().collect::<HashSet<_>>();
                for (set_id, _) in self.cur_epoch().all_selections(*file_id).unwrap() {
                    if set_id.replica_id == self.replica_id() && !buffer_set_ids.contains(&set_id) {
                        buffer_sets.insert(self.gen_local_set_id(), set_id);
                    }
                }
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
        selections: RefCell<HashMap<BufferId, BufferSelectionRanges>>,
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
                selections: RefCell::new(HashMap::new()),
            }
        }

        fn opened_buffer(&self, buffer_id: BufferId, tree: &WorkTree) {
            let text = tree.text(buffer_id).unwrap().collect::<Vec<u16>>();
            self.buffers
                .borrow_mut()
                .insert(buffer_id, buffer::Buffer::new(text));
            self.selections
                .borrow_mut()
                .insert(buffer_id, tree.selection_ranges(buffer_id).unwrap());
        }

        fn text(&self, buffer_id: BufferId) -> String {
            self.buffers.borrow().get(&buffer_id).unwrap().to_string()
        }

        fn selection_ranges(&self, buffer_id: BufferId) -> BufferSelectionRanges {
            self.selections.borrow().get(&buffer_id).unwrap().clone()
        }
    }

    impl ChangeObserver for TestChangeObserver {
        fn changed(
            &self,
            buffer_id: BufferId,
            changes: Vec<Change>,
            selections: BufferSelectionRanges,
        ) {
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

            self.selections.borrow_mut().insert(buffer_id, selections);
        }
    }
}
