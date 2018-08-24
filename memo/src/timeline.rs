use btree::{self, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::{self, Ordering};
// TODO: Replace BTree-based collections with Hash-based collections.
// We're using the B-tree versions to enforce deterministic ordering behavior during development.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::iter::FromIterator;
use std::ops::{Add, AddAssign};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait Store {
    type ReadError: fmt::Debug;
    type MetadataStore: btree::NodeStore<Metadata, ReadError = Self::ReadError>;
    type ParentRefStore: btree::NodeStore<ParentRefValue, ReadError = Self::ReadError>;
    type ChildRefValueStore: btree::NodeStore<ChildRefValue, ReadError = Self::ReadError>;

    fn replica_id(&self) -> id::ReplicaId;

    fn metadata_store(&self) -> &Self::MetadataStore;
    fn parent_ref_store(&self) -> &Self::ParentRefStore;
    fn child_ref_store(&self) -> &Self::ChildRefValueStore;

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
    fn is_dir(&self) -> bool;
}

type Inode = u64;
type VisibleCount = usize;

const ROOT_ID: id::Unique = id::Unique::DEFAULT;

#[derive(Clone)]
pub struct Timeline {
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRefValue>,
    child_refs: btree::Tree<ChildRefValue>,
    inodes_to_file_ids: BTreeMap<Inode, id::Unique>,
}

#[derive(Clone)]
pub struct Cursor {
    path: PathBuf,
    stack: Vec<btree::Cursor<ChildRefValue>>,
    metadata_cursor: btree::Cursor<Metadata>,
}

#[derive(Clone, Debug)]
pub enum Operation {
    InsertMetadata {
        op_id: id::Unique,
        is_dir: bool,
    },
    UpdateParent {
        op_id: id::Unique,
        ref_id: ParentRefId,
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
pub struct ParentRefValue {
    ref_id: ParentRefId,
    timestamp: LamportTimestamp,
    prev_timestamp: LamportTimestamp,
    op_id: id::Unique,
    parent: Option<(id::Unique, Arc<OsString>)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct ParentRefId {
    child_id: id::Unique,
    alias_id: id::Unique,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParentRefValueId {
    ref_id: ParentRefId,
    timestamp: LamportTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildRefValue {
    parent_id: id::Unique,
    name: Arc<OsString>,
    timestamp: LamportTimestamp,
    op_id: id::Unique,
    parent_ref_id: ParentRefId,
    deletions: SmallVec<[id::Unique; 1]>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefValueSummary {
    parent_id: id::Unique,
    name: Arc<OsString>,
    visible: bool,
    timestamp: LamportTimestamp,
    visible_count: VisibleCount,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefValueId {
    parent_id: id::Unique,
    name: Arc<OsString>,
    visible: bool,
    timestamp: LamportTimestamp,
}

#[derive(Clone, Debug, Default, Ord, Eq, PartialEq, PartialOrd)]
pub struct ChildRefId {
    parent_id: id::Unique,
    name: Arc<OsString>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct LamportTimestamp {
    value: u64,
    replica_id: id::ReplicaId,
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
        };
        cursor.descend_into(self.child_refs.cursor(), id, db)?;
        Ok(cursor)
    }

    fn write_to_fs<F, S>(
        &mut self,
        mut refs_to_write: BTreeSet<ParentRefId>,
        mut old_tree: Timeline,
        fs: &mut F,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError>
    where
        F: FileSystem,
        S: Store,
    {
        let mut fixup_ops = Vec::new();
        let mut refs_with_temp_name = BTreeSet::new();

        loop {
            let mut sorted_refs_to_write = self
                .sort_refs_by_path_depth(refs_to_write.iter().cloned(), db)?
                .into_iter()
                .peekable();

            while let Some((ref_id, depth)) = sorted_refs_to_write.peek().cloned() {
                let old_path = old_tree.resolve_path(ref_id, db)?;
                if let Some(old_path) = old_path.as_ref() {
                    if fs.inode_for_path(old_path) != old_tree.inode_for_id(ref_id.child_id, db)? {
                        break;
                    }
                }

                let parent_ref = self.cur_parent_ref_value(ref_id, db)?.unwrap();
                if depth.is_some() {
                    let (parent_id, mut name) = parent_ref.parent.clone().unwrap();

                    if let Some(parent_path) = old_tree.resolve_paths(parent_id, db)?.first_mut() {
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
                            .map_or(false, |old_path| new_path == old_path)
                        {
                            refs_to_write.remove(&ref_id);
                            sorted_refs_to_write.next();
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
                                    refs_with_temp_name.insert(ref_id);
                                }

                                let prev_parent_ref =
                                    old_tree.cur_parent_ref_value(ref_id, db)?.unwrap();
                                old_tree.integrate_op(
                                    Operation::UpdateParent {
                                        op_id: parent_ref.op_id,
                                        ref_id,
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
                                    refs_with_temp_name.insert(ref_id);
                                }

                                if let Some(inode) = fs.inode_for_path(&new_path) {
                                    if let Some(prev_parent_ref) =
                                        old_tree.cur_parent_ref_value(ref_id, db)?
                                    {
                                        old_tree.integrate_op(
                                            Operation::UpdateParent {
                                                op_id: parent_ref.op_id,
                                                ref_id,
                                                timestamp: parent_ref.timestamp,
                                                prev_timestamp: prev_parent_ref.timestamp,
                                                new_parent: Some((parent_id, name)),
                                            },
                                            db,
                                        )?;
                                    } else {
                                        old_tree.integrate_op(
                                            Operation::InsertMetadata {
                                                op_id: ref_id.child_id,
                                                is_dir: true,
                                            },
                                            db,
                                        )?;
                                        old_tree.integrate_op(
                                            Operation::UpdateParent {
                                                op_id: parent_ref.op_id,
                                                ref_id,
                                                timestamp: parent_ref.timestamp,
                                                prev_timestamp: parent_ref.timestamp,
                                                new_parent: Some((parent_id, name)),
                                            },
                                            db,
                                        )?;
                                    };

                                    old_tree.set_inode_for_id(ref_id.child_id, inode, db)?;
                                    self.set_inode_for_id(ref_id.child_id, inode, db)?;
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
                        let prev_parent_ref = old_tree.cur_parent_ref_value(ref_id, db)?.unwrap();
                        old_tree.integrate_op(
                            Operation::UpdateParent {
                                op_id: parent_ref.op_id,
                                ref_id,
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

                refs_to_write.remove(&ref_id);
                sorted_refs_to_write.next();
            }

            if sorted_refs_to_write.peek().is_some() {
                // println!("refreshing old timeline - paths: {:?}", fs.paths());
                let fs_ops = old_tree.read_from_fs(fs.entries(), db)?;
                let fs_fixup_ops = self.integrate_ops::<F, _, _>(&fs_ops, None, db)?;
                for op in &fs_ops {
                    match op {
                        Operation::InsertMetadata { op_id, .. } => {
                            let inode = old_tree.inode_for_id(*op_id, db)?.unwrap();
                            self.set_inode_for_id(*op_id, inode, db)?;
                        }
                        Operation::UpdateParent { ref_id, .. } => {
                            refs_with_temp_name.remove(&ref_id);
                        }
                    }
                }
                for op in &fs_fixup_ops {
                    match op {
                        Operation::UpdateParent { ref_id, .. } => {
                            refs_to_write.insert(ref_id.clone());
                            refs_with_temp_name.remove(ref_id);
                        }
                        _ => {}
                    }
                }
                fixup_ops.extend(fs_ops);
                fixup_ops.extend(fs_fixup_ops);
            } else {
                break;
            }
        }

        for ref_id in refs_with_temp_name {
            if let Some(old_path) = old_tree.resolve_path(ref_id, db)? {
                let mut new_path = old_path.clone();
                new_path.set_file_name(self.resolve_name(ref_id, db)?.unwrap().as_os_str());

                if new_path != old_path {
                    let fs_inode = fs.inode_for_path(&old_path);
                    let tree_inode = old_tree.inode_for_id(ref_id.child_id, db)?;
                    if fs_inode == tree_inode && fs.move_dir(&old_path, &new_path) {
                        old_tree.rename(&old_path, &new_path, db)?;
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
        struct Change<F: FileSystemEntry> {
            inserted: bool,
            entry: F,
            parents: SmallVec<[(id::Unique, Arc<OsString>); 1]>,
        }

        let mut dir_stack = vec![ROOT_ID];
        let mut visited_dir_ids = BTreeSet::from_iter(Some(ROOT_ID));
        let mut occupied_ref_ids = BTreeSet::new();
        let mut changes: BTreeMap<id::Unique, Change<F>> = BTreeMap::new();

        for entry in entries {
            assert!(entry.depth() > 0);
            dir_stack.truncate(entry.depth());

            let cur_parent = Some((*dir_stack.last().unwrap(), Arc::new(entry.name().into())));

            if let Some(file_id) = self.inodes_to_file_ids.get(&entry.inode()).cloned() {
                if entry.is_dir() {
                    dir_stack.push(file_id);

                    if let Some(parent_ref) = self.cur_parent_ref_values(file_id, db)?.pop() {
                        visited_dir_ids.insert(file_id);

                        if parent_ref.parent == cur_parent {
                            occupied_ref_ids.insert(parent_ref.ref_id);
                            changes.remove(&file_id);
                        } else {
                            occupied_ref_ids.remove(&parent_ref.ref_id);
                            changes.insert(
                                file_id,
                                Change {
                                    inserted: false,
                                    entry,
                                    parents: SmallVec::from_iter(cur_parent),
                                },
                            );
                        }
                    } else {
                        changes.insert(
                            file_id,
                            Change {
                                inserted: true,
                                entry,
                                parents: SmallVec::from_iter(cur_parent),
                            },
                        );
                    }
                } else {
                    if let Some(parent_ref) = self
                        .cur_parent_ref_values(file_id, db)?
                        .into_iter()
                        .find(|r| r.parent == cur_parent)
                    {
                        occupied_ref_ids.insert(parent_ref.ref_id);
                    } else {
                        changes
                            .entry(file_id)
                            .or_insert_with(|| Change {
                                inserted: false,
                                entry,
                                parents: SmallVec::new(),
                            }).parents
                            .extend(cur_parent);
                    }
                }
            } else {
                let file_id = db.gen_id();
                self.inodes_to_file_ids.insert(entry.inode(), file_id);
                if entry.is_dir() {
                    dir_stack.push(file_id);
                }
                changes.insert(
                    file_id,
                    Change {
                        inserted: true,
                        entry,
                        parents: SmallVec::from_iter(cur_parent),
                    },
                );
            }
        }

        let mut operations = Vec::new();
        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        // Insert new metadata entries and update refs based on moves and hard-links.
        for (child_id, change) in changes {
            if change.inserted {
                let entry = &change.entry;
                operations.push(self.insert_metadata(
                    child_id,
                    entry.is_dir(),
                    Some(entry.inode()),
                    &mut metadata_edits,
                ));
            }

            for parent in change.parents {
                let available_ref_id = self
                    .cur_parent_ref_values(child_id, db)?
                    .into_iter()
                    .map(|parent_ref| parent_ref.ref_id)
                    .filter(|ref_id| !occupied_ref_ids.contains(&ref_id))
                    .next();
                let alias_id = if let Some(ref_id) = available_ref_id {
                    occupied_ref_ids.insert(ref_id);
                    ref_id.alias_id
                } else {
                    db.gen_id()
                };

                operations.push(self.update_parent_ref(
                    ParentRefId { child_id, alias_id },
                    Some(parent),
                    &mut parent_ref_edits,
                    &mut child_ref_edits,
                    db,
                )?);
            }
        }

        // Delete all file refs that are not reachable anymore from the visited directories.
        for dir_id in visited_dir_ids {
            let mut cursor = self.cursor_at(dir_id, db)?;
            while let Some(ref_id) = cursor.ref_id(db)? {
                if !occupied_ref_ids.contains(&ref_id) {
                    operations.push(self.update_parent_ref(
                        ref_id,
                        None,
                        &mut parent_ref_edits,
                        &mut child_ref_edits,
                        db,
                    )?);
                }
                cursor.next_sibling_or_cousin(db)?;
            }
        }

        self.metadata.edit(metadata_edits, db.metadata_store())?;
        self.parent_refs
            .edit(parent_ref_edits, db.parent_ref_store())?;
        self.child_refs
            .edit(child_ref_edits, db.child_ref_store())?;

        Ok(operations)
    }

    fn insert_metadata(
        &self,
        file_id: id::Unique,
        is_dir: bool,
        inode: Option<Inode>,
        metadata_edits: &mut Vec<btree::Edit<Metadata>>,
    ) -> Operation {
        metadata_edits.push(btree::Edit::Insert(Metadata {
            file_id,
            is_dir,
            inode,
        }));
        Operation::InsertMetadata {
            op_id: file_id,
            is_dir,
        }
    }

    fn update_parent_ref<S: Store>(
        &self,
        ref_id: ParentRefId,
        new_parent: Option<(id::Unique, Arc<OsString>)>,
        parent_ref_edits: &mut Vec<btree::Edit<ParentRefValue>>,
        child_ref_edits: &mut Vec<btree::Edit<ChildRefValue>>,
        db: &S,
    ) -> Result<Operation, S::ReadError> {
        let timestamp = db.gen_timestamp();
        let op_id = db.gen_id();

        let prev_parent_ref = self.cur_parent_ref_value(ref_id, db)?;
        let prev_timestamp = prev_parent_ref.as_ref().map_or(timestamp, |r| r.timestamp);

        parent_ref_edits.push(btree::Edit::Insert(ParentRefValue {
            ref_id,
            timestamp,
            prev_timestamp,
            op_id,
            parent: new_parent.clone(),
        }));

        if let Some(prev_child_ref_value_id) =
            prev_parent_ref.and_then(|r| r.to_child_ref_value_id(true))
        {
            let mut prev_child_ref = self.child_ref_value(prev_child_ref_value_id, db)?.unwrap();
            child_ref_edits.push(btree::Edit::Remove(prev_child_ref.clone()));
            prev_child_ref.deletions.push(op_id);
            child_ref_edits.push(btree::Edit::Insert(prev_child_ref));
        }

        if let Some((new_parent_id, new_name)) = new_parent.as_ref() {
            child_ref_edits.push(btree::Edit::Insert(ChildRefValue {
                parent_id: *new_parent_id,
                name: new_name.clone(),
                timestamp,
                op_id,
                parent_ref_id: ref_id,
                deletions: SmallVec::new(),
            }));
        }

        Ok(Operation::UpdateParent {
            op_id,
            ref_id,
            timestamp,
            prev_timestamp,
            new_parent,
        })
    }

    pub fn insert_dirs<I, S>(&mut self, path: I, db: &S) -> Result<Vec<Operation>, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        self.insert_dirs_internal(path, &mut None, db)
    }

    // TODO: Return an error if there is a name conflict
    pub fn insert_file_internal<I, S>(
        &mut self,
        path: I,
        inode: Option<Inode>,
        db: &S,
    ) -> Result<SmallVec<[Operation; 2]>, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();
        let metadata_db = db.metadata_store();
        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        assert!(self.id_for_path(&path, db)?.is_none());
        let parent_id = if let Some(parent_path) = path.parent() {
            self.id_for_path(parent_path, db)?.unwrap()
        } else {
            ROOT_ID
        };
        let name = Arc::new(path.file_name().unwrap().into());
        let operations = self.local_insert(
            parent_id,
            name,
            ParentRefId {
                child_id: db.gen_id(),
                alias_id: db.gen_id(),
            },
            inode,
            false,
            &mut metadata_edits,
            &mut parent_ref_edits,
            &mut child_ref_edits,
            db,
        )?;
        if let Some(inode) = inode {
            self.inodes_to_file_ids.insert(inode, parent_id);
        }

        self.metadata.edit(metadata_edits, metadata_db)?;
        self.parent_refs.edit(parent_ref_edits, parent_ref_db)?;
        self.child_refs.edit(child_ref_edits, child_ref_db)?;
        Ok(operations)
    }

    pub fn insert_hard_link_internal<I1, I2, S>(
        &mut self,
        file_path: I1,
        link_path: I2,
        db: &S,
    ) -> Result<SmallVec<[Operation; 2]>, S::ReadError>
    where
        I1: Into<PathBuf>,
        I2: Into<PathBuf>,
        S: Store,
    {
        let file_path = file_path.into();
        let link_path = link_path.into();

        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        assert!(self.id_for_path(&link_path, db)?.is_none());
        let parent_id = if let Some(parent_path) = link_path.parent() {
            self.id_for_path(parent_path, db)?.unwrap()
        } else {
            ROOT_ID
        };
        let child_id = self.id_for_path(file_path, db).unwrap().unwrap();
        let name = Arc::new(link_path.file_name().unwrap().into());
        let operations = self.local_insert(
            parent_id,
            name,
            ParentRefId {
                child_id,
                alias_id: db.gen_id(),
            },
            self.inode_for_id(child_id, db).unwrap(),
            false,
            &mut vec![],
            &mut parent_ref_edits,
            &mut child_ref_edits,
            db,
        )?;

        self.parent_refs.edit(parent_ref_edits, parent_ref_db)?;
        self.child_refs.edit(child_ref_edits, child_ref_db)?;
        Ok(operations)
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
        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        let mut cursor = self.child_refs.cursor();
        let mut parent_id = ROOT_ID;
        let mut entry_exists = true;

        for name in path.components() {
            let name = Arc::new(OsString::from(name.as_os_str()));
            if entry_exists {
                let key = ChildRefId {
                    parent_id,
                    name: name.clone(),
                };
                if cursor.seek(&key, SeekBias::Left, child_ref_db)? {
                    let child_ref = cursor.item(child_ref_db)?.unwrap();
                    if child_ref.is_visible() {
                        parent_id = child_ref.parent_ref_id.child_id;
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
                operations.extend(self.local_insert(
                    parent_id,
                    name,
                    ParentRefId {
                        child_id,
                        alias_id: db.gen_id(),
                    },
                    inode,
                    true,
                    &mut metadata_edits,
                    &mut parent_ref_edits,
                    &mut child_ref_edits,
                    db,
                )?);
                if let Some(inode) = inode {
                    self.inodes_to_file_ids.insert(inode, child_id);
                }
                parent_id = child_id;
            }
        }

        self.metadata.edit(metadata_edits, metadata_db)?;
        self.parent_refs.edit(parent_ref_edits, parent_ref_db)?;
        self.child_refs.edit(child_ref_edits, child_ref_db)?;
        Ok(operations)
    }

    pub fn remove<I, S>(&mut self, path: I, db: &S) -> Result<Option<Operation>, S::ReadError>
    where
        I: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();
        let parent_ref_db = db.parent_ref_store();
        let child_ref_db = db.child_ref_store();

        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        if let Some(ref_id) = self.ref_id_for_path(&path, db)? {
            let operation = self.update_parent_ref(
                ref_id,
                None,
                &mut parent_ref_edits,
                &mut child_ref_edits,
                db,
            )?;
            self.parent_refs.edit(parent_ref_edits, parent_ref_db)?;
            self.child_refs.edit(child_ref_edits, child_ref_db)?;
            Ok(Some(operation))
        } else {
            Ok(None)
        }
    }

    pub fn rename<F, S, T>(
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

        let ref_id = self.ref_id_for_path(&from, db)?;
        let new_parent_id = if let Some(parent_path) = to.parent() {
            self.id_for_path(parent_path, db)?
        } else {
            Some(ROOT_ID)
        };

        if let (Some(ref_id), Some(new_parent_id)) = (ref_id, new_parent_id) {
            let mut parent_ref_edits = Vec::new();
            let mut child_ref_edits = Vec::new();

            let new_name = Arc::new(OsString::from(to.file_name().unwrap()));
            let operation = self.update_parent_ref(
                ref_id,
                Some((new_parent_id, new_name)),
                &mut parent_ref_edits,
                &mut child_ref_edits,
                db,
            )?;

            self.parent_refs.edit(parent_ref_edits, parent_ref_db)?;
            self.child_refs.edit(child_ref_edits, child_ref_db)?;

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

        let mut changed_refs = BTreeMap::new();
        for op in ops.clone() {
            // println!("integrate op {:#?}", op);

            match op {
                Operation::UpdateParent {
                    ref_id,
                    timestamp,
                    prev_timestamp,
                    ..
                } => {
                    let moved_dir = timestamp != prev_timestamp
                        && self.metadata(ref_id.child_id, db)?.unwrap().is_dir;
                    changed_refs.insert(*ref_id, moved_dir);
                }
                _ => {}
            }
            self.integrate_op(op.clone(), db)?;
        }

        let mut fixup_ops = Vec::new();
        for (ref_id, moved_dir) in &changed_refs {
            fixup_ops.extend(self.fix_conflicts(*ref_id, *moved_dir, db)?);
        }

        if let Some(fs) = fs {
            let mut refs_to_write = BTreeSet::new();
            for (ref_id, moved_dir) in changed_refs {
                refs_to_write.insert(ref_id);
                if moved_dir && old_tree.resolve_depth(ref_id, db)?.is_none() {
                    let mut cursor = self.cursor_at(ref_id.child_id, db)?;
                    while let Some(descendant_ref_id) = cursor.ref_id(db)? {
                        // println!(
                        //     "resurrecting descendant {:?} {:?}",
                        //     descendant_id,
                        //     cursor.path()
                        // );
                        refs_to_write.insert(descendant_ref_id);
                        cursor.next(db)?;
                    }
                }
            }
            for op in &fixup_ops {
                match op {
                    Operation::UpdateParent { ref_id, .. } => {
                        refs_to_write.insert(*ref_id);
                    }
                    _ => {}
                }
            }

            fixup_ops.extend(self.write_to_fs(refs_to_write, old_tree, fs, db)?);
        }

        // println!("integrate ops <<<<<<<<<<<<");

        Ok(fixup_ops)
    }

    fn sort_refs_by_path_depth<I, S>(
        &self,
        ref_ids: I,
        db: &S,
    ) -> Result<Vec<(ParentRefId, Option<usize>)>, S::ReadError>
    where
        I: Iterator<Item = ParentRefId>,
        S: Store,
    {
        let mut ref_ids_to_depths = BTreeMap::new();
        for ref_id in ref_ids {
            ref_ids_to_depths.insert(ref_id, self.resolve_depth(ref_id, db)?);
        }

        let mut sorted_ref_ids = ref_ids_to_depths.into_iter().collect::<Vec<_>>();
        sorted_ref_ids.sort_by(|(_, depth_a), (_, depth_b)| {
            if depth_a.is_none() {
                Ordering::Greater
            } else if depth_b.is_none() {
                Ordering::Less
            } else {
                depth_a.cmp(&depth_b)
            }
        });

        Ok(sorted_ref_ids)
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

        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();
        let mut new_child_ref;

        // println!("{:?} – integrate op {:?}", db.replica_id(), op);

        match op {
            Operation::InsertMetadata { op_id, is_dir } => {
                metadata_edits.push(btree::Edit::Insert(Metadata {
                    file_id: op_id,
                    is_dir,
                    inode: None,
                }));
            }
            Operation::UpdateParent {
                op_id,
                ref_id,
                timestamp,
                prev_timestamp,
                new_parent,
            } => {
                new_child_ref = new_parent.as_ref().map(|(parent_id, name)| ChildRefValue {
                    parent_id: *parent_id,
                    name: name.clone(),
                    timestamp,
                    op_id,
                    parent_ref_id: ref_id,
                    deletions: SmallVec::new(),
                });

                let mut child_ref_cursor = self.child_refs.cursor();
                let mut parent_ref_cursor = self.parent_refs.cursor();
                parent_ref_cursor.seek(&ref_id, SeekBias::Left, parent_ref_db)?;
                let mut is_latest_parent_ref = true;

                while let Some(parent_ref) = parent_ref_cursor.item(parent_ref_db)? {
                    if parent_ref.ref_id != ref_id {
                        break;
                    } else if parent_ref.timestamp > timestamp {
                        if parent_ref.prev_timestamp < timestamp && new_child_ref.is_some() {
                            let new_child_ref = new_child_ref.as_mut().unwrap();
                            new_child_ref.deletions.push(parent_ref.op_id);
                        }
                    } else if parent_ref.timestamp >= prev_timestamp {
                        if let Some(mut child_ref_value_id) =
                            parent_ref.to_child_ref_value_id(is_latest_parent_ref)
                        {
                            child_ref_cursor.seek(
                                &child_ref_value_id,
                                SeekBias::Left,
                                child_ref_db,
                            )?;
                            let mut child_ref = child_ref_cursor.item(child_ref_db)?.unwrap();
                            if child_ref.is_visible() {
                                child_ref_edits.push(btree::Edit::Remove(child_ref.clone()));
                            }
                            child_ref.deletions.push(op_id);
                            child_ref_edits.push(btree::Edit::Insert(child_ref));
                        }
                    } else {
                        break;
                    }
                    parent_ref_cursor.next(parent_ref_db)?;
                    is_latest_parent_ref = false;
                }

                parent_ref_edits.push(btree::Edit::Insert(ParentRefValue {
                    ref_id,
                    timestamp,
                    prev_timestamp,
                    op_id,
                    parent: new_parent,
                }));
                if let Some(new_child_ref) = new_child_ref {
                    child_ref_edits.push(btree::Edit::Insert(new_child_ref.clone()));
                }

                if db.replica_id() != timestamp.replica_id {
                    db.recv_timestamp(timestamp);
                }
            }
        }

        self.child_refs.edit(child_ref_edits, child_ref_db)?;
        self.metadata.edit(metadata_edits, metadata_db)?;
        self.parent_refs.edit(parent_ref_edits, parent_ref_db)?;
        Ok(())
    }

    fn fix_conflicts<S: Store>(
        &mut self,
        ref_id: ParentRefId,
        moved_dir: bool,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError> {
        use btree::KeyedItem;

        let mut fixup_ops = Vec::new();
        let mut reverted_moves: BTreeMap<ParentRefId, LamportTimestamp> = BTreeMap::new();

        // If the child was moved and is a directory, check for cycles.
        if moved_dir {
            let parent_ref_db = db.parent_ref_store();
            let mut visited = BTreeSet::new();
            let mut latest_move: Option<ParentRefValue> = None;
            let mut cursor = self.parent_refs.cursor();
            cursor.seek(&ref_id, SeekBias::Left, parent_ref_db)?;

            loop {
                let mut parent_ref = cursor.item(parent_ref_db)?.unwrap();
                if visited.contains(&parent_ref.ref_id.child_id) {
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
                            reverted_moves.insert(parent_ref.ref_id, parent_ref.timestamp);
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
                    visited.insert(parent_ref.ref_id.child_id);

                    // If we have already reverted this parent ref to a previous value, interpret
                    // it as having the value we reverted to.
                    if let Some(prev_timestamp) = reverted_moves.get(&parent_ref.ref_id) {
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
                                next_parent_ref.ref_id == parent_ref.ref_id
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
            let mut moved_ref_ids = Vec::new();
            for (ref_id, timestamp) in &reverted_moves {
                cursor.seek(ref_id, SeekBias::Left, parent_ref_db)?;
                let prev_timestamp = cursor.item(parent_ref_db)?.unwrap().timestamp;
                cursor.seek_forward(
                    &ParentRefValueId {
                        ref_id: *ref_id,
                        timestamp: *timestamp,
                    },
                    SeekBias::Left,
                    parent_ref_db,
                )?;
                let new_parent = cursor.item(parent_ref_db)?.unwrap().parent;
                fixup_ops.push(Operation::UpdateParent {
                    op_id: db.gen_id(),
                    ref_id: *ref_id,
                    timestamp: db.gen_timestamp(),
                    prev_timestamp,
                    new_parent,
                });
                moved_ref_ids.push(*ref_id);
            }

            for op in &fixup_ops {
                self.integrate_op(op.clone(), db)?;
            }
            for ref_id in moved_ref_ids {
                fixup_ops.extend(self.fix_name_conflicts(ref_id, db)?);
            }
        }

        if !reverted_moves.contains_key(&ref_id) {
            fixup_ops.extend(self.fix_name_conflicts(ref_id, db)?);
        }

        Ok(fixup_ops)
    }

    fn fix_name_conflicts<S: Store>(
        &mut self,
        ref_id: ParentRefId,
        db: &S,
    ) -> Result<Vec<Operation>, S::ReadError> {
        let child_ref_db = db.child_ref_store();

        let mut fixup_ops = Vec::new();

        let parent_ref = self.cur_parent_ref_value(ref_id, db)?.unwrap();
        if let Some((parent_id, name)) = parent_ref.parent {
            let mut cursor_1 = self.child_refs.cursor();
            cursor_1.seek(
                &ChildRefId {
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
                            &ChildRefId {
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

                    let fixup_op = Operation::UpdateParent {
                        op_id: db.gen_id(),
                        ref_id: child_ref.parent_ref_id,
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
        Ok(self
            .ref_id_for_path(path, db)?
            .map(|ref_id| ref_id.child_id))
    }

    fn ref_id_for_path<P, S>(&self, path: P, db: &S) -> Result<Option<ParentRefId>, S::ReadError>
    where
        P: Into<PathBuf>,
        S: Store,
    {
        let path = path.into();
        let child_ref_db = db.child_ref_store();

        let mut cursor = self.child_refs.cursor();
        let mut ref_id = ParentRefId {
            child_id: ROOT_ID,
            alias_id: ROOT_ID,
        };
        for name in path.components() {
            let name = Arc::new(OsString::from(name.as_os_str()));
            let key = ChildRefId {
                parent_id: ref_id.child_id,
                name,
            };
            if cursor.seek(&key, SeekBias::Left, child_ref_db)? {
                let child_ref = cursor.item(child_ref_db)?.unwrap();
                if child_ref.is_visible() {
                    ref_id = child_ref.parent_ref_id;
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }

        Ok(Some(ref_id))
    }

    fn local_insert<S: Store>(
        &self,
        parent_id: id::Unique,
        name: Arc<OsString>,
        ref_id: ParentRefId,
        inode: Option<Inode>,
        is_dir: bool,
        metadata_edits: &mut Vec<btree::Edit<Metadata>>,
        parent_ref_edits: &mut Vec<btree::Edit<ParentRefValue>>,
        child_ref_edits: &mut Vec<btree::Edit<ChildRefValue>>,
        db: &S,
    ) -> Result<SmallVec<[Operation; 2]>, S::ReadError> {
        let mut operations = SmallVec::new();
        operations.push(self.insert_metadata(ref_id.child_id, is_dir, inode, metadata_edits));
        operations.push(self.update_parent_ref(
            ref_id,
            Some((parent_id, name.clone())),
            parent_ref_edits,
            child_ref_edits,
            db,
        )?);
        Ok(operations)
    }

    pub fn resolve_paths<S>(
        &self,
        file_id: id::Unique,
        db: &S,
    ) -> Result<SmallVec<[PathBuf; 1]>, S::ReadError>
    where
        S: Store,
    {
        let mut paths = SmallVec::new();

        if file_id == ROOT_ID {
            paths.push(PathBuf::new());
        } else {
            for parent_ref in self.cur_parent_ref_values(file_id, db)? {
                paths.extend(self.resolve_path(parent_ref.ref_id, db)?);
            }
        }
        Ok(paths)
    }

    fn resolve_path<S>(&self, ref_id: ParentRefId, db: &S) -> Result<Option<PathBuf>, S::ReadError>
    where
        S: Store,
    {
        let mut path_components = Vec::new();
        if self.visit_ancestors(ref_id, |name| path_components.push(name), db)? {
            let mut path = PathBuf::new();
            for component in path_components.into_iter().rev() {
                path.push(component.as_ref());
            }
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    fn resolve_depth<S>(&self, ref_id: ParentRefId, db: &S) -> Result<Option<usize>, S::ReadError>
    where
        S: Store,
    {
        let mut depth = 0;
        if self.visit_ancestors(ref_id, |_| depth += 1, db)? {
            Ok(Some(depth))
        } else {
            Ok(None)
        }
    }

    fn visit_ancestors<F, S>(
        &self,
        ref_id: ParentRefId,
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
        if ref_id.child_id == ROOT_ID {
            Ok(true)
        } else if cursor.seek(&ref_id, SeekBias::Left, parent_ref_db)? {
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

    fn resolve_name<S: Store>(
        &self,
        ref_id: ParentRefId,
        db: &S,
    ) -> Result<Option<Arc<OsString>>, S::ReadError> {
        Ok(self
            .cur_parent_ref_value(ref_id, db)?
            .and_then(|parent_ref| parent_ref.parent)
            .map(|(_, name)| name))
    }

    fn inode_for_id<S: Store>(
        &self,
        child_id: id::Unique,
        db: &S,
    ) -> Result<Option<Inode>, S::ReadError> {
        Ok(self
            .metadata(child_id, db)?
            .and_then(|metadata| metadata.inode))
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
            self.metadata
                .edit(vec![btree::Edit::Insert(metadata)], metadata_db)?;
            self.inodes_to_file_ids.insert(inode, child_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn metadata<S>(&self, child_id: id::Unique, db: &S) -> Result<Option<Metadata>, S::ReadError>
    where
        S: Store,
    {
        let metadata_db = db.metadata_store();
        let mut cursor = self.metadata.cursor();
        if cursor.seek(&child_id, SeekBias::Left, metadata_db)? {
            Ok(cursor.item(metadata_db)?)
        } else {
            Ok(None)
        }
    }

    fn cur_parent_ref_values<S: Store>(
        &self,
        child_id: id::Unique,
        db: &S,
    ) -> Result<Vec<ParentRefValue>, S::ReadError> {
        let parent_ref_db = db.parent_ref_store();
        let mut cursor = self.parent_refs.cursor();
        cursor.seek(&child_id, SeekBias::Left, parent_ref_db)?;
        let mut parent_ref_values = Vec::new();
        while let Some(parent_ref) = cursor.item(parent_ref_db)? {
            if parent_ref.ref_id.child_id == child_id {
                cursor.seek(&parent_ref.ref_id, SeekBias::Right, parent_ref_db)?;
                parent_ref_values.push(parent_ref);
            } else {
                break;
            }
        }
        Ok(parent_ref_values)
    }

    fn cur_parent_ref_value<S: Store>(
        &self,
        ref_id: ParentRefId,
        db: &S,
    ) -> Result<Option<ParentRefValue>, S::ReadError> {
        let parent_ref_db = db.parent_ref_store();
        let mut cursor = self.parent_refs.cursor();
        if cursor.seek(&ref_id, SeekBias::Left, parent_ref_db)? {
            cursor.item(parent_ref_db)
        } else {
            Ok(None)
        }
    }

    fn child_ref_value<S>(
        &self,
        ref_id: ChildRefValueId,
        db: &S,
    ) -> Result<Option<ChildRefValue>, S::ReadError>
    where
        S: Store,
    {
        let child_ref_db = db.child_ref_store();
        let mut cursor = self.child_refs.cursor();
        if cursor.seek(&ref_id, SeekBias::Left, child_ref_db)? {
            cursor.item(child_ref_db)
        } else {
            Ok(None)
        }
    }
}

impl Cursor {
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
        if self.stack.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                self.stack
                    .last()
                    .unwrap()
                    .item(db.child_ref_store())?
                    .unwrap()
                    .name
                    .clone(),
            ))
        }
    }

    pub fn is_dir<S: Store>(&self, db: &S) -> Result<Option<bool>, S::ReadError> {
        Ok(self.metadata(db)?.map(|metadata| metadata.is_dir))
    }

    pub fn file_id<S: Store>(&self, db: &S) -> Result<Option<id::Unique>, S::ReadError> {
        Ok(self.metadata(db)?.map(|metadata| metadata.file_id))
    }

    pub fn ref_id<S: Store>(&self, db: &S) -> Result<Option<ParentRefId>, S::ReadError> {
        if self.stack.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                self.stack
                    .last()
                    .unwrap()
                    .item(db.child_ref_store())?
                    .unwrap()
                    .parent_ref_id,
            ))
        }
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
        while !self.stack.is_empty() && !self.next_sibling(db)? {
            self.path.pop();
            self.stack.pop();
        }
        Ok(())
    }

    fn descend<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        let cursor = self.stack.last().unwrap().clone();
        let dir_id = cursor
            .item(db.child_ref_store())?
            .unwrap()
            .parent_ref_id
            .child_id;
        self.descend_into(cursor, dir_id, db)
    }

    fn descend_into<S: Store>(
        &mut self,
        mut child_ref_cursor: btree::Cursor<ChildRefValue>,
        dir_id: id::Unique,
        db: &S,
    ) -> Result<bool, S::ReadError> {
        child_ref_cursor.seek(&dir_id, SeekBias::Left, db.child_ref_store())?;
        if let Some(child_ref) = child_ref_cursor.item(db.child_ref_store())? {
            if child_ref.parent_id == dir_id {
                self.path.push(child_ref.name.as_os_str());
                self.stack.push(child_ref_cursor.clone());

                let child_id = child_ref.parent_ref_id.child_id;
                if child_ref.is_visible() {
                    self.metadata_cursor
                        .seek(&child_id, SeekBias::Left, db.metadata_store())?;
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
        let cursor = self.stack.last_mut().unwrap();
        let parent_id = cursor.item(child_ref_db)?.unwrap().parent_id;
        let next_visible_index: usize = cursor.end(child_ref_db)?;
        cursor.seek(&next_visible_index, SeekBias::Right, child_ref_db)?;
        while let Some(child_ref) = cursor.item(child_ref_db)? {
            if child_ref.parent_id == parent_id {
                self.path.pop();
                self.path.push(child_ref.name.as_os_str());
                self.metadata_cursor.seek(
                    &child_ref.parent_ref_id.child_id,
                    SeekBias::Left,
                    db.metadata_store(),
                )?;
                return Ok(true);
            } else {
                break;
            }
        }

        Ok(false)
    }
}

impl btree::Item for Metadata {
    type Summary = id::Unique;

    fn summarize(&self) -> Self::Summary {
        self.file_id
    }
}

impl btree::KeyedItem for Metadata {
    type Key = id::Unique;

    fn key(&self) -> Self::Key {
        self.file_id
    }
}

impl btree::Dimension<id::Unique> for id::Unique {
    fn from_summary(summary: &id::Unique) -> Self {
        *summary
    }
}

impl ParentRefValue {
    fn to_child_ref_value_id(&self, visible: bool) -> Option<ChildRefValueId> {
        self.parent
            .as_ref()
            .map(|(parent_id, name)| ChildRefValueId {
                parent_id: *parent_id,
                name: name.clone(),
                visible,
                timestamp: self.timestamp,
            })
    }
}

impl btree::Item for ParentRefValue {
    type Summary = ParentRefValueId;

    fn summarize(&self) -> Self::Summary {
        use btree::KeyedItem;
        self.key()
    }
}

impl btree::KeyedItem for ParentRefValue {
    type Key = ParentRefValueId;

    fn key(&self) -> Self::Key {
        ParentRefValueId {
            ref_id: self.ref_id,
            timestamp: self.timestamp,
        }
    }
}

impl btree::Dimension<ParentRefValueId> for id::Unique {
    fn from_summary(summary: &ParentRefValueId) -> Self {
        summary.ref_id.child_id
    }
}

impl btree::Dimension<ParentRefValueId> for ParentRefValueId {
    fn from_summary(summary: &ParentRefValueId) -> ParentRefValueId {
        summary.clone()
    }
}

impl Ord for ParentRefValueId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ref_id
            .cmp(&other.ref_id)
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

impl PartialOrd for ParentRefValueId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> AddAssign<&'a Self> for ParentRefValueId {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ParentRefValueId {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self < *other);
        other.clone()
    }
}

impl btree::Dimension<ParentRefValueId> for ParentRefId {
    fn from_summary(summary: &ParentRefValueId) -> Self {
        ParentRefId {
            child_id: summary.ref_id.child_id,
            alias_id: summary.ref_id.alias_id,
        }
    }
}

impl<'a> AddAssign<&'a Self> for ParentRefId {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self <= *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ParentRefId {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self <= *other);
        other.clone()
    }
}

impl ChildRefValue {
    fn is_visible(&self) -> bool {
        self.deletions.is_empty()
    }
}

impl btree::Item for ChildRefValue {
    type Summary = ChildRefValueSummary;

    fn summarize(&self) -> Self::Summary {
        ChildRefValueSummary {
            parent_id: self.parent_id,
            name: self.name.clone(),
            visible: self.is_visible(),
            timestamp: self.timestamp,
            visible_count: if self.is_visible() { 1 } else { 0 },
        }
    }
}

impl btree::KeyedItem for ChildRefValue {
    type Key = ChildRefValueId;

    fn key(&self) -> Self::Key {
        ChildRefValueId {
            parent_id: self.parent_id,
            name: self.name.clone(),
            visible: self.is_visible(),
            timestamp: self.timestamp,
        }
    }
}

impl Ord for ChildRefValueSummary {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parent_id
            .cmp(&other.parent_id)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.visible.cmp(&other.visible).reverse())
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

impl PartialOrd for ChildRefValueSummary {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> AddAssign<&'a Self> for ChildRefValueSummary {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self < *other, "{:?} < {:?}", self, other);

        self.parent_id = other.parent_id;
        self.name = other.name.clone();
        self.visible = other.visible;
        self.timestamp = other.timestamp;
        self.visible_count += other.visible_count;
    }
}

impl btree::Dimension<ChildRefValueSummary> for ChildRefValueId {
    fn from_summary(summary: &ChildRefValueSummary) -> ChildRefValueId {
        ChildRefValueId {
            parent_id: summary.parent_id,
            name: summary.name.clone(),
            visible: summary.visible,
            timestamp: summary.timestamp,
        }
    }
}

impl Ord for ChildRefValueId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parent_id
            .cmp(&other.parent_id)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.visible.cmp(&other.visible).reverse())
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

impl PartialOrd for ChildRefValueId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> AddAssign<&'a Self> for ChildRefValueId {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ChildRefValueId {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self < *other);
        other.clone()
    }
}

impl btree::Dimension<ChildRefValueSummary> for ChildRefId {
    fn from_summary(summary: &ChildRefValueSummary) -> Self {
        ChildRefId {
            parent_id: summary.parent_id,
            name: summary.name.clone(),
        }
    }
}

impl<'a> AddAssign<&'a Self> for ChildRefId {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self <= *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ChildRefId {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self <= *other);
        other.clone()
    }
}

impl btree::Dimension<ChildRefValueSummary> for id::Unique {
    fn from_summary(summary: &ChildRefValueSummary) -> Self {
        summary.parent_id
    }
}

impl btree::Dimension<ChildRefValueSummary> for VisibleCount {
    fn from_summary(summary: &ChildRefValueSummary) -> Self {
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

        timeline.remove("a/b1/c", &db).unwrap();
        assert_eq!(timeline.paths(&db), ["a/", "a/b1/", "a/b1/d/", "a/b2/"]);

        timeline.remove("a/b1", &db).unwrap();
        assert_eq!(timeline.paths(&db), ["a/", "a/b2/"]);

        timeline.insert_dirs("a/b1/c", &db).unwrap();
        timeline.insert_dirs("a/b1/d", &db).unwrap();
        assert_eq!(
            timeline.paths(&db),
            ["a/", "a/b1/", "a/b1/c/", "a/b1/d/", "a/b2/"]
        );

        let moved_id = timeline.id_for_path("a/b1", &db).unwrap().unwrap();
        timeline.rename("a/b1", "b", &db).unwrap();
        assert_eq!(timeline.paths(&db), ["a/", "a/b2/", "b/", "b/c/", "b/d/"]);
        assert_eq!(timeline.id_for_path("b", &db).unwrap().unwrap(), moved_id);

        let moved_id = timeline.id_for_path("b/d", &db).unwrap().unwrap();
        timeline.rename("b/d", "a/b2/d", &db).unwrap();
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

        timeline_1_ops.push(timeline_1.rename("a", "b/a", &db_1).unwrap().unwrap());
        timeline_2_ops.push(timeline_2.rename("b", "a/b", &db_1).unwrap().unwrap());
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
        is_dir: bool,
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
            self.timeline
                .mutate(&mut self.rng, count, &mut self.next_inode, self.db);
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

            if operations.len() == 2 {
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
            if self.timeline.remove(path, self.db).unwrap().is_some() {
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
            if to.starts_with(from) || self.timeline.rename(from, to, self.db).unwrap().is_none() {
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
                let Metadata { is_dir, inode, .. } = cursor.metadata(state.db).unwrap().unwrap();
                Some(FakeFileSystemEntry {
                    depth,
                    name,
                    is_dir,
                    inode: inode.unwrap(),
                })
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

        fn is_dir(&self) -> bool {
            self.is_dir
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
        type ChildRefValueStore = NullStore;

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

    impl btree::NodeStore<ParentRefValue> for NullStore {
        type ReadError = ();

        fn get(
            &self,
            _id: btree::NodeId,
        ) -> Result<Arc<btree::Node<ParentRefValue>>, Self::ReadError> {
            unreachable!()
        }
    }

    impl btree::NodeStore<ChildRefValue> for NullStore {
        type ReadError = ();

        fn get(
            &self,
            _id: btree::NodeId,
        ) -> Result<Arc<btree::Node<ChildRefValue>>, Self::ReadError> {
            unreachable!()
        }
    }

    impl Timeline {
        fn mutate<S, T: Rng>(
            &mut self,
            rng: &mut T,
            count: usize,
            next_inode: &mut Inode,
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
                    if let Some(parent_path) = path.parent() {
                        ops.extend(
                            self.insert_dirs_internal(parent_path, &mut Some(next_inode), db)
                                .unwrap(),
                        );
                    }

                    if rng.gen() {
                        ops.extend(
                            self.insert_dirs_internal(&path, &mut Some(next_inode), db)
                                .unwrap(),
                        );
                    } else {
                        // TODO: Maybe use a more efficient way to select a random file.
                        let mut file_paths = Vec::new();
                        let mut cursor = self.cursor(db).unwrap();
                        while cursor.path().is_some() {
                            if !cursor.is_dir(db).unwrap().unwrap() {
                                file_paths.push(cursor.path().unwrap().to_path_buf());
                            }
                            cursor.next(db).unwrap();
                        }

                        if rng.gen() && !file_paths.is_empty() {
                            ops.extend(
                                self.insert_hard_link_internal(
                                    rng.choose(&file_paths).unwrap(),
                                    &path,
                                    db,
                                ).unwrap(),
                            );
                        } else {
                            let inode = *next_inode;
                            *next_inode += 1;
                            ops.extend(self.insert_file_internal(&path, Some(inode), db).unwrap());
                        }
                    }
                } else if k == 1 {
                    let path = self.select_path(rng, db).unwrap();
                    // println!("{:?} Removing {:?}", db.replica_id(), path);
                    ops.push(self.remove(&path, db).unwrap().unwrap());
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
                    ops.push(self.rename(&old_path, &new_path, db).unwrap().unwrap());
                }
            }
            ops
        }

        fn gen_path<S: Store, T: Rng>(&self, rng: &mut T, depth: usize, db: &S) -> PathBuf {
            loop {
                let mut new_tree = PathBuf::new();
                for _ in 0..depth {
                    new_tree.push(gen_name(rng));
                }

                let path = if self.is_empty(db).unwrap() || rng.gen_weighted_bool(8) {
                    new_tree
                } else {
                    let mut prefix = self.select_path(rng, db).unwrap();
                    prefix.push(new_tree);
                    prefix
                };

                if self.id_for_path(&path, db).unwrap().is_none() {
                    return path;
                }
            }
        }

        fn select_path<S: Store, T: Rng>(&self, rng: &mut T, db: &S) -> Option<PathBuf> {
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
                        return Some(path);
                    } else {
                        let child_ref = rng.choose(&child_refs).unwrap();
                        path.push(child_ref.name.as_os_str());
                        parent_id = child_ref.parent_ref_id.child_id;
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
