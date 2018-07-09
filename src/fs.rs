use btree::{self, NodeStore, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::{Add, AddAssign};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ROOT_ID: id::Unique = id::Unique::DEFAULT;

trait Store {
    type ReadError: fmt::Debug;
    type ItemStore: NodeStore<Item, ReadError = Self::ReadError>;

    fn item_store(&self) -> &Self::ItemStore;
    fn gen_id(&self) -> id::Unique;
    fn gen_timestamp(&self) -> LamportTimestamp;
    fn recv_timestamp(&self, timestamp: LamportTimestamp);
}

trait FileSystem {
    fn insert_dir<I: Into<PathBuf>>(&mut self, path: I) -> Inode;
    fn remove_dir<I: Into<PathBuf>>(&mut self, path: I);
}

type Inode = u64;
type LamportTimestamp = u64;

#[derive(Clone)]
struct Tree {
    items: btree::Tree<Item>,
    inodes_to_file_ids: HashMap<Inode, id::Unique>,
}

#[derive(Clone, Debug)]
enum Operation {
    Insert {
        file_id: id::Unique,
        is_dir: bool,
        ref_id: id::Unique,
        timestamp: LamportTimestamp,
        version: id::Unique,
        parent_id: id::Unique,
        name: Arc<OsString>,
    },
    Move {
        file_id: id::Unique,
        ref_id: id::Unique,
        timestamp: LamportTimestamp,
        version: id::Unique,
        new_parent_id: id::Unique,
    },
    Remove {
        file_id: id::Unique,
        ref_id: id::Unique,
        old_timestamp: LamportTimestamp,
        old_replica_id: id::ReplicaId,
        new_timestamp: LamportTimestamp,
        version: id::Unique,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Item {
    Metadata {
        file_id: id::Unique,
        is_dir: bool,
        inode: Option<Inode>,
    },
    ParentRef {
        child_id: id::Unique,
        ref_id: id::Unique,
        timestamp: LamportTimestamp,
        version: id::Unique,
        parent_id: Option<id::Unique>,
        name: Arc<OsString>,
    },
    ChildRef {
        parent_id: id::Unique,
        is_dir: bool,
        name: Arc<OsString>,
        timestamp: LamportTimestamp,
        version: id::Unique,
        ref_id: id::Unique,
        child_id: id::Unique,
        deletions: SmallVec<[id::Unique; 1]>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InodeToFileId {
    inode: Inode,
    file_id: id::Unique,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Key {
    Metadata {
        file_id: id::Unique,
    },
    ParentRef {
        child_id: id::Unique,
        ref_id: id::Unique,
        timestamp: LamportTimestamp,
        replica_id: id::ReplicaId,
    },
    ChildRef {
        parent_id: id::Unique,
        is_dir: bool,
        name: Arc<OsString>,
        timestamp: LamportTimestamp,
        replica_id: id::ReplicaId,
    },
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
enum KeyKind {
    Metadata,
    ParentRef,
    ChildRef,
}

struct Metadata {
    inode: Inode,
    is_dir: bool,
}

struct Builder {
    tree: Tree,
    dir_stack: Vec<id::Unique>,
    cursor_stack: Vec<(usize, Cursor)>,
    // item_changes: Vec<ItemChange>,
    inodes_to_file_ids: HashMap<Inode, id::Unique>,
    visited_inodes: HashSet<Inode>,
    dir_changes: BTreeMap<id::Unique, DirChange>,
}

enum DirChange {
    Insert {
        inode: Inode,
        parent_id: id::Unique,
        name: Arc<OsString>,
    },
    Update {
        new_inode: Inode,
    },
    Remove,
    Move {
        new_parent_id: id::Unique,
        new_name: Arc<OsString>,
    },
}

struct Cursor {
    path: PathBuf,
    stack: Vec<(btree::Cursor<Item>, HashSet<Arc<OsString>>)>,
}

impl Metadata {
    fn dir(inode: Inode) -> Self {
        Metadata {
            is_dir: true,
            inode,
        }
    }

    fn file(inode: Inode) -> Self {
        Metadata {
            is_dir: false,
            inode: inode,
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

    pub fn id_for_path<I, S>(&self, path: I, db: &S) -> Result<Option<id::Unique>, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();
        let item_db = db.item_store();
        let mut parent_file_id = ROOT_ID;
        let mut cursor = self.items.cursor();
        let mut components_iter = path.components().peekable();

        while let Some(component) = components_iter.next() {
            let component_name = Arc::new(OsString::from(component.as_os_str()));
            if let Some(child_ref) =
                self.find_active_child_ref(parent_file_id, &component_name, true, db)?
            {
                parent_file_id = child_ref.child_id();
            } else if components_iter.peek().is_none() {
                if let Some(child_ref) =
                    self.find_active_child_ref(parent_file_id, &component_name, false, db)?
                {
                    return Ok(Some(child_ref.child_id()));
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }

        Ok(Some(parent_file_id))
    }

    fn path_for_dir_id<S>(&self, id: id::Unique, db: &S) -> Result<Option<PathBuf>, S::ReadError>
    where
        S: Store,
    {
        let item_db = db.item_store();
        let mut path_components = Vec::new();

        let mut cursor = self.items.cursor();

        let mut next_id = id;
        while next_id != ROOT_ID {
            cursor.seek(&Key::metadata(next_id), SeekBias::Right, item_db)?;

            if let Some(Item::ParentRef {
                parent_id, name, ..
            }) = cursor.item(item_db)?
            {
                if let Some(parent_id) = parent_id {
                    let child_ref = self.find_active_child_ref(parent_id, &name, true, db)?;
                    if child_ref.map_or(false, |child_ref| child_ref.child_id() == next_id) {
                        next_id = parent_id;
                        path_components.push(name);
                    } else {
                        return Ok(None);
                    }
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }

        let mut path = PathBuf::new();
        for component in path_components.into_iter().rev() {
            path.push(component.as_ref());
        }
        Ok(Some(path))
    }

    #[cfg(test)]
    fn integrate_ops<F, S>(
        &mut self,
        ops: Vec<Operation>,
        db: &S,
        fs: &mut F,
    ) -> Result<(), S::ReadError>
    where
        F: FileSystem,
        S: Store,
    {
        for op in ops {
            self.integrate_op(op, db, fs)?;
        }
        Ok(())
    }

    pub fn integrate_op<F, S>(
        &mut self,
        op: Operation,
        db: &S,
        fs: &mut F,
    ) -> Result<(), S::ReadError>
    where
        F: FileSystem,
        S: Store,
    {
        let item_db = db.item_store();
        match op {
            Operation::Insert {
                file_id,
                is_dir,
                ref_id,
                timestamp,
                version,
                parent_id,
                name,
            } => {
                let mut new_child_ref = Item::ChildRef {
                    parent_id,
                    is_dir: true,
                    name: name.clone(),
                    ref_id,
                    child_id: file_id,
                    timestamp,
                    version,
                    deletions: SmallVec::new(),
                };
                let inode = if let Some(mut path) = self.path_for_dir_id(parent_id, db)? {
                    path.push(name.as_ref());

                    if let Some(mut old_child_ref) =
                        self.find_child_ref(parent_id, &name, true, db)?
                    {
                        if old_child_ref.key() < new_child_ref.key() {
                            None
                        } else {
                            if !old_child_ref.is_deleted() {
                                fs.remove_dir(&path);
                            }
                            Some(fs.insert_dir(&path))
                        }
                    } else {
                        Some(fs.insert_dir(&path))
                    }
                } else {
                    None
                };

                let mut new_items: SmallVec<[Item; 3]> = SmallVec::new();
                new_items.push(new_child_ref);
                new_items.push(Item::Metadata {
                    file_id,
                    inode,
                    is_dir: true,
                });
                new_items.push(Item::ParentRef {
                    child_id: file_id,
                    ref_id,
                    timestamp,
                    version,
                    parent_id: Some(parent_id),
                    name: name.clone(),
                });
                new_items.sort_unstable_by_key(|item| item.key());

                inode.map(|inode| self.inodes_to_file_ids.insert(inode, file_id));
                self.items = interleave_items(&self.items, new_items, db)?;
                db.recv_timestamp(timestamp);
            }
            Operation::Remove {
                file_id,
                ref_id,
                old_timestamp,
                old_replica_id,
                new_timestamp,
                version,
            } => {
                let active_ref = self.find_active_parent_ref(file_id, ref_id, db)?.unwrap();
                let removed_ref_key =
                    Key::parent_ref(file_id, ref_id, new_timestamp, version.replica_id);
                if removed_ref_key < active_ref.key() {
                    if let Some(path) = self.path_for_dir_id(file_id, db)? {
                        fs.remove_dir(path);
                    }
                }

                let old_parent_ref = self
                    .find_parent_ref(file_id, ref_id, old_timestamp, old_replica_id, db)?
                    .unwrap();
                let mut cursor = self.items.cursor();
                cursor.seek(
                    &Key::child_ref(
                        old_parent_ref.parent_id().unwrap(),
                        true,
                        old_parent_ref.name(),
                        old_parent_ref.timestamp(),
                        old_parent_ref.version().replica_id,
                    ),
                    SeekBias::Left,
                    item_db,
                )?;
                let mut child_ref = cursor.item(item_db)?.unwrap();
                child_ref.deletions_mut().push(version);

                let mut new_items: SmallVec<[Item; 2]> = SmallVec::new();
                new_items.push(child_ref);
                new_items.push(Item::ParentRef {
                    child_id: file_id,
                    ref_id,
                    timestamp: new_timestamp,
                    version,
                    parent_id: None,
                    name: old_parent_ref.name(),
                });
                new_items.sort_unstable_by_key(|item| item.key());

                self.items = interleave_items(&self.items, new_items, db)?;
                db.recv_timestamp(new_timestamp);
            }
            _ => unimplemented!("Integrating op is not implemented: {:?}", op),
        }

        Ok(())
    }

    fn find_active_child_ref<S: Store>(
        &self,
        parent_id: id::Unique,
        name: &Arc<OsString>,
        is_dir: bool,
        db: &S,
    ) -> Result<Option<Item>, S::ReadError> {
        if let Some(child_ref) = self.find_child_ref(parent_id, name, is_dir, db)? {
            if child_ref.is_deleted() {
                Ok(None)
            } else {
                Ok(Some(child_ref))
            }
        } else {
            Ok(None)
        }
    }

    fn find_child_ref<S: Store>(
        &self,
        parent_id: id::Unique,
        name: &Arc<OsString>,
        is_dir: bool,
        db: &S,
    ) -> Result<Option<Item>, S::ReadError> {
        let item_db = db.item_store();
        let mut cursor = self.items.cursor();
        cursor.seek(
            &Key::ChildRef {
                parent_id,
                is_dir,
                name: name.clone(),
                timestamp: LamportTimestamp::max_value(),
                replica_id: id::ReplicaId::min_value(),
            },
            SeekBias::Left,
            item_db,
        )?;

        match cursor.item(item_db)? {
            Some(item) => {
                if item.is_child_ref() && is_dir == item.is_dir() && *name == item.name() {
                    Ok(Some(item))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    fn find_active_parent_ref<S>(
        &self,
        file_id: id::Unique,
        ref_id: id::Unique,
        db: &S,
    ) -> Result<Option<Item>, S::ReadError>
    where
        S: Store,
    {
        self.find_parent_ref(
            file_id,
            ref_id,
            LamportTimestamp::max_value(),
            id::ReplicaId::min_value(),
            db,
        )
    }

    fn find_parent_ref<S>(
        &self,
        file_id: id::Unique,
        ref_id: id::Unique,
        timestamp: LamportTimestamp,
        replica_id: id::ReplicaId,
        db: &S,
    ) -> Result<Option<Item>, S::ReadError>
    where
        S: Store,
    {
        let item_db = db.item_store();
        let mut cursor = self.items.cursor();
        cursor.seek(
            &Key::parent_ref(file_id, ref_id, timestamp, replica_id),
            SeekBias::Left,
            item_db,
        )?;
        cursor.item(item_db)
    }

    #[cfg(test)]
    fn paths<S: Store>(&self, store: &S) -> Vec<String> {
        let mut paths = Vec::new();
        let mut cursor = self.cursor(store).unwrap();
        while let Some(mut path) = cursor.path().map(|p| p.to_string_lossy().into_owned()) {
            if cursor.child_ref_item(store).unwrap().unwrap().is_dir() {
                path.push('/');
            }
            paths.push(path);
            cursor.next(store).unwrap();
        }
        paths
    }
}

impl Item {
    fn key(&self) -> Key {
        match self {
            Item::Metadata { file_id, .. } => Key::Metadata { file_id: *file_id },
            Item::ParentRef {
                child_id,
                ref_id,
                timestamp,
                version,
                ..
            } => Key::ParentRef {
                child_id: *child_id,
                ref_id: *ref_id,
                timestamp: *timestamp,
                replica_id: version.replica_id,
            },
            Item::ChildRef {
                parent_id,
                is_dir,
                name,
                timestamp,
                version,
                ..
            } => Key::ChildRef {
                parent_id: *parent_id,
                is_dir: *is_dir,
                name: name.clone(),
                timestamp: *timestamp,
                replica_id: version.replica_id,
            },
        }
    }

    fn is_dir(&self) -> bool {
        match self {
            Item::Metadata { is_dir, .. } => *is_dir,
            Item::ChildRef { is_dir, .. } => *is_dir,
            _ => panic!(),
        }
    }

    fn inode(&self) -> Option<Inode> {
        match self {
            Item::Metadata { inode, .. } => *inode,
            _ => panic!(),
        }
    }

    fn name(&self) -> Arc<OsString> {
        match self {
            Item::ChildRef { name, .. } | Item::ParentRef { name, .. } => name.clone(),
            _ => panic!(),
        }
    }

    fn file_id(&self) -> id::Unique {
        match self {
            Item::Metadata { file_id, .. } => *file_id,
            Item::ParentRef { child_id, .. } => *child_id,
            Item::ChildRef { parent_id, .. } => *parent_id,
        }
    }

    fn parent_id(&self) -> Option<id::Unique> {
        match self {
            Item::ParentRef { parent_id, .. } => parent_id.clone(),
            _ => panic!(),
        }
    }

    fn child_id(&self) -> id::Unique {
        match self {
            Item::ChildRef { child_id, .. } => *child_id,
            _ => panic!(),
        }
    }

    fn ref_id(&self) -> id::Unique {
        match self {
            Item::Metadata { .. } => panic!(),
            Item::ParentRef { ref_id, .. } => *ref_id,
            Item::ChildRef { ref_id, .. } => *ref_id,
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

    fn is_child_ref(&self) -> bool {
        match self {
            Item::ChildRef { .. } => true,
            _ => false,
        }
    }

    fn is_parent_ref(&self) -> bool {
        match self {
            Item::ParentRef { .. } => true,
            _ => false,
        }
    }

    fn is_ref(&self) -> bool {
        match self {
            Item::ParentRef { .. } => true,
            Item::ChildRef { .. } => true,
            _ => false,
        }
    }

    fn is_deleted(&self) -> bool {
        match self {
            Item::ChildRef { deletions, .. } => !deletions.is_empty(),
            _ => false,
        }
    }

    fn deletions(&self) -> &[id::Unique] {
        match self {
            Item::ChildRef { deletions, .. } => deletions,
            _ => panic!(),
        }
    }

    fn deletions_mut(&mut self) -> &mut SmallVec<[id::Unique; 1]> {
        match self {
            Item::ChildRef { deletions, .. } => deletions,
            _ => panic!(),
        }
    }

    fn timestamp(&self) -> LamportTimestamp {
        match self {
            Item::ParentRef { timestamp, .. } => *timestamp,
            Item::ChildRef { timestamp, .. } => *timestamp,
            _ => panic!(),
        }
    }

    fn version(&self) -> id::Unique {
        match self {
            Item::ParentRef { version, .. } => *version,
            Item::ChildRef { version, .. } => *version,
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
        Key::Metadata { file_id }
    }

    fn parent_ref(
        child_id: id::Unique,
        ref_id: id::Unique,
        timestamp: LamportTimestamp,
        replica_id: id::ReplicaId,
    ) -> Self {
        Key::ParentRef {
            child_id,
            ref_id,
            timestamp,
            replica_id,
        }
    }

    fn child_ref(
        parent_id: id::Unique,
        is_dir: bool,
        name: Arc<OsString>,
        timestamp: LamportTimestamp,
        replica_id: id::ReplicaId,
    ) -> Self {
        Key::ChildRef {
            parent_id,
            is_dir,
            name,
            timestamp,
            replica_id,
        }
    }
}

impl Default for Key {
    fn default() -> Self {
        Key::metadata(ROOT_ID)
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
        debug_assert!(*self < *other);
        *self = other.clone();
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

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        let (file_id, kind) = match self {
            Key::Metadata { file_id, .. } => (file_id, KeyKind::Metadata),
            Key::ParentRef { child_id, .. } => (child_id, KeyKind::ParentRef),
            Key::ChildRef { parent_id, .. } => (parent_id, KeyKind::ChildRef),
        };
        let (other_file_id, other_kind) = match other {
            Key::Metadata { file_id, .. } => (file_id, KeyKind::Metadata),
            Key::ParentRef { child_id, .. } => (child_id, KeyKind::ParentRef),
            Key::ChildRef { parent_id, .. } => (parent_id, KeyKind::ChildRef),
        };

        file_id
            .cmp(other_file_id)
            .then(kind.cmp(&other_kind))
            .then_with(|| match (self, other) {
                (Key::Metadata { .. }, Key::Metadata { .. }) => Ordering::Equal,
                (
                    Key::ParentRef {
                        ref_id,
                        timestamp,
                        replica_id,
                        ..
                    },
                    Key::ParentRef {
                        ref_id: other_ref_id,
                        timestamp: other_timestamp,
                        replica_id: other_replica_id,
                        ..
                    },
                ) => ref_id
                    .cmp(other_ref_id)
                    .then_with(|| timestamp.cmp(other_timestamp).reverse())
                    .then_with(|| replica_id.cmp(other_replica_id)),
                (
                    Key::ChildRef {
                        is_dir,
                        name,
                        timestamp,
                        replica_id,
                        ..
                    },
                    Key::ChildRef {
                        is_dir: other_is_dir,
                        name: other_name,
                        timestamp: other_timestamp,
                        replica_id: other_replica_id,
                        ..
                    },
                ) => is_dir
                    .cmp(other_is_dir)
                    .reverse()
                    .then_with(|| name.cmp(other_name))
                    .then_with(|| timestamp.cmp(other_timestamp).reverse())
                    .then_with(|| replica_id.cmp(other_replica_id)),
                _ => unreachable!(),
            })
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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
            inodes_to_file_ids,
            visited_inodes: HashSet::new(),
            dir_changes: BTreeMap::new(),
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

        self.dir_stack.truncate(new_depth - 1);
        // Delete old entries that precede the pushed entry. If we encounter an equivalent entry,
        // consume it if its inode has not yet been visited by this builder instance.
        loop {
            let old_depth = self.old_depth();
            if old_depth < new_depth {
                break;
            }

            let old_entry = self.old_child_ref_item(db)?.unwrap();
            let old_inode = self.old_inode(db)?;

            if old_depth == new_depth {
                match cmp_dir_entries(
                    new_metadata.is_dir,
                    new_name.as_os_str(),
                    old_entry.is_dir(),
                    old_entry.name().as_os_str(),
                ) {
                    Ordering::Less => break,
                    Ordering::Equal => if let Some(old_inode) = old_inode {
                        if new_metadata.is_dir {
                            if !self.visited_inodes.contains(&old_inode) {
                                self.visited_inodes.insert(old_inode);
                                self.visited_inodes.insert(new_metadata.inode);
                                if new_metadata.inode != old_inode {
                                    self.inodes_to_file_ids
                                        .insert(new_metadata.inode, old_entry.child_id());
                                    self.dir_changes.insert(
                                        old_entry.child_id(),
                                        DirChange::Update {
                                            new_inode: new_metadata.inode,
                                        },
                                    );
                                }
                                self.dir_stack.push(old_entry.child_id());
                                self.next_old_entry(db)?;
                                return Ok(());
                            }
                        } else if old_inode == new_metadata.inode {
                            self.next_old_entry(db)?;
                            return Ok(());
                        }
                    },
                    Ordering::Greater => {}
                }
            }

            self.dir_changes
                .entry(old_entry.child_id())
                .or_insert(DirChange::Remove);
            self.next_old_entry_sibling(db)?;
        }

        // If we make it this far, we did not find an old dir entry that's equivalent to the entry
        // we are pushing. We need to insert a new one. If the inode for the new entry matches an
        // existing file, we recycle its id so long as we have not already visited it.
        let parent_id = self.dir_stack.last().cloned().unwrap_or(ROOT_ID);
        let child_id = {
            let child_id = self.inodes_to_file_ids.get(&new_metadata.inode).cloned();
            if self.visited_inodes.contains(&new_metadata.inode) || child_id.is_none() {
                let child_id = db.gen_id();
                self.dir_changes.insert(
                    child_id,
                    DirChange::Insert {
                        inode: new_metadata.inode,
                        parent_id,
                        name: Arc::new(new_name),
                    },
                );
                self.inodes_to_file_ids.insert(new_metadata.inode, child_id);
                child_id
            } else {
                let child_id = child_id.unwrap();
                self.jump_to_old_entry(new_depth, child_id, db)?;
                self.dir_changes.insert(
                    child_id,
                    DirChange::Move {
                        new_parent_id: parent_id,
                        new_name: Arc::new(new_name),
                    },
                );
                child_id
            }
        };

        if new_metadata.is_dir {
            self.dir_stack.push(child_id);
            self.visited_inodes.insert(new_metadata.inode);
        }
        Ok(())
    }

    pub fn tree<S: Store>(mut self, db: &S) -> Result<(Tree, Vec<Operation>), S::ReadError> {
        let item_db = db.item_store();
        while let Some(old_entry) = self.old_child_ref_item(db)? {
            self.dir_changes
                .entry(old_entry.child_id())
                .or_insert(DirChange::Remove);
            self.next_old_entry_sibling(db)?;
        }

        let mut operations = Vec::new();
        let mut new_items = Vec::new();
        for (child_id, change) in self.dir_changes {
            match change {
                DirChange::Insert {
                    inode,
                    parent_id,
                    name,
                } => {
                    let ref_id = db.gen_id();
                    let timestamp = db.gen_timestamp();
                    let version = ref_id;
                    new_items.push(Item::ChildRef {
                        parent_id,
                        is_dir: true,
                        name: name.clone(),
                        timestamp,
                        version,
                        ref_id,
                        child_id,
                        deletions: SmallVec::new(),
                    });
                    new_items.push(Item::Metadata {
                        file_id: child_id,
                        inode: Some(inode),
                        is_dir: true,
                    });
                    new_items.push(Item::ParentRef {
                        child_id,
                        ref_id,
                        timestamp,
                        version,
                        parent_id: Some(parent_id),
                        name: name.clone(),
                    });
                    operations.push(Operation::Insert {
                        file_id: child_id,
                        is_dir: true,
                        ref_id,
                        timestamp,
                        version,
                        parent_id,
                        name,
                    });
                }
                DirChange::Update { new_inode } => {
                    let mut cursor = self.tree.items.cursor();
                    cursor.seek(&Key::metadata(child_id), SeekBias::Left, item_db)?;
                    match cursor.item(item_db)? {
                        Some(Item::Metadata {
                            file_id, is_dir, ..
                        }) => new_items.push(Item::Metadata {
                            file_id,
                            is_dir,
                            inode: Some(new_inode),
                        }),
                        _ => panic!(),
                    }
                }
                DirChange::Remove | DirChange::Move { .. } => {
                    let new_version = db.gen_id();
                    let new_timestamp = db.gen_timestamp();

                    let mut cursor = self.tree.items.cursor();
                    cursor.seek(&Key::metadata(child_id), SeekBias::Right, item_db)?;
                    let mut parent_ref = cursor.item(item_db)?.unwrap();
                    let ref_id = parent_ref.ref_id();
                    cursor.seek(
                        &Key::child_ref(
                            parent_ref.parent_id().unwrap(),
                            true,
                            parent_ref.name(),
                            parent_ref.timestamp(),
                            parent_ref.version().replica_id,
                        ),
                        SeekBias::Left,
                        item_db,
                    )?;
                    if let Item::ChildRef {
                        mut deletions,
                        timestamp,
                        version,
                        ..
                    } = cursor.item(item_db)?.unwrap()
                    {
                        deletions.push(new_version);
                        new_items.push(Item::ChildRef {
                            parent_id: parent_ref.parent_id().unwrap(),
                            is_dir: true,
                            name: parent_ref.name(),
                            timestamp,
                            version,
                            ref_id,
                            child_id,
                            deletions,
                        });
                    } else {
                        panic!();
                    }

                    if let DirChange::Move {
                        new_parent_id,
                        new_name,
                    } = change
                    {
                        new_items.push(Item::ParentRef {
                            child_id,
                            ref_id,
                            timestamp: new_timestamp,
                            version: new_version,
                            parent_id: Some(new_parent_id),
                            name: new_name.clone(),
                        });
                        new_items.push(Item::ChildRef {
                            parent_id: new_parent_id,
                            is_dir: true,
                            name: new_name,
                            timestamp: new_timestamp,
                            version: new_version,
                            ref_id,
                            child_id,
                            deletions: SmallVec::new(),
                        });
                        operations.push(Operation::Move {
                            file_id: child_id,
                            ref_id,
                            timestamp: new_timestamp,
                            version: new_version,
                            new_parent_id,
                        });
                    } else {
                        new_items.push(Item::ParentRef {
                            child_id,
                            ref_id,
                            timestamp: new_timestamp,
                            version: new_version,
                            parent_id: None,
                            name: parent_ref.name(),
                        });
                        operations.push(Operation::Remove {
                            file_id: child_id,
                            ref_id,
                            old_timestamp: parent_ref.timestamp(),
                            old_replica_id: parent_ref.version().replica_id,
                            new_timestamp,
                            version: new_version,
                        });
                    }
                }
            }
        }
        new_items.sort_unstable_by_key(|item| item.key());

        let new_tree = Tree {
            items: interleave_items(&self.tree.items, new_items, db)?,
            inodes_to_file_ids: self.inodes_to_file_ids,
        };
        Ok((new_tree, operations))
    }

    fn old_depth(&self) -> usize {
        let (base_depth, cursor) = self.cursor_stack.last().unwrap();
        base_depth + cursor.depth()
    }

    fn old_child_ref_item<S: Store>(&self, db: &S) -> Result<Option<Item>, S::ReadError> {
        self.cursor_stack.last().unwrap().1.child_ref_item(db)
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
        Ok(
            Self::within_dir(ROOT_ID, tree, db)?.unwrap_or_else(|| Self {
                stack: vec![],
                path: PathBuf::new(),
            }),
        )
    }

    fn within_dir<S>(file_id: id::Unique, tree: &Tree, db: &S) -> Result<Option<Self>, S::ReadError>
    where
        S: Store,
    {
        let item_db = db.item_store();
        let mut root_cursor = tree.items.cursor();
        root_cursor.seek(&Key::metadata(file_id), SeekBias::Right, item_db)?;
        let mut visited_names = HashSet::new();
        while let Some(item) = root_cursor.item(item_db)? {
            if item.is_metadata() {
                break;
            } else if item.is_child_ref() {
                let name = item.name();
                if item.is_deleted() || visited_names.contains(&name) {
                    visited_names.insert(name);
                    root_cursor.next(item_db)?;
                } else {
                    visited_names.insert(name);
                    let mut cursor = Self {
                        path: PathBuf::new(),
                        stack: vec![(root_cursor, visited_names)],
                    };
                    cursor.follow_entry(db)?;
                    return Ok(Some(cursor));
                }
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

    pub fn child_ref_item<S: Store>(&self, db: &S) -> Result<Option<Item>, S::ReadError> {
        if self.stack.len() > 1 {
            let (cursor, _) = &self.stack[self.stack.len() - 2];
            cursor.item(db.item_store())
        } else {
            Ok(None)
        }
    }

    pub fn inode<S: Store>(&self, db: &S) -> Result<Option<Inode>, S::ReadError> {
        if let Some((cursor, _)) = self.stack.last() {
            match cursor.item(db.item_store())?.unwrap() {
                Item::Metadata { inode, .. } => Ok(inode),
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
                let (cursor, visited_names) = self.stack.last_mut().unwrap();
                let cur_item = cursor.item(item_db)?.unwrap();
                if cur_item.is_ref() || cur_item.is_dir_metadata() {
                    cursor.next(item_db)?;
                    match cursor.item(item_db)? {
                        Some(Item::ParentRef { .. }) => continue,
                        Some(Item::ChildRef {
                            name, deletions, ..
                        }) => {
                            if visited_names.contains(&name) {
                                continue;
                            } else {
                                visited_names.insert(name);
                                if deletions.is_empty() {
                                    break true;
                                } else {
                                    continue;
                                }
                            }
                        }
                        _ => break false,
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
            self.stack.pop();
            self.path.pop();
            self.next(db)
        }
    }

    fn follow_entry<S: Store>(&mut self, db: &S) -> Result<(), S::ReadError> {
        let item_db = db.item_store();
        let mut child_cursor;
        {
            let (entry_cursor, _) = self.stack.last().unwrap();
            match entry_cursor.item(item_db)?.unwrap() {
                Item::ChildRef {
                    parent_id,
                    child_id,
                    name,
                    ..
                } => {
                    child_cursor = entry_cursor.clone();
                    let child_key = Key::metadata(child_id);
                    if child_id > parent_id {
                        child_cursor.seek_forward(&child_key, SeekBias::Left, item_db)?;
                    } else {
                        child_cursor.seek(&child_key, SeekBias::Left, item_db)?;
                    }

                    self.path.push(name.as_ref());
                }
                _ => panic!(),
            }
        }
        self.stack.push((child_cursor, HashSet::new()));
        Ok(())
    }
}

fn cmp_dir_entries(a_is_dir: bool, a_name: &OsStr, b_is_dir: bool, b_name: &OsStr) -> Ordering {
    a_is_dir
        .cmp(&b_is_dir)
        .reverse()
        .then_with(|| a_name.cmp(b_name))
}

fn interleave_items<I, S>(
    tree: &btree::Tree<Item>,
    sorted_items: I,
    db: &S,
) -> Result<btree::Tree<Item>, S::ReadError>
where
    I: IntoIterator<Item = Item>,
    S: Store,
{
    let item_db = db.item_store();
    let mut old_items_cursor = tree.cursor();
    let mut new_tree = btree::Tree::new();
    let mut buffered_items = Vec::new();

    old_items_cursor.seek(&Key::default(), SeekBias::Left, item_db)?;
    for new_item in sorted_items {
        let new_item_key = new_item.key();
        let mut old_item = old_items_cursor.item(item_db)?;
        if old_item
            .as_ref()
            .map_or(false, |old_item| old_item.key() < new_item_key)
        {
            new_tree.extend(buffered_items.drain(..), item_db)?;
            new_tree.push_tree(
                old_items_cursor.slice(&new_item_key, SeekBias::Left, item_db)?,
                item_db,
            )?;
            old_item = old_items_cursor.item(item_db)?;
        }

        if old_item.map_or(false, |old_item| old_item.key() == new_item_key) {
            old_items_cursor.next(item_db)?;
        }
        buffered_items.push(new_item);
    }
    new_tree.extend(buffered_items, item_db)?;
    new_tree.push_tree(old_items_cursor.suffix::<Key, _>(item_db)?, item_db)?;
    Ok(new_tree)
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use self::rand::{Rng, SeedableRng, StdRng};
    use super::*;
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_builder_basic() {
        let db = NullStore::new(1);

        let mut reference_fs = TestFileSystem::new();
        reference_fs.insert_dir("a");
        reference_fs.insert_dir("a/b");
        reference_fs.insert_dir("a/b/c");
        reference_fs.insert_dir("a/b/d");
        reference_fs.insert_dir("a/b/e");
        reference_fs.insert_dir("f");

        let (tree, _) = reference_fs.update_tree(Tree::new(), &db);
        assert_eq!(tree.paths(&db), reference_fs.paths());

        reference_fs.insert_dir("a/b/c2");
        reference_fs.insert_dir("a/b2");
        reference_fs.insert_dir("a/b2/g");
        let (tree, _) = reference_fs.update_tree(Tree::new(), &db);
        assert_eq!(tree.paths(&db), reference_fs.paths());

        reference_fs.remove_dir("a/b/c2");
        reference_fs.remove_dir("a/b2");
        reference_fs.remove_dir("a/b2/g");
        let (tree, _) = reference_fs.update_tree(Tree::new(), &db);
        assert_eq!(tree.paths(&db), reference_fs.paths());
    }

    #[test]
    fn test_builder_random() {
        use std::iter::FromIterator;

        for seed in 0..100 {
            // let seed = 908;
            println!("SEED: {}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut store = NullStore::new(1);
            let store = &store;

            let mut reference_fs = TestFileSystem::gen(&mut rng);
            let (mut tree, _) = reference_fs.update_tree(Tree::new(), store);
            assert_eq!(tree.paths(store), reference_fs.paths());

            for _ in 0..5 {
                let mut old_paths: HashSet<String> = HashSet::from_iter(reference_fs.paths());

                let mut moves = Vec::new();
                let mut touched_paths = HashSet::new();
                reference_fs.mutate(&mut rng, &mut moves, &mut touched_paths);

                // println!("=========================================");
                // println!("existing paths {:#?}", tree.paths(store));
                // println!("new tree paths {:#?}", reference_fs.paths());
                // println!("=========================================");

                let (new_tree, _) = reference_fs.update_tree(tree.clone(), store);

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
                //             Item::ChildRef {
                //                 file_id,
                //                 name,
                //                 child_id,
                //                 ref_id,
                //                 deletions,
                //                 ..
                //             } => format!(
                //                 "ChildRef {{ file_id: {:?}, name: {:?}, child_id: {:?}, ref_id: {:?}, deletions {:?} }}",
                //                 file_id.seq, name, child_id.seq, ref_id.seq, deletions
                //             ),
                //             Item::Metadata { file_id, inode, is_dir, .. } => {
                //                 format!("Metadata {{ file_id: {:?}, inode: {:?}, is_dir: {:?} }}", file_id.seq, inode, is_dir)
                //             }
                //         }
                //         })
                //         .collect::<Vec<_>>()
                // );

                assert_eq!(new_tree.paths(store), reference_fs.paths());
                for m in moves {
                    // println!("verifying move {:?}", m);
                    if let Some(new_path) = m.new_path {
                        if let Some(old_path_id) = tree.id_for_path(&m.old_path, store).unwrap() {
                            let new_path_id = new_tree
                                .id_for_path(&new_path, store)
                                .unwrap()
                                .expect("Path to exist in new tree");

                            if !m.file.is_dir()
                                || (!touched_paths.contains(&m.old_path)
                                    && !old_paths
                                        .contains(&new_path.to_string_lossy().into_owned()))
                            {
                                assert_eq!(new_path_id, old_path_id);
                            }
                        }
                    }
                }

                tree = new_tree;
            }
        }
    }

    #[test]
    fn test_replication_basic() {
        let mut fs_1 = TestFileSystem::new();
        let mut fs_2 = TestFileSystem::new();
        let db_1 = NullStore::new(1);
        let db_2 = NullStore::new(2);
        let tree_1 = Tree::new();
        let tree_2 = Tree::new();

        fs_1.insert_dir("a");
        let (mut tree_1, ops_1) = fs_1.update_tree(tree_1, &db_1);

        fs_2.insert_dir("b");
        let (mut tree_2, ops_2) = fs_2.update_tree(tree_2, &db_2);

        tree_1.integrate_ops(ops_2, &db_1, &mut fs_1).unwrap();
        tree_2.integrate_ops(ops_1, &db_2, &mut fs_2).unwrap();
        assert_eq!(tree_1.paths(&db_1), ["a/", "b/"]);
        assert_eq!(tree_2.paths(&db_2), ["a/", "b/"]);
        assert_eq!(fs_1.paths(), tree_1.paths(&db_1));
        assert_eq!(fs_2.paths(), tree_2.paths(&db_2));

        fs_1.insert_dir("c");
        fs_1.insert_dir("c/d1");
        let (mut tree_1, ops_1) = fs_1.update_tree(tree_1, &db_1);
        fs_2.insert_dir("c");
        fs_2.insert_dir("c/d2");
        let (mut tree_2, ops_2) = fs_2.update_tree(tree_2, &db_2);
        tree_1.integrate_ops(ops_2, &db_1, &mut fs_1).unwrap();
        tree_2.integrate_ops(ops_1, &db_2, &mut fs_2).unwrap();

        assert_eq!(tree_1.paths(&db_1), ["a/", "b/", "c/", "c/d1/"]);
        assert_eq!(tree_2.paths(&db_2), ["a/", "b/", "c/", "c/d1/"]);
        assert_eq!(fs_1.paths(), tree_1.paths(&db_1));
        assert_eq!(fs_2.paths(), tree_2.paths(&db_2));

        fs_1.remove_dir("c");
        let (mut tree_1, ops_1) = fs_1.update_tree(tree_1, &db_1);
        tree_2.integrate_ops(ops_1, &db_2, &mut fs_2).unwrap();

        assert_eq!(tree_1.paths(&db_1), ["a/", "b/"]);
        assert_eq!(tree_2.paths(&db_2), ["a/", "b/"]);
        assert_eq!(fs_1.paths(), tree_1.paths(&db_1));
        assert_eq!(fs_2.paths(), tree_2.paths(&db_2));
    }

    #[test]
    fn test_replication_random() {
        use std::iter::FromIterator;
        use std::mem;

        for seed in 0..100 {
            const PEERS: u64 = 5;
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut fs = Vec::from_iter((0..PEERS).map(|_| TestFileSystem::new()));
            let db = Vec::from_iter((0..PEERS).map(|i| NullStore::new(i + 1)));
            let mut trees = Vec::from_iter((0..PEERS).map(|_| Tree::new()));
            let mut inboxes = Vec::from_iter((0..PEERS).map(|_| Vec::new()));

            for _ in 0..5 {
                let replica_index = rng.gen_range(0, PEERS) as usize;

                if !inboxes[replica_index].is_empty() && rng.gen() {
                    let fs = &mut fs[replica_index];
                    let db = &db[replica_index];
                    let tree = &mut trees[replica_index];
                    let ops = mem::replace(&mut inboxes[replica_index], Vec::new());
                    tree.integrate_ops(ops, db, fs).unwrap();
                } else {
                    let fs = &mut fs[replica_index];
                    let db = &db[replica_index];
                    fs.mutate(&mut rng, &mut Vec::new(), &mut HashSet::new());
                    let (tree, ops) = fs.update_tree(trees[replica_index].clone(), db);
                    trees[replica_index] = tree;

                    for (i, inbox) in inboxes.iter_mut().enumerate() {
                        if i != replica_index {
                            for op in &ops {
                                inbox.push(op.clone());
                            }
                        }
                    }
                }
            }

            for replica_index in 0..PEERS as usize {
                let fs = &mut fs[replica_index];
                let db = &db[replica_index];
                let tree = &mut trees[replica_index];
                let ops = mem::replace(&mut inboxes[replica_index], Vec::new());
                tree.integrate_ops(ops, db, fs).unwrap();
            }

            for i in 0..PEERS as usize {
                if i + 1 < PEERS as usize {
                    assert_eq!(trees[i].paths(&db[i]), trees[i + 1].paths(&db[i + 1]));
                }
                assert_eq!(trees[i].paths(&db[i]), fs[i].paths());
            }
        }
    }

    #[test]
    fn test_key_ordering() {
        let z = Key::child_ref(id::Unique::default(), true, Arc::new("z".into()), 0, 0);
        let a = Key::child_ref(id::Unique::default(), true, Arc::new("a".into()), 0, 0);
        assert!(a < z);
    }

    const MAX_TEST_TREE_DEPTH: usize = 5;

    #[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
    struct TestFile {
        inode: Inode,
        dir_entries: Option<Vec<TestChildRef>>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct TestChildRef {
        name: OsString,
        file: TestFile,
    }

    #[derive(Debug)]
    struct Move {
        file: TestFile,
        old_path: PathBuf,
        new_path: Option<PathBuf>,
    }

    impl TestFile {
        fn dir(inode: Inode) -> Self {
            TestFile {
                inode,
                dir_entries: Some(Vec::new()),
            }
        }

        fn file() -> Self {
            unimplemented!()
            // TestFile {
            //     inode: 0,
            //     dir_entries: None,
            // }
        }

        fn insert_dir<I: Into<PathBuf>>(&mut self, path: I, next_inode: &mut Inode) {
            self.update_path(path.into(), true, true, next_inode);
        }

        fn remove_dir<I: Into<PathBuf>>(&mut self, path: I) {
            self.update_path(path.into(), true, false, &mut 0);
        }

        fn update_path(
            &mut self,
            path: PathBuf,
            is_dir: bool,
            insert: bool,
            next_inode: &mut Inode,
        ) {
            let mut dir = self;
            let mut components = path.components().peekable();

            loop {
                if let Some(component) = components.next() {
                    let dir_entries = { dir }.dir_entries.as_mut().unwrap();

                    let needle = TestChildRef {
                        name: OsString::from(component.as_os_str()),
                        file: if is_dir || components.peek().is_some() {
                            let inode = *next_inode;
                            *next_inode += 1;
                            TestFile::dir(inode)
                        } else {
                            unimplemented!()
                            // TestFile::file()
                        },
                    };
                    dir = match dir_entries.binary_search(&needle) {
                        Ok(index) => {
                            if insert || components.peek().is_some() {
                                &mut dir_entries[index].file
                            } else {
                                dir_entries.remove(index);
                                break;
                            }
                        }
                        Err(index) => {
                            if insert {
                                dir_entries.insert(index, needle);
                                &mut dir_entries[index].file
                            } else {
                                break;
                            }
                        }
                    };
                } else {
                    break;
                }
            }
        }

        fn update_tree<S: Store>(&self, tree: Tree, db: &S) -> (Tree, Vec<Operation>) {
            let mut builder = Builder::new(tree, db).unwrap();
            self.build(&mut builder, 0, db);
            builder.tree(db).unwrap()
        }

        fn gen<T: Rng>(
            rng: &mut T,
            next_inode: &mut u64,
            depth: usize,
            name_blacklist: &mut HashSet<OsString>,
        ) -> Self {
            let new_inode = *next_inode;
            *next_inode += 1;

            // if rng.gen() {
            let mut dir_entries = (0..rng.gen_range(0, MAX_TEST_TREE_DEPTH - depth + 1))
                .map(|_| TestChildRef {
                    name: gen_name(rng, name_blacklist),
                    file: Self::gen(rng, next_inode, depth + 1, name_blacklist),
                })
                .collect::<Vec<_>>();
            dir_entries.sort();
            TestFile {
                inode: new_inode,
                dir_entries: Some(dir_entries),
            }
            // } else {
            //     TestFile {
            //         inode: new_inode,
            //         dir_entries: None,
            //     }
            // }
        }

        fn mutate<T: Rng>(
            &mut self,
            rng: &mut T,
            path: &mut PathBuf,
            next_inode: &mut u64,
            moves: &mut Vec<Move>,
            touched_paths: &mut HashSet<PathBuf>,
            depth: usize,
        ) {
            if let Some(dir_entries) = self.dir_entries.as_mut() {
                // Delete random entries
                dir_entries.retain(|TestChildRef { name, file }| {
                    if rng.gen_weighted_bool(5) {
                        let mut entry_path = path.clone();
                        entry_path.push(name);
                        touched_paths.insert(entry_path.clone());
                        moves.push(Move {
                            file: file.clone(),
                            old_path: entry_path,
                            new_path: None,
                        });
                        false
                    } else {
                        true
                    }
                });

                // Mutate random entries
                let mut indices = (0..dir_entries.len()).collect::<Vec<_>>();
                rng.shuffle(&mut indices);
                indices.truncate(rng.gen_range(0, dir_entries.len() + 1));
                for index in indices {
                    path.push(&dir_entries[index].name);
                    dir_entries[index].file.mutate(
                        rng,
                        path,
                        next_inode,
                        moves,
                        touched_paths,
                        depth + 1,
                    );
                    path.pop();
                }

                // Insert entries if we are less than the max depth
                if depth < MAX_TEST_TREE_DEPTH {
                    let mut blacklist = dir_entries
                        .iter()
                        .map(|TestChildRef { name, .. }| name.clone())
                        .collect();

                    for _ in 0..rng.gen_range(0, 5) {
                        let name = gen_name(rng, &mut blacklist);
                        path.push(&name);
                        touched_paths.insert(path.clone());

                        let mut removals = moves
                            .iter_mut()
                            .filter(|m| m.new_path.is_none())
                            .collect::<Vec<_>>();

                        // TODO: Remove `false` and start handling moves.
                        if false && !removals.is_empty() && rng.gen_weighted_bool(4) {
                            let removal = rng.choose_mut(&mut removals).unwrap();
                            removal.new_path = Some(path.clone());
                            dir_entries.push(TestChildRef {
                                name,
                                file: removal.file.clone(),
                            });
                        } else {
                            let file = Self::gen(rng, next_inode, depth + 1, &mut blacklist);
                            touched_paths.insert(path.clone());
                            dir_entries.push(TestChildRef { name, file });
                        };

                        path.pop();
                    }
                }

                dir_entries.sort();
            }
        }

        fn paths(&self) -> Vec<String> {
            let mut cur_path = PathBuf::new();
            let mut paths = Vec::new();
            self.paths_recursive(&mut cur_path, &mut paths);
            paths
        }

        fn paths_recursive(&self, cur_path: &mut PathBuf, paths: &mut Vec<String>) {
            if let Some(dir_entries) = &self.dir_entries {
                for TestChildRef { name, file } in dir_entries {
                    cur_path.push(name);
                    let mut path = cur_path.clone().to_string_lossy().into_owned();
                    if file.is_dir() {
                        path.push('/');
                    }
                    paths.push(path);
                    file.paths_recursive(cur_path, paths);
                    cur_path.pop();
                }
            }
        }

        fn build<S: Store>(&self, builder: &mut Builder, depth: usize, store: &S) {
            if let Some(dir_entries) = &self.dir_entries {
                for TestChildRef { name, file } in dir_entries {
                    builder
                        .push(
                            name,
                            Metadata {
                                inode: file.inode,
                                is_dir: file.is_dir(),
                            },
                            depth + 1,
                            store,
                        )
                        .unwrap();
                    file.build(builder, depth + 1, store);
                }
            }
        }

        fn is_dir(&self) -> bool {
            self.dir_entries.is_some()
        }
    }

    impl Ord for TestChildRef {
        fn cmp(&self, other: &Self) -> Ordering {
            self.file
                .is_dir()
                .cmp(&other.file.is_dir())
                .reverse()
                .then_with(|| self.name.cmp(&other.name))
        }
    }

    impl PartialOrd for TestChildRef {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
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
        next_id: Cell<id::Unique>,
        lamport_clock: Cell<LamportTimestamp>,
    }

    struct TestFileSystem {
        root: TestFile,
        next_inode: Inode,
    }

    impl NullStore {
        fn new(replica_id: u64) -> Self {
            Self {
                next_id: Cell::new(id::Unique::new(replica_id)),
                lamport_clock: Cell::new(0),
            }
        }
    }

    impl Store for NullStore {
        type ReadError = ();
        type ItemStore = NullStore;

        fn gen_id(&self) -> id::Unique {
            let next_id = self.next_id.get();
            self.next_id.replace(next_id.next());
            next_id
        }

        fn gen_timestamp(&self) -> LamportTimestamp {
            self.lamport_clock.replace(self.lamport_clock.get() + 1);
            self.lamport_clock.get()
        }

        fn recv_timestamp(&self, timestamp: LamportTimestamp) {
            use std::cmp;
            let new_clock = cmp::max(self.lamport_clock.get(), timestamp) + 1;
            self.lamport_clock.set(new_clock);
        }

        fn item_store(&self) -> &Self::ItemStore {
            self
        }
    }

    impl btree::NodeStore<Item> for NullStore {
        type ReadError = ();

        fn get(&self, _id: btree::NodeId) -> Result<Arc<btree::Node<Item>>, Self::ReadError> {
            panic!("get should never be called on a null store")
        }
    }

    impl TestFileSystem {
        fn new() -> Self {
            Self {
                root: TestFile::dir(0),
                next_inode: 1,
            }
        }

        fn gen<T: Rng>(rng: &mut T) -> Self {
            let mut next_inode = 0;
            let root = TestFile::gen(rng, &mut next_inode, 0, &mut HashSet::new());
            TestFileSystem { root, next_inode }
        }

        fn update_tree<S: Store>(&self, tree: Tree, db: &S) -> (Tree, Vec<Operation>) {
            self.root.update_tree(tree, db)
        }

        fn mutate<T: Rng>(
            &mut self,
            rng: &mut T,
            moves: &mut Vec<Move>,
            touched_paths: &mut HashSet<PathBuf>,
        ) {
            self.root.mutate(
                rng,
                &mut PathBuf::new(),
                &mut self.next_inode,
                moves,
                touched_paths,
                0,
            );
        }

        fn paths(&self) -> Vec<String> {
            self.root.paths()
        }
    }

    impl FileSystem for TestFileSystem {
        fn insert_dir<I: Into<PathBuf>>(&mut self, path: I) -> Inode {
            let inode = self.next_inode;
            self.root.insert_dir(path, &mut self.next_inode);
            inode
        }

        fn remove_dir<I: Into<PathBuf>>(&mut self, path: I) {
            self.root.remove_dir(path);
        }
    }
}
