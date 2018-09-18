use super::rpc::{client, Error as RpcError};
use super::tree::{self, SeekBias, Tree};
use fs;
use futures::{unsync, Future, Stream};
use notify_cell::{NotifyCell, NotifyCellObserver};
use seahash::SeaHasher;
use serde::{self, Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::cmp::{self, Ordering};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::BuildHasherDefault;
use std::iter;
use std::marker;
use std::mem;
use std::ops::{Add, AddAssign, Range, Sub};
use std::rc::Rc;
use std::sync::Arc;
use ForegroundExecutor;
use IntoShared;
use UserId;

pub type ReplicaId = usize;
type LocalTimestamp = usize;
type LamportTimestamp = usize;
pub type SelectionSetId = usize;
type SelectionSetVersion = usize;
pub type BufferId = usize;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Version(
    #[serde(serialize_with = "serialize_arc", deserialize_with = "deserialize_arc")]
    Arc<HashMap<ReplicaId, LocalTimestamp>>,
);

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum Error {
    OffsetOutOfRange,
    InvalidAnchor,
    InvalidOperation,
    SelectionSetNotFound,
    IoError(String),
    RpcError(RpcError),
}

pub struct Buffer {
    id: BufferId,
    pub replica_id: ReplicaId,
    next_replica_id: Option<ReplicaId>,
    local_clock: LocalTimestamp,
    lamport_clock: LamportTimestamp,
    fragments: Tree<Fragment>,
    insertion_splits: HashMap<EditId, Tree<InsertionSplit>>,
    anchor_cache: RefCell<HashMap<Anchor, (usize, Point), BuildHasherDefault<SeaHasher>>>,
    offset_cache: RefCell<HashMap<Point, usize, BuildHasherDefault<SeaHasher>>>,
    pub version: Version,
    client: Option<client::Service<rpc::Service>>,
    operation_txs: Vec<unsync::mpsc::UnboundedSender<Arc<Operation>>>,
    updates: NotifyCell<()>,
    next_local_selection_set_id: SelectionSetId,
    selections: HashMap<(ReplicaId, SelectionSetId), SelectionSet, BuildHasherDefault<SeaHasher>>,
    file: Option<Box<fs::File>>,
}

pub struct BufferSnapshot {
    fragments: Tree<Fragment>,
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Hash)]
pub struct Point {
    pub row: u32,
    pub column: u32,
}

#[derive(Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
pub struct Anchor(AnchorInner);

#[derive(Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
enum AnchorInner {
    Start,
    End,
    Middle {
        insertion_id: EditId,
        offset: usize,
        bias: AnchorBias,
    },
}

#[derive(Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
enum AnchorBias {
    Left,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub start: Anchor,
    pub end: Anchor,
    pub reversed: bool,
    pub goal_column: Option<u32>,
}

struct SelectionSet {
    user_id: UserId,
    selections: Vec<Selection>,
    version: SelectionSetVersion,
}

#[derive(Serialize, Deserialize)]
pub struct SelectionSetState {
    user_id: UserId,
    selections: Vec<Selection>,
}

pub struct Iter<'a> {
    fragment_cursor: tree::Cursor<'a, Fragment>,
    fragment_offset: usize,
}

pub struct BackwardIter<'a> {
    fragment_cursor: tree::Cursor<'a, Fragment>,
    fragment_offset: usize,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Insertion {
    id: EditId,
    parent_id: EditId,
    offset_in_parent: usize,
    replica_id: ReplicaId,
    #[serde(serialize_with = "serialize_arc", deserialize_with = "deserialize_arc")]
    text: Arc<Text>,
    timestamp: LamportTimestamp,
}

#[derive(Serialize, Deserialize)]
pub struct Deletion {
    start_id: EditId,
    start_offset: usize,
    end_id: EditId,
    end_offset: usize,
    version_in_range: Version,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Text {
    code_units: Vec<u16>,
    nodes: Vec<LineNode>,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
struct LineNode {
    len: u32,
    longest_row: u32,
    longest_row_len: u32,
    offset: usize,
    rows: u32,
}

struct LineNodeProbe<'a> {
    offset_range: &'a Range<usize>,
    row: u32,
    left_ancestor_end_offset: usize,
    right_ancestor_start_offset: usize,
    node: &'a LineNode,
    left_child: Option<&'a LineNode>,
    right_child: Option<&'a LineNode>,
}

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EditId {
    replica_id: ReplicaId,
    timestamp: LocalTimestamp,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug, Serialize, Deserialize)]
struct FragmentId(
    #[serde(serialize_with = "serialize_arc")]
    #[serde(deserialize_with = "deserialize_arc")]
    Arc<Vec<u16>>,
);

#[derive(Eq, PartialEq, Clone, Debug)]
struct Fragment {
    id: FragmentId,
    insertion: Insertion,
    start_offset: usize,
    end_offset: usize,
    deletions: HashSet<EditId>,
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct FragmentSummary {
    extent: usize,
    extent_2d: Point,
    max_fragment_id: FragmentId,
    first_row_len: u32,
    longest_row: u32,
    longest_row_len: u32,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy, Debug)]
struct CharacterCount(usize);

#[derive(Eq, PartialEq, Clone, Debug, Serialize, Deserialize)]
struct InsertionSplit {
    extent: usize,
    fragment_id: FragmentId,
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct InsertionSplitSummary {
    extent: usize,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy, Debug)]
struct InsertionOffset(usize);

#[derive(Debug, Serialize, Deserialize)]
pub enum Operation {
    Edit {
        id: EditId,
        start_id: EditId,
        start_offset: usize,
        end_id: EditId,
        end_offset: usize,
        version_in_range: Version,
        timestamp: LamportTimestamp,
        #[serde(serialize_with = "serialize_option_arc")]
        #[serde(deserialize_with = "deserialize_option_arc")]
        new_text: Option<Arc<Text>>,
    },
}

impl Version {
    fn new() -> Self {
        Version(Arc::new(HashMap::new()))
    }

    fn set(&mut self, replica_id: ReplicaId, timestamp: LocalTimestamp) {
        let map = Arc::make_mut(&mut self.0);
        *map.entry(replica_id).or_insert(0) = timestamp;
    }

    fn include(&mut self, insertion: &Insertion) {
        let map = Arc::make_mut(&mut self.0);
        let value = map.entry(insertion.id.replica_id).or_insert(0);
        *value = cmp::max(*value, insertion.id.timestamp);
    }

    fn includes(&self, insertion: &Insertion) -> bool {
        if let Some(timestamp) = self.0.get(&insertion.id.replica_id) {
            *timestamp >= insertion.id.timestamp
        } else {
            false
        }
    }
}

pub mod rpc {
    use super::{
        Buffer, BufferId, EditId, FragmentId, Insertion, InsertionSplit, Operation, ReplicaId,
        SelectionSetId, SelectionSetState, SelectionSetVersion, Version,
    };
    use futures::{Async, Future, Stream};
    use never::Never;
    use notify_cell::NotifyCellObserver;
    use rpc;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;
    use std::sync::Arc;

    #[derive(Serialize, Deserialize)]
    pub struct State {
        pub(super) id: BufferId,
        pub(super) replica_id: ReplicaId,
        pub(super) fragments: Vec<Fragment>,
        pub(super) insertions: HashMap<EditId, Insertion>,
        pub(super) insertion_splits: HashMap<EditId, Vec<InsertionSplit>>,
        pub(super) version: Version,
        pub(super) selections: HashMap<(ReplicaId, SelectionSetId), SelectionSetState>,
    }

    #[derive(Serialize, Deserialize)]
    pub enum Request {
        Operation(
            #[serde(serialize_with = "serialize_op", deserialize_with = "deserialize_op")]
            Arc<Operation>,
        ),
        UpdateSelectionSet(SelectionSetId, SelectionSetState),
        RemoveSelectionSet(SelectionSetId),
        Save,
    }

    #[derive(Serialize, Deserialize)]
    pub enum Response {
        Saved,
        Error(super::Error),
    }

    #[derive(Serialize, Deserialize)]
    pub enum Update {
        Operation(
            #[serde(serialize_with = "serialize_op", deserialize_with = "deserialize_op")]
            Arc<Operation>,
        ),
        Selections {
            updated: HashMap<(ReplicaId, SelectionSetId), SelectionSetState>,
            removed: HashSet<(ReplicaId, SelectionSetId)>,
        },
    }

    #[derive(Serialize, Deserialize)]
    pub(super) struct Fragment {
        pub id: FragmentId,
        pub insertion_id: EditId,
        pub start_offset: usize,
        pub end_offset: usize,
        pub deletions: HashSet<EditId>,
    }

    pub struct Service {
        replica_id: ReplicaId,
        buffer_updates: NotifyCellObserver<()>,
        outgoing_ops: Box<Stream<Item = Arc<Operation>, Error = ()>>,
        selection_set_versions: HashMap<(ReplicaId, SelectionSetId), SelectionSetVersion>,
        buffer: Rc<RefCell<Buffer>>,
    }

    impl Service {
        pub fn new(buffer: Rc<RefCell<Buffer>>) -> Self {
            let replica_id = buffer
                .borrow_mut()
                .next_replica_id()
                .expect("Cannot replicate a remote buffer");
            let outgoing_ops = buffer
                .borrow_mut()
                .outgoing_ops()
                .filter(move |op| op.replica_id() != replica_id);
            let buffer_updates = buffer.borrow().updates();
            let selection_set_versions = buffer
                .borrow()
                .selections
                .iter()
                .map(|(key, set)| (*key, set.version))
                .collect();
            Self {
                replica_id,
                buffer_updates,
                outgoing_ops: Box::new(outgoing_ops),
                selection_set_versions,
                buffer,
            }
        }

        fn poll_outgoing_op(&mut self) -> Async<Option<Update>> {
            self.outgoing_ops
                .poll()
                .expect("Receiving on a channel cannot produce an error")
                .map(|option| option.map(|update| Update::Operation(update)))
        }

        fn poll_outgoing_selection_updates(&mut self) -> Async<Option<Update>> {
            loop {
                match self.buffer_updates
                    .poll()
                    .expect("Polling a NotifyCellObserver cannot produce an error")
                {
                    Async::NotReady => return Async::NotReady,
                    Async::Ready(None) => unreachable!(),
                    Async::Ready(Some(())) => {
                        let mut removed = HashSet::new();
                        let mut updated = HashMap::new();

                        let buffer = self.buffer.borrow();
                        self.selection_set_versions
                            .retain(|id, last_polled_version| {
                                if let Some(selection_set) = buffer.selections.get(id) {
                                    if selection_set.version > *last_polled_version {
                                        *last_polled_version = selection_set.version;
                                        updated.insert(*id, selection_set.state());
                                    }
                                    true
                                } else {
                                    removed.insert(*id);
                                    false
                                }
                            });

                        for ((replica_id, set_id), selection_set) in &buffer.selections {
                            if *replica_id != self.replica_id {
                                self.selection_set_versions
                                    .entry((*replica_id, *set_id))
                                    .or_insert_with(|| {
                                        updated
                                            .insert((*replica_id, *set_id), selection_set.state());
                                        selection_set.version
                                    });
                            }
                        }

                        if updated.len() > 0 || removed.len() > 0 {
                            return Async::Ready(Some(Update::Selections { updated, removed }));
                        }
                    }
                }
            }
        }
    }

    impl rpc::server::Service for Service {
        type State = State;
        type Update = Update;
        type Request = Request;
        type Response = Response;

        fn init(&mut self, _: &rpc::server::Connection) -> Self::State {
            let buffer = self.buffer.borrow_mut();
            let mut state = State {
                id: buffer.id,
                replica_id: self.replica_id,
                fragments: Vec::new(),
                insertions: HashMap::new(),
                insertion_splits: HashMap::new(),
                version: buffer.version.clone(),
                selections: HashMap::new(),
            };

            for fragment in buffer.fragments.iter() {
                state
                    .insertions
                    .entry(fragment.insertion.id)
                    .or_insert_with(|| fragment.insertion.clone());

                state.fragments.push(Fragment {
                    id: fragment.id.clone(),
                    insertion_id: fragment.insertion.id,
                    start_offset: fragment.start_offset,
                    end_offset: fragment.end_offset,
                    deletions: fragment.deletions.clone(),
                });
            }

            for (insertion_id, splits) in &buffer.insertion_splits {
                state
                    .insertion_splits
                    .insert(*insertion_id, splits.iter().cloned().collect());
            }

            state.selections = HashMap::new();
            for (id, selection_set) in &buffer.selections {
                state.selections.insert(*id, selection_set.state());
            }

            state
        }

