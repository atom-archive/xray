use btree::{self, NodeStore, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::ops::{Add, AddAssign};
use std::sync::Arc;

struct Tree {
    root_name: OsString,
    items: btree::Tree<Item>,
    // file_ids_by_inode: btree::Tree<InodeToFileId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Item {
    Metadata {
        file_id: id::Unique,
        is_dir: bool,
        inode: u64,
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
    is_dir: bool,
}

struct Builder {
    cursor: Cursor,
    inserted: Vec<Metadata>,
    removed: Vec<Metadata>,
}

struct Cursor {
    stack: Vec<btree::Cursor<Item>>,
}

impl Tree {
    fn new(root_name: OsString) -> Self {
        Self {
            root_name,
            items: btree::Tree::new(),
            // file_ids_by_inode: btree::Tree::new(),
        }
    }
}

impl Item {
    fn is_dir_metadata(&self) -> bool {
        match self {
            Item::Metadata { is_dir, .. } => *is_dir,
            _ => false
        }
    }

    fn is_dir_entry(&self) -> bool {
        match self {
            Item::DirEntry { .. } => true,
            _ => false
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

// impl Builder {
//     fn push_dir<S>(&mut self, name: &OsStr, metadata: Metadata, db: &S) -> Result<(), S::ReadError>
//     where
//         S: NodeStore<Item>,
//     {
//         let item = self.cursor.item(db)?;
//         if item.is_dir() {
//             match name.cmp(item.name()) {
//                 Ordering::Less => {}// Inserted,
//                 Ordering::Equal => {}// Check metadata to see if they're the same?
//                 Ordering::Greater => {}// `item` got removed
//             }
//         } else {
//             // Inserted
//         }
//     }
// }

impl Cursor {
    pub fn new<S>(tree: &Tree, db: &S) -> Result<Self, S::ReadError>
    where
        S: NodeStore<Item>,
    {
        let mut root_cursor = tree.items.cursor();
        root_cursor.seek(&Key::default(), SeekBias::Left, db)?;
        if let Some(item) = root_cursor.item(db)? {
            let mut cursor = Self {
                stack: vec![root_cursor],
            };
            cursor.follow_entry(db)?;
            Ok(cursor)
        } else {
            Ok(Self { stack: vec![] })
        }
    }

    pub fn metadata<S: NodeStore<Item>>(&self, db: &S) -> Result<Option<Metadata>, S::ReadError> {
        if let Some(cursor) = self.stack.last() {
            match cursor.item(db)?.unwrap() {
                Item::Metadata { is_dir, inode, .. } => Ok(Some(Metadata { is_dir, inode })),
                _ => unreachable!(),
            }
        } else {
            Ok(None)
        }
    }

    pub fn name<S: NodeStore<Item>>(&self, db: &S) -> Result<Option<Arc<OsString>>, S::ReadError> {
        if self.stack.len() > 1 {
            let cursor = &self.stack[self.stack.len() - 2];
            match cursor.item(db)?.unwrap() {
                Item::DirEntry { name, .. } => Ok(Some(name.clone())),
                _ => unreachable!(),
            }
        } else {
            Ok(None)
        }
    }

    pub fn depth(&self) -> usize {
        self.stack.len().saturating_sub(1)
    }

    pub fn next<S: NodeStore<Item>>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        while !self.stack.is_empty() {
            let found_entry = {
                let mut cursor = self.stack.last_mut().unwrap();
                let cur_item = cursor.item(db)?.unwrap();
                if cur_item.is_dir_entry() || cur_item.is_dir_metadata() {
                    cursor.next(db)?;
                    if let Some(next_item) = cursor.item(db)? {
                        next_item.is_dir_entry()
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            if found_entry {
                self.follow_entry(db)?;
                return Ok(true);
            } else {
                self.stack.pop();
            }
        }

        Ok(false)
    }

    fn follow_entry<S: NodeStore<Item>>(&mut self, db: &S) -> Result<(), S::ReadError> {
        let mut child_cursor;
        {
            let entry_cursor = self.stack.last().unwrap();
            match entry_cursor.item(db)?.unwrap() {
                Item::DirEntry { child_id, .. } => {
                    child_cursor = entry_cursor.clone();
                    child_cursor.seek(&Key::Metadata { file_id: child_id }, SeekBias::Left, db)?;
                }
                _ => panic!(),
            }
        }
        self.stack.push(child_cursor);
        Ok(())
    }
}
