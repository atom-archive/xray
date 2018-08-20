use btree::{self, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::{self, Ordering};
// TODO: Replace BTree-based collections with Hash-based collections.
// We're using the B-tree versions to enforce deterministic ordering behavior during development.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::{Add, AddAssign};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait Store {
    type ReadError: fmt::Debug;
    type MetadataStore: btree::NodeStore<Metadata, ReadError = Self::ReadError>;
    type ParentRefStore: btree::NodeStore<ParentRef, ReadError = Self::ReadError>;
    type ChildRefStore: btree::NodeStore<ChildRef, ReadError = Self::ReadError>;

    fn replica_id(&self) -> id::ReplicaId;

    fn metadata_store(&self) -> &Self::MetadataStore;
    fn parent_ref_store(&self) -> &Self::ParentRefStore;
    fn child_ref_store(&self) -> &Self::ChildRefStore;

    fn gen_id(&self) -> id::Unique;
    fn gen_timestamp(&self) -> LamportTimestamp;
    fn recv_timestamp(&self, timestamp: LamportTimestamp);
}

// TODO: Return results from these methods to deal with IoErrors
pub trait FileSystem {
    type Entry: FileSystemEntry;
    type EntriesIterator: Iterator<Item = Self::Entry>;

    fn insert_dir(&mut self, path: &Path) -> bool;
    fn remove_dir(&mut self, path: &Path) -> bool;
    fn move_dir(&mut self, from: &Path, to: &Path) -> bool;
    fn inode_for_path(&self, path: &Path) -> Option<Inode>;
    fn entries(&self) -> Self::EntriesIterator;

    // TODO: Remove this
    fn paths(&self) -> Vec<String>;
}

pub trait FileSystemEntry {
    fn depth(&self) -> usize;
    fn name(&self) -> &OsStr;
    fn inode(&self) -> Inode;
}

trait Keyed {
    type Key: Ord;

    fn key(&self) -> Self::Key;
}

type Inode = u64;
type VisibleCount = usize;

const ROOT_ID: id::Unique = id::Unique::DEFAULT;

#[derive(Clone)]
pub struct Timeline {
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRef>,
    child_refs: btree::Tree<ChildRef>,
    inodes_to_file_ids: BTreeMap<Inode, id::Unique>,
}

#[derive(Clone)]
pub struct Cursor {
    path: PathBuf,
    stack: Vec<CursorStackEntry>,
    metadata_cursor: btree::Cursor<Metadata>,
    visited_dir_ids: BTreeSet<id::Unique>,
}