        fn poll_update(&mut self, _: &rpc::server::Connection) -> Async<Option<Self::Update>> {
            match self.poll_outgoing_op() {
                Async::Ready(Some(update)) => Async::Ready(Some(update)),
                Async::Ready(None) => match self.poll_outgoing_selection_updates() {
                    Async::Ready(Some(update)) => Async::Ready(Some(update)),
                    Async::Ready(None) => Async::Ready(None),
                    Async::NotReady => Async::NotReady,
                },
                Async::NotReady => match self.poll_outgoing_selection_updates() {
                    Async::Ready(Some(update)) => Async::Ready(Some(update)),
                    Async::Ready(None) | Async::NotReady => Async::NotReady,
                },
            }
        }

        fn request(
            &mut self,
            request: Self::Request,
            _connection: &rpc::server::Connection,
        ) -> Option<Box<Future<Item = Self::Response, Error = Never>>> {
            match request {
                Request::Operation(op) => {
                    let mut buffer = self.buffer.borrow_mut();
                    buffer.broadcast_op(&op);
                    if buffer.integrate_op(op).is_err() {
                        unimplemented!("Invalid op: terminate the service and respond with error?");
                    }
                    None
                }
                Request::UpdateSelectionSet(set_id, state) => {
                    self.buffer.borrow_mut().update_remote_selection_set(
                        self.replica_id,
                        set_id,
                        state,
                    );
                    None
                }
                Request::RemoveSelectionSet(set_id) => {
                    self.buffer
                        .borrow_mut()
                        .remove_remote_selection_set(self.replica_id, set_id);
                    None
                }
                Request::Save => Some(Box::new(self.buffer.borrow().save().then(|result| {
                    match result {
                        Ok(_) => Ok(Response::Saved),
                        Err(error) => Ok(Response::Error(error)),
                    }
                }))),
            }
        }
    }

    impl Drop for Service {
        fn drop(&mut self) {
            self.buffer
                .borrow_mut()
                .remove_remote_selection_sets(self.replica_id);
        }
    }

    fn serialize_op<S: Serializer>(op: &Arc<Operation>, serializer: S) -> Result<S::Ok, S::Error> {
        op.serialize(serializer)
    }

    fn deserialize_op<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<Operation>, D::Error> {
        Ok(Arc::new(Operation::deserialize(deserializer)?))
    }
}

impl Buffer {
    pub fn new(id: BufferId) -> Self {
        let mut fragments = Tree::new();

        // Push start sentinel.
        let sentinel_id = EditId {
            replica_id: 0,
            timestamp: 0,
        };
        fragments.push(Fragment::new(
            FragmentId::min_value(),
            Insertion {
                id: sentinel_id,
                parent_id: EditId {
                    replica_id: 0,
                    timestamp: 0,
                },
                offset_in_parent: 0,
                replica_id: 0,
                text: Arc::new(Text::new(vec![])),
                timestamp: 0,
            },
        ));
        let mut insertion_splits = HashMap::new();
        insertion_splits.insert(
            sentinel_id,
            Tree::from_item(InsertionSplit {
                fragment_id: FragmentId::min_value(),
                extent: 0,
            }),
        );

        Self {
            id,
            replica_id: 1,
            next_replica_id: Some(2),
            local_clock: 0,
            lamport_clock: 0,
            fragments,
            insertion_splits,
            anchor_cache: RefCell::new(HashMap::default()),
            offset_cache: RefCell::new(HashMap::default()),
            version: Version::new(),
            client: None,
            operation_txs: Vec::new(),
            updates: NotifyCell::new(()),
            selections: HashMap::default(),
            next_local_selection_set_id: 0,
            file: None,
        }
    }

    pub fn remote(
        foreground: ForegroundExecutor,
        client: client::Service<rpc::Service>,
    ) -> Result<Rc<RefCell<Buffer>>, RpcError> {
        let state = client.state()?;
        let incoming_updates = client.updates()?;

        let mut insertions = HashMap::new();
        for (edit_id, insertion) in state.insertions {
            insertions.insert(edit_id, insertion);
        }

        let mut fragments = Tree::new();
        fragments.extend(state.fragments.into_iter().map(|fragment| Fragment {
            id: fragment.id,
            insertion: insertions.get(&fragment.insertion_id).unwrap().clone(),
            start_offset: fragment.start_offset,
            end_offset: fragment.end_offset,
            deletions: fragment.deletions,
        }));

        let mut insertion_splits = HashMap::new();
        for (insertion_id, splits) in state.insertion_splits {
            let mut split_tree = Tree::new();
            split_tree.extend(splits);
            insertion_splits.insert(insertion_id, split_tree);
        }

        let mut selection_sets = HashMap::default();
        for (id, state) in state.selections {
            selection_sets.insert(
                id,
                SelectionSet {
                    user_id: state.user_id,
                    selections: state.selections,
                    version: 0,
                },
            );
        }

        let buffer = Buffer {
            id: state.id,
            replica_id: state.replica_id,
            next_replica_id: None,
            local_clock: 0,
            lamport_clock: 0,
            fragments,
            insertion_splits,
            anchor_cache: RefCell::new(HashMap::default()),
            offset_cache: RefCell::new(HashMap::default()),
            version: state.version,
            client: Some(client),
            operation_txs: Vec::new(),
            updates: NotifyCell::new(()),
            selections: selection_sets,
            next_local_selection_set_id: 0,
            file: None,
        }.into_shared();

        let buffer_weak = Rc::downgrade(&buffer);
        foreground
            .execute(Box::new(incoming_updates.for_each(move |update| {
                if let Some(buffer) = buffer_weak.upgrade() {
                    let mut buffer = buffer.borrow_mut();
                    match update {
                        rpc::Update::Operation(operation) => {
                            if buffer.integrate_op(operation).is_err() {
                                unimplemented!("Invalid op");
                            }
                        }
                        rpc::Update::Selections { updated, removed } => {
                            for ((replica_id, set_id), state) in updated {
                                debug_assert!(replica_id != buffer.replica_id);
                                buffer.update_remote_selection_set(replica_id, set_id, state);
                            }

                            for (replica_id, set_id) in removed {
                                debug_assert!(replica_id != buffer.replica_id);
                                buffer.remove_remote_selection_set(replica_id, set_id);
                            }
                        }
                    }
                }

                Ok(())
            })))
            .unwrap();

        Ok(buffer)
    }

    pub fn id(&self) -> BufferId {
        self.id
    }

    pub fn next_replica_id(&mut self) -> Result<ReplicaId, ()> {
        let replica_id = self.next_replica_id.ok_or(())?;
        self.next_replica_id = Some(replica_id + 1);
        Ok(replica_id)
    }

    pub fn file_id(&self) -> Option<fs::FileId> {
        self.file.as_ref().map(|file| file.id())
    }

    pub fn set_file(&mut self, file: Box<fs::File>) {
        self.file = Some(file);
    }

    pub fn save(&self) -> Option<Box<Future<Item = (), Error = Error>>> {
        use std::error;

        if let Some(ref client) = self.client {
            Some(Box::new(client.request(rpc::Request::Save).then(
                |response| match response {
                    Ok(rpc::Response::Saved) => Ok(()),
                    Ok(rpc::Response::Error(error)) => Err(error),
                    Err(error) => Err(Error::RpcError(error)),
                },
            )))
        } else {
            self.file.as_ref().map(|file| {
                Box::new(
                    file.write_snapshot(self.snapshot()).map_err(|error| {
                        Error::IoError(error::Error::description(&error).to_owned())
                    }),
                ) as Box<Future<Item = (), Error = Error>>
            })
        }
    }

    pub fn len(&self) -> usize {
        self.fragments.len::<CharacterCount>().0
    }

    pub fn len_for_row(&self, row: u32) -> Result<u32, Error> {
        let row_start_offset = self.offset_for_point(Point::new(row, 0))?;
        let row_end_offset = if row >= self.max_point().row {
            self.len()
        } else {
            self.offset_for_point(Point::new(row + 1, 0))? - 1
        };

        Ok((row_end_offset - row_start_offset) as u32)
    }

    pub fn longest_row(&self) -> u32 {
        self.fragments.summary().longest_row
    }

    pub fn max_point(&self) -> Point {
        self.fragments.len::<Point>()
    }

    pub fn clip_point(&self, original: Point) -> Point {
        return cmp::max(cmp::min(original,self.max_point()),Point::new(0,0));
    }

    pub fn line(&self, row: u32) -> Result<Vec<u16>, Error> {
        let mut iterator = self.iter_starting_at_point(Point::new(row, 0)).peekable();
        if iterator.peek().is_none() {
            Err(Error::OffsetOutOfRange)
        } else {
            Ok(iterator.take_while(|c| *c != u16::from(b'\n')).collect())
        }
    }

