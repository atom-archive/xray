use btree::{self, NodeStore, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::ops::AddAssign;
use std::path::PathBuf;
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

type Inode = u64;
type VisibleCount = usize;

const ROOT_ID: id::Unique = id::Unique::DEFAULT;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct LamportTimestamp {
    value: u64,
    replica_id: id::ReplicaId,
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
    op_id: id::Unique,
    prev_timestamp: LamportTimestamp,
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

#[derive(Clone)]
pub struct Tree {
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRef>,
    child_refs: btree::Tree<ChildRef>,
    inodes_to_file_ids: HashMap<Inode, id::Unique>,
}

struct TreeCursor {
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

impl btree::Item for Metadata {
    type Summary = id::Unique;

    fn summarize(&self) -> Self::Summary {
        self.file_id
    }
}

impl btree::Dimension<id::Unique> for id::Unique {
    fn from_summary(summary: &id::Unique) -> &Self {
        summary
    }
}

impl btree::Item for ParentRef {
    type Summary = ParentRefKey;

    fn summarize(&self) -> Self::Summary {
        ParentRefKey {
            child_id: self.child_id,
            ref_id: self.ref_id,
            timestamp: self.timestamp,
        }
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

impl ChildRef {
    fn is_visible(&self) -> bool {
        self.deletions.is_empty()
    }
}

impl btree::Item for ChildRef {
    type Summary = ChildRefSummary;

    fn summarize(&self) -> Self::Summary {
        ChildRefSummary {
            parent_id: self.child_id,
            name: self.name.clone(),
            timestamp: self.timestamp,
            visible_count: if self.deletions.is_empty() { 1 } else { 0 },
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

impl btree::Dimension<ChildRefSummary> for id::Unique {
    fn from_summary(summary: &ChildRefSummary) -> &Self {
        &summary.parent_id
    }
}

impl btree::Dimension<ChildRefSummary> for VisibleCount {
    fn from_summary(summary: &ChildRefSummary) -> &Self {
        &summary.visible_count
    }
}

impl Tree {
    fn cursor<S: Store>(&self, db: &S) -> Result<TreeCursor, S::ReadError> {
        let mut cursor = TreeCursor {
            path: PathBuf::new(),
            stack: Vec::new(),
            metadata_cursor: self.metadata.cursor(),
        };
        cursor.descend(self.child_refs.cursor(), ROOT_ID, db)?;
        Ok(cursor)
    }
}

impl TreeCursor {
    fn depth(&self) -> usize {
        self.stack.len()
    }

    fn name<S: Store>(&self, db: &S) -> Result<Option<Arc<OsString>>, S::ReadError> {
        let db = db.child_ref_store();

        if self.stack.is_empty() {
            Ok(None)
        } else {
            let child_ref = self.stack.last().unwrap().item(db)?.unwrap();
            Ok(Some(child_ref.name.clone()))
        }
    }

    fn inode<S: Store>(&self, db: &S) -> Result<Option<Inode>, S::ReadError> {
        Ok(self
            .metadata_cursor
            .item(db.metadata_store())?
            .and_then(|metadata| metadata.inode))
    }

    fn next<S: Store>(&mut self, db: &S) -> Result<(), S::ReadError> {
        while self.stack.len() > 0 {
            let metadata = self.metadata_cursor.item(db.metadata_store())?.unwrap();
            if metadata.is_dir {
                let child_ref_cursor = self.stack.last().unwrap().clone();
                let dir_id = child_ref_cursor
                    .item(db.child_ref_store())?
                    .unwrap()
                    .child_id;
                if self.descend(child_ref_cursor, dir_id, db)? || self.next_sibling(db)? {
                    break;
                }
            } else if self.next_sibling(db)? {
                break;
            }

            self.path.pop();
            self.stack.pop();
        }

        Ok(())
    }

    fn descend<S: Store>(
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

// impl Iterator for TreeDiff {
//     type Item = TreeDiffItem;
//
//     fn next(&mut self) -> Option<Self::Item> {}
// }