#[derive(Clone)]
struct CursorStackEntry {
    cursor: btree::Cursor<ChildRef>,
    jump: bool,
    depth: usize,
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
        new_parent: Option<(id::Unique, Arc<OsString>)>,
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
    visible: bool,
    timestamp: LamportTimestamp,
    visible_count: VisibleCount,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefKey {
    parent_id: id::Unique,
    name: Arc<OsString>,
    visible: bool,
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

#[derive(Debug)]
enum TreeEdit<T: Keyed> {
    Insert(T),
    Remove(T),
}

impl Timeline {
    pub fn new() -> Self {
        Timeline {
            metadata: btree::Tree::new(),
            parent_refs: btree::Tree::new(),
            child_refs: btree::Tree::new(),
            inodes_to_file_ids: BTreeMap::new(),
        }
    }

    pub fn is_empty<S: Store>(&self, db: &S) -> Result<bool, S::ReadError> {
        Ok(self.cursor(db)?.depth() == 0)
    }

    pub fn cursor<S: Store>(&self, db: &S) -> Result<Cursor, S::ReadError> {
        self.cursor_at(ROOT_ID, db)
    }

    pub fn cursor_at<S: Store>(&self, id: id::Unique, db: &S) -> Result<Cursor, S::ReadError> {
        let mut cursor = Cursor {
            path: PathBuf::new(),
            stack: Vec::new(),
            metadata_cursor: self.metadata.cursor(),
            visited_dir_ids: BTreeSet::new(),
        };
        cursor.descend_into(self.child_refs.cursor(), id, None, db)?;
        Ok(cursor)
    }

    fn write_to_fs<F, S>(
        &mut self,
        mut ids_to_write: BTreeSet<id::Unique>,
        mut old_tree: Timeline,
        fs: &mut F,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError>
    where
        F: FileSystem,
        S: Store,
    {
        let mut fixup_ops = Vec::new();
        let mut files_with_temp_name = BTreeSet::new();

        loop {
            let mut sorted_ids_to_write = self
                .sort_ids_by_path_depth(ids_to_write.iter().cloned(), db)?
                .into_iter()
                .peekable();

            while let Some((child_id, depth)) = sorted_ids_to_write.peek().cloned() {
                let old_path = old_tree.path_for_id(child_id, db)?;
                if let Some(old_path) = old_path.as_ref() {
                    if fs.inode_for_path(old_path) != old_tree.inode_for_id(child_id, db)? {
                        break;
                    }
                }

                let parent_ref = self.find_cur_parent_ref(child_id, db)?.unwrap();
                if depth.is_some() {
                    let (parent_id, mut name) = parent_ref.parent.as_ref().cloned().unwrap();

                    if let Some(parent_path) = old_tree.path_for_id(parent_id, db)? {
                        if parent_id != ROOT_ID
                            && fs.inode_for_path(&parent_path)
                                != old_tree.inode_for_id(parent_id, db)?
                        {
                            break;
                        }

                        let mut new_path = parent_path;
                        new_path.push(name.as_ref());
                        if old_path
                            .as_ref()
                            .map_or(false, |old_path| new_path == *old_path)
                        {
                            ids_to_write.remove(&child_id);
                            sorted_ids_to_write.next();
                            continue;
                        }

                        let assigned_temp_name;
                        if old_tree.id_for_path(&new_path, db)?.is_some() {
                            loop {
                                let name = Arc::make_mut(&mut name);
                                name.push("~");
                                new_path.set_file_name(name);

                                if old_tree.id_for_path(&new_path, db)?.is_none()
                                    && self.id_for_path(&new_path, db)?.is_none()
                                {
                                    break;
                                }
                            }
                            assigned_temp_name = true;
                        } else {
                            assigned_temp_name = false;
                        }

                        if let Some(old_path) = old_path {
                            if fs.move_dir(&old_path, &new_path) {
                                if assigned_temp_name {
                                    files_with_temp_name.insert(child_id);
                                }

                                let prev_parent_ref =
                                    old_tree.find_cur_parent_ref(child_id, db)?.unwrap();
                                old_tree.integrate_op(
                                    Operation::MoveDir {
                                        op_id: parent_ref.op_id,
                                        child_id,
                                        timestamp: parent_ref.timestamp,
                                        prev_timestamp: prev_parent_ref.timestamp,
                                        new_parent: Some((parent_id, name)),
                                    },
                                    db,
                                )?;
                            } else {
                                break;
                            }
                        } else {
                            if fs.insert_dir(&new_path) {
                                if assigned_temp_name {
                                    files_with_temp_name.insert(child_id);
                                }

                                if let Some(inode) = fs.inode_for_path(&new_path) {
                                    let operation = if let Some(prev_parent_ref) =
                                        old_tree.find_cur_parent_ref(child_id, db)?
                                    {
                                        Operation::MoveDir {
                                            op_id: parent_ref.op_id,
                                            child_id,
                                            timestamp: parent_ref.timestamp,
                                            prev_timestamp: prev_parent_ref.timestamp,
                                            new_parent: Some((parent_id, name)),
                                        }
                                    } else {
                                        Operation::InsertDir {
                                            op_id: child_id,
                                            timestamp: parent_ref.timestamp,
                                            parent_id,
                                            name,
                                        }
                                    };
                                    old_tree.integrate_op(operation, db)?;
                                    old_tree.set_inode_for_id(child_id, inode, db)?;
                                    self.set_inode_for_id(child_id, inode, db)?;
                                } else {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                } else if let Some(old_path) = old_path {
                    if fs.remove_dir(&old_path) {
                        let prev_parent_ref = old_tree.find_cur_parent_ref(child_id, db)?.unwrap();
                        old_tree.integrate_op(
                            Operation::MoveDir {
                                op_id: parent_ref.op_id,
                                child_id,
                                timestamp: parent_ref.timestamp,
                                prev_timestamp: prev_parent_ref.timestamp,
                                new_parent: None,
                            },
                            db,
                        )?;
                    } else {
                        break;
                    }
                }

                ids_to_write.remove(&child_id);
                sorted_ids_to_write.next();
            }

            if sorted_ids_to_write.peek().is_some() {
                // println!("refreshing old timeline - paths: {:?}", fs.paths());
                let fs_ops = old_tree.read_from_fs(fs.entries(), db)?;
                let fs_fixup_ops = self.integrate_ops::<F, _, _>(&fs_ops, None, db)?;
                for op in &fs_ops {
                    match op {
                        Operation::InsertDir { op_id, .. } => {
                            let inode = old_tree.inode_for_id(*op_id, db)?.unwrap();
                            self.set_inode_for_id(*op_id, inode, db)?;
                        }
                        Operation::MoveDir { child_id, .. } => {
                            files_with_temp_name.remove(child_id);
                        }
                    }
                }
                for op in &fs_fixup_ops {
                    ids_to_write.insert(op.child_id());
                    files_with_temp_name.remove(&op.child_id());
                }
                fixup_ops.extend(fs_ops);
                fixup_ops.extend(fs_fixup_ops);
            } else {
                break;
            }
        }

        for child_id in files_with_temp_name {
            if let Some(old_path) = old_tree.path_for_id(child_id, db)? {
                let mut new_path = old_path.clone();
                new_path.set_file_name(self.name_for_id(child_id, db)?.unwrap().as_os_str());

                if new_path != old_path {
                    if fs.inode_for_path(&old_path) == old_tree.inode_for_id(child_id, db)?
                        && fs.move_dir(&old_path, &new_path)
                    {
                        old_tree.move_dir(&old_path, &new_path, db)?;
                    }
                }
            }
        }

        Ok(fixup_ops)
    }

    pub fn read_from_fs<'a, F, I, S>(
        &mut self,
        entries: I,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError>
    where
        F: FileSystemEntry,
        I: IntoIterator<Item = F>,
        S: Store,
    {
        #[derive(Debug)]
        struct DirChange {
            order_key: usize,
            inode: Option<Inode>,
            parent: Option<(id::Unique, Arc<OsString>)>,
            moved: bool,
        }

        let metadata_db = db.metadata_store();
        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        let mut dir_stack = vec![ROOT_ID];
        let mut visited_inodes = BTreeSet::new();
        let mut dir_changes: BTreeMap<id::Unique, DirChange> = BTreeMap::new();

        for (order_key, entry) in entries.into_iter().enumerate() {
            assert!(entry.depth() > 0);
            dir_stack.truncate(entry.depth());
            visited_inodes.insert(entry.inode());

            let new_parent_id = *dir_stack.last().unwrap();
            if let Some(file_id) = self.inodes_to_file_ids.get(&entry.inode()).cloned() {
                if dir_changes.contains_key(&file_id) {
                    let dir_change = dir_changes.get_mut(&file_id).unwrap();
                    dir_change.parent = Some((new_parent_id, Arc::new(entry.name().into())));
                } else {
                    let parent_ref = self.find_cur_parent_ref(file_id, db)?.unwrap();
                    let parent = parent_ref
                        .parent
                        .as_ref()
                        .map(|(parent_id, name)| (parent_id, name.as_os_str()));
                    if parent != Some((&new_parent_id, entry.name())) {
                        dir_changes.insert(
                            file_id,
                            DirChange {
                                order_key,
                                inode: Some(entry.inode()),
                                parent: Some((new_parent_id, Arc::new(entry.name().into()))),
                                moved: true,
                            },
                        );
                    }
                }
                dir_stack.push(file_id);
            } else {
                let file_id = db.gen_id();
                dir_changes.insert(
                    file_id,
                    DirChange {
                        order_key,
                        inode: Some(entry.inode()),
                        parent: Some((new_parent_id, Arc::new(entry.name().into()))),
                        moved: false,
                    },
                );
                self.inodes_to_file_ids.insert(entry.inode(), file_id);
                dir_stack.push(file_id);
            }
        }

        let mut dir_changes = dir_changes.into_iter().collect::<Vec<_>>();
        {
            let visited_parent_ids = Some(ROOT_ID).into_iter().chain(
                visited_inodes
                    .iter()
                    .map(|inode| *self.inodes_to_file_ids.get(&inode).unwrap()),
            );
            for parent_id in visited_parent_ids {
                let mut cursor = self.cursor_at(parent_id, db)?;
                while let Some(Metadata { file_id, inode, .. }) = cursor.metadata(db)? {
                    if inode.map_or(true, |inode| !visited_inodes.contains(&inode)) {
                        dir_changes.push((
                            file_id,
                            DirChange {
                                order_key: 0,
                                inode,
                                parent: None,
                                moved: true,
                            },
                        ));
                    }
                    cursor.next_sibling_or_cousin(db)?;
                }
            }
        }

        let mut operations = Vec::new();
        let mut metadata = Vec::new();
        let mut parent_refs = Vec::new();
        let mut child_refs = Vec::new();

        for (child_id, change) in dir_changes {
            if change.moved {
                let operation = self.local_move(
                    child_id,
                    change.parent,
                    &mut parent_refs,
                    &mut child_refs,
                    db,
                )?;
                operations.push(operation);
            } else {
                let (parent_id, name) = change.parent.unwrap();
                let operation = self.local_insert(
                    parent_id,
                    name,
                    child_id,
                    change.inode,
                    &mut metadata,
                    &mut parent_refs,
                    &mut child_refs,
                    db,
                )?;
                operations.push(operation);
            }
        }

        self.metadata = edit_tree(&self.metadata, metadata, metadata_db)?;
        self.parent_refs = edit_tree(&self.parent_refs, parent_refs, parent_ref_db)?;
        self.child_refs = edit_tree(&self.child_refs, child_refs, child_ref_db)?;
        Ok(operations)
    }

    pub fn insert_dirs<I, S>(&mut self, path: I, db: &S) -> Result<Vec<Operation>, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        self.insert_dirs_internal(path, &mut None, db)
    }

    // TODO: Return an error if there is a name conflict.
    fn insert_dirs_internal<I, S>(
        &mut self,
        path: I,
        next_inode: &mut Option<&mut Inode>,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();
        let metadata_db = db.metadata_store();
        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        let mut operations = Vec::new();
        let mut metadata = Vec::new();
        let mut parent_refs = Vec::new();
        let mut child_refs = Vec::new();

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
                let child_id = db.gen_id();
                let inode = next_inode.as_mut().map(|next_inode| {
                    let inode = **next_inode;
                    **next_inode += 1;
                    inode
                });
                let operation = self.local_insert(
                    parent_id,
                    name,
                    child_id,
                    inode,
                    &mut metadata,
                    &mut parent_refs,
                    &mut child_refs,
                    db,
                )?;
                parent_id = child_id;
                operations.push(operation);
                if let Some(inode) = inode {
                    self.inodes_to_file_ids.insert(inode, parent_id);
                }
            }
        }

        self.metadata = edit_tree(&self.metadata, metadata, metadata_db)?;
        self.parent_refs = edit_tree(&self.parent_refs, parent_refs, parent_ref_db)?;
        self.child_refs = edit_tree(&self.child_refs, child_refs, child_ref_db)?;
        Ok(operations)
    }

    pub fn remove_dir<I, S>(&mut self, path: I, db: &S) -> Result<Option<Operation>, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();
        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        let mut parent_refs = Vec::new();
        let mut child_refs = Vec::new();

        if let Some(child_id) = self.id_for_path(&path, db)? {
            let operation =
                self.local_move(child_id, None, &mut parent_refs, &mut child_refs, db)?;
            self.parent_refs = edit_tree(&self.parent_refs, parent_refs, parent_ref_db)?;
            self.child_refs = edit_tree(&self.child_refs, child_refs, child_ref_db)?;
            Ok(Some(operation))
        } else {
            Ok(None)
        }
    }

    pub fn move_dir<F, S, T>(
        &mut self,
        from: F,
        to: T,
        db: &S,
    ) -> Result<Option<Operation>, S::ReadError>
    where
        F: Into<PathBuf>,
        S: Store,
        T: Into<PathBuf>,
    {
        let from = from.into();
        let to = to.into();
        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        if self.id_for_path(&to, db)?.is_some() {
            return Ok(None);
        }

        let from_id = self.id_for_path(&from, db)?;
        let new_parent_id = if let Some(parent_path) = to.parent() {
            self.id_for_path(parent_path, db)?
        } else {
            Some(ROOT_ID)
        };

        if let (Some(from_id), Some(new_parent_id)) = (from_id, new_parent_id) {
            let mut parent_refs = Vec::new();
            let mut child_refs = Vec::new();

            let new_name = Arc::new(OsString::from(to.file_name().unwrap()));
            let operation = self.local_move(
                from_id,
                Some((new_parent_id, new_name)),
                &mut parent_refs,
                &mut child_refs,
                db,
            )?;

            self.parent_refs = edit_tree(&self.parent_refs, parent_refs, parent_ref_db)?;
            self.child_refs = edit_tree(&self.child_refs, child_refs, child_ref_db)?;

            Ok(Some(operation))
        } else {
            Ok(None)
        }
    }

    pub fn integrate_ops<'a, F, O, S>(
        &mut self,
        ops: O,
        fs: Option<&mut F>,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError>
    where
        F: FileSystem,
        O: IntoIterator<Item = &'a Operation> + Clone,
        S: Store,
    {
        // println!("integrate ops >>>>>>>>>>>>");
        let old_tree = self.clone();

        let mut changed_ids = BTreeMap::new();
        for op in ops.clone() {
            // println!("integrate op {:#?}", op);

            match op {
                Operation::InsertDir { op_id, .. } => changed_ids.insert(*op_id, false),
                Operation::MoveDir { child_id, .. } => changed_ids.insert(*child_id, true),
            };
            self.integrate_op(op.clone(), db)?;
        }

        let mut fixup_ops = Vec::new();
        for (child_id, moved) in &changed_ids {
            fixup_ops.extend(self.fix_conflicts(*child_id, *moved, db)?);
        }

        if let Some(fs) = fs {
            let mut ids_to_write = BTreeSet::new();
            for (id, moved) in changed_ids {
                ids_to_write.insert(id);
                if moved && old_tree.depth_for_id(id, db)?.is_none() {
                    let mut cursor = self.cursor_at(id, db)?;
                    while let Some(descendant_id) = cursor.file_id(db)? {
                        // println!(
                        //     "resurrecting descendant {:?} {:?}",
                        //     descendant_id,
                        //     cursor.path()
                        // );
                        ids_to_write.insert(descendant_id);
                        cursor.next(db)?;
                    }
                }
            }
            for op in &fixup_ops {
                ids_to_write.insert(op.child_id());
            }

            fixup_ops.extend(self.write_to_fs(ids_to_write, old_tree, fs, db)?);
        }

        // println!("integrate ops <<<<<<<<<<<<");

        Ok(fixup_ops)
    }

    fn sort_ids_by_path_depth<I, S>(
        &self,
        file_ids: I,
        db: &S,
    ) -> Result<Vec<(id::Unique, Option<usize>)>, S::ReadError>
    where
        I: Iterator<Item = id::Unique>,
        S: Store,
    {
        let mut changed_id_to_depths = BTreeMap::new();
        for file_id in file_ids {
            changed_id_to_depths.insert(file_id, self.depth_for_id(file_id, db)?);
        }

        let mut sorted_file_ids = changed_id_to_depths.into_iter().collect::<Vec<_>>();
        sorted_file_ids.sort_by(|(_, depth_a), (_, depth_b)| {
            if depth_a.is_none() {
                Ordering::Greater
            } else if depth_b.is_none() {
                Ordering::Less
            } else {
                depth_a.cmp(&depth_b)
            }
        });

        Ok(sorted_file_ids)
    }

    // #[cfg(test)]
    pub fn paths<S: Store>(&self, db: &S) -> Vec<String> {
        let mut cursor = self.cursor(db).unwrap();
        let mut paths = Vec::new();
        loop {
            if let Some(path) = cursor.path() {
                let mut path = path.to_string_lossy().into_owned();
                if cursor.metadata(db).unwrap().unwrap().is_dir {
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

    #[cfg(test)]
    fn paths_with_ids<S: Store>(&self, db: &S) -> Vec<(id::Unique, String)> {
        self.paths(db)
            .into_iter()
            .map(|path| (self.id_for_path(&path, db).unwrap().unwrap(), path))
            .collect()
    }

    fn integrate_op<S>(&mut self, op: Operation, db: &S) -> Result<(), S::ReadError>
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

        // println!("{:?} – integrate op {:?}", db.replica_id(), op);

        match op {
            Operation::InsertDir {
                op_id,
                timestamp,
                parent_id,
                name,
            } => {
                new_child_ref = Some(ChildRef {
                    parent_id,
                    name: name.clone(),
                    timestamp,
                    op_id,
                    child_id: op_id,
                    deletions: SmallVec::new(),
                });

                metadata.push(TreeEdit::Insert(Metadata {
                    file_id: op_id,
                    is_dir: true,
                    inode: None,
                }));
                parent_refs.push(TreeEdit::Insert(ParentRef {
                    child_id: op_id,
                    timestamp,
                    prev_timestamp: timestamp,
                    op_id,
                    parent: Some((parent_id, name)),
                }));
                child_refs.push(TreeEdit::Insert(new_child_ref.clone().unwrap()));
                received_timestamp = timestamp;
            }
            Operation::MoveDir {
                op_id,
                child_id,
                timestamp,
                prev_timestamp,
                new_parent,
            } => {
                new_child_ref = new_parent.as_ref().map(|(parent_id, name)| ChildRef {
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
                let mut is_latest_parent_ref = true;

                while let Some(parent_ref) = parent_ref_cursor.item(parent_ref_db)? {
                    if parent_ref.child_id != child_id {
                        break;
                    } else if parent_ref.timestamp > timestamp {
                        if parent_ref.prev_timestamp < timestamp && new_child_ref.is_some() {
                            let new_child_ref = new_child_ref.as_mut().unwrap();
                            new_child_ref.deletions.push(parent_ref.op_id);
                        }
                    } else if parent_ref.timestamp >= prev_timestamp {
                        if let Some(mut child_ref_key) =
                            parent_ref.to_child_ref_key(is_latest_parent_ref)
                        {
                            child_ref_cursor.seek(&child_ref_key, SeekBias::Left, child_ref_db)?;
                            let mut child_ref = child_ref_cursor.item(child_ref_db)?.unwrap();
                            if child_ref.is_visible() {
                                child_refs.push(TreeEdit::Remove(child_ref.clone()));
                            }
                            child_ref.deletions.push(op_id);
                            child_refs.push(TreeEdit::Insert(child_ref));
                        }
                    } else {
                        break;
                    }
                    parent_ref_cursor.next(parent_ref_db)?;
                    is_latest_parent_ref = false;
                }

                parent_refs.push(TreeEdit::Insert(ParentRef {
                    child_id,
                    timestamp,
                    prev_timestamp,
                    op_id,
                    parent: new_parent,
                }));
                if let Some(new_child_ref) = new_child_ref {
                    child_refs.push(TreeEdit::Insert(new_child_ref.clone()));
                }
                received_timestamp = timestamp;
            }
        }

        self.child_refs = edit_tree(&self.child_refs, child_refs, child_ref_db)?;
        self.metadata = edit_tree(&self.metadata, metadata, metadata_db)?;
        self.parent_refs = edit_tree(&self.parent_refs, parent_refs, parent_ref_db)?;
        if db.replica_id() != received_timestamp.replica_id {
            db.recv_timestamp(received_timestamp);
        }
        Ok(())
    }

    fn fix_conflicts<S: Store>(
        &mut self,
        child_id: id::Unique,
        moved: bool,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError> {
        let mut fixup_ops = Vec::new();
        let mut reverted_moves: BTreeMap<id::Unique, LamportTimestamp> = BTreeMap::new();

        // If the child was moved, check for cycles
        if moved {
            let parent_ref_db = db.parent_ref_store();
            let mut visited = BTreeSet::new();
            let mut latest_move: Option<ParentRef> = None;
            let mut cursor = self.parent_refs.cursor();
            cursor.seek(&child_id, SeekBias::Left, parent_ref_db)?;

            loop {
                let mut parent_ref = cursor.item(parent_ref_db)?.unwrap();
                if visited.contains(&parent_ref.child_id) {
                    // println!("cycle detected");
                    // Cycle detected. Revert the most recent move contributing to the cycle.
                    cursor.seek(
                        &latest_move.as_ref().unwrap().key(),
                        SeekBias::Right,
                        parent_ref_db,
                    )?;

                    // Find the previous value for this parent ref that isn't a deletion and store
                    // its timestamp in our reverted_moves map.
                    loop {
                        let parent_ref = cursor.item(parent_ref_db)?.unwrap();
                        if parent_ref.parent.is_some() {
                            reverted_moves.insert(parent_ref.child_id, parent_ref.timestamp);
                            break;
                        } else {
                            cursor.next(parent_ref_db)?;
                        }
                    }

                    // Reverting this move may not have been enough to break the cycle. We clear
                    // the visited set but continue looping, potentially reverting multiple moves.
                    latest_move = None;
                    visited.clear();
                } else {
                    visited.insert(parent_ref.child_id);

                    // If we have already reverted this parent ref to a previous value, interpret
                    // it as having the value we reverted to.
                    if let Some(prev_timestamp) = reverted_moves.get(&parent_ref.child_id) {
                        while parent_ref.timestamp > *prev_timestamp {
                            cursor.next(parent_ref_db)?;
                            parent_ref = cursor.item(parent_ref_db)?.unwrap();
                        }
                    }

                    // Check if this parent ref is a move and has the latest timestamp of any move
                    // we have seen so far. If so, it is a candidate to be reverted.
                    if latest_move
                        .as_ref()
                        .map_or(true, |m| parent_ref.timestamp > m.timestamp)
                    {
                        cursor.next(parent_ref_db)?;
                        if cursor
                            .item(parent_ref_db)?
                            .map_or(false, |next_parent_ref| {
                                next_parent_ref.child_id == parent_ref.child_id
                            }) {
                            latest_move = Some(parent_ref.clone());
                        }
                    }

                    // Walk up to the next parent or break if none exists or the parent is the root
                    if let Some((parent_id, _)) = parent_ref.parent {
                        if parent_id == ROOT_ID {
                            break;
                        } else {
                            cursor.seek(&parent_id, SeekBias::Left, parent_ref_db)?;
                        }
                    } else {
                        break;
                    }
                }
            }

            // Convert the reverted moves into new move operations.
            let mut moved_child_ids = Vec::new();
            for (child_id, timestamp) in &reverted_moves {
                cursor.seek(child_id, SeekBias::Left, parent_ref_db)?;
                let prev_timestamp = cursor.item(parent_ref_db)?.unwrap().timestamp;
                cursor.seek_forward(
                    &ParentRefKey {
                        child_id: *child_id,
                        timestamp: *timestamp,
                    },
                    SeekBias::Left,
                    parent_ref_db,
                )?;
                let new_parent = cursor.item(parent_ref_db)?.unwrap().parent;
                fixup_ops.push(Operation::MoveDir {
                    op_id: db.gen_id(),
                    child_id: *child_id,
                    timestamp: db.gen_timestamp(),
                    prev_timestamp,
                    new_parent,
                });
                moved_child_ids.push(*child_id);
            }

            for op in &fixup_ops {
                self.integrate_op(op.clone(), db)?;
            }
            for child_id in moved_child_ids {
                fixup_ops.extend(self.fix_name_conflicts(child_id, db)?);
            }
        }

        if !reverted_moves.contains_key(&child_id) {
            fixup_ops.extend(self.fix_name_conflicts(child_id, db)?);
        }

        Ok(fixup_ops)
    }

    fn fix_name_conflicts<S: Store>(
        &mut self,
        child_id: id::Unique,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError> {
        let child_ref_db = db.child_ref_store();

        let mut fixup_ops = Vec::new();

        let parent_ref = self.find_cur_parent_ref(child_id, db)?.unwrap();
        if let Some((parent_id, name)) = parent_ref.parent {
            let mut cursor_1 = self.child_refs.cursor();
            cursor_1.seek(
                &ParentIdAndName {
                    parent_id,
                    name: name.clone(),
                },
                SeekBias::Left,
                child_ref_db,
            )?;
            cursor_1.next(child_ref_db)?;

            let mut cursor_2 = cursor_1.clone();
            let mut unique_name = name.clone();

            while let Some(child_ref) = cursor_1.item(child_ref_db)? {
                if child_ref.is_visible()
                    && child_ref.parent_id == parent_id
                    && child_ref.name == name
                {
                    loop {
                        Arc::make_mut(&mut unique_name).push("~");
                        cursor_2.seek_forward(
                            &ParentIdAndName {
                                parent_id,
                                name: unique_name.clone(),
                            },
                            SeekBias::Left,
                            child_ref_db,
                        )?;
                        if let Some(conflicting_child_ref) = cursor_2.item(child_ref_db)? {
                            if !conflicting_child_ref.is_visible()
                                || conflicting_child_ref.parent_id != parent_id
                                || conflicting_child_ref.name != unique_name
                            {
                                break;
                            }
                        } else {
                            break;
                        }
                    }

                    let fixup_op = Operation::MoveDir {
                        op_id: db.gen_id(),
                        child_id: child_ref.child_id,
                        timestamp: db.gen_timestamp(),
                        prev_timestamp: child_ref.timestamp,
                        new_parent: Some((parent_id, unique_name.clone())),
                    };
                    self.integrate_op(fixup_op.clone(), db)?;
                    fixup_ops.push(fixup_op);

                    let visible_index = cursor_1.end::<usize, _>(child_ref_db)?;
                    cursor_1.seek_forward(&visible_index, SeekBias::Right, child_ref_db)?;
                } else {
                    break;
                }
            }
        }

        Ok(fixup_ops)
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

    fn local_insert<S: Store>(
        &self,
        parent_id: id::Unique,
        name: Arc<OsString>,
        child_id: id::Unique,
        inode: Option<Inode>,
        metadata: &mut Vec<TreeEdit<Metadata>>,
        parent_refs: &mut Vec<TreeEdit<ParentRef>>,
        child_refs: &mut Vec<TreeEdit<ChildRef>>,
        db: &S,
    ) -> Result<Operation, S::ReadError> {
        let timestamp = db.gen_timestamp();

        metadata.push(TreeEdit::Insert(Metadata {
            file_id: child_id,
            is_dir: true,
            inode,
        }));
        parent_refs.push(TreeEdit::Insert(ParentRef {
            child_id,
            timestamp,
            prev_timestamp: timestamp,
            op_id: child_id,
            parent: Some((parent_id, name.clone())),
        }));
        child_refs.push(TreeEdit::Insert(ChildRef {
            parent_id,
            name: name.clone(),
            timestamp,
            op_id: child_id,
            child_id,
            deletions: SmallVec::new(),
        }));
        Ok(Operation::InsertDir {
            op_id: child_id,
            timestamp,
            parent_id,
            name,
        })
    }

    fn local_move<S: Store>(
        &self,
        child_id: id::Unique,
        new_parent: Option<(id::Unique, Arc<OsString>)>,
        parent_refs: &mut Vec<TreeEdit<ParentRef>>,
        child_refs: &mut Vec<TreeEdit<ChildRef>>,
        db: &S,
    ) -> Result<Operation, S::ReadError> {
        let timestamp = db.gen_timestamp();
        let op_id = db.gen_id();

        let prev_parent_ref = self.find_cur_parent_ref(child_id, db)?.unwrap();
        parent_refs.push(TreeEdit::Insert(ParentRef {
            child_id,
            timestamp,
            prev_timestamp: prev_parent_ref.timestamp,
            op_id,
            parent: new_parent.clone(),
        }));

        if let Some(prev_child_ref_key) = prev_parent_ref.to_child_ref_key(true) {
            let mut prev_child_ref = self.find_child_ref(prev_child_ref_key, db)?.unwrap();
            child_refs.push(TreeEdit::Remove(prev_child_ref.clone()));
            prev_child_ref.deletions.push(op_id);
            child_refs.push(TreeEdit::Insert(prev_child_ref));
        }

        if let Some((new_parent_id, new_name)) = new_parent.as_ref() {
            child_refs.push(TreeEdit::Insert(ChildRef {
                parent_id: *new_parent_id,
                name: new_name.clone(),
                timestamp,
                op_id,
                child_id,
                deletions: SmallVec::new(),
            }));
        }

        Ok(Operation::MoveDir {
            op_id,
            child_id,
            timestamp,
            prev_timestamp: prev_parent_ref.timestamp,
            new_parent,
        })
    }

    fn path_for_id<S>(&self, child_id: id::Unique, db: &S) -> Result<Option<PathBuf>, S::ReadError>
    where
        S: Store,
    {
        let mut path_components = Vec::new();
        if self.visit_ancestors(child_id, |name| path_components.push(name), db)? {
            let mut path = PathBuf::new();
            for component in path_components.into_iter().rev() {
                path.push(component.as_ref());
            }
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    fn depth_for_id<S>(&self, child_id: id::Unique, db: &S) -> Result<Option<usize>, S::ReadError>
    where
        S: Store,
    {
        let mut depth = 0;
        if self.visit_ancestors(child_id, |_| depth += 1, db)? {
            Ok(Some(depth))
        } else {
            Ok(None)
        }
    }

    fn visit_ancestors<F, S>(
        &self,
        child_id: id::Unique,
        mut f: F,
        db: &S,
    ) -> Result<bool, S::ReadError>
    where
        F: FnMut(Arc<OsString>),
        S: Store,
    {
        let parent_ref_db = db.parent_ref_store();

        let mut visited = BTreeSet::new();
        let mut cursor = self.parent_refs.cursor();
        if child_id == ROOT_ID {
            Ok(true)
        } else if cursor.seek(&child_id, SeekBias::Left, parent_ref_db)? {
            loop {
                if let Some((parent_id, name)) = cursor.item(parent_ref_db)?.and_then(|r| r.parent)
                {
                    // TODO: Only check for cycles in debug mode
                    if visited.contains(&parent_id) {
                        panic!("Cycle detected when visiting ancestors");
                    } else {
                        visited.insert(parent_id);
                    }

                    f(name);
                    if parent_id == ROOT_ID {
                        break;
                    } else {
                        cursor.seek(&parent_id, SeekBias::Left, parent_ref_db)?;
                    }
                } else {
                    return Ok(false);
                }
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn name_for_id<S: Store>(
        &self,
        child_id: id::Unique,
        db: &S,
    ) -> Result<Option<Arc<OsString>>, S::ReadError> {
        Ok(self
            .find_cur_parent_ref(child_id, db)?
            .and_then(|parent_ref| parent_ref.parent)
            .map(|(_, name)| name))
    }

    fn inode_for_id<S: Store>(
        &self,
        child_id: id::Unique,
        db: &S,
    ) -> Result<Option<Inode>, S::ReadError> {
        let metadata_db = db.metadata_store();
        let mut cursor = self.metadata.cursor();
        if cursor.seek(&child_id, SeekBias::Left, metadata_db)? {
            Ok(cursor.item(metadata_db)?.unwrap().inode)
        } else {
            Ok(None)
        }
    }

    fn set_inode_for_id<S: Store>(
        &mut self,
        child_id: id::Unique,
        inode: Inode,
        db: &S,
    ) -> Result<bool, S::ReadError> {
        let metadata_db = db.metadata_store();
        let mut cursor = self.metadata.cursor();
        if cursor.seek(&child_id, SeekBias::Left, metadata_db)? {
            let mut metadata = cursor.item(metadata_db)?.unwrap();
            if let Some(inode) = metadata.inode {
                self.inodes_to_file_ids.remove(&inode);
            }

            metadata.inode = Some(inode);
            self.metadata = edit_tree(
                &self.metadata,
                vec![TreeEdit::Insert(metadata)],
                metadata_db,
            )?;
            self.inodes_to_file_ids.insert(inode, child_id);
            Ok(true)
        } else {
            Ok(false)
        }
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

    fn find_child_ref<S>(&self, key: ChildRefKey, db: &S) -> Result<Option<ChildRef>, S::ReadError>
    where
        S: Store,
    {
        let child_ref_db = db.child_ref_store();
        let mut cursor = self.child_refs.cursor();
        if cursor.seek(&key, SeekBias::Left, child_ref_db)? {
            cursor.item(child_ref_db)
        } else {
            Ok(None)
        }
    }
}

impl Cursor {
    pub fn depth(&self) -> usize {
        self.stack.last().map_or(0, |entry| entry.depth)
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
            let stack_entry = self.stack.last().unwrap();
            Ok(Some(stack_entry.cursor.item(db)?.unwrap().name.clone()))
        }
    }

    pub fn file_id<S: Store>(&self, db: &S) -> Result<Option<id::Unique>, S::ReadError> {
        Ok(self.metadata(db)?.map(|metadata| metadata.file_id))
    }

    fn metadata<S: Store>(&self, db: &S) -> Result<Option<Metadata>, S::ReadError> {
        if self.stack.is_empty() {
            Ok(None)
        } else {
            self.metadata_cursor.item(db.metadata_store())
        }
    }

    pub fn next<S: Store>(&mut self, db: &S) -> Result<(), S::ReadError> {
        let metadata_db = db.metadata_store();

        if !self.stack.is_empty() {
            let metadata = self.metadata_cursor.item(metadata_db)?.unwrap();
            if !metadata.is_dir || !self.descend(db)? {
                self.next_sibling_or_cousin(db)?;
            }
        }
        Ok(())
    }

    pub fn next_sibling_or_cousin<S: Store>(&mut self, db: &S) -> Result<(), S::ReadError> {
        let metadata_db = db.metadata_store();
        let child_ref_db = db.child_ref_store();

        while !self.stack.is_empty() && !self.next_sibling(db)? {
            self.path.pop();
            if self.stack.pop().map_or(false, |entry| entry.jump) {
                if let Some(entry) = self.stack.last() {
                    let child_ref = entry.cursor.item(child_ref_db)?.unwrap();
                    self.metadata_cursor
                        .seek(&child_ref.child_id, SeekBias::Left, metadata_db)?;
                }
                return Ok(());
            }
        }
        Ok(())
    }

    pub fn jump<S: Store>(
        &mut self,
        child_id: id::Unique,
        depth: usize,
        db: &S,
    ) -> Result<bool, S::ReadError> {
        let stack_entry = self.stack.last().unwrap().clone();
        self.visited_dir_ids.insert(child_id);
        self.descend_into(stack_entry.cursor, child_id, Some(depth), db)
    }

    fn descend<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        let stack_entry = self.stack.last().unwrap().clone();
        let dir_id = stack_entry
            .cursor
            .item(db.child_ref_store())?
            .unwrap()
            .child_id;
        self.descend_into(stack_entry.cursor, dir_id, None, db)
    }

    fn descend_into<S: Store>(
        &mut self,
        mut child_ref_cursor: btree::Cursor<ChildRef>,
        dir_id: id::Unique,
        depth: Option<usize>,
        db: &S,
    ) -> Result<bool, S::ReadError> {
        child_ref_cursor.seek(&dir_id, SeekBias::Left, db.child_ref_store())?;
        if let Some(child_ref) = child_ref_cursor.item(db.child_ref_store())? {
            if child_ref.parent_id == dir_id {
                let prev_depth = self.depth();
                self.path.push(child_ref.name.as_os_str());
                self.stack.push(CursorStackEntry {
                    cursor: child_ref_cursor.clone(),
                    jump: depth.is_some(),
                    depth: depth.unwrap_or(prev_depth) + 1,
                });
                if child_ref.is_visible() && !self.visited_dir_ids.contains(&child_ref.child_id) {
                    self.metadata_cursor.seek(
                        &child_ref.child_id,
                        SeekBias::Left,
                        db.metadata_store(),
                    )?;
                    self.visited_dir_ids.insert(child_ref.child_id);
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
        let stack_entry = self.stack.last_mut().unwrap();
        let parent_id = stack_entry.cursor.item(child_ref_db)?.unwrap().parent_id;
        let next_visible_index: usize = stack_entry.cursor.end(child_ref_db)?;
        stack_entry
            .cursor
            .seek(&next_visible_index, SeekBias::Right, child_ref_db)?;
        while let Some(child_ref) = stack_entry.cursor.item(child_ref_db)? {
            if child_ref.parent_id == parent_id {
                if self.visited_dir_ids.contains(&child_ref.child_id) {
                    let next_visible_index: usize = stack_entry.cursor.end(child_ref_db)?;
                    stack_entry
                        .cursor
                        .seek(&next_visible_index, SeekBias::Right, child_ref_db)?;
                } else {
                    self.path.pop();
                    self.path.push(child_ref.name.as_os_str());
                    self.metadata_cursor.seek(
                        &child_ref.child_id,
                        SeekBias::Left,
                        db.metadata_store(),
                    )?;
                    self.visited_dir_ids.insert(child_ref.child_id);
                    return Ok(true);
                }
            } else {
                break;
            }
        }

        Ok(false)
    }
}

impl Operation {
    fn child_id(&self) -> id::Unique {
        match self {
            Operation::InsertDir { op_id, .. } => *op_id,
            Operation::MoveDir { child_id, .. } => *child_id,
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
    fn to_child_ref_key(&self, visible: bool) -> Option<ChildRefKey> {
        self.parent.as_ref().map(|(parent_id, name)| ChildRefKey {
            parent_id: *parent_id,
            name: name.clone(),
            visible,
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
        assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ParentRefKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self < *other);
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
            visible: self.is_visible(),
            timestamp: self.timestamp,
            visible_count: if self.is_visible() { 1 } else { 0 },
        }
    }
}

impl Keyed for ChildRef {
    type Key = ChildRefKey;

    fn key(&self) -> Self::Key {
        ChildRefKey {
            parent_id: self.parent_id,
            name: self.name.clone(),
            visible: self.is_visible(),
            timestamp: self.timestamp,
        }
    }
}

impl Ord for ChildRefSummary {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parent_id
            .cmp(&other.parent_id)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.visible.cmp(&other.visible).reverse())
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
        assert!(*self < *other, "{:?} < {:?}", self, other);

        self.parent_id = other.parent_id;
        self.name = other.name.clone();
        self.visible = other.visible;
        self.timestamp = other.timestamp;
        self.visible_count += other.visible_count;
    }
}

impl btree::Dimension<ChildRefSummary> for ChildRefKey {
    fn from_summary(summary: &ChildRefSummary) -> ChildRefKey {
        ChildRefKey {
            parent_id: summary.parent_id,
            name: summary.name.clone(),
            visible: summary.visible,
            timestamp: summary.timestamp,
        }
    }
}

impl Ord for ChildRefKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parent_id
            .cmp(&other.parent_id)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.visible.cmp(&other.visible).reverse())
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
        assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ChildRefKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self < *other);
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
        assert!(*self <= *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ParentIdAndName {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self <= *other);
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

impl<T: Keyed> TreeEdit<T> {
    fn key(&self) -> T::Key {
        match self {
            TreeEdit::Insert(item) | TreeEdit::Remove(item) => item.key(),
        }
    }
}

fn edit_tree<T, S>(
    old_tree: &btree::Tree<T>,
    mut ops: Vec<TreeEdit<T>>,
    db: &S,
) -> Result<btree::Tree<T>, S::ReadError>
where
    T: btree::Item + Keyed,
    T::Key: btree::Dimension<T::Summary> + Default,
    S: btree::NodeStore<T>,
{
    ops.sort_unstable_by_key(|item| item.key());

    let mut old_cursor = old_tree.cursor();
    let mut new_tree = btree::Tree::new();
    let mut buffered_items = Vec::new();

    old_cursor.seek(&T::Key::default(), SeekBias::Left, db)?;
    for op in ops {
        let new_key = op.key();
        let mut old_item = old_cursor.item(db)?;

        if old_item
            .as_ref()
            .map_or(false, |old_item| old_item.key() < new_key)
        {
            new_tree.extend(buffered_items.drain(..), db)?;
            let slice = old_cursor.slice(&new_key, SeekBias::Left, db)?;
            new_tree.push_tree(slice, db)?;
            old_item = old_cursor.item(db)?;
        }
        if old_item.map_or(false, |old_item| old_item.key() == new_key) {
            old_cursor.next(db)?;
        }
        match op {
            TreeEdit::Insert(item) => {
                buffered_items.push(item);
            }
            TreeEdit::Remove(_) => {}
        }
    }

    new_tree.extend(buffered_items, db)?;
    new_tree.push_tree(old_cursor.suffix::<T::Key, _>(db)?, db)?;

    Ok(new_tree)
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use self::rand::{Rng, SeedableRng, StdRng};
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn test_local_dir_ops() {
        let db = NullStore::new(1);
        let mut timeline = Timeline::new();
        timeline.insert_dirs("a/b2/", &db).unwrap();
        assert_eq!(timeline.paths(&db), ["a/", "a/b2/"]);

        timeline.insert_dirs("a/b1/c", &db).unwrap();
        assert_eq!(timeline.paths(&db), ["a/", "a/b1/", "a/b1/c/", "a/b2/"]);

        timeline.insert_dirs("a/b1/d", &db).unwrap();
        assert_eq!(
            timeline.paths(&db),
            ["a/", "a/b1/", "a/b1/c/", "a/b1/d/", "a/b2/"]
        );

        timeline.remove_dir("a/b1/c", &db).unwrap();
        assert_eq!(timeline.paths(&db), ["a/", "a/b1/", "a/b1/d/", "a/b2/"]);

        timeline.remove_dir("a/b1", &db).unwrap();
        assert_eq!(timeline.paths(&db), ["a/", "a/b2/"]);

        timeline.insert_dirs("a/b1/c", &db).unwrap();
        timeline.insert_dirs("a/b1/d", &db).unwrap();
        assert_eq!(
            timeline.paths(&db),
            ["a/", "a/b1/", "a/b1/c/", "a/b1/d/", "a/b2/"]
        );

        let moved_id = timeline.id_for_path("a/b1", &db).unwrap().unwrap();
        timeline.move_dir("a/b1", "b", &db).unwrap();
        assert_eq!(timeline.paths(&db), ["a/", "a/b2/", "b/", "b/c/", "b/d/"]);
        assert_eq!(timeline.id_for_path("b", &db).unwrap().unwrap(), moved_id);

        let moved_id = timeline.id_for_path("b/d", &db).unwrap().unwrap();
        timeline.move_dir("b/d", "a/b2/d", &db).unwrap();
        assert_eq!(
            timeline.paths(&db),
            ["a/", "a/b2/", "a/b2/d/", "b/", "b/c/"]
        );
        assert_eq!(
            timeline.id_for_path("a/b2/d", &db).unwrap().unwrap(),
            moved_id
        );
    }

    #[test]
    fn test_fs_sync_random() {
        for seed in 0..100 {
            println!("SEED: {:?}", seed);

            let mut rng = StdRng::from_seed(&[seed]);
            let db = NullStore::new(1);

            let mut fs_1 = FakeFileSystem::new(&db, rng.clone());
            fs_1.mutate(5);
            let mut fs_2 = fs_1.clone();
            let mut prev_fs_1_version = fs_1.version();
            let mut prev_fs_2_version = fs_2.version();
            let mut index_1 = fs_1.timeline();
            let mut index_2 = index_1.clone();
            let mut ops_1 = Vec::new();
            let mut ops_2 = Vec::new();

            fs_1.mutate(5);

            loop {
                if fs_1.version() > prev_fs_1_version && rng.gen() {
                    prev_fs_1_version = fs_1.version();
                    ops_1.extend(index_1.read_from_fs(fs_1.entries(), &db).unwrap());
                }

                if fs_2.version() > prev_fs_2_version && rng.gen() {
                    prev_fs_2_version = fs_2.version();
                    ops_2.extend(index_2.read_from_fs(fs_2.entries(), &db).unwrap());
                }

                if !ops_2.is_empty() && rng.gen() {
                    ops_1.extend(
                        index_1
                            .integrate_ops(
                                &ops_2.drain(..).collect::<Vec<_>>(),
                                Some(&mut fs_1),
                                &db,
                            ).unwrap(),
                    );
                }

                if !ops_1.is_empty() && rng.gen() {
                    ops_2.extend(
                        index_2
                            .integrate_ops(
                                &ops_1.drain(..).collect::<Vec<_>>(),
                                Some(&mut fs_2),
                                &db,
                            ).unwrap(),
                    );
                }

                if ops_1.is_empty()
                    && ops_2.is_empty()
                    && fs_1.version() == prev_fs_1_version
                    && fs_2.version() == prev_fs_2_version
                {
                    break;
                }
            }

            assert_eq!(index_2.paths_with_ids(&db), index_1.paths_with_ids(&db));
            assert_eq!(fs_2.paths(), fs_1.paths());
            assert_eq!(index_1.paths(&db), fs_1.paths());
        }
    }

    #[test]
    fn test_name_conflict_fixups() {
        let db_1 = NullStore::new(1);
        let mut timeline_1 = Timeline::new();
        let mut timeline_1_ops = Vec::new();

        let db_2 = NullStore::new(2);
        let mut timeline_2 = Timeline::new();
        let mut timeline_2_ops = Vec::new();

        timeline_1_ops.extend(timeline_1.insert_dirs("a", &db_1).unwrap());
        let id_1 = timeline_1.id_for_path("a", &db_1).unwrap().unwrap();

        timeline_2_ops.extend(timeline_2.insert_dirs("a", &db_2).unwrap());
        timeline_2_ops.extend(timeline_2.insert_dirs("a~", &db_2).unwrap());
        let id_2 = timeline_2.id_for_path("a", &db_2).unwrap().unwrap();
        let id_3 = timeline_2.id_for_path("a~", &db_2).unwrap().unwrap();

        while !timeline_1_ops.is_empty() || !timeline_2_ops.is_empty() {
            let ops_from_timeline_2_to_timeline_1 = timeline_2_ops.drain(..).collect::<Vec<_>>();
            let ops_from_timeline_1_to_timeline_2 = timeline_1_ops.drain(..).collect::<Vec<_>>();
            timeline_1_ops.extend(
                timeline_1
                    .integrate_ops::<FakeFileSystem<StdRng>, _, _>(
                        &ops_from_timeline_2_to_timeline_1,
                        None,
                        &db_1,
                    ).unwrap(),
            );
            timeline_2_ops.extend(
                timeline_2
                    .integrate_ops::<FakeFileSystem<StdRng>, _, _>(
                        &ops_from_timeline_1_to_timeline_2,
                        None,
                        &db_2,
                    ).unwrap(),
            );
        }

        assert_eq!(
            timeline_1.paths_with_ids(&db_1),
            timeline_2.paths_with_ids(&db_2)
        );
        assert_eq!(timeline_1.paths(&db_1), ["a/", "a~/", "a~~/"]);
        assert_eq!(timeline_1.id_for_path("a", &db_1).unwrap().unwrap(), id_2);
        assert_eq!(timeline_1.id_for_path("a~", &db_1).unwrap().unwrap(), id_3);
        assert_eq!(timeline_1.id_for_path("a~~", &db_1).unwrap().unwrap(), id_1);
    }

    #[test]
    fn test_cycle_fixups() {
        let db_1 = NullStore::new(1);
        let mut timeline_1 = Timeline::new();
        timeline_1.insert_dirs("a", &db_1).unwrap();
        timeline_1.insert_dirs("b", &db_1).unwrap();
        let mut timeline_1_ops = Vec::new();

        let db_2 = NullStore::new(2);
        let mut timeline_2 = timeline_1.clone();
        let mut timeline_2_ops = Vec::new();

        timeline_1_ops.push(timeline_1.move_dir("a", "b/a", &db_1).unwrap().unwrap());
        timeline_2_ops.push(timeline_2.move_dir("b", "a/b", &db_1).unwrap().unwrap());
        while !timeline_1_ops.is_empty() || !timeline_2_ops.is_empty() {
            let ops_from_timeline_2_to_timeline_1 = timeline_2_ops.drain(..).collect::<Vec<_>>();
            let ops_from_timeline_1_to_timeline_2 = timeline_1_ops.drain(..).collect::<Vec<_>>();
            timeline_1_ops.extend(
                timeline_1
                    .integrate_ops::<FakeFileSystem<StdRng>, _, _>(
                        &ops_from_timeline_2_to_timeline_1,
                        None,
                        &db_1,
                    ).unwrap(),
            );
            timeline_2_ops.extend(
                timeline_2
                    .integrate_ops::<FakeFileSystem<StdRng>, _, _>(
                        &ops_from_timeline_1_to_timeline_2,
                        None,
                        &db_2,
                    ).unwrap(),
            );
        }

        assert_eq!(
            timeline_1.paths_with_ids(&db_1),
            timeline_2.paths_with_ids(&db_2)
        );
        assert_eq!(timeline_1.paths(&db_1), ["b/", "b/a/"]);
    }

    #[test]
    fn test_replication_random() {
        use std::iter::FromIterator;
        use std::mem;
        const PEERS: usize = 3;

        for seed in 0..100 {
            // let seed = 44373;
            println!("SEED: {:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let db = Vec::from_iter((0..PEERS).map(|i| NullStore::new(i as u64 + 1)));
            let mut fs = Vec::from_iter((0..PEERS).map(|i| FakeFileSystem::new(&db[i], rng)));
            let mut prev_fs_versions = Vec::from_iter((0..PEERS).map(|_| 0));
            let mut timelines = Vec::from_iter((0..PEERS).map(|_| Timeline::new()));
            let mut inboxes = Vec::from_iter((0..PEERS).map(|_| Vec::new()));

            // Generate and deliver random mutations
            for _ in 0..5 {
                let replica_index = rng.gen_range(0, PEERS);
                let db = &db[replica_index];
                let fs = &mut fs[replica_index];
                let timeline = &mut timelines[replica_index];

                if !inboxes[replica_index].is_empty() && rng.gen() {
                    let ops = mem::replace(&mut inboxes[replica_index], Vec::new());
                    let fixup_ops = timeline.integrate_ops(&ops, Some(fs), db).unwrap();
                    deliver_ops(replica_index, &mut inboxes, fixup_ops);
                } else {
                    if prev_fs_versions[replica_index] == fs.version() || rng.gen() {
                        fs.mutate(rng.gen_range(1, 5));
                    }

                    prev_fs_versions[replica_index] = fs.version();
                    let ops = timeline.read_from_fs(fs.entries(), db).unwrap();
                    deliver_ops(replica_index, &mut inboxes, ops);
                }
            }

            // Allow system to quiesce
            loop {
                let mut done = true;
                for replica_index in 0..PEERS {
                    let db = &db[replica_index];
                    let fs = &mut fs[replica_index];
                    let timeline = &mut timelines[replica_index];

                    if prev_fs_versions[replica_index] < fs.version() {
                        prev_fs_versions[replica_index] = fs.version();
                        let ops = timeline.read_from_fs(fs.entries(), db).unwrap();
                        deliver_ops(replica_index, &mut inboxes, ops);
                        done = false;
                    }

                    let ops = mem::replace(&mut inboxes[replica_index], Vec::new());
                    if !ops.is_empty() {
                        let fixup_ops = timeline.integrate_ops(&ops, Some(fs), db).unwrap();
                        deliver_ops(replica_index, &mut inboxes, fixup_ops);
                        done = false;
                    }
                }

                if done {
                    break;
                }
            }

            // Ensure all timelines have the same contents
            for i in 0..PEERS - 1 {
                assert_eq!(
                    timelines[i].paths_with_ids(&db[i]),
                    timelines[i + 1].paths_with_ids(&db[i + 1])
                );
            }

            // Ensure all timelines match their underlying file system
            for i in 0..PEERS {
                assert_eq!(timelines[i].paths(&db[i]), fs[i].paths());
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

    struct FakeFileSystem<'a, T: Rng + Clone>(Rc<RefCell<FakeFileSystemState<'a, T>>>);

    #[derive(Clone)]
    struct FakeFileSystemState<'a, T: Rng + Clone> {
        timeline: Timeline,
        next_inode: Inode,
        db: &'a NullStore,
        rng: T,
        version: usize,
    }

    struct FakeFileSystemIter<'a, T: Rng + Clone> {
        state: Rc<RefCell<FakeFileSystemState<'a, T>>>,
        version: usize,
        cursor: Option<Cursor>,
    }

    #[derive(Debug)]
    struct FakeFileSystemEntry {
        depth: usize,
        name: Arc<OsString>,
        inode: Inode,
    }

    struct NullStore {
        next_id: Cell<id::Unique>,
        lamport_clock: Cell<LamportTimestamp>,
    }

    impl<'a, T: Rng + Clone> FakeFileSystem<'a, T> {
        fn new(db: &'a NullStore, rng: T) -> Self {
            FakeFileSystem(Rc::new(RefCell::new(FakeFileSystemState::new(db, rng))))
        }

        fn mutate(&self, count: usize) {
            self.0.borrow_mut().mutate(count);
        }

        fn timeline(&self) -> Timeline {
            self.0.borrow().timeline.clone()
        }

        // fn paths(&self) -> Vec<String> {
        //     self.0.borrow().paths()
        // }

        fn version(&self) -> usize {
            self.0.borrow().version
        }
    }

    impl<'a, T: Rng + Clone> Clone for FakeFileSystem<'a, T> {
        fn clone(&self) -> Self {
            FakeFileSystem(Rc::new(RefCell::new(self.0.borrow().clone())))
        }
    }

    impl<'a, T: Rng + Clone> FileSystem for FakeFileSystem<'a, T> {
        type Entry = FakeFileSystemEntry;
        type EntriesIterator = FakeFileSystemIter<'a, T>;

        fn paths(&self) -> Vec<String> {
            self.0.borrow().paths()
        }

        fn insert_dir(&mut self, path: &Path) -> bool {
            self.0.borrow_mut().insert_dir(path)
        }

        fn remove_dir(&mut self, path: &Path) -> bool {
            self.0.borrow_mut().remove_dir(path)
        }

        fn move_dir(&mut self, from: &Path, to: &Path) -> bool {
            self.0.borrow_mut().move_dir(from, to)
        }

        fn inode_for_path(&self, path: &Path) -> Option<Inode> {
            self.0.borrow_mut().inode_for_path(path)
        }

        fn entries(&self) -> Self::EntriesIterator {
            FakeFileSystemIter {
                state: self.0.clone(),
                version: self.0.borrow().version,
                cursor: None,
            }
        }
    }

    impl<'a, T: Rng + Clone> FakeFileSystemState<'a, T> {
        fn new(db: &'a NullStore, rng: T) -> Self {
            let timeline = Timeline::new();
            Self {
                timeline,
                next_inode: 0,
                db,
                rng,
                version: 0,
            }
        }

        fn mutate(&mut self, count: usize) {
            self.version += 1;
            self.timeline.mutate(
                &mut self.rng,
                count,
                &mut Some(&mut self.next_inode),
                self.db,
            );
        }

        fn paths(&self) -> Vec<String> {
            self.timeline.paths(self.db)
        }

        fn insert_dir(&mut self, path: &Path) -> bool {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before insert_dir");
                self.mutate(1);
            } else {
                self.version += 1;
            }

            // println!("FileSystem: insert {:?}", path);
            let mut new_timeline = self.timeline.clone();
            let operations = new_timeline
                .insert_dirs_internal(path, &mut Some(&mut self.next_inode), self.db)
                .unwrap();

            if operations.len() == 1 {
                self.timeline = new_timeline;
                true
            } else {
                false
            }
        }

        fn remove_dir(&mut self, path: &Path) -> bool {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before remove_dir");
                self.mutate(1);
            } else {
                self.version += 1;
            }

            // println!("FileSystem: remove {:?}", path);
            if self.timeline.remove_dir(path, self.db).unwrap().is_some() {
                true
            } else {
                false
            }
        }

        fn move_dir(&mut self, from: &Path, to: &Path) -> bool {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before move_dir");
                self.mutate(1);
            } else {
                self.version += 1;
            }

            // println!("FileSystem: move from {:?} to {:?}", from, to);
            if to.starts_with(from) || self.timeline.move_dir(from, to, self.db).unwrap().is_none()
            {
                false
            } else {
                true
            }
        }

        fn inode_for_path(&mut self, path: &Path) -> Option<Inode> {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before inode_for_path");
                self.mutate(1);
            }

            self.timeline.id_for_path(path, self.db).unwrap().map(|id| {
                let mut cursor = self.timeline.metadata.cursor();
                cursor.seek(&id, SeekBias::Left, self.db).unwrap();
                cursor.item(self.db).unwrap().unwrap().inode.unwrap()
            })
        }
    }

    impl<'a, T: Rng + Clone> FakeFileSystemIter<'a, T> {
        fn build_cursor(&self) -> Cursor {
            let state = self.state.borrow();
            let mut new_cursor = state.timeline.cursor(state.db).unwrap();
            if let Some(old_cursor) = self.cursor.as_ref() {
                loop {
                    let advance = if let (Some(new_path), Some(old_path)) =
                        (new_cursor.path(), old_cursor.path())
                    {
                        new_path <= old_path
                    } else {
                        false
                    };

                    if advance {
                        new_cursor.next(state.db).unwrap();
                    } else {
                        break;
                    }
                }
            }
            new_cursor
        }
    }

    impl<'a, T: Rng + Clone> Iterator for FakeFileSystemIter<'a, T> {
        type Item = FakeFileSystemEntry;

        fn next(&mut self) -> Option<Self::Item> {
            {
                let mut state = self.state.borrow_mut();
                if state.rng.gen_weighted_bool(20) {
                    // println!("mutate while scanning entries");
                    state.mutate(1);
                }
            }

            let state = self.state.borrow();

            if self.cursor.is_none() || self.version < state.version {
                self.cursor = Some(self.build_cursor());
                self.version = state.version;
            } else {
                self.cursor.as_mut().unwrap().next(state.db).unwrap();
            }

            let cursor = self.cursor.as_mut().unwrap();

            let depth = cursor.depth();
            if depth == 0 {
                None
            } else {
                let name = cursor.name(state.db).unwrap().unwrap();
                let inode = cursor.metadata(state.db).unwrap().unwrap().inode.unwrap();
                Some(FakeFileSystemEntry { depth, name, inode })
            }
        }
    }

    impl FileSystemEntry for FakeFileSystemEntry {
        fn depth(&self) -> usize {
            self.depth
        }

        fn name(&self) -> &OsStr {
            self.name.as_ref()
        }

        fn inode(&self) -> Inode {
            self.inode
        }
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

    impl Timeline {
        fn mutate<S, T: Rng>(
            &mut self,
            rng: &mut T,
            count: usize,
            next_inode: &mut Option<&mut Inode>,
            db: &S,
        ) -> Vec<Operation>
        where
            S: Store,
        {
            let mut ops = Vec::new();
            for _ in 0..count {
                let k = rng.gen_range(0, 3);
                if self.is_empty(db).unwrap() || k == 0 {
                    let subtree_depth = rng.gen_range(1, 5);
                    let path = self.gen_path(rng, subtree_depth, db);
                    // println!("{:?} Inserting {:?}", db.replica_id(), path);
                    ops.extend(self.insert_dirs_internal(&path, next_inode, db).unwrap());
                } else if k == 1 {
                    let path = self.select_path(rng, db).unwrap();
                    // println!("{:?} Removing {:?}", db.replica_id(), path);
                    ops.push(self.remove_dir(&path, db).unwrap().unwrap());
                } else {
                    let (old_path, new_path) = loop {
                        let old_path = self.select_path(rng, db).unwrap();
                        let new_path = self.gen_path(rng, 1, db);
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
                    ops.push(self.move_dir(&old_path, &new_path, db).unwrap().unwrap());
                }
            }
            ops
        }

        fn gen_path<S: Store, T: Rng>(&self, rng: &mut T, depth: usize, db: &S) -> String {
            loop {
                let mut new_tree = String::new();
                for _ in 0..depth {
                    new_tree.push_str(&gen_name(rng));
                    new_tree.push('/');
                }

                let path = if self.is_empty(db).unwrap() || rng.gen_weighted_bool(8) {
                    new_tree
                } else {
                    format!("{}/{}", self.select_path(rng, db).unwrap(), new_tree)
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
    }

    fn gen_name<T: Rng>(rng: &mut T) -> String {
        let mut name = String::new();
        for _ in 0..rng.gen_range(1, 4) {
            name.push(rng.gen_range(b'a', b'z' + 1).into());
        }
        if rng.gen_weighted_bool(5) {
            for _ in 0..rng.gen_range(1, 2) {
                name.push('~');
            }
        }

        name
    }
}