    pub fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            fragments: self.fragments.clone(),
        }
    }

    pub fn to_u16_chars(&self) -> Vec<u16> {
        let mut result = Vec::with_capacity(self.len());
        result.extend(self.iter());
        result
    }

    #[cfg(test)]
    pub fn to_string(&self) -> String {
        String::from_utf16_lossy(self.iter().collect::<Vec<u16>>().as_slice())
    }

    pub fn iter(&self) -> Iter {
        Iter::new(self)
    }

    pub fn iter_starting_at_point(&self, point: Point) -> Iter {
        Iter::starting_at_point(self, point)
    }

    pub fn backward_iter_starting_at_point(&self, point: Point) -> BackwardIter {
        BackwardIter::starting_at_point(self, point)
    }

    pub fn edit<'a, I, T>(&mut self, old_ranges: I, new_text: T) -> Vec<Arc<Operation>>
    where
        I: IntoIterator<Item = &'a Range<usize>>,
        T: Into<Text>,
    {
        let new_text = new_text.into();
        let new_text = if new_text.len() > 0 {
            Some(Arc::new(new_text))
        } else {
            None
        };

        self.anchor_cache.borrow_mut().clear();
        self.offset_cache.borrow_mut().clear();
        let ops = self.splice_fragments(
            old_ranges
                .into_iter()
                .filter(|old_range| new_text.is_some() || old_range.end > old_range.start),
            new_text.clone(),
        );
        for op in &ops {
            self.broadcast_op(op);
        }
        self.version.set(self.replica_id, self.local_clock);
        self.updates.set(());
        ops
    }

    pub fn add_selection_set(
        &mut self,
        user_id: UserId,
        selections: Vec<Selection>,
    ) -> SelectionSetId {
        let id = self.next_local_selection_set_id;

        let set = SelectionSet {
            version: 0,
            selections,
            user_id,
        };

        if let Some(ref client) = self.client {
            client.request(rpc::Request::UpdateSelectionSet(id, set.state()));
        }

        self.next_local_selection_set_id += 1;
        self.selections.insert((self.replica_id, id), set);
        self.updates.set(());
        id
    }

    pub fn remove_selection_set(&mut self, id: SelectionSetId) -> Result<(), ()> {
        if let Some(ref client) = self.client {
            client.request(rpc::Request::RemoveSelectionSet(id));
        }

        self.selections.remove(&(self.replica_id, id)).ok_or(())?;
        self.updates.set(());
        Ok(())
    }

    pub fn selections(&self, set_id: SelectionSetId) -> Result<&[Selection], ()> {
        self.selections
            .get(&(self.replica_id, set_id))
            .ok_or(())
            .map(|set| set.selections.as_slice())
    }

    pub fn insert_selections<F>(&mut self, set_id: SelectionSetId, f: F) -> Result<(), Error>
    where
        F: FnOnce(&Buffer, &[Selection]) -> Vec<Selection>,
    {
        self.mutate_selections(set_id, |buffer, old_selections| {
            let mut new_selections = f(buffer, old_selections);
            new_selections.sort_unstable_by(|a, b| buffer.cmp_anchors(&a.start, &b.start).unwrap());

            let mut selections = Vec::with_capacity(old_selections.len() + new_selections.len());
            {
                let mut old_selections = old_selections.drain(..).peekable();
                let mut new_selections = new_selections.drain(..).peekable();
                loop {
                    if old_selections.peek().is_some() {
                        if new_selections.peek().is_some() {
                            match buffer
                                .cmp_anchors(
                                    &old_selections.peek().unwrap().start,
                                    &new_selections.peek().unwrap().start,
                                )
                                .unwrap()
                            {
                                Ordering::Less => {
                                    selections.push(old_selections.next().unwrap());
                                }
                                Ordering::Equal => {
                                    selections.push(old_selections.next().unwrap());
                                    selections.push(new_selections.next().unwrap());
                                }
                                Ordering::Greater => {
                                    selections.push(new_selections.next().unwrap());
                                }
                            }
                        } else {
                            selections.push(old_selections.next().unwrap());
                        }
                    } else if new_selections.peek().is_some() {
                        selections.push(new_selections.next().unwrap());
                    } else {
                        break;
                    }
                }
            }
            *old_selections = selections;
        })
    }

    pub fn mutate_selections<F>(&mut self, set_id: SelectionSetId, f: F) -> Result<(), Error>
    where
        F: FnOnce(&Buffer, &mut Vec<Selection>),
    {
        let id = (self.replica_id, set_id);
        let mut set = self.selections
            .remove(&id)
            .ok_or(Error::SelectionSetNotFound)?;
        f(self, &mut set.selections);
        self.merge_selections(&mut set.selections);
        set.version += 1;
        if let Some(ref client) = self.client {
            client.request(rpc::Request::UpdateSelectionSet(id.1, set.state()));
        }
        self.selections.insert(id, set);
        self.updates.set(());
        Ok(())
    }

    fn merge_selections(&mut self, selections: &mut Vec<Selection>) {
        let mut new_selections = Vec::with_capacity(selections.len());
        {
            let mut old_selections = selections.drain(..);
            if let Some(mut prev_selection) = old_selections.next() {
                for selection in old_selections {
                    if self.cmp_anchors(&prev_selection.end, &selection.start)
                        .unwrap() >= Ordering::Equal
                    {
                        if self.cmp_anchors(&selection.end, &prev_selection.end)
                            .unwrap() > Ordering::Equal
                        {
                            prev_selection.end = selection.end;
                        }
                    } else {
                        new_selections.push(mem::replace(&mut prev_selection, selection));
                    }
                }
                new_selections.push(prev_selection);
            }
        }
        *selections = new_selections;
    }

    pub fn remote_selections(&self) -> impl Iterator<Item = (UserId, &[Selection])> {
        let local_replica_id = self.replica_id;
        self.selections
            .iter()
            .filter_map(move |((replica_id, _), set)| {
                if *replica_id != local_replica_id {
                    Some((set.user_id, set.selections.as_slice()))
                } else {
                    None
                }
            })
    }

    pub fn updates(&self) -> NotifyCellObserver<()> {
        self.updates.observe()
    }

    fn broadcast_op(&mut self, op: &Arc<Operation>) {
        for i in (0..self.operation_txs.len()).rev() {
            if self.operation_txs[i].unbounded_send(op.clone()).is_err() {
                self.operation_txs.swap_remove(i);
            }
        }

        if let Some(ref client) = self.client {
            client.request(rpc::Request::Operation(op.clone()));
        }
    }

    fn integrate_op(&mut self, op: Arc<Operation>) -> Result<(), Error> {
        match op.as_ref() {
            &Operation::Edit {
                ref id,
                ref start_id,
                ref start_offset,
                ref end_id,
                ref end_offset,
                ref new_text,
                ref version_in_range,
                ref timestamp,
            } => self.integrate_edit(
                *id,
                *start_id,
                *start_offset,
                *end_id,
                *end_offset,
                new_text.as_ref().cloned(),
                version_in_range,
                *timestamp,
            )?,
        }
        self.anchor_cache.borrow_mut().clear();
        self.offset_cache.borrow_mut().clear();
        self.updates.set(());
        Ok(())
    }

    fn integrate_edit(
        &mut self,
        id: EditId,
        start_id: EditId,
        start_offset: usize,
        end_id: EditId,
        end_offset: usize,
        new_text: Option<Arc<Text>>,
        version_in_range: &Version,
        timestamp: LamportTimestamp,
    ) -> Result<(), Error> {
        let mut new_text = new_text.as_ref().cloned();
        let start_fragment_id = self.resolve_fragment_id(start_id, start_offset)?;
        let end_fragment_id = self.resolve_fragment_id(end_id, end_offset)?;

        let old_fragments = self.fragments.clone();
        let mut cursor = old_fragments.cursor();
        let mut new_fragments = cursor.slice(&start_fragment_id, SeekBias::Left);

        if start_offset == cursor.item().unwrap().end_offset {
            new_fragments.push(cursor.item().unwrap().clone());
            cursor.next();
        }

        while let Some(fragment) = cursor.item() {
            if new_text.is_none() && fragment.id > end_fragment_id {
                break;
            }

            if fragment.id == start_fragment_id || fragment.id == end_fragment_id {
                let split_start = if start_fragment_id == fragment.id {
                    start_offset
                } else {
                    fragment.start_offset
                };
                let split_end = if end_fragment_id == fragment.id {
                    end_offset
                } else {
                    fragment.end_offset
                };
                let (before_range, within_range, after_range) = self.split_fragment(
                    cursor.prev_item().unwrap(),
                    fragment,
                    split_start..split_end,
                );
                let insertion = new_text.take().map(|new_text| {
                    self.build_fragment_to_insert(
                        id,
                        before_range.as_ref().or(cursor.prev_item()).unwrap(),
                        within_range.as_ref().or(after_range.as_ref()),
                        new_text,
                        timestamp,
                    )
                });
                if let Some(fragment) = before_range {
                    new_fragments.push(fragment);
                }
                if let Some(fragment) = insertion {
                    new_fragments.push(fragment);
                }
                if let Some(mut fragment) = within_range {
                    if version_in_range.includes(&fragment.insertion) {
                        fragment.deletions.insert(id);
                    }
                    new_fragments.push(fragment);
                }
                if let Some(fragment) = after_range {
                    new_fragments.push(fragment);
                }
            } else {
                if new_text.is_some()
                    && should_insert_before(&fragment.insertion, timestamp, id.replica_id)
                {
                    new_fragments.push(self.build_fragment_to_insert(
                        id,
                        cursor.prev_item().unwrap(),
                        Some(fragment),
                        new_text.take().unwrap(),
                        timestamp,
                    ));
                }

                let mut fragment = fragment.clone();
                if version_in_range.includes(&fragment.insertion) {
                    fragment.deletions.insert(id);
                }
                new_fragments.push(fragment);
            }

            cursor.next();
        }

        if let Some(new_text) = new_text {
            new_fragments.push(self.build_fragment_to_insert(
                id,
                cursor.prev_item().unwrap(),
                None,
                new_text,
                timestamp,
            ));
        }

        new_fragments
            .push_tree(cursor.slice(&old_fragments.len::<CharacterCount>(), SeekBias::Right));
        self.fragments = new_fragments;
        self.lamport_clock = cmp::max(self.lamport_clock, timestamp) + 1;
        Ok(())
    }

    fn update_remote_selection_set(
        &mut self,
        replica_id: ReplicaId,
        set_id: SelectionSetId,
        state: SelectionSetState,
    ) {
        let set = self.selections
            .entry((replica_id, set_id))
            .or_insert(SelectionSet {
                user_id: state.user_id,
                selections: Vec::new(),
                version: 0,
            });
        set.version += 1;
        set.selections = state.selections;
        self.updates.set(());
    }

    fn remove_remote_selection_set(&mut self, replica_id: ReplicaId, set_id: SelectionSetId) {
        self.selections.remove(&(replica_id, set_id));
        self.updates.set(());
    }

    fn remove_remote_selection_sets(&mut self, id: ReplicaId) {
        self.selections
            .retain(|(replica_id, _), _| *replica_id != id);
        self.updates.set(());
    }

    fn resolve_fragment_id(&self, edit_id: EditId, offset: usize) -> Result<FragmentId, Error> {
        let split_tree = self.insertion_splits
            .get(&edit_id)
            .ok_or(Error::InvalidOperation)?;
        let mut cursor = split_tree.cursor();
        cursor.seek(&InsertionOffset(offset), SeekBias::Left);
        Ok(cursor
            .item()
            .ok_or(Error::InvalidOperation)?
            .fragment_id
            .clone())
    }

    fn outgoing_ops(&mut self) -> unsync::mpsc::UnboundedReceiver<Arc<Operation>> {
        let (tx, rx) = unsync::mpsc::unbounded();
        self.operation_txs.push(tx);
        rx
    }

    fn splice_fragments<'a, I>(
        &mut self,
        mut old_ranges: I,
        new_text: Option<Arc<Text>>,
    ) -> Vec<Arc<Operation>>
    where
        I: Iterator<Item = &'a Range<usize>>,
    {
        let mut cur_range = old_ranges.next();
        if cur_range.is_none() {
            return Vec::new();
        }

        let replica_id = self.replica_id;
        let mut ops = Vec::with_capacity(old_ranges.size_hint().0);

        let old_fragments = self.fragments.clone();
        let mut cursor = old_fragments.cursor();
        let mut new_fragments = Tree::new();
        new_fragments.push_tree(cursor.slice(
            &CharacterCount(cur_range.as_ref().unwrap().start),
            SeekBias::Right,
        ));

        self.local_clock += 1;
        self.lamport_clock += 1;
        let mut start_id = None;
        let mut start_offset = None;
        let mut end_id = None;
        let mut end_offset = None;
        let mut version_in_range = Version::new();

        while cur_range.is_some() && cursor.item().is_some() {
            let mut fragment = cursor.item().unwrap().clone();
            let mut fragment_start = cursor.start::<CharacterCount>().0;
            let mut fragment_end = fragment_start + fragment.len();

            let old_split_tree = self.insertion_splits
                .remove(&fragment.insertion.id)
                .unwrap();
            let mut splits_cursor = old_split_tree.cursor();
            let mut new_split_tree =
                splits_cursor.slice(&InsertionOffset(fragment.start_offset), SeekBias::Right);

            // Find all splices that start or end within the current fragment. Then, split the
            // fragment and reassemble it in both trees accounting for the deleted and the newly
            // inserted text.
            while cur_range.map_or(false, |range| range.start < fragment_end) {
                let range = cur_range.clone().unwrap();
                if range.start > fragment_start {
                    let mut prefix = fragment.clone();
                    prefix.end_offset = prefix.start_offset + (range.start - fragment_start);
                    prefix.id =
                        FragmentId::between(&new_fragments.last().unwrap().id, &fragment.id);
                    fragment.start_offset = prefix.end_offset;
                    new_fragments.push(prefix.clone());
                    new_split_tree.push(InsertionSplit {
                        extent: prefix.end_offset - prefix.start_offset,
                        fragment_id: prefix.id,
                    });
                    fragment_start = range.start;
                }

                if range.end == fragment_start {
                    end_id = Some(new_fragments.last().unwrap().insertion.id);
                    end_offset = Some(new_fragments.last().unwrap().end_offset);
                } else if range.end == fragment_end {
                    end_id = Some(fragment.insertion.id);
                    end_offset = Some(fragment.end_offset);
                }

                if range.start == fragment_start {
                    let local_timestamp = self.local_clock;
                    let lamport_timestamp = self.lamport_clock;

                    start_id = Some(new_fragments.last().unwrap().insertion.id);
                    start_offset = Some(new_fragments.last().unwrap().end_offset);

                    if let Some(new_text) = new_text.clone() {
                        let new_fragment = self.build_fragment_to_insert(
                            EditId {
                                replica_id,
                                timestamp: local_timestamp,
                            },
                            new_fragments.last().unwrap(),
                            Some(&fragment),
                            new_text,
                            lamport_timestamp,
                        );
                        new_fragments.push(new_fragment);
                    }
                }

                if range.end < fragment_end {
                    if range.end > fragment_start {
                        let mut prefix = fragment.clone();
                        prefix.end_offset = prefix.start_offset + (range.end - fragment_start);
                        prefix.id =
                            FragmentId::between(&new_fragments.last().unwrap().id, &fragment.id);
                        if fragment.is_visible() {
                            prefix.deletions.insert(EditId {
                                replica_id,
                                timestamp: self.local_clock,
                            });
                        }
                        fragment.start_offset = prefix.end_offset;
                        new_fragments.push(prefix.clone());
                        new_split_tree.push(InsertionSplit {
                            extent: prefix.end_offset - prefix.start_offset,
                            fragment_id: prefix.id,
                        });
                        fragment_start = range.end;
                        end_id = Some(fragment.insertion.id);
                        end_offset = Some(fragment.start_offset);
                        version_in_range.include(&fragment.insertion);
                    }
                } else {
                    version_in_range.include(&fragment.insertion);
                    if fragment.is_visible() {
                        fragment.deletions.insert(EditId {
                            replica_id,
                            timestamp: self.local_clock,
                        });
                    }
                }

                // If the splice ends inside this fragment, we can advance to the next splice and
                // check if it also intersects the current fragment. Otherwise we break out of the
                // loop and find the first fragment that the splice does not contain fully.
                if range.end <= fragment_end {
                    ops.push(Arc::new(Operation::Edit {
                        id: EditId {
                            replica_id,
                            timestamp: self.local_clock,
                        },
                        start_id: start_id.unwrap(),
                        start_offset: start_offset.unwrap(),
                        end_id: end_id.unwrap(),
                        end_offset: end_offset.unwrap(),
                        new_text: new_text.clone(),
                        timestamp: self.lamport_clock,
                        version_in_range,
                    }));

                    start_id = None;
                    start_offset = None;
                    end_id = None;
                    end_offset = None;
                    version_in_range = Version::new();
                    cur_range = old_ranges.next();
                    if cur_range.is_some() {
                        self.local_clock += 1;
                        self.lamport_clock += 1;
                    }
                } else {
                    break;
                }
            }
            new_split_tree.push(InsertionSplit {
                extent: fragment.end_offset - fragment.start_offset,
                fragment_id: fragment.id.clone(),
            });
            splits_cursor.next();
            new_split_tree.push_tree(
                splits_cursor.slice(&old_split_tree.len::<InsertionOffset>(), SeekBias::Right),
            );
            self.insertion_splits
                .insert(fragment.insertion.id, new_split_tree);
            new_fragments.push(fragment);

            // Scan forward until we find a fragment that is not fully contained by the current splice.
            cursor.next();
            if let Some(range) = cur_range.clone() {
                while let Some(mut fragment) = cursor.item().cloned() {
                    fragment_start = cursor.start::<CharacterCount>().0;
                    fragment_end = fragment_start + fragment.len();
                    if range.start < fragment_start && range.end >= fragment_end {
                        if fragment.is_visible() {
                            fragment.deletions.insert(EditId {
                                replica_id,
                                timestamp: self.local_clock,
                            });
                        }
                        version_in_range.include(&fragment.insertion);
                        new_fragments.push(fragment.clone());
                        cursor.next();

                        if range.end == fragment_end {
                            end_id = Some(fragment.insertion.id);
                            end_offset = Some(fragment.end_offset);
                            ops.push(Arc::new(Operation::Edit {
                                id: EditId {
                                    replica_id,
                                    timestamp: self.local_clock,
                                },
                                start_id: start_id.unwrap(),
                                start_offset: start_offset.unwrap(),
                                end_id: end_id.unwrap(),
                                end_offset: end_offset.unwrap(),
                                new_text: new_text.clone(),
                                timestamp: self.lamport_clock,
                                version_in_range,
                            }));

                            start_id = None;
                            start_offset = None;
                            end_id = None;
                            end_offset = None;
                            version_in_range = Version::new();

                            cur_range = old_ranges.next();
                            if cur_range.is_some() {
                                self.local_clock += 1;
                                self.lamport_clock += 1;
                            }
                            break;
                        }
                    } else {
                        break;
                    }
                }

                // If the splice we are currently evaluating starts after the end of the fragment
                // that the cursor is parked at, we should seek to the next splice's start range
                // and push all the fragments in between into the new tree.
                if cur_range.map_or(false, |range| range.start > fragment_end) {
                    new_fragments.push_tree(cursor.slice(
                        &CharacterCount(cur_range.as_ref().unwrap().start),
                        SeekBias::Right,
                    ));
                }
            }
        }

        // Handle range that is at the end of the buffer if it exists. There should never be
        // multiple because ranges must be disjoint.
        if cur_range.is_some() {
            debug_assert_eq!(old_ranges.next(), None);
            let local_timestamp = self.local_clock;
            let lamport_timestamp = self.lamport_clock;
            let id = EditId {
                replica_id,
                timestamp: local_timestamp,
            };
            ops.push(Arc::new(Operation::Edit {
                id,
                start_id: new_fragments.last().unwrap().insertion.id,
                start_offset: new_fragments.last().unwrap().end_offset,
                end_id: new_fragments.last().unwrap().insertion.id,
                end_offset: new_fragments.last().unwrap().end_offset,
                new_text: new_text.clone(),
                timestamp: lamport_timestamp,
                version_in_range: Version::new(),
            }));

            if let Some(new_text) = new_text {
                let new_fragment = self.build_fragment_to_insert(
                    id,
                    new_fragments.last().unwrap(),
                    None,
                    new_text,
                    lamport_timestamp,
                );
                new_fragments.push(new_fragment);
            }
        } else {
            new_fragments
                .push_tree(cursor.slice(&old_fragments.len::<CharacterCount>(), SeekBias::Right));
        }

        self.fragments = new_fragments;
        ops
    }

    fn split_fragment(
        &mut self,
        prev_fragment: &Fragment,
        fragment: &Fragment,
        range: Range<usize>,
    ) -> (Option<Fragment>, Option<Fragment>, Option<Fragment>) {
        debug_assert!(range.start >= fragment.start_offset);
        debug_assert!(range.start <= fragment.end_offset);
        debug_assert!(range.end <= fragment.end_offset);
        debug_assert!(range.end >= fragment.start_offset);

        if range.end == fragment.start_offset {
            (None, None, Some(fragment.clone()))
        } else if range.start == fragment.end_offset {
            (Some(fragment.clone()), None, None)
        } else if range.start == fragment.start_offset && range.end == fragment.end_offset {
            (None, Some(fragment.clone()), None)
        } else {
            let mut prefix = fragment.clone();

            let after_range = if range.end < fragment.end_offset {
                let mut suffix = prefix.clone();
                suffix.start_offset = range.end;
                prefix.end_offset = range.end;
                prefix.id = FragmentId::between(&prev_fragment.id, &suffix.id);
                Some(suffix)
            } else {
                None
            };

            let within_range = if range.start != range.end {
                let mut suffix = prefix.clone();
                suffix.start_offset = range.start;
                prefix.end_offset = range.start;
                prefix.id = FragmentId::between(&prev_fragment.id, &suffix.id);
                Some(suffix)
            } else {
                None
            };

            let before_range = if range.start > fragment.start_offset {
                Some(prefix)
            } else {
                None
            };

            let old_split_tree = self.insertion_splits
                .remove(&fragment.insertion.id)
                .unwrap();
            let mut cursor = old_split_tree.cursor();
            let mut new_split_tree =
                cursor.slice(&InsertionOffset(fragment.start_offset), SeekBias::Right);

            if let Some(ref fragment) = before_range {
                new_split_tree.push(InsertionSplit {
                    extent: range.start - fragment.start_offset,
                    fragment_id: fragment.id.clone(),
                })
            }

            if let Some(ref fragment) = within_range {
                new_split_tree.push(InsertionSplit {
                    extent: range.end - range.start,
                    fragment_id: fragment.id.clone(),
                })
            }

            if let Some(ref fragment) = after_range {
                new_split_tree.push(InsertionSplit {
                    extent: fragment.end_offset - range.end,
                    fragment_id: fragment.id.clone(),
                })
            }

            cursor.next();
            new_split_tree
                .push_tree(cursor.slice(&old_split_tree.len::<InsertionOffset>(), SeekBias::Right));

            self.insertion_splits
                .insert(fragment.insertion.id, new_split_tree);

            (before_range, within_range, after_range)
        }
    }

    fn build_fragment_to_insert(
        &mut self,
        edit_id: EditId,
        prev_fragment: &Fragment,
        next_fragment: Option<&Fragment>,
        text: Arc<Text>,
        timestamp: LamportTimestamp,
    ) -> Fragment {
        let new_fragment_id = FragmentId::between(
            &prev_fragment.id,
            next_fragment
                .map(|f| &f.id)
                .unwrap_or(&FragmentId::max_value()),
        );

        let mut split_tree = Tree::new();
        split_tree.push(InsertionSplit {
            extent: text.len(),
            fragment_id: new_fragment_id.clone(),
        });
        self.insertion_splits.insert(edit_id, split_tree);

        Fragment::new(
            new_fragment_id,
            Insertion {
                id: edit_id,
                parent_id: prev_fragment.insertion.id,
                offset_in_parent: prev_fragment.end_offset,
                replica_id: self.replica_id,
                text,
                timestamp,
            },
        )
    }

    pub fn anchor_before_offset(&self, offset: usize) -> Result<Anchor, Error> {
        self.anchor_for_offset(offset, AnchorBias::Left)
    }

    pub fn anchor_after_offset(&self, offset: usize) -> Result<Anchor, Error> {
        self.anchor_for_offset(offset, AnchorBias::Right)
    }

    fn anchor_for_offset(&self, offset: usize, bias: AnchorBias) -> Result<Anchor, Error> {
        let max_offset = self.len();
        if offset > max_offset {
            return Err(Error::OffsetOutOfRange);
        }

        let seek_bias;
        match bias {
            AnchorBias::Left => {
                if offset == 0 {
                    return Ok(Anchor(AnchorInner::Start));
                } else {
                    seek_bias = SeekBias::Left;
                }
            }
            AnchorBias::Right => {
                if offset == max_offset {
                    return Ok(Anchor(AnchorInner::End));
                } else {
                    seek_bias = SeekBias::Right;
                }
            }
        };

        let mut cursor = self.fragments.cursor();
        cursor.seek(&CharacterCount(offset), seek_bias);
        let fragment = cursor.item().unwrap();
        let offset_in_fragment = offset - cursor.start::<CharacterCount>().0;
        let offset_in_insertion = fragment.start_offset + offset_in_fragment;
        let point = cursor.start::<Point>() + &fragment.point_for_offset(offset_in_fragment)?;
        let anchor = Anchor(AnchorInner::Middle {
            insertion_id: fragment.insertion.id,
            offset: offset_in_insertion,
            bias,
        });
        self.cache_position(Some(anchor.clone()), offset, point);
        Ok(anchor)
    }

    pub fn anchor_before_point(&self, point: Point) -> Result<Anchor, Error> {
        self.anchor_for_point(point, AnchorBias::Left)
    }

    pub fn anchor_after_point(&self, point: Point) -> Result<Anchor, Error> {
        self.anchor_for_point(point, AnchorBias::Right)
    }

    fn anchor_for_point(&self, point: Point, bias: AnchorBias) -> Result<Anchor, Error> {
        let max_point = self.max_point();
        if point > max_point {
            return Err(Error::OffsetOutOfRange);
        }

        let seek_bias;
        match bias {
            AnchorBias::Left => {
                if point.is_zero() {
                    return Ok(Anchor(AnchorInner::Start));
                } else {
                    seek_bias = SeekBias::Left;
                }
            }
            AnchorBias::Right => {
                if point == max_point {
                    return Ok(Anchor(AnchorInner::End));
                } else {
                    seek_bias = SeekBias::Right;
                }
            }
        };

        let mut cursor = self.fragments.cursor();
        cursor.seek(&point, seek_bias);
        let fragment = cursor.item().unwrap();
        let offset_in_fragment = fragment.offset_for_point(point - &cursor.start::<Point>())?;
        let offset_in_insertion = fragment.start_offset + offset_in_fragment;
        let anchor = Anchor(AnchorInner::Middle {
            insertion_id: fragment.insertion.id,
            offset: offset_in_insertion,
            bias,
        });
        let offset = cursor.start::<CharacterCount>().0 + offset_in_fragment;
        self.cache_position(Some(anchor.clone()), offset, point);
        Ok(anchor)
    }

    pub fn offset_for_anchor(&self, anchor: &Anchor) -> Result<usize, Error> {
        Ok(self.position_for_anchor(anchor)?.0)
    }

    pub fn point_for_anchor(&self, anchor: &Anchor) -> Result<Point, Error> {
        Ok(self.position_for_anchor(anchor)?.1)
    }

    fn position_for_anchor(&self, anchor: &Anchor) -> Result<(usize, Point), Error> {
        match &anchor.0 {
            &AnchorInner::Start => Ok((0, Point { row: 0, column: 0 })),
            &AnchorInner::End => Ok((self.len(), self.fragments.len::<Point>())),
            &AnchorInner::Middle {
                ref insertion_id,
                offset,
                ref bias,
            } => {
                let cached_position = {
                    let anchor_cache = self.anchor_cache.try_borrow().ok();
                    anchor_cache
                        .as_ref()
                        .and_then(|cache| cache.get(anchor).cloned())
                };

                if let Some(cached_position) = cached_position {
                    Ok(cached_position)
                } else {
                    let seek_bias = match bias {
                        &AnchorBias::Left => SeekBias::Left,
                        &AnchorBias::Right => SeekBias::Right,
                    };

                    let splits = self.insertion_splits
                        .get(&insertion_id)
                        .ok_or(Error::InvalidAnchor)?;
                    let mut splits_cursor = splits.cursor();
                    splits_cursor.seek(&InsertionOffset(offset), seek_bias);
                    splits_cursor
                        .item()
                        .ok_or(Error::InvalidAnchor)
                        .and_then(|split| {
                            let mut fragments_cursor = self.fragments.cursor();
                            fragments_cursor.seek(&split.fragment_id, SeekBias::Left);
                            fragments_cursor
                                .item()
                                .ok_or(Error::InvalidAnchor)
                                .and_then(|fragment| {
                                    let overshoot = if fragment.is_visible() {
                                        offset - fragment.start_offset
                                    } else {
                                        0
                                    };
                                    let offset =
                                        fragments_cursor.start::<CharacterCount>().0 + overshoot;
                                    let point = fragments_cursor.start::<Point>()
                                        + &fragment.point_for_offset(overshoot)?;
                                    self.cache_position(Some(anchor.clone()), offset, point);
                                    Ok((offset, point))
                                })
                        })
                }
            }
        }
    }

    fn offset_for_point(&self, point: Point) -> Result<usize, Error> {
        let cached_offset = {
            let offset_cache = self.offset_cache.try_borrow().ok();
            offset_cache
                .as_ref()
                .and_then(|cache| cache.get(&point).cloned())
        };

        if let Some(cached_offset) = cached_offset {
            Ok(cached_offset)
        } else {
            let mut fragments_cursor = self.fragments.cursor();
            fragments_cursor.seek(&point, SeekBias::Left);
            fragments_cursor
                .item()
                .ok_or(Error::OffsetOutOfRange)
                .map(|fragment| {
                    let overshoot = fragment
                        .offset_for_point(point - &fragments_cursor.start::<Point>())
                        .unwrap();
                    let offset = &fragments_cursor.start::<CharacterCount>().0 + &overshoot;
                    self.cache_position(None, offset, point);
                    offset
                })
        }
    }

    pub fn cmp_anchors(&self, a: &Anchor, b: &Anchor) -> Result<Ordering, Error> {
        let a_offset = self.offset_for_anchor(a)?;
        let b_offset = self.offset_for_anchor(b)?;
        Ok(a_offset.cmp(&b_offset))
    }

    fn cache_position(&self, anchor: Option<Anchor>, offset: usize, point: Point) {
        anchor.map(|anchor| {
            if let Ok(mut anchor_cache) = self.anchor_cache.try_borrow_mut() {
                anchor_cache.insert(anchor, (offset, point));
            }
        });

        if let Ok(mut offset_cache) = self.offset_cache.try_borrow_mut() {
            offset_cache.insert(point, offset);
        }
    }
}

