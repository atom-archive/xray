use btree::{self, NodeStore, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::{self, Ordering};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::{Add, AddAssign};
use std::path::{Path, PathBuf};
use std::sync::Arc;

trait Store {
    type ReadError: fmt::Debug;
    type ItemStore: NodeStore<Item, ReadError = Self::ReadError>;
    type InodeToFileIdStore: NodeStore<InodeToFileId, ReadError = Self::ReadError>;

    fn item_store(&self) -> &Self::ItemStore;
    fn inode_to_file_id_store(&self) -> &Self::InodeToFileIdStore;
    fn gen_id(&self) -> id::Unique;
}

struct Tree {
    items: btree::Tree<Item>,
    file_ids_by_inode: btree::Tree<InodeToFileId>,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct Inode(u64);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Item {
    Metadata {
        file_id: id::Unique,
        is_dir: bool,
        inode: Inode,
    },
    DirEntry {
        file_id: id::Unique,
        entry_id: id::Unique,
        child_id: id::Unique,
        name: Arc<OsString>,
        is_dir: bool,
        deletions: SmallVec<[id::Unique; 1]>,
        moves: SmallVec<[Move; 1]>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Move {
    file_id: id::Unique,
    entry_id: id::Unique,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InodeToFileId {
    inode: Inode,
    file_id: id::Unique,
}

#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
struct Key {
    file_id: id::Unique,
    kind: KeyKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum KeyKind {
    Metadata,
    DirEntry { is_dir: bool, name: Arc<OsString> },
}

struct Metadata {
    inode: Inode,
    is_dir: bool,
}

struct Builder {
    tree: Tree,
    stack: Vec<id::Unique>,
    cursor: Cursor,
    item_changes: Vec<ItemChange>,
    new_mappings: Vec<InodeToFileId>,
    insertions_by_inode: HashMap<Inode, id::Unique>,
}

enum ItemChange {
    InsertDirEntry {
        file_id: id::Unique,
        name: OsString,
        is_dir: bool,
        child_id: id::Unique,
        child_inode: Inode,
    },
    RemoveDirEntry {
        entry: Item,
        // Use inode to determine if the removed entry was actually moved elsewhere. We can maintain
        // a temporary mapping of inodes to inserted entry ids on the builder so we know what moved
        // in the second pass.
        inode: Inode,
    },
}

struct Cursor {
    path: PathBuf,
    stack: Vec<btree::Cursor<Item>>,
}

impl Metadata {
    fn dir<I: Into<Inode>>(inode: I) -> Self {
        Metadata {
            is_dir: true,
            inode: inode.into(),
        }
    }

    fn file<I: Into<Inode>>(inode: I) -> Self {
        Metadata {
            is_dir: false,
            inode: inode.into(),
        }
    }
}

impl Tree {
    pub fn new() -> Self {
        Self {
            items: btree::Tree::new(),
            file_ids_by_inode: btree::Tree::new(),
        }
    }

    pub fn cursor<S: Store>(&self, db: &S) -> Result<Cursor, S::ReadError> {
        Cursor::new(self, db)
    }

    #[cfg(test)]
    fn paths<S: Store>(&self, store: &S) -> Vec<String> {
        let mut paths = Vec::new();
        let mut cursor = self.cursor(store).unwrap();
        loop {
            paths.push(cursor.path().to_string_lossy().into_owned());
            if !cursor.next(store).unwrap() {
                return paths;
            }
        }
    }
}

impl From<u64> for Inode {
    fn from(inode: u64) -> Self {
        Inode(inode)
    }
}

impl<'a> Add<&'a Self> for Inode {
    type Output = Inode;

    fn add(self, other: &Self) -> Self::Output {
        cmp::max(self, *other)
    }
}

impl<'a> AddAssign<&'a Self> for Inode {
    fn add_assign(&mut self, other: &Self) {
        *self = cmp::max(*self, *other);
    }
}

impl btree::Dimension for Inode {
    type Summary = Self;

    fn from_summary(summary: &Self) -> &Self {
        summary
    }
}

impl Item {
    fn key(&self) -> Key {
        match self {
            Item::Metadata { file_id, .. } => Key::metadata(*file_id),
            Item::DirEntry {
                file_id,
                is_dir,
                name,
                ..
            } => Key::dir_entry(*file_id, *is_dir, name.clone()),
        }
    }

    fn is_dir_metadata(&self) -> bool {
        match self {
            Item::Metadata { is_dir, .. } => *is_dir,
            _ => false,
        }
    }

    fn is_dir_entry(&self) -> bool {
        match self {
            Item::DirEntry { .. } => true,
            _ => false,
        }
    }
}

impl btree::Item for Item {
    type Summary = Key;

    fn summarize(&self) -> Self::Summary {
        self.key()
    }
}

impl Key {
    fn metadata(file_id: id::Unique) -> Self {
        Key {
            file_id,
            kind: KeyKind::Metadata,
        }
    }

    fn dir_entry(file_id: id::Unique, is_dir: bool, name: Arc<OsString>) -> Self {
        Key {
            file_id,
            kind: KeyKind::DirEntry { is_dir, name },
        }
    }
}

impl Default for Key {
    fn default() -> Self {
        Key::metadata(id::Unique::default())
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

impl PartialOrd for KeyKind {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KeyKind {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (KeyKind::Metadata, KeyKind::Metadata) => Ordering::Equal,
            (KeyKind::Metadata, KeyKind::DirEntry { .. }) => Ordering::Less,
            (KeyKind::DirEntry { .. }, KeyKind::Metadata) => Ordering::Greater,
            (
                KeyKind::DirEntry { is_dir, name },
                KeyKind::DirEntry {
                    is_dir: other_is_dir,
                    name: other_name,
                },
            ) => is_dir
                .cmp(other_is_dir)
                .reverse()
                .then_with(|| name.cmp(other_name)),
        }
    }
}

impl btree::Item for InodeToFileId {
    type Summary = Inode;

    fn summarize(&self) -> Inode {
        self.inode
    }
}

impl Builder {
    fn new<S: Store>(tree: Tree, db: &S) -> Result<Self, S::ReadError> {
        let cursor = tree.cursor(db)?;
        Ok(Self {
            tree,
            cursor,
            stack: Vec::new(),
            item_changes: Vec::new(),
            new_mappings: Vec::new(),
            insertions_by_inode: HashMap::new(),
        })
    }

    fn depth(&self) -> usize {
        self.stack.len()
    }

    fn push<N, S>(
        &mut self,
        name: N,
        metadata: Metadata,
        depth: usize,
        db: &S,
    ) -> Result<(), S::ReadError>
    where
        N: Into<OsString>,
        S: Store,
    {
        let name = name.into();
        match depth.cmp(&self.cursor.depth()) {
            Ordering::Less => {}
            Ordering::Equal => {}
            Ordering::Greater => {
                if self.stack.len() >= depth {
                    self.stack.truncate(depth - 1);
                }

                let parent_id = self.stack.last().cloned().unwrap_or(id::Unique::default());
                let child_id = db.gen_id();
                self.item_changes.push(ItemChange::InsertDirEntry {
                    file_id: parent_id,
                    is_dir: metadata.is_dir,
                    child_id,
                    child_inode: metadata.inode,
                    name,
                });
                if metadata.is_dir {
                    self.stack.push(child_id);
                }
            }
        }

        // loop {
        //     if let Some(existing_metadata) = self.cursor.metadata(db)? {
        //         let ordering = if metadata.is_dir {
        //             if existing_metadata.is_dir {
        //                 name.cmp(&self.cursor.name(db)?.unwrap())
        //             } else {
        //                 Ordering::Less
        //             }
        //         } else {
        //             if existing_metadata.is_dir {
        //                 Ordering::Greater
        //             } else {
        //                 name.cmp(&self.cursor.name(db)?.unwrap())
        //             }
        //         };
        //
        //         match ordering {
        //             Ordering::Less => {
        //                 let file_id = self.cursor.file_id(db)?.unwrap();
        //                 let entry_id = db.gen_id();
        //                 let child_id = self.find_or_create_file_id(&metadata, db)?;
        //                 self.item_changes.push(ItemChange::Insert(Item::DirEntry {
        //                     file_id,
        //                     entry_id,
        //                     child_id,
        //                     name: Arc::new(name.to_os_string()),
        //                     is_dir: metadata.is_dir,
        //                     deletions: SmallVec::new(),
        //                     moves: SmallVec::new(),
        //                 }));
        //                 if metadata.is_dir {
        //                     self.insertions_by_inode.insert(metadata.inode, entry_id);
        //                 }
        //                 break;
        //             }
        //             Ordering::Equal => {
        //                 self.cursor.next(db)?;
        //                 break;
        //             }
        //             Ordering::Greater => {
        //                 self.item_changes.push(ItemChange::Remove {
        //                     entry: self.cursor.dir_entry(db)?.unwrap(),
        //                     inode: existing_metadata.inode,
        //                 });
        //                 self.cursor.next_sibling(db)?;
        //             }
        //         }
        //     } else {
        //         let entry_id = db.gen_id();
        //         let child_id = self.find_or_create_file_id(&metadata, db)?;
        //         self.item_changes.push(ItemChange::Insert(Item::DirEntry {
        //             file_id: self.cursor.file_id(db)?.unwrap(),
        //             entry_id,
        //             child_id,
        //             name: Arc::new(name.to_os_string()),
        //             is_dir: metadata.is_dir,
        //             deletions: SmallVec::new(),
        //             moves: SmallVec::new(),
        //         }));
        //         if metadata.is_dir {
        //             self.insertions_by_inode.insert(metadata.inode, entry_id);
        //         }
        //         break;
        //     }
        // }

        Ok(())
    }

    // fn ascend<S>(&mut self, db: &S) -> Result<(), S::ReadError>
    // where
    //     S: Store,
    // {
    //     loop {
    //         self.item_changes.push(ItemChange::Remove {
    //             entry: self.cursor.dir_entry(db)?.unwrap(),
    //             inode: self.cursor.metadata(db)?.unwrap().inode,
    //         });
    //
    //         if !self.cursor.next_sibling(db)? {
    //             break;
    //         }
    //     }
    //
    //     Ok(())
    // }

    fn tree<S: Store>(self, db: &S) -> Result<Tree, S::ReadError> {
        let item_db = db.item_store();

        let mut new_items = Vec::new();
        for change in self.item_changes {
            match change {
                ItemChange::InsertDirEntry {
                    file_id: parent_dir_id,
                    name,
                    is_dir,
                    child_id,
                    child_inode,
                } => {
                    new_items.push(Item::Metadata {
                        file_id: child_id,
                        is_dir,
                        inode: child_inode,
                    });
                    new_items.push(Item::DirEntry {
                        file_id: parent_dir_id,
                        entry_id: db.gen_id(),
                        child_id,
                        name: Arc::new(name),
                        is_dir,
                        deletions: SmallVec::new(),
                        moves: SmallVec::new(),
                    });
                }
                ItemChange::RemoveDirEntry { .. } => unimplemented!(),
            }
        }

        new_items.sort_unstable_by_key(|item| item.key());
        let mut old_items_cursor = self.tree.items.cursor();
        let mut new_tree = Tree::new();
        for item in new_items {
            new_tree.items.push_tree(
                old_items_cursor.slice(&item.key(), SeekBias::Right, item_db)?,
                item_db,
            )?;
            new_tree.items.push(item, item_db)?;
        }
        new_tree
            .items
            .push_tree(old_items_cursor.suffix::<Key, _>(item_db)?, item_db)?;
        Ok(new_tree)
    }

    // fn find_or_create_file_id<S>(
    //     &mut self,
    //     metadata: &Metadata,
    //     db: &S,
    // ) -> Result<id::Unique, S::ReadError>
    // where
    //     S: Store,
    // {
    //     let inode_db = db.inode_to_file_id_store();
    //     let mut cursor = self.tree.file_ids_by_inode.cursor();
    //     cursor.seek(&metadata.inode, SeekBias::Left, inode_db)?;
    //     let mapping = cursor.item(inode_db)?;
    //     if mapping
    //         .as_ref()
    //         .map_or(false, |mapping| metadata.inode == mapping.inode)
    //     {
    //         Ok(mapping.unwrap().file_id)
    //     } else {
    //         let file_id = db.gen_id();
    //         self.new_mappings.push(InodeToFileId {
    //             inode: metadata.inode,
    //             file_id,
    //         });
    //         self.item_changes.push(ItemChange::Insert(Item::Metadata {
    //             file_id,
    //             is_dir: metadata.is_dir,
    //             inode: metadata.inode,
    //         }));
    //         Ok(file_id)
    //     }
    // }
}

impl Cursor {
    pub fn new<S>(tree: &Tree, db: &S) -> Result<Self, S::ReadError>
    where
        S: Store,
    {
        let item_db = db.item_store();
        let mut root_cursor = tree.items.cursor();
        root_cursor.seek(&Key::default(), SeekBias::Left, item_db)?;
        if let Some(item) = root_cursor.item(item_db)? {
            let mut cursor = Self {
                path: PathBuf::new(),
                stack: vec![root_cursor],
            };
            cursor.follow_entry(db)?;
            Ok(cursor)
        } else {
            Ok(Self {
                path: PathBuf::new(),
                stack: vec![],
            })
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata<S: Store>(&self, db: &S) -> Result<Option<Metadata>, S::ReadError> {
        if let Some(cursor) = self.stack.last() {
            match cursor.item(db.item_store())?.unwrap() {
                Item::Metadata { is_dir, inode, .. } => Ok(Some(Metadata { is_dir, inode })),
                _ => unreachable!(),
            }
        } else {
            Ok(None)
        }
    }

    pub fn dir_entry<S: Store>(&self, db: &S) -> Result<Option<Item>, S::ReadError> {
        if self.stack.len() > 1 {
            let cursor = &self.stack[self.stack.len() - 2];
            cursor.item(db.item_store())
        } else {
            Ok(None)
        }
    }

    pub fn file_id<S: Store>(&self, db: &S) -> Result<Option<id::Unique>, S::ReadError> {
        if self.stack.len() > 1 {
            let cursor = &self.stack[self.stack.len() - 2];
            match cursor.item(db.item_store())?.unwrap() {
                Item::DirEntry { file_id, .. } => Ok(Some(file_id)),
                _ => unreachable!(),
            }
        } else {
            Ok(None)
        }
    }

    pub fn name<S: Store>(&self, db: &S) -> Result<Option<Arc<OsString>>, S::ReadError> {
        if self.stack.len() > 1 {
            let cursor = &self.stack[self.stack.len() - 2];
            match cursor.item(db.item_store())?.unwrap() {
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

    pub fn next<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        let item_db = db.item_store();
        while !self.stack.is_empty() {
            let found_entry = {
                let mut cursor = self.stack.last_mut().unwrap();
                let cur_item = cursor.item(item_db)?.unwrap();
                if cur_item.is_dir_entry() || cur_item.is_dir_metadata() {
                    cursor.next(item_db)?;
                    if let Some(next_item) = cursor.item(item_db)? {
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
                self.path.pop();
                self.stack.pop();
            }
        }

        Ok(false)
    }

    pub fn next_sibling<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        if self.stack.is_empty() {
            Ok(false)
        } else {
            let prev_depth = self.depth();
            self.stack.pop();
            self.path.pop();
            self.next(db)?;
            Ok(self.depth() == prev_depth)
        }
    }

    fn follow_entry<S: Store>(&mut self, db: &S) -> Result<(), S::ReadError> {
        let item_db = db.item_store();
        let mut child_cursor;
        {
            let entry_cursor = self.stack.last().unwrap();
            match entry_cursor.item(item_db)?.unwrap() {
                Item::DirEntry { child_id, name, .. } => {
                    child_cursor = entry_cursor.clone();
                    child_cursor.seek(&Key::metadata(child_id), SeekBias::Left, item_db)?;
                    self.path.push(name.as_ref());
                }
                _ => panic!(),
            }
        }
        self.stack.push(child_cursor);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use self::rand::{Rng, SeedableRng, StdRng};
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_builder_basic() {
        let db = NullStore::new();
        let tree = Tree::new();
        let mut builder = Builder::new(tree, &db).unwrap();
        builder.push("a", Metadata::dir(1), 1, &db).unwrap();
        builder.push("b", Metadata::dir(2), 2, &db).unwrap();
        builder.push("c", Metadata::dir(3), 3, &db).unwrap();
        builder.push("d", Metadata::dir(4), 3, &db).unwrap();
        builder.push("e", Metadata::file(5), 3, &db).unwrap();
        builder.push("f", Metadata::dir(6), 1, &db).unwrap();
        let tree = builder.tree(&db).unwrap();
        assert_eq!(
            tree.paths(&db),
            ["a", "a/b", "a/b/c", "a/b/d", "a/b/e", "f"]
        );
    }

    #[test]
    fn test_key_ordering() {
        assert!(
            Key::dir_entry(id::Unique::default(), true, Arc::new("z".into()))
                < Key::dir_entry(id::Unique::default(), false, Arc::new("a".into()))
        );
    }

    #[derive(Debug)]
    struct NullStore {
        next_id: RefCell<id::Unique>,
    }

    impl NullStore {
        fn new() -> Self {
            Self {
                next_id: RefCell::new(id::Unique::random()),
            }
        }
    }

    impl Store for NullStore {
        type ReadError = ();
        type ItemStore = NullStore;
        type InodeToFileIdStore = NullStore;

        fn gen_id(&self) -> id::Unique {
            let next_id = self.next_id.borrow().clone();
            self.next_id.borrow_mut().inc();
            next_id
        }

        fn item_store(&self) -> &Self::ItemStore {
            self
        }

        fn inode_to_file_id_store(&self) -> &Self::InodeToFileIdStore {
            self
        }
    }

    impl btree::NodeStore<Item> for NullStore {
        type ReadError = ();

        fn get(&self, _id: btree::NodeId) -> Result<Arc<btree::Node<Item>>, Self::ReadError> {
            panic!("get should never be called on a null store")
        }
    }

    impl btree::NodeStore<InodeToFileId> for NullStore {
        type ReadError = ();

        fn get(
            &self,
            _id: btree::NodeId,
        ) -> Result<Arc<btree::Node<InodeToFileId>>, Self::ReadError> {
            panic!("get should never be called on a null store")
        }
    }
}
