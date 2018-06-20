use btree::{self, NodeStore, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::{self, Ordering};
use std::collections::{HashMap, HashSet};
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

#[derive(Clone)]
struct Tree {
    items: btree::Tree<Item>,
    inodes_to_file_ids: HashMap<Inode, id::Unique>,
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
    dir_stack: Vec<id::Unique>,
    cursor_stack: Vec<(usize, Cursor)>,
    item_changes: Vec<ItemChange>,
    inodes_to_file_ids: HashMap<Inode, id::Unique>,
    visited_inodes: HashSet<Inode>,
}

#[derive(Debug)]
enum ItemChange {
    InsertMetadata {
        file_id: id::Unique,
        is_dir: bool,
        inode: Inode,
    },
    InsertDirEntry {
        file_id: id::Unique,
        name: OsString,
        is_dir: bool,
        child_id: id::Unique,
        child_inode: Inode,
    },
    RemoveDirEntry {
        entry: Item,
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
            inodes_to_file_ids: HashMap::new(),
        }
    }

    pub fn cursor<S: Store>(&self, db: &S) -> Result<Cursor, S::ReadError> {
        Cursor::within_root(self, db)
    }

    pub fn id_for_path<I: Into<PathBuf>, S: Store>(
        &self,
        path: I,
        db: &S,
    ) -> Result<Option<id::Unique>, S::ReadError> {
        let path = path.into();
        let item_db = db.item_store();
        let mut parent_file_id = id::Unique::default();
        let mut cursor = self.items.cursor();

        for component in path.components() {
            cursor.seek(
                &Key {
                    file_id: parent_file_id,
                    kind: KeyKind::Metadata,
                },
                SeekBias::Right,
                item_db,
            )?;

            let component_name = Arc::new(OsString::from(component.as_os_str()));

            cursor.seek_forward(
                &Key {
                    file_id: parent_file_id,
                    kind: KeyKind::DirEntry {
                        is_dir: true,
                        name: component_name.clone(),
                    },
                },
                SeekBias::Left,
                item_db,
            )?;

            match cursor.item(item_db)? {
                Some(Item::DirEntry {
                    name: entry_name,
                    child_id,
                    ..
                }) => {
                    if component_name == entry_name {
                        parent_file_id = child_id;
                    } else {
                        return Ok(None);
                    }
                }
                _ => return Ok(None),
            }
        }

        Ok(Some(parent_file_id))
    }

    #[cfg(test)]
    fn paths<S: Store>(&self, store: &S) -> Vec<String> {
        let mut paths = Vec::new();
        let mut cursor = self.cursor(store).unwrap();
        while let Some(path) = cursor.path().map(|p| p.to_string_lossy().into_owned()) {
            paths.push(path);
            cursor.next(store).unwrap();
        }
        paths
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

    fn is_dir(&self) -> bool {
        match self {
            Item::Metadata { is_dir, .. } => *is_dir,
            Item::DirEntry { is_dir, .. } => *is_dir,
        }
    }

    fn inode(&self) -> Inode {
        match self {
            Item::Metadata { inode, .. } => *inode,
            Item::DirEntry { .. } => panic!(),
        }
    }

    fn name(&self) -> Arc<OsString> {
        match self {
            Item::DirEntry { name, .. } => name.clone(),
            Item::Metadata { .. } => panic!(),
        }
    }

    fn child_id(&self) -> id::Unique {
        match self {
            Item::DirEntry { child_id, .. } => *child_id,
            Item::Metadata { .. } => panic!(),
        }
    }

    fn is_metadata(&self) -> bool {
        match self {
            Item::Metadata { .. } => true,
            _ => false,
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

    fn is_deleted(&self) -> bool {
        match self {
            Item::DirEntry { deletions, .. } => !deletions.is_empty(),
            _ => false,
        }
    }

    fn deletions_mut(&mut self) -> &mut SmallVec<[id::Unique; 1]> {
        match self {
            Item::DirEntry { deletions, .. } => deletions,
            _ => panic!(),
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
    pub fn new<S: Store>(tree: Tree, db: &S) -> Result<Self, S::ReadError> {
        let cursor = tree.cursor(db)?;
        let inodes_to_file_ids = tree.inodes_to_file_ids.clone();
        Ok(Self {
            tree,
            dir_stack: Vec::new(),
            cursor_stack: vec![(0, cursor)],
            item_changes: Vec::new(),
            inodes_to_file_ids,
            visited_inodes: HashSet::new(),
        })
    }

    pub fn push<N, S>(
        &mut self,
        new_name: N,
        new_metadata: Metadata,
        new_depth: usize,
        db: &S,
    ) -> Result<(), S::ReadError>
    where
        N: Into<OsString>,
        S: Store,
    {
        debug_assert!(new_depth > 0);
        let new_name = new_name.into();
        println!("push {:?} {:?}", new_name, new_metadata.inode);

        self.dir_stack.truncate(new_depth - 1);

        // println!("Cursor stack depth at beginning of push {:?}", self.cursor_stack.len());
        // Delete old entries that precede the pushed entry. If we encounter an equivalent entry,
        // consume it if its inode has not yet been visited by this builder instance.
        loop {
            let old_depth = self.old_depth();
            if old_depth < new_depth {
                println!(
                    "BREAK. Old Depth: {:?}. New Depth: {:?}",
                    old_depth, new_depth
                );
                break;
            }

            let old_entry = self.old_dir_entry_item(db)?.unwrap();
            let old_inode = self.old_inode(db)?.unwrap();

            println!("{:?}", old_entry);
            if old_depth == new_depth {
                match cmp_dir_entries(
                    new_metadata.is_dir,
                    new_name.as_os_str(),
                    old_entry.is_dir(),
                    old_entry.name().as_os_str(),
                ) {
                    Ordering::Less => break,
                    Ordering::Equal => {
                        if !self.visited_inodes.contains(&old_inode) {
                            println!("{:?}", self.visited_inodes);
                            self.visited_inodes.insert(old_inode);
                            self.visited_inodes.insert(new_metadata.inode);
                            self.dir_stack.push(old_entry.child_id());
                            self.next_old_entry(db)?;
                            return Ok(());
                        }
                    }
                    Ordering::Greater => {}
                }
            }

            println!(
                "Inside push: Removing {:?} {:?}",
                old_entry.name(),
                old_inode
            );
            // println!("Cursor stack depth {:?}", self.cursor_stack.len());
            self.item_changes.push(ItemChange::RemoveDirEntry {
                entry: old_entry,
                inode: old_inode,
            });
            self.next_old_entry_sibling(db)?;
        }

        // If we make it this far, we did not find an old dir entry that's equivalent to the entry
        // we are pushing. We need to insert a new one. If the inode for the new entry matches an
        // existing file, we recycle its id so long as we have not already visited it.

        let parent_id = self
            .dir_stack
            .last()
            .cloned()
            .unwrap_or(id::Unique::default());
        let child_id = {
            let file_id = self.inodes_to_file_ids.get(&new_metadata.inode).cloned();
            if self.visited_inodes.contains(&new_metadata.inode) || file_id.is_none() {
                let child_id = db.gen_id();
                self.item_changes.push(ItemChange::InsertMetadata {
                    file_id: child_id,
                    is_dir: new_metadata.is_dir,
                    inode: new_metadata.inode,
                });
                self.inodes_to_file_ids.insert(new_metadata.inode, child_id);
                child_id
            } else {
                let file_id = file_id.unwrap();
                self.jump_to_old_entry(new_depth, file_id, db)?;
                file_id
            }
        };

        self.dir_stack.push(child_id);
        self.visited_inodes.insert(new_metadata.inode);
        self.item_changes.push(ItemChange::InsertDirEntry {
            file_id: parent_id,
            is_dir: new_metadata.is_dir,
            child_id,
            child_inode: new_metadata.inode,
            name: new_name.clone(),
        });
        Ok(())
    }

    pub fn tree<S: Store>(mut self, db: &S) -> Result<Tree, S::ReadError> {
        let item_db = db.item_store();
        while let Some(entry) = self.old_dir_entry_item(db)? {
            let inode = self.old_inode(db)?.unwrap();
            self.item_changes
                .push(ItemChange::RemoveDirEntry { entry, inode });
            self.next_old_entry_sibling(db)?;
        }

        // println!("tree, item changes: {:#?}", self.item_changes);

        let mut new_items = Vec::new();
        for change in self.item_changes {
            match change {
                ItemChange::InsertMetadata {
                    file_id,
                    is_dir,
                    inode,
                } => {
                    new_items.push(Item::Metadata {
                        file_id,
                        is_dir,
                        inode,
                    });
                }
                ItemChange::InsertDirEntry {
                    file_id: parent_dir_id,
                    name,
                    is_dir,
                    child_id,
                    child_inode,
                } => {
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
                ItemChange::RemoveDirEntry { mut entry, .. } => {
                    entry.deletions_mut().push(db.gen_id());
                    new_items.push(entry);
                }
            }
        }

        new_items.sort_unstable_by_key(|item| item.key());
        let mut old_items_cursor = self.tree.items.cursor();
        let mut new_tree = Tree {
            items: btree::Tree::new(),
            inodes_to_file_ids: self.inodes_to_file_ids,
        };
        for item in new_items {
            new_tree.items.push_tree(
                old_items_cursor.slice(&item.key(), SeekBias::Left, item_db)?,
                item_db,
            )?;
            if item.is_deleted() {
                old_items_cursor.next(item_db)?;
            }
            new_tree.items.push(item, item_db)?;
        }
        new_tree
            .items
            .push_tree(old_items_cursor.suffix::<Key, _>(item_db)?, item_db)?;
        Ok(new_tree)
    }

    fn old_depth(&self) -> usize {
        let (base_depth, cursor) = self.cursor_stack.last().unwrap();
        base_depth + cursor.depth()
    }

    fn old_dir_entry_item<S: Store>(&self, db: &S) -> Result<Option<Item>, S::ReadError> {
        self.cursor_stack.last().unwrap().1.dir_entry_item(db)
    }

    fn old_inode<S: Store>(&self, db: &S) -> Result<Option<Inode>, S::ReadError> {
        self.cursor_stack.last().unwrap().1.inode(db)
    }

    fn next_old_entry<S: Store>(&mut self, db: &S) -> Result<(), S::ReadError> {
        if !self.cursor_stack.last_mut().unwrap().1.next(db)? {
            if self.cursor_stack.len() > 1 {
                self.cursor_stack.pop();
            }
        }
        Ok(())
    }

    fn next_old_entry_sibling<S: Store>(&mut self, db: &S) -> Result<(), S::ReadError> {
        if !self.cursor_stack.last_mut().unwrap().1.next_sibling(db)? {
            if self.cursor_stack.len() > 1 {
                self.cursor_stack.pop();
            }
        }
        Ok(())
    }

    fn jump_to_old_entry<S>(
        &mut self,
        base_depth: usize,
        file_id: id::Unique,
        db: &S,
    ) -> Result<(), S::ReadError>
    where
        S: Store,
    {
        println!("Jumping to {:?}", file_id);
        if let Some(cursor) = Cursor::within_dir(file_id, &self.tree, db)? {
            self.cursor_stack.push((base_depth, cursor));
        }
        Ok(())
    }
}

impl Cursor {
    fn within_root<S>(tree: &Tree, db: &S) -> Result<Self, S::ReadError>
    where
        S: Store,
    {
        let item_db = db.item_store();
        let mut root_cursor = tree.items.cursor();
        root_cursor.seek(&Key::default(), SeekBias::Left, item_db)?;
        while let Some(item) = root_cursor.item(item_db)? {
            if item.is_metadata() {
                break;
            } else if item.is_dir_entry() && !item.is_deleted() {
                let mut cursor = Self {
                    path: PathBuf::new(),
                    stack: vec![root_cursor],
                };
                cursor.follow_entry(db)?;
                return Ok(cursor);
            } else {
                root_cursor.next(item_db)?;
            }
        }

        Ok(Self {
            path: PathBuf::new(),
            stack: vec![],
        })
    }

    fn within_dir<S>(file_id: id::Unique, tree: &Tree, db: &S) -> Result<Option<Self>, S::ReadError>
    where
        S: Store,
    {
        let item_db = db.item_store();
        let mut root_cursor = tree.items.cursor();
        root_cursor.seek(&Key::metadata(file_id), SeekBias::Right, item_db)?;
        while let Some(item) = root_cursor.item(item_db)? {
            if item.is_metadata() {
                break;
            } else if item.is_dir_entry() && !item.is_deleted() {
                let mut cursor = Self {
                    path: PathBuf::new(),
                    stack: vec![root_cursor],
                };
                cursor.follow_entry(db)?;
                return Ok(Some(cursor));
            } else {
                root_cursor.next(item_db)?;
            }
        }

        Ok(None)
    }

    pub fn depth(&self) -> usize {
        self.stack.len().saturating_sub(1)
    }

    pub fn path(&self) -> Option<&Path> {
        if self.stack.is_empty() {
            None
        } else {
            Some(&self.path)
        }
    }

    pub fn dir_entry_item<S: Store>(&self, db: &S) -> Result<Option<Item>, S::ReadError> {
        if self.stack.len() > 1 {
            let cursor = &self.stack[self.stack.len() - 2];
            cursor.item(db.item_store())
        } else {
            Ok(None)
        }
    }

    pub fn inode<S: Store>(&self, db: &S) -> Result<Option<Inode>, S::ReadError> {
        if let Some(cursor) = self.stack.last() {
            match cursor.item(db.item_store())?.unwrap() {
                Item::Metadata { inode, .. } => Ok(Some(inode)),
                _ => unreachable!(),
            }
        } else {
            Ok(None)
        }
    }

    pub fn next<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        let item_db = db.item_store();
        while !self.stack.is_empty() {
            let found_entry = loop {
                let mut cursor = self.stack.last_mut().unwrap();
                let cur_item = cursor.item(item_db)?.unwrap();
                if cur_item.is_dir_entry() || cur_item.is_dir_metadata() {
                    cursor.next(item_db)?;
                    let next_item = cursor.item(item_db)?;
                    if next_item.as_ref().map_or(false, |i| i.is_dir_entry()) {
                        if next_item.unwrap().is_deleted() {
                            continue;
                        } else {
                            break true;
                        }
                    } else {
                        break false;
                    }
                } else {
                    break false;
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

fn cmp_dir_entries(a_is_dir: bool, a_name: &OsStr, b_is_dir: bool, b_name: &OsStr) -> Ordering {
    a_is_dir
        .cmp(&b_is_dir)
        .reverse()
        .then_with(|| a_name.cmp(b_name))
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

        let mut builder = Builder::new(tree, &db).unwrap();
        builder.push("a", Metadata::dir(1), 1, &db).unwrap();
        builder.push("b", Metadata::dir(2), 2, &db).unwrap();
        builder.push("c", Metadata::dir(3), 3, &db).unwrap();
        builder.push("c2", Metadata::dir(7), 3, &db).unwrap();
        builder.push("d", Metadata::dir(4), 3, &db).unwrap();
        builder.push("e", Metadata::file(5), 3, &db).unwrap();
        builder.push("b2", Metadata::dir(8), 2, &db).unwrap();
        builder.push("g", Metadata::dir(9), 3, &db).unwrap();
        builder.push("f", Metadata::dir(6), 1, &db).unwrap();
        let tree = builder.tree(&db).unwrap();
        assert_eq!(
            tree.paths(&db),
            ["a", "a/b", "a/b/c", "a/b/c2", "a/b/d", "a/b/e", "a/b2", "a/b2/g", "f"]
        );

        return;

        let mut builder = Builder::new(tree, &db).unwrap();
        builder.push("a", Metadata::dir(1), 1, &db).unwrap();
        builder.push("b", Metadata::dir(2), 2, &db).unwrap();
        builder.push("d", Metadata::dir(4), 3, &db).unwrap();
        builder.push("e", Metadata::file(5), 3, &db).unwrap();
        builder.push("f", Metadata::dir(6), 1, &db).unwrap();
        let tree = builder.tree(&db).unwrap();
        assert_eq!(tree.paths(&db), ["a", "a/b", "a/b/d", "a/b/e", "f"]);
    }

    #[test]
    fn test_builder_removals_within_moved_directory() {
        let db = NullStore::new();
        let tree = Tree::new();
        let mut builder = Builder::new(tree, &db).unwrap();
        builder.push("a", Metadata::dir(1), 1, &db).unwrap();
        builder.push("b", Metadata::dir(2), 2, &db).unwrap();
        builder.push("x", Metadata::dir(3), 1, &db).unwrap();

        let tree = builder.tree(&db).unwrap();
        let mut builder = Builder::new(tree, &db).unwrap();
        builder.push("c", Metadata::dir(1), 1, &db).unwrap();
        builder.push("x", Metadata::dir(3), 1, &db).unwrap();

        let tree = builder.tree(&db).unwrap();
        println!("{:?}", tree.paths(&db));
        println!(
            "{:#?}",
            tree.items
                .items(&db)
                .unwrap()
                .iter()
                .map(|item| match item {
                    Item::DirEntry {
                        file_id,
                        name,
                        child_id,
                        deletions,
                        ..
                    } => format!(
                        "DirEntry {{ file_id: {:?}, name: {:?}, child_id: {:?} deletions {:?} }}",
                        file_id.seq, name, child_id.seq, deletions
                    ),
                    Item::Metadata { file_id, .. } => {
                        format!("Metadata {{ file_id: {:?} }}", file_id.seq)
                    }
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_foo() {
        let db = NullStore::new();
        let tree = Tree::new();
        let mut builder = Builder::new(tree, &db).unwrap();
        builder.push("a", Metadata::dir(1), 1, &db).unwrap();
        builder.push("b", Metadata::dir(2), 2, &db).unwrap();
        builder.push("x", Metadata::dir(3), 1, &db).unwrap();
        builder.push("q", Metadata::dir(4), 2, &db).unwrap();
        builder.push("y", Metadata::dir(5), 1, &db).unwrap();

        let tree = builder.tree(&db).unwrap();
        println!(
            "{:#?}",
            tree.items
                .items(&db)
                .unwrap()
                .iter()
                .map(|item| match item {
                    Item::DirEntry {
                        file_id,
                        name,
                        child_id,
                        deletions,
                        ..
                    } => format!(
                        "DirEntry {{ file_id: {:?}, name: {:?}, child_id: {:?} deletions {:?} }}",
                        file_id.seq, name, child_id, deletions
                    ),
                    Item::Metadata { file_id, .. } => {
                        format!("Metadata {{ file_id: {:?} }}", file_id)
                    }
                })
                .collect::<Vec<_>>()
        );

        let mut builder = Builder::new(tree, &db).unwrap();
        builder.push("a", Metadata::dir(1), 1, &db).unwrap();
        builder.push("b", Metadata::dir(2), 2, &db).unwrap();
        builder.push("x", Metadata::dir(3), 2, &db).unwrap();
        builder.push("q", Metadata::dir(4), 3, &db).unwrap();
        builder.push("x", Metadata::dir(6), 1, &db).unwrap();
        builder.push("y", Metadata::dir(5), 1, &db).unwrap();

        let tree = builder.tree(&db).unwrap();
        println!("{:?}", tree.paths(&db));
        println!(
            "{:#?}",
            tree.items
                .items(&db)
                .unwrap()
                .iter()
                .map(|item| match item {
                    Item::DirEntry {
                        file_id,
                        name,
                        child_id,
                        deletions,
                        ..
                    } => format!(
                        "DirEntry {{ file_id: {:?}, name: {:?}, child_id: {:?} deletions {:?} }}",
                        file_id, name, child_id, deletions
                    ),
                    Item::Metadata { file_id, .. } => {
                        format!("Metadata {{ file_id: {:?} }}", file_id)
                    }
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_builder_random() {
        for seed in 0..5000 {
            // let seed = 116;
            println!("SEED: {}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut store = NullStore::new();
            let store = &store;
            let mut next_inode = 0;

            let mut reference_tree =
                TestDir::gen(&mut rng, &mut next_inode, 0, &mut HashSet::new());
            let mut tree = Tree::new();
            let mut builder = Builder::new(tree.clone(), store).unwrap();
            reference_tree.build(&mut builder, 0, store);
            tree = builder.tree(store).unwrap();
            assert_eq!(tree.paths(store), reference_tree.paths());

            for _ in 0..5 {
                let mut moves = Vec::new();
                reference_tree.mutate(
                    &mut rng,
                    &mut PathBuf::new(),
                    &mut next_inode,
                    &mut moves,
                    0,
                );

                // eprintln!("=========================================");
                // eprintln!("existing paths {:#?}", tree.paths(store));
                // eprintln!("new tree paths {:#?}", reference_tree.paths());
                // eprintln!("=========================================");

                let mut builder = Builder::new(tree.clone(), store).unwrap();
                reference_tree.build(&mut builder, 0, store);
                let new_tree = builder.tree(store).unwrap();

                // println!("moves: {:?}", moves);
                // println!("================================");
                // println!(
                //     "{:#?}",
                //     new_tree
                //         .items
                //         .items(store.item_store())
                //         .unwrap()
                //         .iter()
                //         .map(|item| {
                //             match item {
                //             Item::DirEntry {
                //                 file_id,
                //                 name,
                //                 child_id,
                //                 deletions,
                //                 ..
                //             } => format!(
                //                 "DirEntry {{ file_id: {:?}, name: {:?}, child_id: {:?} deletions {:?} }}",
                //                 file_id.seq, name, child_id.seq, deletions
                //             ),
                //             Item::Metadata { file_id, .. } => {
                //                 format!("Metadata {{ file_id: {:?} }}", file_id.seq)
                //             }
                //         }
                //         })
                //         .collect::<Vec<_>>()
                // );

                assert_eq!(new_tree.paths(store), reference_tree.paths());
                for m in moves {
                    println!("verifying move {:?}", m);
                    if let Some(new_path) = m.new_path {
                        if let Some(old_id) = tree.id_for_path(&m.old_path, store).unwrap() {
                            let new_id = new_tree
                                .id_for_path(&new_path, store)
                                .unwrap()
                                .expect("Path to exist in new tree");
                            assert_eq!(new_id, old_id);
                        }
                    }
                }

                tree = new_tree;
            }
        }
    }

    #[test]
    fn test_key_ordering() {
        assert!(
            Key::dir_entry(id::Unique::default(), true, Arc::new("z".into()))
                < Key::dir_entry(id::Unique::default(), false, Arc::new("a".into()))
        );
    }

    const MAX_TEST_TREE_DEPTH: usize = 5;

    #[derive(Clone, Debug)]
    struct TestDir {
        name: OsString,
        inode: Inode,
        dir_entries: Vec<TestDir>,
    }

    #[derive(Debug)]
    struct Move {
        dir: TestDir,
        old_path: PathBuf,
        new_path: Option<PathBuf>,
    }

    impl TestDir {
        fn gen<T: Rng>(
            rng: &mut T,
            next_inode: &mut u64,
            depth: usize,
            name_blacklist: &mut HashSet<OsString>,
        ) -> Self {
            let new_inode = *next_inode;
            *next_inode += 1;

            let mut tree = Self {
                name: gen_name(rng, name_blacklist),
                inode: Inode(new_inode),
                dir_entries: (0..rng.gen_range(0, MAX_TEST_TREE_DEPTH - depth + 1))
                    .map(|_| Self::gen(rng, next_inode, depth + 1, name_blacklist))
                    .collect(),
            };
            tree.sort_entries();
            tree
        }

        fn move_entry<T: Rng>(
            rng: &mut T,
            path: &mut PathBuf,
            moves: &mut Vec<Move>,
            name_blacklist: &mut HashSet<OsString>,
        ) -> Option<TestDir> {
            let name = gen_name(rng, name_blacklist);
            path.push(&name);
            let mut removes = moves
                .iter_mut()
                .filter(|m| m.new_path.is_none())
                .collect::<Vec<_>>();
            if let Some(remove) = rng.choose_mut(&mut removes) {
                println!("Moving {:?} to {:?}", remove.dir.name, path);

                remove.new_path = Some(path.clone());
                let mut dir = remove.dir.clone();
                dir.name = name;
                path.pop();
                return Some(dir);
            } else {
                path.pop();
                None
            }
        }

        fn mutate<T: Rng>(
            &mut self,
            rng: &mut T,
            path: &mut PathBuf,
            next_inode: &mut u64,
            moves: &mut Vec<Move>,
            depth: usize,
        ) {
            if depth != 0 {
                path.push(&self.name);
            }

            let mut blacklist = self.dir_entries.iter().map(|d| d.name.clone()).collect();
            self.dir_entries.retain(|entry| {
                if rng.gen_weighted_bool(5) {
                    let mut entry_path = path.clone();
                    entry_path.push(&entry.name);
                    println!("Removing {:?}", entry_path);
                    moves.push(Move {
                        dir: entry.clone(),
                        old_path: entry_path,
                        new_path: None,
                    });
                    false
                } else {
                    true
                }
            });
            let mut indices = (0..self.dir_entries.len()).collect::<Vec<_>>();
            rng.shuffle(&mut indices);
            indices.truncate(rng.gen_range(0, self.dir_entries.len() + 1));
            for index in indices {
                self.dir_entries[index].mutate(rng, path, next_inode, moves, depth + 1);
            }
            if depth < MAX_TEST_TREE_DEPTH {
                for _ in 0..rng.gen_range(0, 2) {
                    let moved_entry = if rng.gen_weighted_bool(4) {
                        Self::move_entry(rng, path, moves, &mut blacklist)
                    } else {
                        None
                    };
                    if let Some(moved_entry) = moved_entry {
                        self.dir_entries.push(moved_entry);
                    } else {
                        let new_entry = Self::gen(rng, next_inode, depth + 1, &mut blacklist);
                        path.push(&new_entry.name);
                        println!("Generating {:?}", path);
                        path.pop();
                        self.dir_entries.push(new_entry);
                    }
                }
            }
            self.sort_entries();
            path.pop();
        }

        fn sort_entries(&mut self) {
            self.dir_entries.sort_by(|a, b| a.name.cmp(&b.name));
        }

        fn paths(&self) -> Vec<String> {
            let mut cur_path = PathBuf::new();
            let mut paths = Vec::new();
            self.paths_recursive(&mut cur_path, &mut paths);
            paths
        }

        fn paths_recursive(&self, cur_path: &mut PathBuf, paths: &mut Vec<String>) {
            for dir in &self.dir_entries {
                cur_path.push(&dir.name);
                paths.push(cur_path.clone().to_string_lossy().into_owned());
                dir.paths_recursive(cur_path, paths);
                cur_path.pop();
            }
        }

        fn build<S: Store>(&self, builder: &mut Builder, depth: usize, store: &S) {
            for dir in &self.dir_entries {
                builder
                    .push(&dir.name, Metadata::dir(dir.inode), depth + 1, store)
                    .unwrap();
                dir.build(builder, depth + 1, store);
            }
        }
    }

    fn gen_name<T: Rng>(rng: &mut T, blacklist: &mut HashSet<OsString>) -> OsString {
        loop {
            let mut name = String::new();
            for _ in 0..rng.gen_range(1, 4) {
                name.push(rng.gen_range(b'a', b'z' + 1).into());
            }
            let name = OsString::from(name);
            if !blacklist.contains(&name) {
                blacklist.insert(name.clone());
                return name;
            }
        }
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