impl BufferSnapshot {
    pub fn iter<'a>(&'a self) -> impl 'a + Iterator<Item = &'a [u16]> {
        self.fragments.iter().filter_map(|fragment| {
            if fragment.is_visible() {
                let range = fragment.start_offset..fragment.end_offset;
                Some(&fragment.insertion.text.code_units[range])
            } else {
                None
            }
        })
    }

    #[cfg(test)]
    pub fn to_string(&self) -> String {
        String::from_utf16_lossy(&self.iter().flat_map(|c| c).cloned().collect::<Vec<u16>>())
    }
}

impl Point {
    pub fn new(row: u32, column: u32) -> Self {
        Point { row, column }
    }

    #[cfg(test)]
    pub fn zero() -> Self {
        Point::new(0, 0)
    }

    pub fn is_zero(&self) -> bool {
        self.row == 0 && self.column == 0
    }
}

impl tree::Dimension for Point {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.extent_2d
    }
}

impl<'a> Add<&'a Self> for Point {
    type Output = Point;

    fn add(self, other: &'a Self) -> Self::Output {
        if other.row == 0 {
            Point::new(self.row, self.column + other.column)
        } else {
            Point::new(self.row + other.row, other.column)
        }
    }
}

impl<'a> Sub<&'a Self> for Point {
    type Output = Point;

