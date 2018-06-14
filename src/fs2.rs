use btree::{self, NodeStore, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::ops::{Add, AddAssign};
use std::path::PathBuf;
use std::sync::Arc;

struct Tree {
    root_name: OsString,
    items: btree::Tree<Item>,
    file_ids_by_inode: btree::Tree<InodeToFileId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Item {
    Metadata {
        file_id: id::Unique,
        is_dir: bool,
    },
    DirEntry {
        file_id: id::Unique,
        entry_id: id::Unique,
        child_id: id::Unique,
        name: Arc<OsString>,
        is_dir: bool,
        deletions: SmallVec<[id::Unique; 1]>,
    },
}

struct InodeToFileId {
    inode: u64,
    file_id: id::Unique,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Key {
    Metadata {
        file_id: id::Unique,
    },
    DirEntry {
        file_id: id::Unique,
        is_dir: bool,
        name: Arc<OsString>,
    },
}

struct Metadata {
    inode: u64,
}

struct Builder {
    cursor: Cursor,
    inserted: Vec<Metadata>,
    removed: Vec<Metadata>,
}

struct Cursor {
    stack: Vec<id::Unique>,
    tree_cursor: btree::Cursor<Item>,
}

impl Tree {}

impl Item {
    fn name(&self) -> &OsStr {
        match self {
            Item::DirEntry { name, .. } => name,
            Item::Metadata { .. } => panic!(),
        }
    }

    fn is_dir(&self) -> bool {
        match self {
            Item::DirEntry { is_dir, .. } => *is_dir,
            Item::Metadata { .. } => panic!(),
        }
    }
}

impl btree::Item for Item {
    type Summary = Key;

    fn summarize(&self) -> Key {
        match self {
            Item::Metadata { file_id, .. } => Key::Metadata { file_id: *file_id },
            Item::DirEntry {
                file_id,
                is_dir,
                name,
                ..
            } => Key::DirEntry {
                file_id: *file_id,
                is_dir: *is_dir,
                name: name.clone(),
            },
        }
    }
}

impl Default for Key {
    fn default() -> Self {
        Key::Metadata {
            file_id: id::Unique::default(),
        }
    }
}

impl btree::Dimension for Key {
    type Summary = Self;

    fn from_summary(summary: &Self::Summary) -> &Self {
        summary
    }
}

impl<'a> AddAssign<&'a Self> for Key {
    fn add_assign(&mut self, other: &Self) {
        if *self < *other {
            *self = other.clone();
        }
    }
}

impl<'a> Add<&'a Self> for Key {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        if self < *other {
            other.clone()
        } else {
            self
        }
    }
}

impl Builder {
    fn push_dir<S>(&mut self, name: &OsStr, metadata: Metadata, db: &S) -> Result<(), S::ReadError>
    where
        S: NodeStore<Item>,
    {
        let item = self.cursor.item(db)?;
        if item.is_dir() {
            match name.cmp(item.name()) {
                Ordering::Less => {}// Inserted,
                Ordering::Equal => {}// Check metadata to see if they're the same?
                Ordering::Greater => {}// `item` got removed
            }
        } else {
            // Inserted
        }
    }
}

impl Cursor {
    fn new<S>(root_name: &OsStr, tree: &Tree, db: &S) -> Result<Self, S::ReadError>
    where
        S: NodeStore<Item>,
    {
        let mut tree_cursor = tree.items.cursor();
        tree_cursor.seek(&Key::default(), SeekBias::Left, db)?;
        Ok(Self {
            tree_cursor,
            stack: vec![],
        })
    }

    fn item<S>(&self, db: &S) -> Result<Item, S::ReadError>
    where
        S: NodeStore<Item>,
    {
        Ok(self.tree_cursor.item(db)?.unwrap())
    }
}
