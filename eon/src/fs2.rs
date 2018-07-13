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
pub struct ChildRefKey {
    parent_id: id::Unique,
    name: Arc<OsString>,
    timestamp: LamportTimestamp,
}

pub struct Tree {
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRef>,
    child_refs: btree::Tree<ChildRef>,
    inodes_to_file_ids: HashMap<Inode, id::Unique>,
}

impl btree::Item for Metadata {
    type Summary = id::Unique;

    fn summarize(&self) -> Self::Summary {
        self.file_id
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

impl btree::Item for ChildRef {
    type Summary = ChildRefKey;

    fn summarize(&self) -> Self::Summary {
        ChildRefKey {
            parent_id: self.child_id,
            name: self.name.clone(),
            timestamp: self.timestamp,
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