    fn sub(self, other: &'a Self) -> Self::Output {
        debug_assert!(*other <= self);

        if self.row == other.row {
            Point::new(0, self.column - other.column)
        } else {
            Point::new(self.row - other.row, self.column)
        }
    }
}

impl AddAssign for Point {
    fn add_assign(&mut self, other: Self) {
        if other.row == 0 {
            self.column += other.column;
        } else {
            self.row += other.row;
            self.column = other.column;
        }
    }
}

impl PartialOrd for Point {
    fn partial_cmp(&self, other: &Point) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Point {
    #[cfg(target_pointer_width = "64")]
    fn cmp(&self, other: &Point) -> Ordering {
        let a = (self.row as usize) << 32 | self.column as usize;
        let b = (other.row as usize) << 32 | other.column as usize;
        a.cmp(&b)
    }

    #[cfg(target_pointer_width = "32")]
    fn cmp(&self, other: &Point) -> Ordering {
        match self.row.cmp(&other.row) {
            Ordering::Equal => self.column.cmp(&other.column),
            comparison @ _ => comparison,
        }
    }
}

impl SelectionSet {
    fn state(&self) -> SelectionSetState {
        SelectionSetState {
            user_id: self.user_id,
            selections: self.selections.clone(),
        }
    }
}

impl<'a> Iter<'a> {
    fn new(buffer: &'a Buffer) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(&CharacterCount(0), SeekBias::Right);
        Self {
            fragment_cursor,
            fragment_offset: 0,
        }
    }

    fn starting_at_point(buffer: &'a Buffer, point: Point) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(&point, SeekBias::Right);
        let fragment_offset = if let Some(fragment) = fragment_cursor.item() {
            let point_in_fragment = point - &fragment_cursor.start::<Point>();
            fragment.offset_for_point(point_in_fragment).unwrap()
        } else {
            0
        };

        Self {
            fragment_cursor,
            fragment_offset,
        }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(fragment) = self.fragment_cursor.item() {
            if let Some(c) = fragment.get_code_unit(self.fragment_offset) {
                self.fragment_offset += 1;
                return Some(c);
            }
        }

        loop {
            self.fragment_cursor.next();
            if let Some(fragment) = self.fragment_cursor.item() {
                if let Some(c) = fragment.get_code_unit(0) {
                    self.fragment_offset = 1;
                    return Some(c);
                }
            } else {
                break;
            }
        }

        None
    }
}

impl<'a> BackwardIter<'a> {
    fn starting_at_point(buffer: &'a Buffer, point: Point) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(&point, SeekBias::Left);
        let fragment_offset = if let Some(fragment) = fragment_cursor.item() {
            let point_in_fragment = point - &fragment_cursor.start::<Point>();
            fragment.offset_for_point(point_in_fragment).unwrap()
        } else {
            0
        };

        Self {
            fragment_cursor,
            fragment_offset,
        }
    }
}

impl<'a> Iterator for BackwardIter<'a> {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(fragment) = self.fragment_cursor.item() {
            if self.fragment_offset > 0 {
                self.fragment_offset -= 1;
                if let Some(c) = fragment.get_code_unit(self.fragment_offset) {
                    return Some(c);
                }
            }
        }

        loop {
            self.fragment_cursor.prev();
            if let Some(fragment) = self.fragment_cursor.item() {
                if fragment.len() > 0 {
                    self.fragment_offset = fragment.len() - 1;
                    return fragment.get_code_unit(self.fragment_offset);
                }
            } else {
                break;
            }
        }

        None
    }
}

impl Selection {
    pub fn head(&self) -> &Anchor {
        if self.reversed {
            &self.start
        } else {
            &self.end
        }
    }

    pub fn set_head(&mut self, buffer: &Buffer, cursor: Anchor) {
        if buffer.cmp_anchors(&cursor, self.tail()).unwrap() < Ordering::Equal {
            if !self.reversed {
                mem::swap(&mut self.start, &mut self.end);
                self.reversed = true;
            }
            self.start = cursor;
        } else {
            if self.reversed {
                mem::swap(&mut self.start, &mut self.end);
                self.reversed = false;
            }
            self.end = cursor;
        }
    }

    pub fn tail(&self) -> &Anchor {
        if self.reversed {
            &self.end
        } else {
            &self.start
        }
    }

    pub fn is_empty(&self, buffer: &Buffer) -> bool {
        buffer.cmp_anchors(&self.start, &self.end).unwrap() == Ordering::Equal
    }

    pub fn anchor_range(&self) -> Range<Anchor> {
        self.start.clone()..self.end.clone()
    }
}

impl Text {
    fn new(code_units: Vec<u16>) -> Self {
        fn build_tree(index: usize, line_lengths: &[u32], mut tree: &mut [LineNode]) {
            if line_lengths.is_empty() {
                return;
            }

            let mid = if line_lengths.len() == 1 {
                0
            } else {
                let depth = log2_fast(line_lengths.len());
                let max_elements = (1 << (depth)) - 1;
                let right_subtree_elements = 1 << (depth - 1);
                cmp::min(line_lengths.len() - right_subtree_elements, max_elements)
            };
            let len = line_lengths[mid];
            let lower = &line_lengths[0..mid];
            let upper = &line_lengths[mid + 1..];

            let left_child_index = index * 2 + 1;
            let right_child_index = index * 2 + 2;
            build_tree(left_child_index, lower, &mut tree);
            build_tree(right_child_index, upper, &mut tree);
            tree[index] = {
                let mut left_child_longest_row = 0;
                let mut left_child_longest_row_len = 0;
                let mut left_child_offset = 0;
                let mut left_child_rows = 0;
                if let Some(left_child) = tree.get(left_child_index) {
                    left_child_longest_row = left_child.longest_row;
                    left_child_longest_row_len = left_child.longest_row_len;
                    left_child_offset = left_child.offset;
                    left_child_rows = left_child.rows;
                }
                let mut right_child_longest_row = 0;
                let mut right_child_longest_row_len = 0;
                let mut right_child_offset = 0;
                let mut right_child_rows = 0;
                if let Some(right_child) = tree.get(right_child_index) {
                    right_child_longest_row = right_child.longest_row;
                    right_child_longest_row_len = right_child.longest_row_len;
                    right_child_offset = right_child.offset;
                    right_child_rows = right_child.rows;
                }

                let mut longest_row = 0;
                let mut longest_row_len = 0;
                if left_child_longest_row_len > longest_row_len {
                    longest_row = left_child_longest_row;
                    longest_row_len = left_child_longest_row_len;
                }
                if len > longest_row_len {
                    longest_row = left_child_rows;
                    longest_row_len = len;
                }
                if right_child_longest_row_len > longest_row_len {
                    longest_row = left_child_rows + right_child_longest_row + 1;
                    longest_row_len = right_child_longest_row_len;
                }

                LineNode {
                    len,
                    longest_row,
                    longest_row_len,
                    offset: left_child_offset + len as usize + right_child_offset + 1,
                    rows: left_child_rows + right_child_rows + 1,
                }
            };
        }

        let mut line_lengths = Vec::new();
        let mut prev_offset = 0;
        for (offset, code_unit) in code_units.iter().enumerate() {
            if code_unit == &u16::from(b'\n') {
                line_lengths.push((offset - prev_offset) as u32);
                prev_offset = offset + 1;
            }
        }
        line_lengths.push((code_units.len() - prev_offset) as u32);

        let mut nodes = Vec::new();
        nodes.resize(
            line_lengths.len(),
            LineNode {
                len: 0,
                longest_row_len: 0,
                longest_row: 0,
                offset: 0,
                rows: 0,
            },
        );
        build_tree(0, &line_lengths, &mut nodes);

        Self { code_units, nodes }
    }

    fn len(&self) -> usize {
        self.code_units.len()
    }

