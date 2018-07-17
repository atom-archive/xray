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

struct TreeDiff {
    tree: Tree,
    prev_tree: Tree,
}

pub enum TreeDiffItem {
    Keep {
        depth: usize,
        name: Arc<OsString>,
    },
    Insert {
        depth: usize,
        name: Arc<OsString>,
        src_path: Option<PathBuf>,
    },
    Remove {
        depth: usize,
        name: Arc<OsString>,
        dst_path: Option<PathBuf>,
    },
}

pub enum Operation {
    InsertDir {
        op_id: id::Unique,
        timestamp: LamportTimestamp,
        parent_id: id::Unique,
        name: Arc<OsString>,
    },
    RemoveDir {
        op_id: id::Unique,
        child_id: id::Unique,
        timestamp: LamportTimestamp,
        prev_timestamp: LamportTimestamp,
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
    ref_id: id::Unique,
    timestamp: LamportTimestamp,
    prev_timestamp: LamportTimestamp,
    op_id: id::Unique,
    parent: Option<(id::Unique, Arc<OsString>)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParentRefKey {
    child_id: id::Unique,
    ref_id: id::Unique,
    timestamp: LamportTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildRef {
    parent_id: id::Unique,
    name: Arc<OsString>,
    timestamp: LamportTimestamp,
    ref_id: id::Unique,
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
pub struct ParentIdAndName(id::Unique, Arc<OsString>);

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

    pub fn cursor<S: Store>(&self, db: &S) -> Result<TreeCursor, S::ReadError> {
        let mut cursor = TreeCursor {
            path: PathBuf::new(),
            stack: Vec::new(),
            metadata_cursor: self.metadata.cursor(),
        };
        cursor.descend_into(self.child_refs.cursor(), ROOT_ID, db)?;
        Ok(cursor)
    }

    pub fn insert_dirs<I: Into<PathBuf>, S: Store>(
        &mut self,
        path: I,
        db: &S,
    ) -> Result<(), S::ReadError> {
        let path = path.into();
        let child_ref_db = db.child_ref_store();

        let mut operations = Vec::new();
        let mut cursor = self.child_refs.cursor();
        let mut parent_id = ROOT_ID;
        let mut entry_exists = true;

        for name in path.components() {
            let name = Arc::new(OsString::from(name.as_os_str()));
            if entry_exists {
                let key = ParentIdAndName(parent_id, name.clone());
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

        self.integrate_ops(operations, db)
    }

    pub fn remove_dir<I: Into<PathBuf>, S: Store>(
        &mut self,
        path: I,
        db: &S,
    ) -> Result<(), S::ReadError> {
        let path = path.into();
        let child_ref_db = db.child_ref_store();
        let parent_ref_db = db.parent_ref_store();

        let mut cursor = self.child_refs.cursor();
        let mut child_id = ROOT_ID;
        for name in path.components() {
            let name = Arc::new(OsString::from(name.as_os_str()));
            let key = ParentIdAndName(child_id, name);
            if cursor.seek(&key, SeekBias::Left, child_ref_db)? {
                let child_ref = cursor.item(child_ref_db)?.unwrap();
                if child_ref.is_visible() {
                    child_id = child_ref.child_id;
                } else {
                    panic!("Directory does not exist");
                }
            } else {
                panic!("Directory does not exist");
            }
        }

        let mut parent_ref_cursor = self.parent_refs.cursor();
        parent_ref_cursor.seek(&child_id, SeekBias::Left, parent_ref_db)?;
        let prev_parent_ref = parent_ref_cursor.item(parent_ref_db)?.unwrap();
        let operation = Operation::RemoveDir {
            op_id: db.gen_id(),
            child_id,
            timestamp: db.gen_timestamp(),
            prev_timestamp: prev_parent_ref.timestamp,
        };
        self.integrate_ops(Some(operation), db)
    }

    fn integrate_ops<I, S>(&mut self, ops: I, db: &S) -> Result<(), S::ReadError>
    where
        I: IntoIterator<Item = Operation>,
        S: Store,
    {
        let metadata_db = db.metadata_store();
        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        let mut metadata = Vec::new();
        let mut parent_refs = Vec::new();
        let mut child_refs = Vec::new();

        for op in ops {
            match op {
                Operation::InsertDir {
                    op_id,
                    timestamp,
                    parent_id,
                    name,
                } => {
                    metadata.push(Metadata {
                        file_id: op_id,
                        is_dir: true,
                        inode: None,
                    });
                    parent_refs.push(ParentRef {
                        child_id: op_id,
                        ref_id: op_id,
                        timestamp,
                        prev_timestamp: timestamp,
                        op_id,
                        parent: Some((parent_id, name.clone())),
                    });
                    child_refs.push(ChildRef {
                        parent_id,
                        name,
                        timestamp,
                        ref_id: op_id,
                        op_id,
                        child_id: op_id,
                        deletions: SmallVec::new(),
                    });
                }
                Operation::RemoveDir {
                    op_id,
                    child_id,
                    timestamp,
                    prev_timestamp,
                } => {
                    let new_parent_ref = ParentRef {
                        child_id,
                        ref_id: child_id,
                        timestamp,
                        prev_timestamp,
                        op_id,
                        parent: None,
                    };

                    let mut child_ref_cursor = self.child_refs.cursor();
                    let mut parent_ref_cursor = self.parent_refs.cursor();
                    parent_ref_cursor.seek(&new_parent_ref.key(), SeekBias::Left, parent_ref_db)?;
                    while let Some(parent_ref) = parent_ref_cursor.item(parent_ref_db)? {
                        if parent_ref.child_id != child_id {
                            break;
                        } else if parent_ref.timestamp >= prev_timestamp {
                            if let Some(child_ref_key) = parent_ref.to_child_ref_key() {
                                child_ref_cursor.seek(
                                    &child_ref_key,
                                    SeekBias::Left,
                                    child_ref_db,
                                )?;
                                let mut child_ref = child_ref_cursor.item(child_ref_db)?.unwrap();
                                child_ref.deletions.push(op_id);
                                child_refs.push(child_ref);
                            }
                            parent_ref_cursor.next(parent_ref_db)?;
                        } else {
                            break;
                        }
                    }

                    parent_refs.push(new_parent_ref);
                }
            }
        }

        self.metadata = interleave(&self.metadata, metadata, metadata_db)?;
        self.parent_refs = interleave(&self.parent_refs, parent_refs, parent_ref_db)?;
        self.child_refs = interleave(&self.child_refs, child_refs, child_ref_db)?;

        Ok(())
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
                } else {
                    if self.next_sibling(db)? {
                        Ok(true)
                    } else {
                        self.path.pop();
                        self.stack.pop();
                        Ok(false)
                    }
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
            ref_id: self.ref_id,
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
            .then_with(|| self.ref_id.cmp(&other.ref_id))
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
        ParentIdAndName(summary.parent_id, summary.name.clone())
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
    use super::*;
    use std::cell::Cell;

    #[test]
    fn test_insert_dirs() {
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
    }
}
