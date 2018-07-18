use btree::{self, NodeStore, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::{self, Ordering};
use std::ffi::OsString;
use std::fmt;
use std::ops::{Add, AddAssign};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait Store {
    type ReadError: fmt::Debug;
    type MetadataStore: NodeStore<Metadata, ReadError = Self::ReadError>;
    type ParentRefStore: NodeStore<ParentRef, ReadError = Self::ReadError>;
    type ChildRefStore: NodeStore<ChildRef, ReadError = Self::ReadError>;

    fn replica_id(&self) -> id::ReplicaId;

    fn metadata_store(&self) -> &Self::MetadataStore;
    fn parent_ref_store(&self) -> &Self::ParentRefStore;
    fn child_ref_store(&self) -> &Self::ChildRefStore;

    fn gen_id(&self) -> id::Unique;
    fn gen_timestamp(&self) -> LamportTimestamp;
    fn recv_timestamp(&self, timestamp: LamportTimestamp);
}

pub trait FileSystem {
    // TODO: Replace PathBuf with Path. There's no need to have ownership in these methods.
    fn insert_dir<I: Into<PathBuf>>(&mut self, path: I) -> Inode;
    fn remove_dir<I: Into<PathBuf>>(&mut self, path: I);
    fn move_dir<I1: Into<PathBuf>, I2: Into<PathBuf>>(&mut self, from: I1, to: I2);
}

trait Keyed {
    type Key: Ord;

    fn key(&self) -> Self::Key;
}

type Inode = u64;
type VisibleCount = usize;

const ROOT_ID: id::Unique = id::Unique::DEFAULT;

#[derive(Clone)]
pub struct Tree {
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRef>,
    child_refs: btree::Tree<ChildRef>,
}

pub struct TreeCursor {
    path: PathBuf,
    stack: Vec<btree::Cursor<ChildRef>>,
    metadata_cursor: btree::Cursor<Metadata>,
}

#[derive(Clone, Debug)]
pub enum Operation {
    InsertDir {
        op_id: id::Unique,
        timestamp: LamportTimestamp,
        parent_id: id::Unique,
        name: Arc<OsString>,
    },
    MoveDir {
        op_id: id::Unique,
        child_id: id::Unique,
        timestamp: LamportTimestamp,
        prev_timestamp: LamportTimestamp,
        new_location: Option<(id::Unique, Arc<OsString>)>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    file_id: id::Unique,
    is_dir: bool,
    inode: Option<Inode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParentRef {
    child_id: id::Unique,
    timestamp: LamportTimestamp,
    prev_timestamp: LamportTimestamp,
    op_id: id::Unique,
    parent: Option<(id::Unique, Arc<OsString>)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParentRefKey {
    child_id: id::Unique,
    timestamp: LamportTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildRef {
    parent_id: id::Unique,
    name: Arc<OsString>,
    timestamp: LamportTimestamp,
    op_id: id::Unique,
    child_id: id::Unique,
    deletions: SmallVec<[id::Unique; 1]>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefSummary {
    parent_id: id::Unique,
    name: Arc<OsString>,
    timestamp: LamportTimestamp,
    visible_count: VisibleCount,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefKey {
    parent_id: id::Unique,
    name: Arc<OsString>,
    timestamp: LamportTimestamp,
}

#[derive(Clone, Debug, Default, Ord, Eq, PartialEq, PartialOrd)]
pub struct ParentIdAndName {
    parent_id: id::Unique,
    name: Arc<OsString>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct LamportTimestamp {
    value: u64,
    replica_id: id::ReplicaId,
}

impl Tree {
    pub fn new() -> Self {
        Tree {
            metadata: btree::Tree::new(),
            parent_refs: btree::Tree::new(),
            child_refs: btree::Tree::new(),
        }
    }

    pub fn is_empty<S: Store>(&self, db: &S) -> Result<bool, S::ReadError> {
        Ok(self.cursor(db)?.depth() == 0)
    }

    pub fn cursor<S: Store>(&self, db: &S) -> Result<TreeCursor, S::ReadError> {
        let mut cursor = TreeCursor {
            path: PathBuf::new(),
            stack: Vec::new(),
            metadata_cursor: self.metadata.cursor(),
        };
        cursor.descend_into(self.child_refs.cursor(), ROOT_ID, db)?;
        Ok(cursor)
    }

    pub fn insert_dirs<I, S>(&mut self, path: I, db: &S) -> Result<Vec<Operation>, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();
        let child_ref_db = db.child_ref_store();

        let mut operations = Vec::new();
        let mut cursor = self.child_refs.cursor();
        let mut parent_id = ROOT_ID;
        let mut entry_exists = true;

        for name in path.components() {
            let name = Arc::new(OsString::from(name.as_os_str()));
            if entry_exists {
                let key = ParentIdAndName {
                    parent_id,
                    name: name.clone(),
                };
                if cursor.seek(&key, SeekBias::Left, child_ref_db)? {
                    let child_ref = cursor.item(child_ref_db)?.unwrap();
                    if child_ref.is_visible() {
                        parent_id = child_ref.child_id;
                    } else {
                        entry_exists = false;
                    }
                } else {
                    entry_exists = false;
                }
            }
            if !entry_exists {
                let op_id = db.gen_id();
                operations.push(Operation::InsertDir {
                    op_id,
                    timestamp: db.gen_timestamp(),
                    parent_id,
                    name,
                });
                parent_id = op_id;
            }
        }

        self.integrate_ops(&operations, db)?;
        Ok(operations)
    }

    pub fn remove_dir<I, S>(&mut self, path: I, db: &S) -> Result<Operation, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();

        let child_id = self.id_for_path(&path, db)?.unwrap();
        let prev_parent_ref = self.find_cur_parent_ref(child_id, db)?.unwrap();
        let operation = Operation::MoveDir {
            op_id: db.gen_id(),
            child_id,
            timestamp: db.gen_timestamp(),
            prev_timestamp: prev_parent_ref.timestamp,
            new_location: None,
        };
        self.integrate_ops(Some(&operation), db)?;
        Ok(operation)
    }

    pub fn move_dir<F, S, T>(&mut self, from: F, to: T, db: &S) -> Result<Operation, S::ReadError>
    where
        F: Into<PathBuf>,
        S: Store,
        T: Into<PathBuf>,
    {
        let from = from.into();
        let to = to.into();

        let from_id = self.id_for_path(&from, db)?.unwrap();
        let new_parent_id = if let Some(parent_path) = to.parent() {
            self.id_for_path(parent_path, db)?.unwrap()
        } else {
            ROOT_ID
        };
        let new_name = Arc::new(OsString::from(to.file_name().unwrap()));

        let prev_parent_ref = self.find_cur_parent_ref(from_id, db)?.unwrap();
        let operation = Operation::MoveDir {
            op_id: db.gen_id(),
            child_id: from_id,
            timestamp: db.gen_timestamp(),
            prev_timestamp: prev_parent_ref.timestamp,
            new_location: Some((new_parent_id, new_name)),
        };
        self.integrate_ops(Some(&operation), db)?;
        Ok(operation)
    }

    fn integrate_ops<'a, I, S>(&mut self, ops: I, db: &S) -> Result<Vec<Operation>, S::ReadError>
    where
        I: IntoIterator<Item = &'a Operation>,
        S: Store,
    {
        let mut fixup_ops = Vec::new();
        for op in ops {
            fixup_ops.extend(self.integrate_op(op.clone(), db)?);
        }
        Ok(fixup_ops)
    }

    fn integrate_op<S>(&mut self, op: Operation, db: &S) -> Result<Option<Operation>, S::ReadError>
    where
        S: Store,
    {
        let metadata_db = db.metadata_store();
        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        let mut metadata = Vec::new();
        let mut parent_refs = Vec::new();
        let mut child_refs = Vec::new();
        let mut new_child_ref;
        let received_timestamp;

        match op {
            Operation::InsertDir {
                op_id,
                timestamp,
                parent_id,
                name,
            } => {
                // println!("integrate insertion {:?}", op_id);
                new_child_ref = Some(ChildRef {
                    parent_id,
                    name: name.clone(),
                    timestamp,
                    op_id,
                    child_id: op_id,
                    deletions: SmallVec::new(),
                });

                metadata.push(Metadata {
                    file_id: op_id,
                    is_dir: true,
                    inode: None,
                });
                parent_refs.push(ParentRef {
                    child_id: op_id,
                    timestamp,
                    prev_timestamp: timestamp,
                    op_id,
                    parent: Some((parent_id, name)),
                });
                child_refs.extend(new_child_ref.clone());
                received_timestamp = timestamp;
            }
            Operation::MoveDir {
                op_id,
                child_id,
                timestamp,
                prev_timestamp,
                new_location,
            } => {
                // println!("integrate move {:?}", op_id);

                new_child_ref = new_location.as_ref().map(|(parent_id, name)| ChildRef {
                    parent_id: *parent_id,
                    name: name.clone(),
                    timestamp,
                    op_id,
                    child_id,
                    deletions: SmallVec::new(),
                });

                let mut child_ref_cursor = self.child_refs.cursor();
                let mut parent_ref_cursor = self.parent_refs.cursor();
                parent_ref_cursor.seek(&child_id, SeekBias::Left, parent_ref_db)?;
                while let Some(parent_ref) = parent_ref_cursor.item(parent_ref_db)? {
                    if parent_ref.child_id != child_id {
                        break;
                    } else if parent_ref.timestamp > timestamp
                        && parent_ref.prev_timestamp < timestamp
                    {
                        if let Some(new_child_ref) = new_child_ref.as_mut() {
                            new_child_ref.deletions.push(parent_ref.op_id);
                        }
                    } else if parent_ref.timestamp >= prev_timestamp {
                        if let Some(child_ref_key) = parent_ref.to_child_ref_key() {
                            child_ref_cursor.seek(&child_ref_key, SeekBias::Left, child_ref_db)?;
                            let mut child_ref = child_ref_cursor.item(child_ref_db)?.unwrap();
                            child_ref.deletions.push(op_id);
                            child_refs.push(child_ref);
                        }
                    } else {
                        break;
                    }
                    parent_ref_cursor.next(parent_ref_db)?;
                }

                parent_refs.push(ParentRef {
                    child_id,
                    timestamp,
                    prev_timestamp,
                    op_id,
                    parent: new_location,
                });
                child_refs.extend(new_child_ref.clone());
                received_timestamp = timestamp;
            }
        }

        self.metadata = interleave(&self.metadata, metadata, metadata_db)?;
        self.parent_refs = interleave(&self.parent_refs, parent_refs, parent_ref_db)?;
        self.child_refs = interleave(&self.child_refs, child_refs, child_ref_db)?;
        if db.replica_id() != received_timestamp.replica_id {
            db.recv_timestamp(received_timestamp);
        }

        // println!("{:#?}", self.child_refs.items(child_ref_db)?);

        if let Some(child_ref) = new_child_ref {
            self.fix_name_conflicts(child_ref.parent_id, child_ref.name, db)
        } else {
            Ok(None)
        }
    }

    fn fix_name_conflicts<S: Store>(
        &mut self,
        parent_id: id::Unique,
        name: Arc<OsString>,
        db: &S,
    ) -> Result<Option<Operation>, S::ReadError> {
        let child_ref_db = db.child_ref_store();
        let mut cursor = self.child_refs.cursor();
        let mut key = ParentIdAndName { parent_id, name };
        cursor.seek(&key, SeekBias::Left, child_ref_db)?;
        let next_visible_index = cursor.end::<usize, _>(child_ref_db)? + 1;
        cursor.seek_forward(&next_visible_index, SeekBias::Left, child_ref_db)?;
        if let Some(child_ref) = cursor.item(child_ref_db)? {
            if child_ref.parent_id == parent_id && child_ref.name == key.name {
                // Append tildes to the name until we don't have a name conflict.
                loop {
                    Arc::make_mut(&mut key.name).push("~");
                    if !cursor.seek_forward(&key, SeekBias::Left, child_ref_db)? {
                        break;
                    }
                }

                // Move the conflicting child ref to the new name.
                let operation = Some(Operation::MoveDir {
                    op_id: db.gen_id(),
                    child_id: child_ref.child_id,
                    timestamp: db.gen_timestamp(),
                    prev_timestamp: child_ref.timestamp,
                    new_location: Some((parent_id, key.name)),
                });
                self.integrate_ops(&operation, db)?;

                return Ok(operation);
            }
        }
        Ok(None)
    }

    fn id_for_path<P, S>(&self, path: P, db: &S) -> Result<Option<id::Unique>, S::ReadError>
    where
        P: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();
        let child_ref_db = db.child_ref_store();

        let mut cursor = self.child_refs.cursor();
        let mut child_id = ROOT_ID;
        for name in path.components() {
            let name = Arc::new(OsString::from(name.as_os_str()));
            let key = ParentIdAndName {
                parent_id: child_id,
                name,
            };
            if cursor.seek(&key, SeekBias::Left, child_ref_db)? {
                let child_ref = cursor.item(child_ref_db)?.unwrap();
                if child_ref.is_visible() {
                    child_id = child_ref.child_id;
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }

        Ok(Some(child_id))
    }

    fn find_cur_parent_ref<S>(
        &self,
        child_id: id::Unique,
        db: &S,
    ) -> Result<Option<ParentRef>, S::ReadError>
    where
        S: Store,
    {
        let parent_ref_db = db.parent_ref_store();
        let mut cursor = self.parent_refs.cursor();
        cursor.seek(&child_id, SeekBias::Left, parent_ref_db)?;
        let parent_ref = cursor.item(parent_ref_db)?;
        if parent_ref
            .as_ref()
            .map_or(false, |parent_ref| parent_ref.child_id == child_id)
        {
            Ok(parent_ref)
        } else {
            Ok(None)
        }
    }
}

fn interleave<T, S>(
    old_tree: &btree::Tree<T>,
    mut new_items: Vec<T>,
    db: &S,
) -> Result<btree::Tree<T>, S::ReadError>
where
    T: btree::Item + Keyed,
    T::Key: btree::Dimension<T::Summary> + Default,
    S: NodeStore<T>,
{
    new_items.sort_unstable_by_key(|item| item.key());

    let mut old_cursor = old_tree.cursor();
    let mut new_tree = btree::Tree::new();
    let mut buffered_items = Vec::new();

    old_cursor.seek(&T::Key::default(), SeekBias::Left, db)?;
    for new_item in new_items {
        let new_item_key = new_item.key();
        let mut old_item = old_cursor.item(db)?;
        if old_item
            .as_ref()
            .map_or(false, |old_item| old_item.key() < new_item_key)
        {
            new_tree.extend(buffered_items.drain(..), db)?;
            new_tree.push_tree(old_cursor.slice(&new_item_key, SeekBias::Left, db)?, db)?;
            old_item = old_cursor.item(db)?;
        }
        if old_item.map_or(false, |old_item| old_item.key() == new_item_key) {
            old_cursor.next(db)?;
        }
        buffered_items.push(new_item);
    }
    new_tree.extend(buffered_items, db)?;
    new_tree.push_tree(old_cursor.suffix::<T::Key, _>(db)?, db)?;

    Ok(new_tree)
}

impl TreeCursor {
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    pub fn path(&self) -> Option<&Path> {
        if self.stack.is_empty() {
            None
        } else {
            Some(&self.path)
        }
    }

    pub fn name<S: Store>(&self, db: &S) -> Result<Option<Arc<OsString>>, S::ReadError> {
        let db = db.child_ref_store();

        if self.stack.is_empty() {
            Ok(None)
        } else {
            let child_ref = self.stack.last().unwrap().item(db)?.unwrap();
            Ok(Some(child_ref.name.clone()))
        }
    }

    pub fn inode<S: Store>(&self, db: &S) -> Result<Option<Inode>, S::ReadError> {
        Ok(self
            .metadata_cursor
            .item(db.metadata_store())?
            .and_then(|metadata| metadata.inode))
    }

    pub fn is_dir<S: Store>(&self, db: &S) -> Result<Option<bool>, S::ReadError> {
        Ok(self
            .metadata_cursor
            .item(db.metadata_store())?
            .map(|metadata| metadata.is_dir))
    }

    pub fn next<S: Store>(&mut self, db: &S) -> Result<(), S::ReadError> {
        if self.stack.is_empty() {
            Ok(())
        } else {
            let metadata = self.metadata_cursor.item(db.metadata_store())?.unwrap();
            if (metadata.is_dir && self.descend(db)?) || self.next_sibling(db)? {
                Ok(())
            } else {
                loop {
                    self.path.pop();
                    self.stack.pop();
                    if self.stack.is_empty() || self.next_sibling(db)? {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn descend<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        let child_ref_cursor = self.stack.last().unwrap().clone();
        let dir_id = child_ref_cursor
            .item(db.child_ref_store())?
            .unwrap()
            .child_id;
        self.descend_into(child_ref_cursor, dir_id, db)
    }

    fn descend_into<S: Store>(
        &mut self,
        mut child_ref_cursor: btree::Cursor<ChildRef>,
        dir_id: id::Unique,
        db: &S,
    ) -> Result<bool, S::ReadError> {
        child_ref_cursor.seek(&dir_id, SeekBias::Left, db.child_ref_store())?;
        if let Some(child_ref) = child_ref_cursor.item(db.child_ref_store())? {
            if child_ref.parent_id == dir_id {
                self.path.push(child_ref.name.as_os_str());
                self.stack.push(child_ref_cursor);
                if child_ref.is_visible() {
                    self.metadata_cursor.seek(
                        &child_ref.child_id,
                        SeekBias::Left,
                        db.metadata_store(),
                    )?;
                    Ok(true)
                } else if self.next_sibling(db)? {
                    Ok(true)
                } else {
                    self.path.pop();
                    self.stack.pop();
                    Ok(false)
                }
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    fn next_sibling<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        let child_ref_db = db.child_ref_store();
        let child_ref_cursor = self.stack.last_mut().unwrap();
        let parent_id = child_ref_cursor.item(child_ref_db)?.unwrap().parent_id;
        let next_visible_index: usize = child_ref_cursor.end(child_ref_db)?;
        child_ref_cursor.seek(&next_visible_index, SeekBias::Right, child_ref_db)?;
        if let Some(child_ref) = child_ref_cursor.item(child_ref_db)? {
            if child_ref.parent_id == parent_id {
                self.path.pop();
                self.path.push(child_ref.name.as_os_str());
                self.metadata_cursor.seek(
                    &child_ref.child_id,
                    SeekBias::Left,
                    db.metadata_store(),
                )?;
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }
}

impl btree::Item for Metadata {
    type Summary = id::Unique;

    fn summarize(&self) -> Self::Summary {
        self.file_id
    }
}

impl btree::Dimension<id::Unique> for id::Unique {
    fn from_summary(summary: &id::Unique) -> Self {
        *summary
    }
}

impl Keyed for Metadata {
    type Key = id::Unique;

    fn key(&self) -> Self::Key {
        self.file_id
    }
}

impl ParentRef {
    fn to_child_ref_key(&self) -> Option<ChildRefKey> {
        self.parent.as_ref().map(|(parent_id, name)| ChildRefKey {
            parent_id: *parent_id,
            name: name.clone(),
            timestamp: self.timestamp,
        })
    }
}

impl btree::Item for ParentRef {
    type Summary = ParentRefKey;

    fn summarize(&self) -> Self::Summary {
        self.key()
    }
}

impl Keyed for ParentRef {
    type Key = ParentRefKey;

    fn key(&self) -> Self::Key {
        ParentRefKey {
            child_id: self.child_id,
            timestamp: self.timestamp,
        }
    }
}

impl btree::Dimension<ParentRefKey> for id::Unique {
    fn from_summary(summary: &ParentRefKey) -> Self {
        summary.child_id
    }
}

impl btree::Dimension<ParentRefKey> for ParentRefKey {
    fn from_summary(summary: &ParentRefKey) -> ParentRefKey {
        summary.clone()
    }
}

impl Ord for ParentRefKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.child_id
            .cmp(&other.child_id)
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

impl PartialOrd for ParentRefKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> AddAssign<&'a Self> for ParentRefKey {
    fn add_assign(&mut self, other: &Self) {
        debug_assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ParentRefKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        debug_assert!(self < *other);
        other.clone()
    }
}

impl ChildRef {
    fn is_visible(&self) -> bool {
        self.deletions.is_empty()
    }
}

impl btree::Item for ChildRef {
    type Summary = ChildRefSummary;

    fn summarize(&self) -> Self::Summary {
        ChildRefSummary {
            parent_id: self.parent_id,
            name: self.name.clone(),
            timestamp: self.timestamp,
            visible_count: if self.deletions.is_empty() { 1 } else { 0 },
        }
    }
}

impl Keyed for ChildRef {
    type Key = ChildRefKey;

    fn key(&self) -> Self::Key {
        ChildRefKey {
            parent_id: self.parent_id,
            name: self.name.clone(),
            timestamp: self.timestamp,
        }
    }
}

impl Ord for ChildRefSummary {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parent_id
            .cmp(&other.parent_id)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

impl PartialOrd for ChildRefSummary {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> AddAssign<&'a Self> for ChildRefSummary {
    fn add_assign(&mut self, other: &Self) {
        debug_assert!(*self < *other);
        self.parent_id = other.parent_id;
        self.name = other.name.clone();
        self.timestamp = other.timestamp;
        self.visible_count += other.visible_count;
    }
}

impl btree::Dimension<ChildRefSummary> for ChildRefKey {
    fn from_summary(summary: &ChildRefSummary) -> ChildRefKey {
        ChildRefKey {
            parent_id: summary.parent_id,
            name: summary.name.clone(),
            timestamp: summary.timestamp,
        }
    }
}

impl Ord for ChildRefKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parent_id
            .cmp(&other.parent_id)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

impl PartialOrd for ChildRefKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> AddAssign<&'a Self> for ChildRefKey {
    fn add_assign(&mut self, other: &Self) {
        debug_assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ChildRefKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        debug_assert!(self < *other);
        other.clone()
    }
}

impl btree::Dimension<ChildRefSummary> for ParentIdAndName {
    fn from_summary(summary: &ChildRefSummary) -> Self {
        ParentIdAndName {
            parent_id: summary.parent_id,
            name: summary.name.clone(),
        }
    }
}

impl<'a> AddAssign<&'a Self> for ParentIdAndName {
    fn add_assign(&mut self, other: &Self) {
        debug_assert!(*self <= *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ParentIdAndName {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        debug_assert!(self <= *other);
        other.clone()
    }
}

impl btree::Dimension<ChildRefSummary> for id::Unique {
    fn from_summary(summary: &ChildRefSummary) -> Self {
        summary.parent_id
    }
}

impl btree::Dimension<ChildRefSummary> for VisibleCount {
    fn from_summary(summary: &ChildRefSummary) -> Self {
        summary.visible_count
    }
}

impl LamportTimestamp {
    pub fn new(replica_id: id::ReplicaId) -> Self {
        Self {
            value: 0,
            replica_id,
        }
    }

    pub fn max_value() -> Self {
        Self {
            value: u64::max_value(),
            replica_id: id::ReplicaId::max_value(),
        }
    }

    pub fn inc(self) -> Self {
        Self {
            value: self.value + 1,
            replica_id: self.replica_id,
        }
    }

    pub fn update(self, other: Self) -> Self {
        Self {
            value: cmp::max(self.value, other.value) + 1,
            replica_id: self.replica_id,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use self::rand::{Rng, SeedableRng, StdRng};
    use super::*;
    use std::cell::Cell;

    #[test]
    fn test_local_dir_ops() {
        let db = NullStore::new(1);
        let mut tree = Tree::new();
        tree.insert_dirs("a/b2/", &db).unwrap();
        assert_eq!(tree.paths(&db), ["a/", "a/b2/"]);

        tree.insert_dirs("a/b1/c", &db).unwrap();
        assert_eq!(tree.paths(&db), ["a/", "a/b1/", "a/b1/c/", "a/b2/"]);

        tree.insert_dirs("a/b1/d", &db).unwrap();
        assert_eq!(
            tree.paths(&db),
            ["a/", "a/b1/", "a/b1/c/", "a/b1/d/", "a/b2/"]
        );

        tree.remove_dir("a/b1/c", &db).unwrap();
        assert_eq!(tree.paths(&db), ["a/", "a/b1/", "a/b1/d/", "a/b2/"]);

        tree.remove_dir("a/b1", &db).unwrap();
        assert_eq!(tree.paths(&db), ["a/", "a/b2/"]);

        tree.insert_dirs("a/b1/c", &db).unwrap();
        tree.insert_dirs("a/b1/d", &db).unwrap();
        assert_eq!(
            tree.paths(&db),
            ["a/", "a/b1/", "a/b1/c/", "a/b1/d/", "a/b2/"]
        );

        let moved_id = tree.id_for_path("a/b1", &db).unwrap().unwrap();
        tree.move_dir("a/b1", "b", &db).unwrap();
        assert_eq!(tree.paths(&db), ["a/", "a/b2/", "b/", "b/c/", "b/d/"]);
        assert_eq!(tree.id_for_path("b", &db).unwrap().unwrap(), moved_id);

        let moved_id = tree.id_for_path("b/d", &db).unwrap().unwrap();
        tree.move_dir("b/d", "a/b2/d", &db).unwrap();
        assert_eq!(tree.paths(&db), ["a/", "a/b2/", "a/b2/d/", "b/", "b/c/"]);
        assert_eq!(tree.id_for_path("a/b2/d", &db).unwrap().unwrap(), moved_id);
    }

    #[test]
    fn test_name_conflict_fixups() {
        let db_1 = NullStore::new(1);
        let mut tree_1 = Tree::new();
        let mut tree_1_ops = Vec::new();

        let db_2 = NullStore::new(2);
        let mut tree_2 = Tree::new();
        let mut tree_2_ops = Vec::new();

        tree_1_ops.extend(tree_1.insert_dirs("a", &db_1).unwrap());
        let id_1 = tree_1.id_for_path("a", &db_1).unwrap().unwrap();

        tree_2_ops.extend(tree_2.insert_dirs("a", &db_2).unwrap());
        tree_2_ops.extend(tree_2.insert_dirs("a~", &db_2).unwrap());
        tree_2_ops.push(tree_2.remove_dir("a", &db_2).unwrap());
        let id_2 = tree_2.id_for_path("a~", &db_2).unwrap().unwrap();

        tree_1_ops.extend(
            tree_1
                .integrate_ops(&tree_2_ops.drain(..).collect::<Vec<_>>(), &db_1)
                .unwrap(),
        );

        assert!(!tree_1_ops.is_empty());

        tree_2_ops.extend(
            tree_2
                .integrate_ops(&tree_1_ops.drain(..).collect::<Vec<_>>(), &db_2)
                .unwrap(),
        );

        assert!(!tree_2_ops.is_empty());

        tree_1_ops.extend(
            tree_1
                .integrate_ops(&tree_2_ops.drain(..).collect::<Vec<_>>(), &db_1)
                .unwrap(),
        );

        assert!(tree_1_ops.is_empty());
        assert!(tree_2_ops.is_empty());

        assert_eq!(tree_1.paths_with_ids(&db_1), tree_2.paths_with_ids(&db_2));
        assert_eq!(tree_1.paths(&db_1), ["a~/", "a~~/"]);
        assert_eq!(tree_1.id_for_path("a~~", &db_1).unwrap().unwrap(), id_1);
        assert_eq!(tree_1.id_for_path("a~", &db_1).unwrap().unwrap(), id_2);
    }

    #[test]
    fn test_replication_random() {
        use std::iter::FromIterator;
        use std::mem;
        const PEERS: usize = 2;

        for seed in 0..100000 {
            let seed = 20429;
            println!("SEED: {:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let db = Vec::from_iter((0..PEERS).map(|i| NullStore::new(i as u64 + 1)));
            let mut trees = Vec::from_iter((0..PEERS).map(|_| Tree::new()));
            let mut inboxes = Vec::from_iter((0..PEERS).map(|_| Vec::new()));

            for _ in 0..2 {
                let replica_index = rng.gen_range(0, PEERS);

                if !inboxes[replica_index].is_empty() && rng.gen() {
                    let db = &db[replica_index];
                    let tree = &mut trees[replica_index];
                    let ops = mem::replace(&mut inboxes[replica_index], Vec::new());
                    let fixup_ops = tree.integrate_ops(&ops, db).unwrap();
                    deliver_ops(replica_index, &mut inboxes, fixup_ops);
                } else {
                    let db = &db[replica_index];
                    let tree = &mut trees[replica_index];
                    let ops = tree.mutate(&mut rng, db);
                    deliver_ops(replica_index, &mut inboxes, ops);
                }
            }

            while inboxes.iter().any(|inbox| !inbox.is_empty()) {
                for replica_index in 0..PEERS {
                    let db = &db[replica_index];
                    let tree = &mut trees[replica_index];
                    let ops = mem::replace(&mut inboxes[replica_index], Vec::new());
                    let fixup_ops = tree.integrate_ops(&ops, db).unwrap();
                    deliver_ops(replica_index, &mut inboxes, fixup_ops);
                }
            }

            for i in 0..PEERS - 1 {
                assert_eq!(
                    trees[i].paths_with_ids(&db[i]),
                    trees[i + 1].paths_with_ids(&db[i + 1])
                );
            }

            fn deliver_ops(sender: usize, inboxes: &mut Vec<Vec<Operation>>, ops: Vec<Operation>) {
                for (i, inbox) in inboxes.iter_mut().enumerate() {
                    if i != sender {
                        inbox.extend(ops.iter().cloned());
                    }
                }
            }
        }
    }

    struct NullStore {
        next_id: Cell<id::Unique>,
        lamport_clock: Cell<LamportTimestamp>,
    }

    impl NullStore {
        fn new(replica_id: id::ReplicaId) -> Self {
            Self {
                next_id: Cell::new(id::Unique::new(replica_id)),
                lamport_clock: Cell::new(LamportTimestamp::new(replica_id)),
            }
        }
    }

    impl Store for NullStore {
        type ReadError = ();
        type MetadataStore = NullStore;
        type ParentRefStore = NullStore;
        type ChildRefStore = NullStore;

        fn replica_id(&self) -> id::ReplicaId {
            self.lamport_clock.get().replica_id
        }

        fn gen_id(&self) -> id::Unique {
            let next_id = self.next_id.get();
            self.next_id.replace(next_id.next());
            next_id
        }

        fn gen_timestamp(&self) -> LamportTimestamp {
            self.lamport_clock.replace(self.lamport_clock.get().inc());
            self.lamport_clock.get()
        }

        fn recv_timestamp(&self, timestamp: LamportTimestamp) {
            self.lamport_clock
                .set(self.lamport_clock.get().update(timestamp));
        }

        fn metadata_store(&self) -> &Self::MetadataStore {
            self
        }

        fn parent_ref_store(&self) -> &Self::ParentRefStore {
            self
        }

        fn child_ref_store(&self) -> &Self::ParentRefStore {
            self
        }
    }

    impl btree::NodeStore<Metadata> for NullStore {
        type ReadError = ();

        fn get(&self, _id: btree::NodeId) -> Result<Arc<btree::Node<Metadata>>, Self::ReadError> {
            unreachable!()
        }
    }

    impl btree::NodeStore<ParentRef> for NullStore {
        type ReadError = ();

        fn get(&self, _id: btree::NodeId) -> Result<Arc<btree::Node<ParentRef>>, Self::ReadError> {
            unreachable!()
        }
    }

    impl btree::NodeStore<ChildRef> for NullStore {
        type ReadError = ();

        fn get(&self, _id: btree::NodeId) -> Result<Arc<btree::Node<ChildRef>>, Self::ReadError> {
            unreachable!()
        }
    }

    impl Tree {
        fn mutate<S, T: Rng>(&mut self, rng: &mut T, db: &S) -> Vec<Operation>
        where
            S: Store,
        {
            let mut ops = Vec::new();
            for _ in 0..rng.gen_range(1, 5) {
                let k = rng.gen_range(0, 3);
                if self.is_empty(db).unwrap() || k == 0 {
                    let path = self.gen_path(rng, db);
                    // println!("{:?} Inserting {:?}", db.replica_id(), path);
                    ops.extend(self.insert_dirs(&path, db).unwrap());
                // println!("{:#?} ", self.child_refs.items(db.child_ref_store()));
                } else if k == 1 {
                    let path = self.select_path(rng, db).unwrap();
                    // println!("{:?} Removing {:?}", db.replica_id(), path);
                    ops.push(self.remove_dir(&path, db).unwrap());
                // println!("{:#?} ", self.child_refs.items(db.child_ref_store()));
                } else {
                    let (old_path, new_path) = loop {
                        let old_path = self.select_path(rng, db).unwrap();
                        let new_path = self.gen_path(rng, db);
                        if !new_path.starts_with(&old_path) {
                            break (old_path, new_path);
                        }
                    };

                    // println!(
                    //     "{:?} Moving {:?} to {:?}",
                    //     db.replica_id(),
                    //     old_path,
                    //     new_path
                    // );
                    ops.push(self.move_dir(&old_path, &new_path, db).unwrap());
                    // println!("{:#?} ", self.child_refs.items(db.child_ref_store()));
                }
            }
            ops
        }

        fn gen_path<S: Store, T: Rng>(&self, rng: &mut T, db: &S) -> String {
            loop {
                let path = if self.is_empty(db).unwrap() || rng.gen_weighted_bool(8) {
                    gen_name(rng)
                } else {
                    self.select_path(rng, db).unwrap()
                };

                if self.id_for_path(&path, db).unwrap().is_none() {
                    return path;
                }
            }
        }

        fn select_path<S: Store, T: Rng>(&self, rng: &mut T, db: &S) -> Option<String> {
            if self.is_empty(db).unwrap() {
                None
            } else {
                let child_ref_db = db.child_ref_store();
                let mut depth = 0;
                let mut path = PathBuf::new();
                let mut cursor = self.child_refs.cursor();
                let mut parent_id = ROOT_ID;

                loop {
                    cursor
                        .seek(&parent_id, SeekBias::Left, child_ref_db)
                        .unwrap();
                    let mut child_refs = Vec::new();
                    while let Some(child_ref) = cursor.item(child_ref_db).unwrap() {
                        if child_ref.parent_id == parent_id {
                            if child_ref.is_visible() {
                                child_refs.push(child_ref);
                            }
                            let next_visible_index =
                                cursor.end::<usize, _>(child_ref_db).unwrap() + 1;
                            cursor
                                .seek_forward(&next_visible_index, SeekBias::Left, child_ref_db)
                                .unwrap();
                        } else {
                            break;
                        }
                    }

                    if child_refs.len() == 0 || depth > 0 && rng.gen_weighted_bool(5) {
                        return Some(path.to_string_lossy().into_owned());
                    } else {
                        let child_ref = rng.choose(&child_refs).unwrap();
                        path.push(child_ref.name.as_os_str());
                        parent_id = child_ref.child_id;
                        depth += 1;
                    }
                }
            }
        }

        fn paths<S: Store>(&self, db: &S) -> Vec<String> {
            let mut cursor = self.cursor(db).unwrap();
            let mut paths = Vec::new();
            loop {
                if let Some(path) = cursor.path() {
                    let mut path = path.to_string_lossy().into_owned();
                    if cursor.is_dir(db).unwrap().unwrap() {
                        path += "/";
                    }
                    paths.push(path);
                } else {
                    break;
                }
                cursor.next(db).unwrap();
            }
            paths
        }

        fn paths_with_ids<S: Store>(&self, db: &S) -> Vec<(id::Unique, String)> {
            self.paths(db)
                .into_iter()
                .map(|path| (self.id_for_path(&path, db).unwrap().unwrap(), path))
                .collect()
        }
    }

    fn gen_name<T: Rng>(rng: &mut T) -> String {
        let mut name = String::new();
        for _ in 0..rng.gen_range(1, 4) {
            name.push(rng.gen_range(b'a', b'z' + 1).into());
        }
        name
    }
}