    fn longest_row_in_range(&self, target_range: Range<usize>) -> Result<(u32, u32), Error> {
        let mut longest_row = 0;
        let mut longest_row_len = 0;

        self.search(|probe| {
            if target_range.start <= probe.offset_range.end
                && probe.right_ancestor_start_offset <= target_range.end
            {
                if let Some(right_child) = probe.right_child {
                    if right_child.longest_row_len >= longest_row_len {
                        longest_row = probe.row + 1 + right_child.longest_row;
                        longest_row_len = right_child.longest_row_len;
                    }
                }
            }

            if target_range.start < probe.offset_range.start {
                if probe.offset_range.end < target_range.end && probe.node.len >= longest_row_len {
                    longest_row = probe.row;
                    longest_row_len = probe.node.len;
                }

                Ordering::Less
            } else if target_range.start > probe.offset_range.end {
                Ordering::Greater
            } else {
                let node_end = cmp::min(probe.offset_range.end, target_range.end);
                let node_len = (node_end - target_range.start) as u32;
                if node_len >= longest_row_len {
                    longest_row = probe.row;
                    longest_row_len = node_len;
                }
                Ordering::Equal
            }
        }).ok_or(Error::OffsetOutOfRange)?;

        self.search(|probe| {
            if target_range.end >= probe.offset_range.start
                && probe.left_ancestor_end_offset >= target_range.start
            {
                if let Some(left_child) = probe.left_child {
                    if left_child.longest_row_len > longest_row_len {
                        let left_ancestor_row = probe.row - left_child.rows;
                        longest_row = left_ancestor_row + left_child.longest_row;
                        longest_row_len = left_child.longest_row_len;
                    }
                }
            }

            if target_range.end < probe.offset_range.start {
                Ordering::Less
            } else if target_range.end > probe.offset_range.end {
                if target_range.start < probe.offset_range.start && probe.node.len > longest_row_len
                {
                    longest_row = probe.row;
                    longest_row_len = probe.node.len;
                }

                Ordering::Greater
            } else {
                let node_start = cmp::max(target_range.start, probe.offset_range.start);
                let node_len = (target_range.end - node_start) as u32;
                if node_len > longest_row_len {
                    longest_row = probe.row;
                    longest_row_len = node_len;
                }
                Ordering::Equal
            }
        }).ok_or(Error::OffsetOutOfRange)?;

        Ok((longest_row, longest_row_len))
    }

    fn point_for_offset(&self, offset: usize) -> Result<Point, Error> {
        let search_result = self.search(|probe| {
            if offset < probe.offset_range.start {
                Ordering::Less
            } else if offset > probe.offset_range.end {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });
        if let Some((offset_range, row, _)) = search_result {
            Ok(Point::new(row, (offset - offset_range.start) as u32))
        } else {
            Err(Error::OffsetOutOfRange)
        }
    }

    fn offset_for_point(&self, point: Point) -> Result<usize, Error> {
        if let Some((offset_range, _, node)) = self.search(|probe| point.row.cmp(&probe.row)) {
            if point.column <= node.len {
                Ok(offset_range.start + point.column as usize)
            } else {
                Err(Error::OffsetOutOfRange)
            }
        } else {
            Err(Error::OffsetOutOfRange)
        }
    }

    fn search<F>(&self, mut f: F) -> Option<(Range<usize>, u32, &LineNode)>
    where
        F: FnMut(LineNodeProbe) -> Ordering,
    {
        let mut left_ancestor_end_offset = 0;
        let mut left_ancestor_row = 0;
        let mut right_ancestor_start_offset = self.nodes[0].offset;
        let mut cur_node_index = 0;
        while let Some(cur_node) = self.nodes.get(cur_node_index) {
            let left_child = self.nodes.get(cur_node_index * 2 + 1);
            let right_child = self.nodes.get(cur_node_index * 2 + 2);
            let cur_offset_range = {
                let start = left_ancestor_end_offset + left_child.map_or(0, |node| node.offset);
                let end = start + cur_node.len as usize;
                start..end
            };
            let cur_row = left_ancestor_row + left_child.map_or(0, |node| node.rows);
            match f(LineNodeProbe {
                offset_range: &cur_offset_range,
                row: cur_row,
                left_ancestor_end_offset,
                right_ancestor_start_offset,
                node: cur_node,
                left_child,
                right_child,
            }) {
                Ordering::Less => {
                    cur_node_index = cur_node_index * 2 + 1;
                    right_ancestor_start_offset = cur_offset_range.start;
                }
                Ordering::Equal => return Some((cur_offset_range, cur_row, cur_node)),
                Ordering::Greater => {
                    cur_node_index = cur_node_index * 2 + 2;
                    left_ancestor_end_offset = cur_offset_range.end + 1;
                    left_ancestor_row = cur_row + 1;
                }
            }
        }
        None
    }
}

impl<'a> From<&'a str> for Text {
    fn from(s: &'a str) -> Self {
        Self::new(s.encode_utf16().collect())
    }
}

impl<'a> From<Vec<u16>> for Text {
    fn from(s: Vec<u16>) -> Self {
        Self::new(s)
    }
}

#[inline(always)]
fn log2_fast(x: usize) -> usize {
    8 * mem::size_of::<usize>() - (x.leading_zeros() as usize) - 1
}

lazy_static! {
    static ref FRAGMENT_ID_MIN_VALUE: FragmentId = FragmentId(Arc::new(vec![0 as u16]));
    static ref FRAGMENT_ID_MAX_VALUE: FragmentId = FragmentId(Arc::new(vec![u16::max_value()]));
}

impl FragmentId {
    fn min_value() -> Self {
        FRAGMENT_ID_MIN_VALUE.clone()
    }

    fn max_value() -> Self {
        FRAGMENT_ID_MAX_VALUE.clone()
    }

    fn between(left: &Self, right: &Self) -> Self {
        Self::between_with_max(left, right, u16::max_value())
    }

    fn between_with_max(left: &Self, right: &Self, max_value: u16) -> Self {
        let mut new_entries = Vec::new();

        let left_entries = left.0.iter().cloned().chain(iter::repeat(0));
        let right_entries = right.0.iter().cloned().chain(iter::repeat(max_value));
        for (l, r) in left_entries.zip(right_entries) {
            let interval = r - l;
            if interval > 1 {
                new_entries.push(l + interval / 2);
                break;
            } else {
                new_entries.push(l);
            }
        }

        FragmentId(Arc::new(new_entries))
    }
}

impl tree::Dimension for FragmentId {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.max_fragment_id.clone()
    }
}

impl<'a> Add<&'a Self> for FragmentId {
    type Output = FragmentId;

    fn add(self, other: &'a Self) -> Self::Output {
        cmp::max(&self, other).clone()
    }
}

impl AddAssign for FragmentId {
    fn add_assign(&mut self, other: Self) {
        if *self < other {
            *self = other
        }
    }
}

fn serialize_option_arc<T, S>(option: &Option<Arc<T>>, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    if let &Some(ref arc) = option {
        serializer.serialize_some(arc.as_ref())
    } else {
        serializer.serialize_none()
    }
}

fn deserialize_option_arc<'de, T, D>(deserializer: D) -> Result<Option<Arc<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    struct OptionArcVisitor<T>(marker::PhantomData<T>);

    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for OptionArcVisitor<T> {
        type Value = Option<Arc<T>>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "an Option<Arc<T>>")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            Ok(Some(Arc::new(T::deserialize(deserializer)?)))
        }
    }

    let visitor = OptionArcVisitor(marker::PhantomData);
    deserializer.deserialize_option(visitor)
}

fn serialize_arc<T, S>(arc: &Arc<T>, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    arc.serialize(serializer)
}

fn deserialize_arc<'de, T, D>(deserializer: D) -> Result<Arc<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Arc::new(T::deserialize(deserializer)?))
}

impl Fragment {
    fn new(id: FragmentId, insertion: Insertion) -> Self {
        let end_offset = insertion.text.len();
        Self {
            id,
            insertion,
            start_offset: 0,
            end_offset,
            deletions: HashSet::new(),
        }
    }

    fn get_code_unit(&self, offset: usize) -> Option<u16> {
        if offset < self.len() {
            Some(self.insertion.text.code_units[self.start_offset + offset].clone())
        } else {
            None
        }
    }

    fn len(&self) -> usize {
        if self.is_visible() {
            self.end_offset - self.start_offset
        } else {
            0
        }
    }

    fn is_visible(&self) -> bool {
        self.deletions.is_empty()
    }

    fn point_for_offset(&self, offset: usize) -> Result<Point, Error> {
        let text = &self.insertion.text;
        let offset_in_insertion = self.start_offset + offset;
        Ok(
            text.point_for_offset(offset_in_insertion)?
                - &text.point_for_offset(self.start_offset)?,
        )
    }

    fn offset_for_point(&self, point: Point) -> Result<usize, Error> {
        let text = &self.insertion.text;
        let point_in_insertion = text.point_for_offset(self.start_offset)? + &point;
        Ok(text.offset_for_point(point_in_insertion)? - self.start_offset)
    }
}

impl tree::Item for Fragment {
    type Summary = FragmentSummary;

    fn summarize(&self) -> Self::Summary {
        if self.is_visible() {
            let fragment_2d_start = self.insertion
                .text
                .point_for_offset(self.start_offset)
                .unwrap();
            let fragment_2d_end = self.insertion
                .text
                .point_for_offset(self.end_offset)
                .unwrap();

            let first_row_len = if fragment_2d_start.row == fragment_2d_end.row {
                (self.end_offset - self.start_offset) as u32
            } else {
                let first_row_end = self.insertion
                    .text
                    .offset_for_point(Point::new(fragment_2d_start.row + 1, 0))
                    .unwrap() - 1;
                (first_row_end - self.start_offset) as u32
            };
            let (longest_row, longest_row_len) = self.insertion
                .text
                .longest_row_in_range(self.start_offset..self.end_offset)
                .unwrap();
            FragmentSummary {
                extent: self.len(),
                extent_2d: fragment_2d_end - &fragment_2d_start,
                max_fragment_id: self.id.clone(),
                first_row_len,
                longest_row: longest_row - fragment_2d_start.row,
                longest_row_len,
            }
        } else {
            FragmentSummary {
                extent: 0,
                extent_2d: Point { row: 0, column: 0 },
                max_fragment_id: self.id.clone(),
                first_row_len: 0,
                longest_row: 0,
                longest_row_len: 0,
            }
        }
    }
}

impl<'a> AddAssign<&'a FragmentSummary> for FragmentSummary {
    fn add_assign(&mut self, other: &Self) {
        let last_row_len = self.extent_2d.column + other.first_row_len;
        if last_row_len > self.longest_row_len {
            self.longest_row = self.extent_2d.row;
            self.longest_row_len = last_row_len;
        }
        if other.longest_row_len > self.longest_row_len {
            self.longest_row = self.extent_2d.row + other.longest_row;
            self.longest_row_len = other.longest_row_len;
        }

        self.extent += other.extent;
        self.extent_2d += other.extent_2d;
        if self.max_fragment_id < other.max_fragment_id {
            self.max_fragment_id = other.max_fragment_id.clone();
        }
    }
}

impl Default for FragmentSummary {
    fn default() -> Self {
        FragmentSummary {
            extent: 0,
            extent_2d: Point { row: 0, column: 0 },
            max_fragment_id: FragmentId::min_value(),
            first_row_len: 0,
            longest_row: 0,
            longest_row_len: 0,
        }
    }
}

impl tree::Dimension for CharacterCount {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        CharacterCount(summary.extent)
    }
}

impl<'a> Add<&'a Self> for CharacterCount {
    type Output = CharacterCount;

    fn add(self, other: &'a Self) -> Self::Output {
        CharacterCount(self.0 + other.0)
    }
}

impl AddAssign for CharacterCount {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl tree::Item for InsertionSplit {
    type Summary = InsertionSplitSummary;

    fn summarize(&self) -> Self::Summary {
        InsertionSplitSummary {
            extent: self.extent,
        }
    }
}

impl<'a> AddAssign<&'a InsertionSplitSummary> for InsertionSplitSummary {
    fn add_assign(&mut self, other: &Self) {
        self.extent += other.extent;
    }
}

impl Default for InsertionSplitSummary {
    fn default() -> Self {
        InsertionSplitSummary { extent: 0 }
    }
}

impl tree::Dimension for InsertionOffset {
    type Summary = InsertionSplitSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        InsertionOffset(summary.extent)
    }
}

impl<'a> Add<&'a Self> for InsertionOffset {
    type Output = InsertionOffset;

    fn add(self, other: &'a Self) -> Self::Output {
        InsertionOffset(self.0 + other.0)
    }
}

