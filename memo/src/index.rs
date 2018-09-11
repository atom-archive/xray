use btree::{Dimension, Item, SeekBias, Tree};
#[cfg(test)]
use rand::Rng;
use std::ffi::{OsStr, OsString};
use std::ops::{Add, AddAssign};
use std::path::Path;
use std::sync::Arc;

const ROOT_DIR_ID: usize = 0;

type FileId = usize;

#[derive(Clone)]
pub struct Index {
    files: Tree<File>,
    dir_entries: Tree<DirEntry>,
    next_id: FileId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct File {
    file_id: FileId,
    parent_id: FileId,
    name: Arc<OsString>,
    is_dir: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirEntry {
    parent_id: FileId,
    name: Arc<OsString>,
    child_id: FileId,
}

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct DirEntryKey {
    parent_id: FileId,
    name: Arc<OsString>,
}

impl Index {
    pub fn new() -> Self {
        Self {
            files: Tree::new(),
            dir_entries: Tree::new(),
            next_id: 1,
        }
    }

    /// This method will create any directories along the path that don't already exist in the
    /// index. It returns an array of file ids. The first is the id of the last directory that
    /// already exists. After that are the ids of newly created directories.
    pub fn create_dir_all(&mut self, path: &Path) -> Vec<FileId> {
        unimplemented!()
    }
}

impl Item for File {
    type Summary = FileId;

    fn summarize(&self) -> Self::Summary {
        self.file_id
    }
}

impl Item for DirEntry {
    type Summary = DirEntryKey;

    fn summarize(&self) -> Self::Summary {
        DirEntryKey {
            parent_id: self.parent_id,
            name: self.name.clone(),
        }
    }
}

impl Dimension<DirEntryKey> for DirEntryKey {
    fn from_summary(summary: &DirEntryKey) -> Self {
        summary.clone()
    }
}

impl<'a> Add<&'a Self> for DirEntryKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self <= *other);
        other.clone()
    }
}

impl<'a> AddAssign<&'a Self> for DirEntryKey {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self < *other);
        *self = other.clone();
    }
}
