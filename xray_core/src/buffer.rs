use super::rpc::{client, Error as RpcError};
use super::tree::{self, SeekBias, Tree};
use futures::{unsync, Stream};
use notify_cell::{NotifyCell, NotifyCellObserver};
use serde::{self, Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::cmp;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::iter;
use std::marker;
use std::ops::{Add, AddAssign, Deref, DerefMut, Range, Sub};
use std::rc::Rc;
use std::rc::Weak;
use std::sync::Arc;
use ForegroundExecutor;
use IntoShared;

pub type ReplicaId = usize;
type LocalTimestamp = usize;
type LamportTimestamp = usize;
type SelectionSetId = usize;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Version(
    #[serde(serialize_with = "serialize_arc", deserialize_with = "deserialize_arc")]
    Arc<HashMap<ReplicaId, LocalTimestamp>>,
);

#[derive(Eq, PartialEq, Debug)]
pub enum Error {
    OffsetOutOfRange,
    InvalidAnchor,
    InvalidOperation,
}

pub struct Buffer {
    replica_id: ReplicaId,
    next_replica_id: Option<ReplicaId>,
    local_clock: LocalTimestamp,
    lamport_clock: LamportTimestamp,
    fragments: Tree<Fragment>,
    insertion_splits: HashMap<EditId, Tree<InsertionSplit>>,
    anchor_cache: RefCell<HashMap<Anchor, (usize, Point)>>,
    offset_cache: RefCell<HashMap<Point, usize>>,
    pub version: Version,
    client: Option<client::Service<rpc::Service>>,
    operation_txs: Vec<unsync::mpsc::UnboundedSender<Arc<Operation>>>,
    updates: NotifyCell<()>,
    local_selections: HashMap<SelectionSetId, Weak<RefCell<SelectionSet>>>,
    remote_selections: HashMap<(ReplicaId, SelectionSetId), Rc<RefCell<SelectionSet>>>,
    next_local_selection_set_id: SelectionSetId,
    updated_selections_txs: Vec<unsync::mpsc::UnboundedSender<(ReplicaId, SelectionSetId)>>,
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

pub struct SelectionSet {
    id: SelectionSetId,
    replica_id: ReplicaId,
    selections: Vec<Selection>,
    buffer: Weak<RefCell<Buffer>>,
}

pub struct Iter<'a> {
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
    newline_offsets: Vec<usize>,
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
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy, Debug)]
struct CharacterCount(usize);

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy, Debug)]
struct NewlineCount(usize);

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

    fn inc(&mut self, replica_id: ReplicaId) {
        let map = Arc::make_mut(&mut self.0);
        *map.entry(replica_id).or_insert(0) += 1;
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
    use super::{Buffer, EditId, FragmentId, Insertion, InsertionSplit, Operation, ReplicaId,
                Selection, SelectionSetId, Version};
    use futures::{Async, Future, Stream};
    use never::Never;
    use rpc;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;
    use std::sync::Arc;

    #[derive(Serialize, Deserialize)]
    pub struct State {
        pub(super) replica_id: ReplicaId,
        pub(super) fragments: Vec<Fragment>,
        pub(super) insertions: HashMap<EditId, Insertion>,
        pub(super) insertion_splits: HashMap<EditId, Vec<InsertionSplit>>,
        pub(super) version: Version,
        pub(super) selections: HashMap<(ReplicaId, SelectionSetId), Vec<Selection>>,
    }

    #[derive(Serialize, Deserialize)]
    pub enum Request {
        Operation(
            #[serde(serialize_with = "serialize_op", deserialize_with = "deserialize_op")]
            Arc<Operation>,
        ),
        UpdateSelectionSet(SelectionSetId, Vec<Selection>),
        RemoveSelectionSet(SelectionSetId),
    }

    #[derive(Serialize, Deserialize)]
    pub enum Update {
        Operation(
            #[serde(serialize_with = "serialize_op", deserialize_with = "deserialize_op")]
            Arc<Operation>,
        ),
        Selections {
            updated: HashMap<(ReplicaId, SelectionSetId), Vec<Selection>>,
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
        outgoing_ops: Box<Stream<Item = Arc<Operation>, Error = ()>>,
        selection_updates: Box<Stream<Item = (ReplicaId, SelectionSetId), Error = ()>>,
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
            let selection_updates = buffer
                .borrow_mut()
                .selection_updates()
                .filter(move |update| update.0 != replica_id);
            Self {
                replica_id,
                outgoing_ops: Box::new(outgoing_ops),
                selection_updates: Box::new(selection_updates),
                buffer,
            }
        }

        fn poll_outgoing_op(&mut self) -> Async<Option<Update>> {
            self.outgoing_ops
                .poll()
                .expect("Receiving on a channel cannot produce an error")
                .map(|option| option.map(|update| Update::Operation(update)))
        }

        fn poll_outgoing_selection(&mut self) -> Async<Option<Update>> {
            let mut updated = HashMap::new();
            let mut removed = HashSet::new();
            loop {
                match self.selection_updates
                    .poll()
                    .expect("Receiving on a channel cannot produce an error")
                {
                    Async::Ready(Some((replica_id, set_id))) => {
                        let buffer = self.buffer.borrow();
                        let selection_set = if buffer.replica_id == replica_id {
                            buffer
                                .local_selections
                                .get(&set_id)
                                .and_then(|set| set.upgrade())
                        } else {
                            buffer.remote_selections.get(&(replica_id, set_id)).cloned()
                        };
                        if let Some(selection_set) = selection_set {
                            updated
                                .entry((replica_id, set_id))
                                .or_insert_with(|| selection_set.borrow().clone());
                        } else {
                            removed.insert((replica_id, set_id));
                        }
                    }
                    Async::Ready(None) => if updated.is_empty() && removed.is_empty() {
                        return Async::Ready(None);
                    } else {
                        break;
                    },
                    Async::NotReady => if updated.is_empty() && removed.is_empty() {
                        return Async::NotReady;
                    } else {
                        break;
                    },
                }
            }
            Async::Ready(Some(Update::Selections { updated, removed }))
        }
    }

    impl rpc::server::Service for Service {
        type State = State;
        type Update = Update;
        type Request = Request;
        type Response = ();

        fn init(&mut self, _: &rpc::server::Connection) -> Self::State {
            let buffer = self.buffer.borrow_mut();
            let mut state = State {
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

            for (id, selection_set) in &buffer.local_selections {
                if let Some(selection_set) = selection_set.upgrade() {
                    state
                        .selections
                        .insert((buffer.replica_id, *id), selection_set.borrow().clone());
                }
            }
            for (id, selection_set) in &buffer.remote_selections {
                state.selections.insert(*id, selection_set.borrow().clone());
            }

            state
        }

        fn poll_update(&mut self, _: &rpc::server::Connection) -> Async<Option<Self::Update>> {
            match self.poll_outgoing_op() {
                Async::Ready(Some(update)) => Async::Ready(Some(update)),
                Async::Ready(None) => match self.poll_outgoing_selection() {
                    Async::Ready(Some(update)) => Async::Ready(Some(update)),
                    Async::Ready(None) => Async::Ready(None),
                    Async::NotReady => Async::NotReady,
                },
                Async::NotReady => match self.poll_outgoing_selection() {
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
                }
                Request::UpdateSelectionSet(set_id, selections) => {
                    Buffer::integrate_selections_update(
                        &self.buffer,
                        self.replica_id,
                        set_id,
                        selections,
                    );
                }
                Request::RemoveSelectionSet(set_id) => {
                    Buffer::integrate_selections_removal(&self.buffer, self.replica_id, set_id);
                }
            };

            None
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
    pub fn new() -> Self {
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
            replica_id: 1,
            next_replica_id: Some(2),
            local_clock: 0,
            lamport_clock: 0,
            fragments,
            insertion_splits,
            anchor_cache: RefCell::new(HashMap::new()),
            offset_cache: RefCell::new(HashMap::new()),
            version: Version::new(),
            client: None,
            operation_txs: Vec::new(),
            updates: NotifyCell::new(()),
            local_selections: HashMap::new(),
            remote_selections: HashMap::new(),
            next_local_selection_set_id: 0,
            updated_selections_txs: Vec::new(),
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

        let buffer = Buffer {
            replica_id: state.replica_id,
            next_replica_id: None,
            local_clock: 0,
            lamport_clock: 0,
            fragments,
            insertion_splits,
            anchor_cache: RefCell::new(HashMap::new()),
            offset_cache: RefCell::new(HashMap::new()),
            version: state.version,
            client: Some(client),
            operation_txs: Vec::new(),
            updates: NotifyCell::new(()),
            local_selections: HashMap::new(),
            remote_selections: HashMap::new(),
            next_local_selection_set_id: 0,
            updated_selections_txs: Vec::new(),
        }.into_shared();

        for ((replica_id, set_id), selections) in state.selections {
            let selection_set = Buffer::add_remote_selection_set(&buffer, replica_id, set_id);
            **selection_set.borrow_mut() = selections;
        }

        let buffer_clone = buffer.clone();
        foreground
            .execute(Box::new(incoming_updates.for_each(move |update| {
                match update {
                    rpc::Update::Operation(operation) => {
                        if buffer_clone.borrow_mut().integrate_op(operation).is_err() {
                            unimplemented!("Invalid op");
                        }
                    }
                    rpc::Update::Selections { updated, removed } => {
                        for ((replica_id, set_id), selections) in updated {
                            Buffer::integrate_selections_update(
                                &buffer_clone,
                                replica_id,
                                set_id,
                                selections,
                            );
                        }

                        for (replica_id, set_id) in removed {
                            Buffer::integrate_selections_removal(&buffer_clone, replica_id, set_id);
                        }
                    }
                }

                Ok(())
            })))
            .unwrap();

        Ok(buffer)
    }

    pub fn next_replica_id(&mut self) -> Result<ReplicaId, ()> {
        let replica_id = self.next_replica_id.ok_or(())?;
        self.next_replica_id = Some(replica_id + 1);
        Ok(replica_id)
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

    pub fn max_point(&self) -> Point {
        self.fragments.len::<Point>()
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

    pub fn iter_starting_at_row(&self, row: u32) -> Iter {
        Iter::starting_at_row(self, row)
    }

    pub fn edit<T: Into<Text>>(
        &mut self,
        old_range: Range<usize>,
        new_text: T,
    ) -> Option<Arc<Operation>> {
        let new_text = new_text.into();
        let new_text = if new_text.len() > 0 {
            Some(Arc::new(new_text))
        } else {
            None
        };

        if new_text.is_some() || old_range.end > old_range.start {
            let op = Arc::new(self.splice_fragments(old_range, new_text));
            self.anchor_cache.borrow_mut().clear();
            self.offset_cache.borrow_mut().clear();
            self.version.inc(self.replica_id);
            self.broadcast_op(&op);
            self.updates.set(());
            Some(op)
        } else {
            None
        }
    }

    pub fn add_local_selection_set(buffer_rc: &Rc<RefCell<Buffer>>) -> Rc<RefCell<SelectionSet>> {
        let mut buffer = buffer_rc.borrow_mut();
        let buffer = &mut *buffer;

        let id = buffer.next_local_selection_set_id;
        buffer.next_local_selection_set_id += 1;

        let set = Rc::new(RefCell::new(SelectionSet {
            id,
            replica_id: buffer.replica_id,
            selections: Vec::new(),
            buffer: Rc::downgrade(&buffer_rc),
        }));
        buffer.local_selections.insert(id, Rc::downgrade(&set));

        for updated_selections_tx in &buffer.updated_selections_txs {
            let _ = updated_selections_tx.unbounded_send((buffer.replica_id, id));
        }
        buffer.updates.set(());

        set
    }

    fn add_remote_selection_set(
        buffer_rc: &Rc<RefCell<Buffer>>,
        replica_id: ReplicaId,
        set_id: SelectionSetId,
    ) -> Rc<RefCell<SelectionSet>> {
        let mut buffer = buffer_rc.borrow_mut();

        let set = Rc::new(RefCell::new(SelectionSet {
            id: set_id,
            replica_id: replica_id,
            selections: Vec::new(),
            buffer: Rc::downgrade(&buffer_rc),
        }));
        buffer
            .remote_selections
            .insert((replica_id, set_id), set.clone());

        for updated_selections_tx in &buffer.updated_selections_txs {
            let _ = updated_selections_tx.unbounded_send((replica_id, set_id));
        }
        buffer.updates.set(());

        set
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
        let mut new_fragments = cursor.build_prefix(&start_fragment_id, SeekBias::Left);

        if start_offset == cursor.item().unwrap().end_offset {
            new_fragments.push(cursor.item().unwrap().clone());
            cursor.next();
        }

        while cursor.item().is_some() {
            let fragment = cursor.item().unwrap();

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

        new_fragments.push_tree(cursor.build_suffix());
        self.fragments = new_fragments;
        self.lamport_clock = cmp::max(self.lamport_clock, timestamp) + 1;
        Ok(())
    }

    fn integrate_selections_update(
        buffer: &Rc<RefCell<Buffer>>,
        replica_id: ReplicaId,
        set_id: SelectionSetId,
        selections: Vec<Selection>,
    ) {
        let selection_set = {
            buffer
                .borrow()
                .remote_selections
                .get(&(replica_id, set_id))
                .cloned()
        };
        if let Some(selection_set) = selection_set {
            let mut selection_set = selection_set.borrow_mut();
            **selection_set = selections;
            selection_set.updated();
        } else {
            let selection_set = Buffer::add_remote_selection_set(&buffer, replica_id, set_id);
            **selection_set.borrow_mut() = selections;
        }
    }

    fn integrate_selections_removal(
        buffer: &Rc<RefCell<Buffer>>,
        replica_id: ReplicaId,
        set_id: SelectionSetId,
    ) {
        // Force the compiler to drop SelectionSet after dropping the mutable borrow to the buffer.
        // This prevents a double borrow error caused by SelectionSet's Drop trait.
        let _ = {
            let mut buffer = buffer.borrow_mut();
            buffer.remote_selections.remove(&(replica_id, set_id))
        };
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

    fn selection_updates(
        &mut self,
    ) -> unsync::mpsc::UnboundedReceiver<(ReplicaId, SelectionSetId)> {
        let (tx, rx) = unsync::mpsc::unbounded();
        self.updated_selections_txs.push(tx);
        rx
    }

    fn splice_fragments(
        &mut self,
        old_range: Range<usize>,
        new_text: Option<Arc<Text>>,
    ) -> Operation {
        self.local_clock += 1;
        self.lamport_clock += 1;
        let lamport_timestamp = self.lamport_clock;

        let edit_id = EditId {
            replica_id: self.replica_id,
            timestamp: self.local_clock,
        };

        let old_fragments = self.fragments.clone();
        let mut cursor = old_fragments.cursor();
        let mut new_fragments =
            cursor.build_prefix(&CharacterCount(old_range.start), SeekBias::Right);

        let mut start_id = None;
        let mut start_offset = None;
        let mut end_id = None;
        let mut end_offset = None;
        let mut version_in_range = Version::new();

        if cursor.item().is_none() {
            let prev_fragment = cursor.prev_item().unwrap();
            start_id = Some(prev_fragment.insertion.id);
            start_offset = Some(prev_fragment.end_offset);
            end_id = start_id.clone();
            end_offset = start_offset.clone();
            if let Some(new_text) = new_text.clone() {
                new_fragments.push(self.build_fragment_to_insert(
                    edit_id,
                    prev_fragment,
                    None,
                    new_text,
                    lamport_timestamp,
                ));
            }
        } else {
            let mut fragment_start = cursor.start::<CharacterCount>().0;
            while cursor.item().is_some() && fragment_start <= old_range.end {
                let fragment = cursor.item().unwrap();
                let fragment_end = fragment_start + fragment.len();

                let split_start = if old_range.start > fragment_start {
                    fragment.start_offset + (old_range.start - fragment_start)
                } else {
                    fragment.start_offset
                };
                let split_end = if old_range.end < fragment_end {
                    fragment.start_offset + (old_range.end - fragment_start)
                } else {
                    fragment.end_offset
                };

                if old_range.start == fragment_start {
                    let prev_fragment = cursor.prev_item().unwrap();
                    start_id = Some(prev_fragment.insertion.id);
                    start_offset = Some(prev_fragment.end_offset);
                } else if old_range.start > fragment_start {
                    start_id = Some(fragment.insertion.id);
                    start_offset = Some(split_start);
                }

                if old_range.end == fragment_start {
                    let prev_fragment = cursor.prev_item().unwrap();
                    end_id = Some(prev_fragment.insertion.id);
                    end_offset = Some(prev_fragment.end_offset);
                } else if old_range.end <= fragment_end {
                    end_id = Some(fragment.insertion.id);
                    end_offset = Some(split_end);
                }

                let (before_range, within_range, after_range) = self.split_fragment(
                    cursor.prev_item().unwrap(),
                    fragment,
                    split_start..split_end,
                );
                let insertion = if new_text.is_some() && old_range.start >= fragment_start {
                    Some(self.build_fragment_to_insert(
                        edit_id,
                        before_range.as_ref().or(cursor.prev_item()).unwrap(),
                        within_range.as_ref().or(after_range.as_ref()),
                        new_text.clone().unwrap(),
                        lamport_timestamp,
                    ))
                } else {
                    None
                };
                if let Some(fragment) = before_range {
                    new_fragments.push(fragment);
                }
                if let Some(fragment) = insertion {
                    new_fragments.push(fragment);
                }
                if let Some(mut fragment) = within_range {
                    if fragment.is_visible() {
                        fragment.deletions.insert(edit_id.clone());
                        version_in_range.include(&fragment.insertion);
                    }
                    new_fragments.push(fragment);
                }
                if let Some(fragment) = after_range {
                    new_fragments.push(fragment);
                }

                fragment_start = fragment_end;
                cursor.next();
            }
        }

        new_fragments.push_tree(cursor.build_suffix());
        self.fragments = new_fragments;

        Operation::Edit {
            id: edit_id,
            start_id: start_id.unwrap(),
            start_offset: start_offset.unwrap(),
            end_id: end_id.unwrap(),
            end_offset: end_offset.unwrap(),
            new_text,
            version_in_range,
            timestamp: self.lamport_clock,
        }
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
                cursor.build_prefix(&InsertionOffset(fragment.start_offset), SeekBias::Right);

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
            new_split_tree.push_tree(cursor.build_suffix());

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

    pub fn cmp_anchors(&self, a: &Anchor, b: &Anchor) -> Result<cmp::Ordering, Error> {
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

impl Point {
    pub fn new(row: u32, column: u32) -> Self {
        Point { row, column }
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
    fn partial_cmp(&self, other: &Point) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Point {
    #[cfg(target_pointer_width = "64")]
    fn cmp(&self, other: &Point) -> cmp::Ordering {
        let a = (self.row as usize) << 32 | self.column as usize;
        let b = (other.row as usize) << 32 | other.column as usize;
        a.cmp(&b)
    }

    #[cfg(target_pointer_width = "32")]
    fn cmp(&self, other: &Point) -> cmp::Ordering {
        match self.row.cmp(&other.row) {
            cmp::Ordering::Equal => self.column.cmp(&other.column),
            comparison @ _ => comparison,
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

    fn starting_at_row(buffer: &'a Buffer, target_row: u32) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(
            &Point {
                row: target_row,
                column: 0,
            },
            SeekBias::Right,
        );

        let mut fragment_offset = 0;
        if let Some(fragment) = fragment_cursor.item() {
            let fragment_start_row = fragment_cursor.start::<Point>().row;
            if target_row != fragment_start_row {
                let target_row_within_fragment = target_row - fragment_start_row - 1;
                fragment_offset = fragment.insertion.text.newline_offsets
                    [target_row_within_fragment as usize] + 1;
            }
        }

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

impl SelectionSet {
    pub fn updated(&self) {
        if let Some(buffer) = self.buffer.upgrade() {
            let buffer = buffer.borrow();
            for updated_selections_tx in &buffer.updated_selections_txs {
                let _ = updated_selections_tx.unbounded_send((self.replica_id, self.id));
            }

            if let Some(ref client) = buffer.client {
                client.request(rpc::Request::UpdateSelectionSet(
                    self.id,
                    self.selections.clone(),
                ));
            }

            buffer.updates.set(());
        }
    }
}

impl Deref for SelectionSet {
    type Target = Vec<Selection>;

    fn deref(&self) -> &Self::Target {
        &self.selections
    }
}

impl<'a> DerefMut for SelectionSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.selections
    }
}

impl Drop for SelectionSet {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.upgrade() {
            let mut buffer = buffer.borrow_mut();
            if buffer.replica_id == self.replica_id {
                buffer.local_selections.remove(&self.id);
            } else {
                buffer.remote_selections.remove(&(self.replica_id, self.id));
            }

            for updated_selections_tx in &buffer.updated_selections_txs {
                let _ = updated_selections_tx.unbounded_send((self.replica_id, self.id));
            }

            if let Some(ref client) = buffer.client {
                client.request(rpc::Request::RemoveSelectionSet(self.id));
            }

            buffer.updates.set(());
        }
    }
}

impl Text {
    fn new(code_units: Vec<u16>) -> Self {
        let newline_offsets = code_units
            .iter()
            .enumerate()
            .filter_map(|(offset, c)| {
                if *c == u16::from(b'\n') {
                    Some(offset)
                } else {
                    None
                }
            })
            .collect();

        Self {
            code_units,
            newline_offsets,
        }
    }

    fn len(&self) -> usize {
        self.code_units.len()
    }

    fn point_for_offset(&self, offset: usize) -> Result<Point, Error> {
        if offset > self.len() {
            Err(Error::OffsetOutOfRange)
        } else {
            let row = find_insertion_index(&self.newline_offsets, &offset);
            let row_start_offset = if row == 0 {
                0
            } else {
                self.newline_offsets[row - 1] + 1
            };

            Ok(Point {
                row: row as u32,
                column: (offset - row_start_offset) as u32,
            })
        }
    }

    fn offset_for_point(&self, point: Point) -> Result<usize, Error> {
        let row_start_offset = if point.row == 0 {
            0
        } else {
            self.newline_offsets[(point.row - 1) as usize] + 1
        };

        let row_end_offset = if self.newline_offsets.len() > point.row as usize {
            self.newline_offsets[point.row as usize]
        } else {
            self.len()
        };

        let target_offset = row_start_offset + point.column as usize;
        if target_offset <= row_end_offset {
            Ok(target_offset)
        } else {
            Err(Error::OffsetOutOfRange)
        }
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
            FragmentSummary {
                extent: self.len(),
                extent_2d: fragment_2d_end - &fragment_2d_start,
                max_fragment_id: self.id.clone(),
            }
        } else {
            FragmentSummary {
                extent: 0,
                extent_2d: Point { row: 0, column: 0 },
                max_fragment_id: self.id.clone(),
            }
        }
    }
}

impl<'a> AddAssign<&'a FragmentSummary> for FragmentSummary {
    fn add_assign(&mut self, other: &Self) {
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

fn find_insertion_index<T: Ord>(v: &Vec<T>, x: &T) -> usize {
    match v.binary_search(x) {
        Ok(index) => index,
        Err(index) => index,
    }
}

fn should_insert_before(
    insertion: &Insertion,
    other_timestamp: LamportTimestamp,
    other_replica_id: ReplicaId,
) -> bool {
    match insertion.timestamp.cmp(&other_timestamp) {
        cmp::Ordering::Less => true,
        cmp::Ordering::Equal => insertion.id.replica_id < other_replica_id,
        cmp::Ordering::Greater => false,
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use self::rand::{Rng, SeedableRng, StdRng};
    use super::*;
    use rpc;
    use std::cmp::Ordering;
    use std::time::Duration;
    use tokio_core::reactor;
    use IntoShared;

    #[test]
    fn test_edit() {
        let mut buffer = Buffer::new();
        buffer.edit(0..0, "abc");
        assert_eq!(buffer.to_string(), "abc");
        buffer.edit(3..3, "def");
        assert_eq!(buffer.to_string(), "abcdef");
        buffer.edit(0..0, "ghi");
        assert_eq!(buffer.to_string(), "ghiabcdef");
        buffer.edit(5..5, "jkl");
        assert_eq!(buffer.to_string(), "ghiabjklcdef");
        buffer.edit(6..7, "");
        assert_eq!(buffer.to_string(), "ghiabjlcdef");
        buffer.edit(4..9, "mno");
        assert_eq!(buffer.to_string(), "ghiamnoef");
    }

    #[test]
    fn test_random_edits() {
        for seed in 0..100 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut buffer = Buffer::new();
            let mut reference_string = String::new();

            for _i in 0..30 {
                let end = rng.gen_range::<usize>(0, buffer.len() + 1);
                let start = rng.gen_range::<usize>(0, end + 1);
                let new_text = RandomCharIter(rng)
                    .take(rng.gen_range(0, 10))
                    .collect::<String>();

                buffer.edit(start..end, new_text.as_str());
                reference_string = [
                    &reference_string[0..start],
                    new_text.as_str(),
                    &reference_string[end..],
                ].concat();
                assert_eq!(buffer.to_string(), reference_string);
            }
        }
    }

    #[test]
    fn test_len_for_row() {
        let mut buffer = Buffer::new();
        buffer.edit(0..0, "abcd\nefg\nhij");
        buffer.edit(12..12, "kl\nmno");
        buffer.edit(18..18, "\npqrs\n");
        buffer.edit(18..21, "\nPQ");

        assert_eq!(buffer.len_for_row(0), Ok(4));
        assert_eq!(buffer.len_for_row(1), Ok(3));
        assert_eq!(buffer.len_for_row(2), Ok(5));
        assert_eq!(buffer.len_for_row(3), Ok(3));
        assert_eq!(buffer.len_for_row(4), Ok(4));
        assert_eq!(buffer.len_for_row(5), Ok(0));
        assert_eq!(buffer.len_for_row(6), Err(Error::OffsetOutOfRange));
    }

    #[test]
    fn iter_starting_at_row() {
        let mut buffer = Buffer::new();
        buffer.edit(0..0, "abcd\nefgh\nij");
        buffer.edit(12..12, "kl\nmno");
        buffer.edit(18..18, "\npqrs");
        buffer.edit(18..21, "\nPQ");

        let iter = buffer.iter_starting_at_row(0);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "abcd\nefgh\nijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_row(1);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "efgh\nijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_row(2);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "ijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_row(3);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "mno\nPQrs"
        );

        let iter = buffer.iter_starting_at_row(4);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "PQrs"
        );

        let iter = buffer.iter_starting_at_row(5);
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "");
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
        let mut buffer = Buffer::new();
        buffer.edit(0..0, "abc");
        let left_anchor = buffer.anchor_before_offset(2).unwrap();
        let right_anchor = buffer.anchor_after_offset(2).unwrap();

        buffer.edit(1..1, "def\n");
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

        buffer.edit(2..3, "");
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

        buffer.edit(5..5, "ghi\n");
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

        buffer.edit(7..9, "");
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
        let mut buffer = Buffer::new();
        let before_start_anchor = buffer.anchor_before_offset(0).unwrap();
        let after_end_anchor = buffer.anchor_after_offset(0).unwrap();

        buffer.edit(0..0, "abc");
        assert_eq!(buffer.to_string(), "abc");
        assert_eq!(buffer.offset_for_anchor(&before_start_anchor).unwrap(), 0);
        assert_eq!(buffer.offset_for_anchor(&after_end_anchor).unwrap(), 3);

        let after_start_anchor = buffer.anchor_after_offset(0).unwrap();
        let before_end_anchor = buffer.anchor_before_offset(3).unwrap();

        buffer.edit(3..3, "def");
        buffer.edit(0..0, "ghi");
        assert_eq!(buffer.to_string(), "ghiabcdef");
        assert_eq!(buffer.offset_for_anchor(&before_start_anchor).unwrap(), 0);
        assert_eq!(buffer.offset_for_anchor(&after_start_anchor).unwrap(), 3);
        assert_eq!(buffer.offset_for_anchor(&before_end_anchor).unwrap(), 6);
        assert_eq!(buffer.offset_for_anchor(&after_end_anchor).unwrap(), 9);
    }

    #[test]
    fn test_selection_sets() {
        let buffer = Buffer::new().into_shared();
        buffer.borrow_mut().edit(0..0, "abcdef");
        buffer.borrow_mut().edit(2..4, "ghi");

        let _set_1 = Buffer::add_local_selection_set(&buffer);
        let set_2 = Buffer::add_local_selection_set(&buffer);
        assert_eq!(buffer.borrow().local_selections.len(), 2);

        drop(set_2);
        assert_eq!(buffer.borrow().local_selections.len(), 1);
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
                let mut buffer = Buffer::new();
                buffer.replica_id = i + 1;
                buffers.push(buffer);
                queues.push(Vec::new());
            }

            let mut edit_count = 10;
            loop {
                let replica_index = rng.gen_range::<usize>(site_range.start, site_range.end);
                let buffer = &mut buffers[replica_index];
                if edit_count > 0 && rng.gen() {
                    let end = rng.gen_range::<usize>(0, buffer.len() + 1);
                    let start = rng.gen_range::<usize>(0, end + 1);
                    let new_text = RandomCharIter(rng)
                        .take(rng.gen_range(0, 10))
                        .collect::<String>();

                    if let Some(op) = buffer.edit(start..end, new_text.as_str()) {
                        for (index, queue) in queues.iter_mut().enumerate() {
                            if index != replica_index {
                                queue.push(op.clone());
                            }
                        }

                        edit_count -= 1;
                    }
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
        let local_buffer = Buffer::new().into_shared();
        local_buffer.borrow_mut().edit(0..0, "abcdef");
        local_buffer.borrow_mut().edit(2..4, "ghi");

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

        local_buffer.borrow_mut().edit(3..6, "jk");
        remote_buffer_1.borrow_mut().edit(7..7, "lmn");
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

        let buffer_1 = Buffer::new().into_shared();
        buffer_1.borrow_mut().edit(0..0, "abcdef");
        let buffer_1_selections_1 = Buffer::add_local_selection_set(&buffer_1);
        **buffer_1_selections_1.borrow_mut() = vec![
            empty_selection(&buffer_1.borrow(), 1),
            empty_selection(&buffer_1.borrow(), 3),
        ];
        let buffer_1_selections_2 = Buffer::add_local_selection_set(&buffer_1);
        **buffer_1_selections_2.borrow_mut() = vec![
            empty_selection(&buffer_1.borrow(), 2),
            empty_selection(&buffer_1.borrow(), 4),
        ];

        let mut reactor = reactor::Core::new().unwrap();
        let foreground = Rc::new(reactor.handle());
        let buffer_2 = Buffer::remote(
            foreground.clone(),
            rpc::tests::connect(&mut reactor, super::rpc::Service::new(buffer_1.clone())),
        ).unwrap();
        assert_eq!(
            buffer_1.borrow().selections(),
            buffer_2.borrow().selections(),
        );

        let buffer_3 = Buffer::remote(
            foreground,
            rpc::tests::connect(&mut reactor, super::rpc::Service::new(buffer_1.clone())),
        ).unwrap();
        assert_eq!(
            buffer_1.borrow().selections(),
            buffer_3.borrow().selections(),
        );

        let mut buffer_1_updates = buffer_1.borrow().updates();
        let mut buffer_2_updates = buffer_2.borrow().updates();
        let mut buffer_3_updates = buffer_3.borrow().updates();

        drop(buffer_1_selections_2);
        buffer_1_updates.wait_next(&mut reactor).unwrap();
        buffer_2_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(
            buffer_1.borrow().selections(),
            buffer_2.borrow().selections(),
        );
        buffer_3_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(
            buffer_1.borrow().selections(),
            buffer_3.borrow().selections(),
        );

        let buffer_2_selections = Buffer::add_local_selection_set(&buffer_2);
        buffer_2_selections
            .borrow_mut()
            .push(empty_selection(&buffer_2.borrow(), 1));
        buffer_2_selections.borrow().updated();
        buffer_1_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(
            buffer_1.borrow().selections(),
            buffer_2.borrow().selections(),
        );
        buffer_3_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(
            buffer_1.borrow().selections(),
            buffer_3.borrow().selections(),
        );

        drop(buffer_2_selections);
        buffer_2_updates.wait_next(&mut reactor).unwrap();
        buffer_1_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(
            buffer_1.borrow().selections(),
            buffer_2.borrow().selections(),
        );
        buffer_3_updates.wait_next(&mut reactor).unwrap();
        assert_eq!(
            buffer_1.borrow().selections(),
            buffer_3.borrow().selections(),
        );
    }

    struct RandomCharIter<T: Rng>(T);

    impl<T: Rng> Iterator for RandomCharIter<T> {
        type Item = char;

        fn next(&mut self) -> Option<Self::Item> {
            Some(self.0.gen_range(b'a', b'z' + 1).into())
        }
    }

    impl Buffer {
        fn selections(&self) -> Vec<(ReplicaId, SelectionSetId, Selection)> {
            let mut selections = Vec::new();
            for (set_id, selection_set) in &self.local_selections {
                let selection_set = selection_set.upgrade().unwrap();
                for selection in selection_set.borrow().iter() {
                    selections.push((self.replica_id, *set_id, selection.clone()));
                }
            }
            for ((replica_id, set_id), selection_set) in &self.remote_selections {
                for selection in selection_set.borrow().iter() {
                    selections.push((*replica_id, *set_id, selection.clone()));
                }
            }
            selections.sort_by(|a, b| match a.0.cmp(&b.0) {
                cmp::Ordering::Equal => a.1.cmp(&b.1),
                comparison @ _ => comparison,
            });
            selections
        }
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