impl AddAssign for InsertionOffset {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl Operation {
    fn replica_id(&self) -> ReplicaId {
        match *self {
            Operation::Edit { ref id, .. } => id.replica_id,
        }
    }
}

fn should_insert_before(
    insertion: &Insertion,
    other_timestamp: LamportTimestamp,
    other_replica_id: ReplicaId,
) -> bool {
    match insertion.timestamp.cmp(&other_timestamp) {
        Ordering::Less => true,
        Ordering::Equal => insertion.id.replica_id < other_replica_id,
        Ordering::Greater => false,
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use self::rand::{Rng, SeedableRng, StdRng};
    use super::*;
    use rpc;
    use std::time::Duration;
    use tokio_core::reactor;
    use IntoShared;

    #[test]
    fn test_edit() {
        let mut buffer = Buffer::new(0);
        buffer.edit(&[0..0], "abc");
        assert_eq!(buffer.to_string(), "abc");
        buffer.edit(&[3..3], "def");
        assert_eq!(buffer.to_string(), "abcdef");
        buffer.edit(&[0..0], "ghi");
        assert_eq!(buffer.to_string(), "ghiabcdef");
        buffer.edit(&[5..5], "jkl");
        assert_eq!(buffer.to_string(), "ghiabjklcdef");
        buffer.edit(&[6..7], "");
        assert_eq!(buffer.to_string(), "ghiabjlcdef");
        buffer.edit(&[4..9], "mno");
        assert_eq!(buffer.to_string(), "ghiamnoef");
    }

    #[test]
    fn test_random_edits() {
        for seed in 0..100 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut buffer = Buffer::new(0);
            let mut reference_string = String::new();

            for _i in 0..10 {
                let mut old_ranges: Vec<Range<usize>> = Vec::new();
                for _ in 0..5 {
                    let last_end = old_ranges.last().map_or(0, |last_range| last_range.end + 1);
                    if last_end > buffer.len() {
                        break;
                    }
                    let end = rng.gen_range::<usize>(last_end, buffer.len() + 1);
                    let start = rng.gen_range::<usize>(last_end, end + 1);
                    old_ranges.push(start..end);
                }
                let new_text = RandomCharIter(rng)
                    .take(rng.gen_range(0, 10))
                    .collect::<String>();

                buffer.edit(&old_ranges, new_text.as_str());
                for old_range in old_ranges.iter().rev() {
                    reference_string = [
                        &reference_string[0..old_range.start],
                        new_text.as_str(),
                        &reference_string[old_range.end..],
                    ].concat();
                }
                assert_eq!(buffer.to_string(), reference_string);
            }
        }
    }

    #[test]
    fn test_len_for_row() {
        let mut buffer = Buffer::new(0);
        buffer.edit(&[0..0], "abcd\nefg\nhij");
        buffer.edit(&[12..12], "kl\nmno");
        buffer.edit(&[18..18], "\npqrs\n");
        buffer.edit(&[18..21], "\nPQ");

        assert_eq!(buffer.len_for_row(0), Ok(4));
        assert_eq!(buffer.len_for_row(1), Ok(3));
        assert_eq!(buffer.len_for_row(2), Ok(5));
        assert_eq!(buffer.len_for_row(3), Ok(3));
        assert_eq!(buffer.len_for_row(4), Ok(4));
        assert_eq!(buffer.len_for_row(5), Ok(0));
        assert_eq!(buffer.len_for_row(6), Err(Error::OffsetOutOfRange));
    }

    #[test]
    fn test_longest_row() {
        let mut buffer = Buffer::new(0);
        assert_eq!(buffer.longest_row(), 0);
        buffer.edit(&[0..0], "abcd\nefg\nhij");
        assert_eq!(buffer.longest_row(), 0);
        buffer.edit(&[12..12], "kl\nmno");
        assert_eq!(buffer.longest_row(), 2);
        buffer.edit(&[18..18], "\npqrs");
        assert_eq!(buffer.longest_row(), 2);
        buffer.edit(&[10..12], "");
        assert_eq!(buffer.longest_row(), 0);
        buffer.edit(&[24..24], "tuv");
        assert_eq!(buffer.longest_row(), 4);
    }

    #[test]
    fn iter_starting_at_point() {
        let mut buffer = Buffer::new(0);
        buffer.edit(&[0..0], "abcd\nefgh\nij");
        buffer.edit(&[12..12], "kl\nmno");
        buffer.edit(&[18..18], "\npqrs");
        buffer.edit(&[18..21], "\nPQ");

        let iter = buffer.iter_starting_at_point(Point::new(0, 0));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "abcd\nefgh\nijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_point(Point::new(1, 0));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "efgh\nijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_point(Point::new(2, 0));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "ijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_point(Point::new(3, 0));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "mno\nPQrs"
        );

        let iter = buffer.iter_starting_at_point(Point::new(4, 0));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "PQrs"
        );

        let iter = buffer.iter_starting_at_point(Point::new(5, 0));
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "");

        // Regression test:
        let mut buffer = Buffer::new(0);
        buffer.edit(&[0..0], "[workspace]\nmembers = [\n    \"xray_core\",\n    \"xray_server\",\n    \"xray_cli\",\n    \"xray_wasm\",\n]\n");
        buffer.edit(&[60..60], "\n");

        let iter = buffer.iter_starting_at_point(Point::new(6, 0));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "    \"xray_wasm\",\n]\n"
        );
    }

    #[test]
    fn backward_iter_starting_at_point() {
        let mut buffer = Buffer::new(0);
        buffer.edit(&[0..0], "abcd\nefgh\nij");
        buffer.edit(&[12..12], "kl\nmno");
        buffer.edit(&[18..18], "\npqrs");
        buffer.edit(&[18..21], "\nPQ");

        let iter = buffer.backward_iter_starting_at_point(Point::new(0, 0));
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "");

        let iter = buffer.backward_iter_starting_at_point(Point::new(0, 3));
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "cba");

        let iter = buffer.backward_iter_starting_at_point(Point::new(1, 4));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "hgfe\ndcba"
        );

        let iter = buffer.backward_iter_starting_at_point(Point::new(3, 2));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "nm\nlkji\nhgfe\ndcba"
        );

        let iter = buffer.backward_iter_starting_at_point(Point::new(4, 4));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "srQP\nonm\nlkji\nhgfe\ndcba"
        );

        let iter = buffer.backward_iter_starting_at_point(Point::new(5, 0));
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "srQP\nonm\nlkji\nhgfe\ndcba"
        );
    }

    #[test]
    fn test_point_for_offset() {
        let text = Text::from("abc\ndefgh\nijklm\nopq");
        assert_eq!(text.point_for_offset(0), Ok(Point { row: 0, column: 0 }));
        assert_eq!(text.point_for_offset(1), Ok(Point { row: 0, column: 1 }));
        assert_eq!(text.point_for_offset(2), Ok(Point { row: 0, column: 2 }));
        assert_eq!(text.point_for_offset(3), Ok(Point { row: 0, column: 3 }));
        assert_eq!(text.point_for_offset(4), Ok(Point { row: 1, column: 0 }));
        assert_eq!(text.point_for_offset(5), Ok(Point { row: 1, column: 1 }));
        assert_eq!(text.point_for_offset(9), Ok(Point { row: 1, column: 5 }));
        assert_eq!(text.point_for_offset(10), Ok(Point { row: 2, column: 0 }));
        assert_eq!(text.point_for_offset(14), Ok(Point { row: 2, column: 4 }));
        assert_eq!(text.point_for_offset(15), Ok(Point { row: 2, column: 5 }));
        assert_eq!(text.point_for_offset(16), Ok(Point { row: 3, column: 0 }));
        assert_eq!(text.point_for_offset(17), Ok(Point { row: 3, column: 1 }));
        assert_eq!(text.point_for_offset(19), Ok(Point { row: 3, column: 3 }));
        assert_eq!(text.point_for_offset(20), Err(Error::OffsetOutOfRange));

        let text = Text::from("abc");
        assert_eq!(text.point_for_offset(0), Ok(Point { row: 0, column: 0 }));
        assert_eq!(text.point_for_offset(1), Ok(Point { row: 0, column: 1 }));
        assert_eq!(text.point_for_offset(2), Ok(Point { row: 0, column: 2 }));
        assert_eq!(text.point_for_offset(3), Ok(Point { row: 0, column: 3 }));
        assert_eq!(text.point_for_offset(4), Err(Error::OffsetOutOfRange));
    }

    #[test]
    fn test_offset_for_point() {
        let text = Text::from("abc\ndefgh");
        assert_eq!(text.offset_for_point(Point { row: 0, column: 0 }), Ok(0));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 1 }), Ok(1));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 2 }), Ok(2));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 3 }), Ok(3));
        assert_eq!(
            text.offset_for_point(Point { row: 0, column: 4 }),
            Err(Error::OffsetOutOfRange)
        );
        assert_eq!(text.offset_for_point(Point { row: 1, column: 0 }), Ok(4));
        assert_eq!(text.offset_for_point(Point { row: 1, column: 1 }), Ok(5));
        assert_eq!(text.offset_for_point(Point { row: 1, column: 5 }), Ok(9));
        assert_eq!(
            text.offset_for_point(Point { row: 1, column: 6 }),
            Err(Error::OffsetOutOfRange)
        );

        let text = Text::from("abc");
        assert_eq!(text.offset_for_point(Point { row: 0, column: 0 }), Ok(0));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 1 }), Ok(1));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 2 }), Ok(2));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 3 }), Ok(3));
        assert_eq!(
            text.offset_for_point(Point { row: 0, column: 4 }),
            Err(Error::OffsetOutOfRange)
        );
    }

    #[test]
    fn test_longest_row_in_range() {
        for seed in 0..100 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);
            let string = RandomCharIter(rng)
                .take(rng.gen_range(1, 10))
                .collect::<String>();
            let text = Text::from(string.as_ref());

            for _i in 0..10 {
                let end = rng.gen_range(1, string.len() + 1);
                let start = rng.gen_range(0, end);

                let mut cur_row = string[0..start].chars().filter(|c| *c == '\n').count() as u32;
                let mut cur_row_len = 0;
                let mut expected_longest_row = cur_row;
                let mut expected_longest_row_len = cur_row_len;
                for ch in string[start..end].chars() {
                    if ch == '\n' {
                        if cur_row_len > expected_longest_row_len {
                            expected_longest_row = cur_row;
                            expected_longest_row_len = cur_row_len;
                        }
                        cur_row += 1;
                        cur_row_len = 0;
                    } else {
                        cur_row_len += 1;
                    }
                }
                if cur_row_len > expected_longest_row_len {
                    expected_longest_row = cur_row;
                    expected_longest_row_len = cur_row_len;
                }

                assert_eq!(
                    text.longest_row_in_range(start..end),
                    Ok((expected_longest_row, expected_longest_row_len))
                );
            }
        }
    }

    #[test]
    fn fragment_ids() {
        for seed in 0..10 {
            use self::rand::{Rng, SeedableRng, StdRng};
            let mut rng = StdRng::from_seed(&[seed]);

            let mut ids = vec![FragmentId(Arc::new(vec![0])), FragmentId(Arc::new(vec![4]))];
            for _i in 0..100 {
                let index = rng.gen_range::<usize>(1, ids.len());

                let left = ids[index - 1].clone();
                let right = ids[index].clone();
                ids.insert(index, FragmentId::between_with_max(&left, &right, 4));

                let mut sorted_ids = ids.clone();
                sorted_ids.sort();
                assert_eq!(ids, sorted_ids);
            }
        }
    }

    #[test]
    fn test_anchors() {
        let mut buffer = Buffer::new(0);
        buffer.edit(&[0..0], "abc");
        let left_anchor = buffer.anchor_before_offset(2).unwrap();
        let right_anchor = buffer.anchor_after_offset(2).unwrap();

        buffer.edit(&[1..1], "def\n");
        assert_eq!(buffer.to_string(), "adef\nbc");
        assert_eq!(buffer.offset_for_anchor(&left_anchor).unwrap(), 6);
        assert_eq!(buffer.offset_for_anchor(&right_anchor).unwrap(), 6);
        assert_eq!(
            buffer.point_for_anchor(&left_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );
        assert_eq!(
            buffer.point_for_anchor(&right_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );

        buffer.edit(&[2..3], "");
        assert_eq!(buffer.to_string(), "adf\nbc");
        assert_eq!(buffer.offset_for_anchor(&left_anchor).unwrap(), 5);
        assert_eq!(buffer.offset_for_anchor(&right_anchor).unwrap(), 5);
        assert_eq!(
            buffer.point_for_anchor(&left_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );
        assert_eq!(
            buffer.point_for_anchor(&right_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );

        buffer.edit(&[5..5], "ghi\n");
        assert_eq!(buffer.to_string(), "adf\nbghi\nc");
        assert_eq!(buffer.offset_for_anchor(&left_anchor).unwrap(), 5);
        assert_eq!(buffer.offset_for_anchor(&right_anchor).unwrap(), 9);
        assert_eq!(
            buffer.point_for_anchor(&left_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );
        assert_eq!(
            buffer.point_for_anchor(&right_anchor).unwrap(),
            Point { row: 2, column: 0 }
        );

        buffer.edit(&[7..9], "");
        assert_eq!(buffer.to_string(), "adf\nbghc");
        assert_eq!(buffer.offset_for_anchor(&left_anchor).unwrap(), 5);
        assert_eq!(buffer.offset_for_anchor(&right_anchor).unwrap(), 7);
        assert_eq!(
            buffer.point_for_anchor(&left_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );
        assert_eq!(
            buffer.point_for_anchor(&right_anchor).unwrap(),
            Point { row: 1, column: 3 }
        );

        // Ensure anchoring to a point is equivalent to anchoring to an offset.
        assert_eq!(
            buffer.anchor_before_point(Point { row: 0, column: 0 }),
            buffer.anchor_before_offset(0)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 0, column: 1 }),
            buffer.anchor_before_offset(1)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 0, column: 2 }),
            buffer.anchor_before_offset(2)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 0, column: 3 }),
            buffer.anchor_before_offset(3)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 0 }),
            buffer.anchor_before_offset(4)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 1 }),
            buffer.anchor_before_offset(5)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 2 }),
            buffer.anchor_before_offset(6)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 3 }),
            buffer.anchor_before_offset(7)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 4 }),
            buffer.anchor_before_offset(8)
        );

        // Comparison between anchors.
        let anchor_at_offset_0 = buffer.anchor_before_offset(0).unwrap();
        let anchor_at_offset_1 = buffer.anchor_before_offset(1).unwrap();
        let anchor_at_offset_2 = buffer.anchor_before_offset(2).unwrap();

        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_0, &anchor_at_offset_0),
            Ok(Ordering::Equal)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_1, &anchor_at_offset_1),
            Ok(Ordering::Equal)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_2, &anchor_at_offset_2),
            Ok(Ordering::Equal)
        );

        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_0, &anchor_at_offset_1),
            Ok(Ordering::Less)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_1, &anchor_at_offset_2),
            Ok(Ordering::Less)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_0, &anchor_at_offset_2),
            Ok(Ordering::Less)
        );

        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_1, &anchor_at_offset_0),
            Ok(Ordering::Greater)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_2, &anchor_at_offset_1),
            Ok(Ordering::Greater)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_2, &anchor_at_offset_0),
            Ok(Ordering::Greater)
        );
    }

    #[test]
    fn anchors_at_start_and_end() {
        let mut buffer = Buffer::new(0);
        let before_start_anchor = buffer.anchor_before_offset(0).unwrap();
        let after_end_anchor = buffer.anchor_after_offset(0).unwrap();

        buffer.edit(&[0..0], "abc");
        assert_eq!(buffer.to_string(), "abc");
        assert_eq!(buffer.offset_for_anchor(&before_start_anchor).unwrap(), 0);
        assert_eq!(buffer.offset_for_anchor(&after_end_anchor).unwrap(), 3);

        let after_start_anchor = buffer.anchor_after_offset(0).unwrap();
        let before_end_anchor = buffer.anchor_before_offset(3).unwrap();

        buffer.edit(&[3..3], "def");
        buffer.edit(&[0..0], "ghi");
        assert_eq!(buffer.to_string(), "ghiabcdef");
        assert_eq!(buffer.offset_for_anchor(&before_start_anchor).unwrap(), 0);
        assert_eq!(buffer.offset_for_anchor(&after_start_anchor).unwrap(), 3);
        assert_eq!(buffer.offset_for_anchor(&before_end_anchor).unwrap(), 6);
        assert_eq!(buffer.offset_for_anchor(&after_end_anchor).unwrap(), 9);
    }

    #[test]
    fn test_snapshot() {
        let mut buffer = Buffer::new(0);
        buffer.edit(&[0..0], "abcdefghi");
        buffer.edit(&[3..6], "DEF");

        let snapshot = buffer.snapshot();
        assert_eq!(buffer.to_string(), String::from("abcDEFghi"));
        assert_eq!(snapshot.to_string(), String::from("abcDEFghi"));

        buffer.edit(&[0..1], "A");
        buffer.edit(&[8..9], "I");
        assert_eq!(buffer.to_string(), String::from("AbcDEFghI"));
        assert_eq!(snapshot.to_string(), String::from("abcDEFghi"));
    }

    #[test]
    fn test_random_concurrent_edits() {
        for seed in 0..100 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let site_range = 0..5;
            let mut buffers = Vec::new();
            let mut queues = Vec::new();
            for i in site_range.clone() {
                let mut buffer = Buffer::new(0);
                buffer.replica_id = i + 1;
                buffers.push(buffer);
                queues.push(Vec::new());
            }

            let mut edit_count = 10;
            loop {
                let replica_index = rng.gen_range::<usize>(site_range.start, site_range.end);
                let buffer = &mut buffers[replica_index];
                if edit_count > 0 && rng.gen() {
                    let mut old_ranges: Vec<Range<usize>> = Vec::new();
                    for _ in 0..5 {
                        let last_end = old_ranges.last().map_or(0, |last_range| last_range.end + 1);
                        if last_end > buffer.len() {
                            break;
                        }
                        let end = rng.gen_range::<usize>(last_end, buffer.len() + 1);
                        let start = rng.gen_range::<usize>(last_end, end + 1);
                        old_ranges.push(start..end);
                    }
                    let new_text = RandomCharIter(rng)
                        .take(rng.gen_range(0, 10))
                        .collect::<String>();

                    for op in buffer.edit(&old_ranges, new_text.as_str()) {
                        for (index, queue) in queues.iter_mut().enumerate() {
                            if index != replica_index {
                                queue.push(op.clone());
                            }
                        }
                    }

                    edit_count -= 1;
                } else if !queues[replica_index].is_empty() {
                    buffer
                        .integrate_op(queues[replica_index].remove(0))
                        .unwrap();
                }

                if edit_count == 0 && queues.iter().all(|q| q.is_empty()) {
                    break;
                }
            }

            for buffer in &buffers[1..] {
                assert_eq!(buffer.to_string(), buffers[0].to_string());
            }
        }
    }

    #[test]
    fn test_edit_replication() {
        let local_buffer = Buffer::new(0).into_shared();
        local_buffer.borrow_mut().edit(&[0..0], "abcdef");
        local_buffer.borrow_mut().edit(&[2..4], "ghi");

        let mut reactor = reactor::Core::new().unwrap();
        let foreground = Rc::new(reactor.handle());
        let client_1 =
            rpc::tests::connect(&mut reactor, super::rpc::Service::new(local_buffer.clone()));
        let remote_buffer_1 = Buffer::remote(foreground.clone(), client_1).unwrap();
        let client_2 =
            rpc::tests::connect(&mut reactor, super::rpc::Service::new(local_buffer.clone()));
        let remote_buffer_2 = Buffer::remote(foreground, client_2).unwrap();
        assert_eq!(
            remote_buffer_1.borrow().to_string(),
            local_buffer.borrow().to_string()
        );
        assert_eq!(
            remote_buffer_2.borrow().to_string(),
            local_buffer.borrow().to_string()
        );

        local_buffer.borrow_mut().edit(&[3..6], "jk");
        remote_buffer_1.borrow_mut().edit(&[7..7], "lmn");
        let anchor = remote_buffer_1.borrow().anchor_before_offset(8).unwrap();

        let mut remaining_tries = 10;
        while remote_buffer_1.borrow().to_string() != local_buffer.borrow().to_string()
            || remote_buffer_2.borrow().to_string() != local_buffer.borrow().to_string()
        {
            remaining_tries -= 1;
            assert!(
                remaining_tries > 0,
                "Ran out of patience waiting for buffers to converge"
            );
            reactor.turn(Some(Duration::from_millis(0)));
        }

        assert_eq!(local_buffer.borrow().offset_for_anchor(&anchor).unwrap(), 7);
        assert_eq!(
            remote_buffer_1.borrow().offset_for_anchor(&anchor).unwrap(),
            7
        );
        assert_eq!(
            remote_buffer_2.borrow().offset_for_anchor(&anchor).unwrap(),
            7
        );
    }

    #[test]
    fn test_selection_replication() {
        use stream_ext::StreamExt;

        let mut buffer_1 = Buffer::new(0);
        buffer_1.edit(&[0..0], "abcdef");
        let sels = vec![empty_selection(&buffer_1, 1), empty_selection(&buffer_1, 3)];
        buffer_1.add_selection_set(0, sels);
        let sels = vec![empty_selection(&buffer_1, 2), empty_selection(&buffer_1, 4)];
        let buffer_1_set_id = buffer_1.add_selection_set(0, sels);
        let buffer_1 = buffer_1.into_shared();

        let mut reactor = reactor::Core::new().unwrap();
        let foreground = Rc::new(reactor.handle());
        let buffer_2 = Buffer::remote(
            foreground.clone(),
            rpc::tests::connect(&mut reactor, super::rpc::Service::new(buffer_1.clone())),
        ).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_2));

        let buffer_3 = Buffer::remote(
            foreground,
            rpc::tests::connect(&mut reactor, super::rpc::Service::new(buffer_1.clone())),
        ).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_3));

        let mut buffer_1_updates = buffer_1.borrow().updates();
        let mut buffer_2_updates = buffer_2.borrow().updates();
        let mut buffer_3_updates = buffer_3.borrow().updates();

        buffer_1
            .borrow_mut()
            .mutate_selections(buffer_1_set_id, |buffer, selections| {
                for selection in selections {
                    selection.start = buffer
                        .anchor_before_offset(
                            buffer.offset_for_anchor(&selection.start).unwrap() + 1,
                        )
                        .unwrap();
                }
            })
            .unwrap();
        buffer_2_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_3));
        buffer_3_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_3));

        buffer_1
            .borrow_mut()
            .remove_selection_set(buffer_1_set_id)
            .unwrap();
        buffer_1_updates.wait_next(&mut reactor).unwrap();

        buffer_2_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_2));
        buffer_3_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_3));

        let sels = vec![empty_selection(&buffer_2.borrow(), 1)];
        let buffer_2_set_id = buffer_2.borrow_mut().add_selection_set(0, sels);
        buffer_2_updates.wait_next(&mut reactor).unwrap();

        buffer_1_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_2));
        buffer_3_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_3));

        buffer_2
            .borrow_mut()
            .mutate_selections(buffer_2_set_id, |buffer, selections| {
                for selection in selections {
                    selection.start = buffer
                        .anchor_before_offset(
                            buffer.offset_for_anchor(&selection.start).unwrap() + 1,
                        )
                        .unwrap();
                }
            })
            .unwrap();

        buffer_1_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_2), selections(&buffer_1));
        buffer_3_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_2), selections(&buffer_3));

        buffer_2
            .borrow_mut()
            .remove_selection_set(buffer_2_set_id)
            .unwrap();
        buffer_2_updates.wait_next(&mut reactor).unwrap();

        buffer_1_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_2));
        buffer_3_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(selections(&buffer_1), selections(&buffer_3));

        drop(buffer_3);
        buffer_1_updates.wait_next(&mut reactor).unwrap();
        for (replica_id, _, _) in selections(&buffer_1) {
            assert_eq!(buffer_1.borrow().replica_id, replica_id);
        }
    }

    struct RandomCharIter<T: Rng>(T);

    impl<T: Rng> Iterator for RandomCharIter<T> {
        type Item = char;

        fn next(&mut self) -> Option<Self::Item> {
            if self.0.gen_weighted_bool(5) {
                Some('\n')
            } else {
                Some(self.0.gen_range(b'a', b'z' + 1).into())
            }
        }
    }

    fn selections(buffer: &Rc<RefCell<Buffer>>) -> Vec<(ReplicaId, SelectionSetId, Selection)> {
        let buffer = buffer.borrow();

        let mut selections = Vec::new();
        for ((replica_id, set_id), selection_set) in &buffer.selections {
            for selection in selection_set.selections.iter() {
                selections.push((*replica_id, *set_id, selection.clone()));
            }
        }
        selections.sort_by(|a, b| match a.0.cmp(&b.0) {
            Ordering::Equal => a.1.cmp(&b.1),
            comparison @ _ => comparison,
        });

        selections
    }

    fn empty_selection(buffer: &Buffer, offset: usize) -> Selection {
        let anchor = buffer.anchor_before_offset(offset).unwrap();
        Selection {
            start: anchor.clone(),
            end: anchor,
            reversed: false,
            goal_column: None,
        }
    }
}
