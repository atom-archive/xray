use btree::{self, SeekBias};
use time;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::iter::FromIterator;
use std::ops::{Add, AddAssign};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ReplicaId;

pub trait ReplicaContext {
    fn replica_id(&self) -> ReplicaId;
    fn local_time(&self) -> time::Local;
    fn lamport_time(&self) -> time::Lamport;
    fn observe_lamport_timestamp(&self, timestamp: time::Lamport);
}

// TODO: Return results from these methods to deal with IoErrors
pub trait FileSystem {
    type Entry: FileSystemEntry;
    type EntriesIterator: Iterator<Item = Self::Entry>;

    fn create_file(&mut self, path: &Path) -> bool;
    fn create_dir(&mut self, path: &Path) -> bool;
    fn hard_link(&mut self, src: &Path, dst: &Path) -> bool;
    fn remove(&mut self, path: &Path, is_dir: bool) -> bool;
    fn rename(&mut self, from: &Path, to: &Path) -> bool;
    fn inode(&self, path: &Path) -> Option<Inode>;
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

const ROOT_ID: time::Local = time::Local::DEFAULT;

#[derive(Clone)]
pub struct Timeline {
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRefValue>,
    child_refs: btree::Tree<ChildRefValue>,
    inodes_to_file_ids: HashMap<Inode, time::Local>,
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
        op_id: time::Local,
        is_dir: bool,
    },
    UpdateParent {
        op_id: time::Local,
        ref_id: ParentRefId,
        timestamp: time::Lamport,
        prev_timestamp: time::Lamport,
        new_parent: Option<(time::Local, Arc<OsString>)>,
    },
}

#[derive(Debug)]
pub enum Error {
    InvalidPath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    file_id: time::Local,
    is_dir: bool,
    inode: Option<Inode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParentRefValue {
    ref_id: ParentRefId,
    timestamp: time::Lamport,
    prev_timestamp: time::Lamport,
    op_id: time::Local,
    parent: Option<(time::Local, Arc<OsString>)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ParentRefId {
    child_id: time::Local,
    alias_id: time::Local,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParentRefValueId {
    ref_id: ParentRefId,
    timestamp: time::Lamport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildRefValue {
    parent_id: time::Local,
    name: Arc<OsString>,
    timestamp: time::Lamport,
    op_id: time::Local,
    parent_ref_id: ParentRefId,
    deletions: SmallVec<[time::Local; 1]>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefValueSummary {
    parent_id: time::Local,
    name: Arc<OsString>,
    visible: bool,
    timestamp: time::Lamport,
    visible_count: VisibleCount,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefValueId {
    parent_id: time::Local,
    name: Arc<OsString>,
    visible: bool,
    timestamp: time::Lamport,
}

#[derive(Clone, Debug, Default, Ord, Eq, PartialEq, PartialOrd)]
pub struct ChildRefId {
    parent_id: time::Local,
    name: Arc<OsString>,
}

impl Timeline {
    pub fn new() -> Self {
        Timeline {
            metadata: btree::Tree::new(),
            parent_refs: btree::Tree::new(),
            child_refs: btree::Tree::new(),
            inodes_to_file_ids: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cursor().depth() == 0
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor_at(ROOT_ID)
    }

    pub fn cursor_at(&self, id: time::Local) -> Cursor {
        let mut cursor = Cursor {
            path: PathBuf::new(),
            stack: Vec::new(),
            metadata_cursor: self.metadata.cursor(),
        };
        cursor.descend_into(self.child_refs.cursor(), id);
        cursor
    }

    fn write_to_fs<F, R>(
        &mut self,
        mut refs_to_write: HashSet<ParentRefId>,
        mut old_tree: Timeline,
        fs: &mut F,
        ctx: &R,
    ) -> Vec<Operation>
    where
        F: FileSystem,
        R: ReplicaContext,
    {
        let mut fixup_ops = Vec::new();
        let mut refs_with_temp_name = HashSet::new();

        loop {
            let mut sorted_refs_to_write = self
                .sort_refs_by_path_depth(refs_to_write.iter().cloned())
                .into_iter()
                .peekable();

            while let Some((ref_id, depth)) = sorted_refs_to_write.peek().cloned() {
                let old_path = old_tree.resolve_path(ref_id);
                if let Some(old_path) = old_path.as_ref() {
                    if fs.inode(old_path) != old_tree.inode_for_id(ref_id.child_id) {
                        break;
                    }
                }

                let parent_ref = self.cur_parent_ref_value(ref_id).unwrap();
                let metadata = self.metadata(ref_id.child_id).unwrap();
                if depth.is_some() {
                    let (parent_id, mut name) = parent_ref.parent.clone().unwrap();

                    if let Some(parent_path) = old_tree.resolve_paths(parent_id).first_mut() {
                        if parent_id != ROOT_ID
                            && fs.inode(&parent_path) != old_tree.inode_for_id(parent_id)
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
                        if old_tree.id_for_path(&new_path).is_some() {
                            loop {
                                let name = Arc::make_mut(&mut name);
                                name.push("~");
                                new_path.set_file_name(name);

                                if old_tree.id_for_path(&new_path).is_none()
                                    && self.id_for_path(&new_path).is_none()
                                {
                                    break;
                                }
                            }
                            assigned_temp_name = true;
                        } else {
                            assigned_temp_name = false;
                        }

                        if let Some(old_path) = old_path {
                            if fs.rename(&old_path, &new_path) {
                                if assigned_temp_name {
                                    refs_with_temp_name.insert(ref_id);
                                }

                                let prev_parent_ref =
                                    old_tree.cur_parent_ref_value(ref_id).unwrap();
                                old_tree.integrate_op(
                                    Operation::UpdateParent {
                                        op_id: parent_ref.op_id,
                                        ref_id,
                                        timestamp: parent_ref.timestamp,
                                        prev_timestamp: prev_parent_ref.timestamp,
                                        new_parent: Some((parent_id, name)),
                                    },
                                    ctx,
                                );
                            } else {
                                break;
                            }
                        } else {
                            let mut success = false;
                            if metadata.is_dir {
                                success = fs.create_dir(&new_path);
                            } else {
                                let existing_paths = old_tree.resolve_paths(ref_id.child_id);
                                if existing_paths.is_empty() {
                                    success = fs.create_file(&new_path);
                                } else {
                                    for existing_path in existing_paths {
                                        if fs.inode(&existing_path) == metadata.inode
                                            && fs.hard_link(&existing_path, new_path)
                                        {
                                            success = true;
                                            break;
                                        }
                                    }
                                }
                            }

                            if success {
                                if assigned_temp_name {
                                    refs_with_temp_name.insert(ref_id);
                                }

                                if let Some(inode) = fs.inode(&new_path) {
                                    if let Some(prev_parent_ref) =
                                        old_tree.cur_parent_ref_value(ref_id)
                                    {
                                        old_tree.integrate_op(
                                            Operation::UpdateParent {
                                                op_id: parent_ref.op_id,
                                                ref_id,
                                                timestamp: parent_ref.timestamp,
                                                prev_timestamp: prev_parent_ref.timestamp,
                                                new_parent: Some((parent_id, name)),
                                            },
                                            ctx,
                                        );
                                    } else {
                                        old_tree.integrate_op(
                                            Operation::InsertMetadata {
                                                op_id: ref_id.child_id,
                                                is_dir: true,
                                            },
                                            ctx,
                                        );
                                        old_tree.integrate_op(
                                            Operation::UpdateParent {
                                                op_id: parent_ref.op_id,
                                                ref_id,
                                                timestamp: parent_ref.timestamp,
                                                prev_timestamp: parent_ref.timestamp,
                                                new_parent: Some((parent_id, name)),
                                            },
                                            ctx,
                                        );
                                    };

                                    old_tree.set_inode_for_id(ref_id.child_id, inode);
                                    self.set_inode_for_id(ref_id.child_id, inode);
                                } else {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                } else if let Some(old_path) = old_path {
                    if fs.remove(&old_path, metadata.is_dir) {
                        let prev_parent_ref = old_tree.cur_parent_ref_value(ref_id).unwrap();
                        old_tree.integrate_op(
                            Operation::UpdateParent {
                                op_id: parent_ref.op_id,
                                ref_id,
                                timestamp: parent_ref.timestamp,
                                prev_timestamp: prev_parent_ref.timestamp,
                                new_parent: None,
                            },
                            ctx,
                        );
                    } else {
                        break;
                    }
                }

                refs_to_write.remove(&ref_id);
                sorted_refs_to_write.next();
            }

            if sorted_refs_to_write.peek().is_some() {
                // println!("refreshing old timeline - paths: {:?}", fs.paths());
                let fs_ops = old_tree.read_from_fs(fs.entries(), ctx);
                let fs_fixup_ops = self.integrate_ops::<F, _, _>(&fs_ops, None, ctx);
                for op in &fs_ops {
                    match op {
                        Operation::InsertMetadata { op_id, .. } => {
                            let inode = old_tree.inode_for_id(*op_id).unwrap();
                            self.set_inode_for_id(*op_id, inode);
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
            if let Some(old_path) = old_tree.resolve_path(ref_id) {
                let mut new_path = old_path.clone();
                new_path.set_file_name(self.resolve_name(ref_id).unwrap().as_os_str());

                if new_path != old_path && old_tree.id_for_path(&new_path).is_none() {
                    let fs_inode = fs.inode(&old_path);
                    let tree_inode = old_tree.inode_for_id(ref_id.child_id);
                    if fs_inode == tree_inode && fs.rename(&old_path, &new_path) {
                        old_tree.rename(&old_path, &new_path, ctx).unwrap();
                    }
                }
            }
        }

        fixup_ops
    }

    pub fn read_from_fs<'a, F, I, R>(&mut self, entries: I, ctx: &R) -> Vec<Operation>
    where
        F: FileSystemEntry,
        I: IntoIterator<Item = F>,
        R: ReplicaContext,
    {
        struct Change<F: FileSystemEntry> {
            inserted: bool,
            entry: F,
            parents: SmallVec<[(time::Local, Arc<OsString>); 1]>,
        }

        let mut dir_stack = vec![ROOT_ID];
        let mut visited_dir_ids = HashSet::new();
        let mut occupied_ref_ids = HashSet::new();
        let mut changes = HashMap::new();

        visited_dir_ids.insert(ROOT_ID);
        for entry in entries {
            assert!(entry.depth() > 0);
            dir_stack.truncate(entry.depth());

            let cur_parent = Some((*dir_stack.last().unwrap(), Arc::new(entry.name().into())));

            if let Some(file_id) = self.inodes_to_file_ids.get(&entry.inode()).cloned() {
                if entry.is_dir() {
                    dir_stack.push(file_id);

                    if let Some(parent_ref) = self.cur_parent_ref_values(file_id).pop() {
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
                        .cur_parent_ref_values(file_id)
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
                let file_id = ctx.local_time();
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
                    .cur_parent_ref_values(child_id)
                    .into_iter()
                    .map(|parent_ref| parent_ref.ref_id)
                    .filter(|ref_id| !occupied_ref_ids.contains(&ref_id))
                    .next();
                let alias_id = if let Some(ref_id) = available_ref_id {
                    occupied_ref_ids.insert(ref_id);
                    ref_id.alias_id
                } else {
                    ctx.local_time()
                };

                operations.push(self.update_parent_ref(
                    ParentRefId { child_id, alias_id },
                    Some(parent),
                    &mut parent_ref_edits,
                    &mut child_ref_edits,
                    ctx,
                ));
            }
        }

        // Delete all file refs that are not reachable anymore from the visited directories.
        for dir_id in visited_dir_ids {
            let mut cursor = self.cursor_at(dir_id);
            while let Some(ref_id) = cursor.ref_id() {
                if !occupied_ref_ids.contains(&ref_id) {
                    operations.push(self.update_parent_ref(
                        ref_id,
                        None,
                        &mut parent_ref_edits,
                        &mut child_ref_edits,
                        ctx,
                    ));
                }
                cursor.next_sibling_or_cousin();
            }
        }

        self.metadata.edit(metadata_edits);
        self.parent_refs.edit(parent_ref_edits);
        self.child_refs.edit(child_ref_edits);

        operations
    }

    fn insert_metadata(
        &self,
        file_id: time::Local,
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

    fn update_parent_ref<R: ReplicaContext>(
        &self,
        ref_id: ParentRefId,
        new_parent: Option<(time::Local, Arc<OsString>)>,
        parent_ref_edits: &mut Vec<btree::Edit<ParentRefValue>>,
        child_ref_edits: &mut Vec<btree::Edit<ChildRefValue>>,
        ctx: &R,
    ) -> Operation {
        let timestamp = ctx.lamport_time();
        let op_id = ctx.local_time();

        let prev_parent_ref = self.cur_parent_ref_value(ref_id);
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
            let mut prev_child_ref = self.child_ref_value(prev_child_ref_value_id).unwrap();
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

        Operation::UpdateParent {
            op_id,
            ref_id,
            timestamp,
            prev_timestamp,
            new_parent,
        }
    }

    pub fn create_dir_all<I, R>(&mut self, path: I, ctx: &R) -> Result<Vec<Operation>, Error>
    where
        I: Into<PathBuf>,
        R: ReplicaContext,
    {
        self.create_dir_all_internal(path, &mut None, ctx)
    }

    pub fn create_file_internal<I, R>(
        &mut self,
        path: I,
        is_dir: bool,
        inode: Option<Inode>,
        ctx: &R,
    ) -> Result<SmallVec<[Operation; 2]>, Error>
    where
        I: Into<PathBuf>,
        R: ReplicaContext,
    {
        let path = path.into();
        let mut operations = SmallVec::new();
        if self.id_for_path(&path).is_some() {
            return Err(Error::InvalidPath);
        }

        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        let parent_id = if let Some(parent_path) = path.parent() {
            self.id_for_path(parent_path).ok_or(Error::InvalidPath)?
        } else {
            ROOT_ID
        };
        let child_id = ctx.local_time();

        operations.push(self.insert_metadata(child_id, is_dir, inode, &mut metadata_edits));
        operations.push(self.update_parent_ref(
            ParentRefId {
                child_id,
                alias_id: ctx.local_time(),
            },
            Some((parent_id, Arc::new(path.file_name().unwrap().into()))),
            &mut parent_ref_edits,
            &mut child_ref_edits,
            ctx,
        ));

        if let Some(inode) = inode {
            self.inodes_to_file_ids.insert(inode, child_id);
        }

        self.metadata.edit(metadata_edits);
        self.parent_refs.edit(parent_ref_edits);
        self.child_refs.edit(child_ref_edits);
        Ok(operations)
    }

    pub fn hard_link<I1, I2, R>(&mut self, src: I1, dst: I2, ctx: &R) -> Result<Operation, Error>
    where
        I1: Into<PathBuf>,
        I2: Into<PathBuf>,
        R: ReplicaContext,
    {
        let src = src.into();
        let dst = dst.into();

        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        if self.id_for_path(&dst).is_some() {
            return Err(Error::InvalidPath);
        }

        let parent_id = if let Some(parent_path) = dst.parent() {
            self.id_for_path(parent_path).ok_or(Error::InvalidPath)?
        } else {
            ROOT_ID
        };

        if let Some(child_id) = self.id_for_path(src) {
            let operation = self.update_parent_ref(
                ParentRefId {
                    child_id,
                    alias_id: ctx.local_time(),
                },
                Some((parent_id, Arc::new(dst.file_name().unwrap().into()))),
                &mut parent_ref_edits,
                &mut child_ref_edits,
                ctx,
            );

            self.parent_refs.edit(parent_ref_edits);
            self.child_refs.edit(child_ref_edits);
            Ok(operation)
        } else {
            Err(Error::InvalidPath)
        }
    }

    fn create_dir_all_internal<I, R>(
        &mut self,
        path: I,
        next_inode: &mut Option<&mut Inode>,
        ctx: &R,
    ) -> Result<Vec<Operation>, Error>
    where
        I: Into<PathBuf>,
        R: ReplicaContext,
    {
        let path = path.into();

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
                if cursor.seek(&key, SeekBias::Left) {
                    let child_ref = cursor.item().unwrap();
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
                let child_id = ctx.local_time();
                let inode = next_inode.as_mut().map(|next_inode| {
                    let inode = **next_inode;
                    **next_inode += 1;
                    inode
                });

                operations.push(self.insert_metadata(child_id, true, inode, &mut metadata_edits));
                operations.push(self.update_parent_ref(
                    ParentRefId {
                        child_id,
                        alias_id: ctx.local_time(),
                    },
                    Some((parent_id, name.clone())),
                    &mut parent_ref_edits,
                    &mut child_ref_edits,
                    ctx,
                ));

                if let Some(inode) = inode {
                    self.inodes_to_file_ids.insert(inode, child_id);
                }
                parent_id = child_id;
            }
        }

        self.metadata.edit(metadata_edits);
        self.parent_refs.edit(parent_ref_edits);
        self.child_refs.edit(child_ref_edits);
        Ok(operations)
    }

    pub fn remove<I, R>(&mut self, path: I, ctx: &R) -> Result<Operation, Error>
    where
        I: Into<PathBuf>,
        R: ReplicaContext,
    {
        let path = path.into();

        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        let ref_id = self.ref_id_for_path(&path).ok_or(Error::InvalidPath)?;
        let operation = self.update_parent_ref(
            ref_id,
            None,
            &mut parent_ref_edits,
            &mut child_ref_edits,
            ctx,
        );
        self.parent_refs.edit(parent_ref_edits);
        self.child_refs.edit(child_ref_edits);

        Ok(operation)
    }

    pub fn rename<F, R, T>(&mut self, from: F, to: T, ctx: &R) -> Result<Operation, Error>
    where
        F: Into<PathBuf>,
        R: ReplicaContext,
        T: Into<PathBuf>,
    {
        let from = from.into();
        let to = to.into();

        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        if self.id_for_path(&to).is_some() {
            return Err(Error::InvalidPath);
        }

        let ref_id = self.ref_id_for_path(&from).ok_or(Error::InvalidPath)?;
        let new_parent_id = if let Some(parent_path) = to.parent() {
            self.id_for_path(parent_path).ok_or(Error::InvalidPath)?
        } else {
            ROOT_ID
        };

        let new_name = Arc::new(OsString::from(to.file_name().unwrap()));
        let operation = self.update_parent_ref(
            ref_id,
            Some((new_parent_id, new_name)),
            &mut parent_ref_edits,
            &mut child_ref_edits,
            ctx,
        );
        self.parent_refs.edit(parent_ref_edits);
        self.child_refs.edit(child_ref_edits);

        Ok(operation)
    }

    pub fn integrate_ops<'a, F, O, R>(
        &mut self,
        ops: O,
        fs: Option<&mut F>,
        ctx: &R,
    ) -> Vec<Operation>
    where
        F: FileSystem,
        O: IntoIterator<Item = &'a Operation> + Clone,
        R: ReplicaContext,
    {
        // println!("integrate ops >>>>>>>>>>>>");
        let old_tree = self.clone();

        let mut changed_refs = HashMap::new();
        for op in ops.clone() {
            match op {
                Operation::UpdateParent {
                    ref_id,
                    timestamp,
                    prev_timestamp,
                    ..
                } => {
                    let moved_dir = timestamp != prev_timestamp
                        && self.metadata(ref_id.child_id).unwrap().is_dir;
                    changed_refs.insert(*ref_id, moved_dir);
                }
                _ => {}
            }
            self.integrate_op(op.clone(), ctx);
        }

        let mut fixup_ops = Vec::new();
        for (ref_id, moved_dir) in &changed_refs {
            fixup_ops.extend(self.fix_conflicts(*ref_id, *moved_dir, ctx));
        }

        if let Some(fs) = fs {
            let mut refs_to_write = HashSet::new();
            for (ref_id, moved_dir) in changed_refs {
                refs_to_write.insert(ref_id);
                if moved_dir && old_tree.resolve_depth(ref_id).is_none() {
                    let mut cursor = self.cursor_at(ref_id.child_id);
                    while let Some(descendant_ref_id) = cursor.ref_id() {
                        refs_to_write.insert(descendant_ref_id);
                        cursor.next();
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

            fixup_ops.extend(self.write_to_fs(refs_to_write, old_tree, fs, ctx));
        }

        // println!("integrate ops <<<<<<<<<<<<");

        fixup_ops
    }

    fn sort_refs_by_path_depth<I>(&self, ref_ids: I) -> Vec<(ParentRefId, Option<usize>)>
    where
        I: Iterator<Item = ParentRefId>,
    {
        let mut ref_ids_to_depths = HashMap::new();
        for ref_id in ref_ids {
            ref_ids_to_depths.insert(ref_id, self.resolve_depth(ref_id));
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

        sorted_ref_ids
    }

    #[cfg(test)]
    pub fn paths(&self) -> Vec<String> {
        let mut cursor = self.cursor();
        let mut paths = Vec::new();
        loop {
            if let Some(path) = cursor.path() {
                let mut path = path.to_string_lossy().into_owned();
                if cursor.metadata().unwrap().is_dir {
                    path += "/";
                }
                paths.push(path);
            } else {
                break;
            }
            cursor.next();
        }
        paths
    }

    #[cfg(test)]
    fn paths_with_ids(&self) -> Vec<(time::Local, String)> {
        self.paths()
            .into_iter()
            .map(|path| (self.id_for_path(&path).unwrap(), path))
            .collect()
    }

    fn integrate_op<R>(&mut self, op: Operation, ctx: &R)
    where
        R: ReplicaContext,
    {
        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();
        let mut new_child_ref;

        // println!("{:?} – integrate op {:?}", ctx.replica_id(), op);

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
                parent_ref_cursor.seek(&ref_id, SeekBias::Left);
                let mut is_latest_parent_ref = true;

                while let Some(parent_ref) = parent_ref_cursor.item() {
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
                            child_ref_cursor.seek(&child_ref_value_id, SeekBias::Left);
                            let mut child_ref = child_ref_cursor.item().unwrap();
                            if child_ref.is_visible() {
                                child_ref_edits.push(btree::Edit::Remove(child_ref.clone()));
                            }
                            child_ref.deletions.push(op_id);
                            child_ref_edits.push(btree::Edit::Insert(child_ref));
                        }
                    } else {
                        break;
                    }
                    parent_ref_cursor.next();
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

                if ctx.replica_id() != timestamp.replica_id {
                    ctx.observe_lamport_timestamp(timestamp);
                }
            }
        }

        self.child_refs.edit(child_ref_edits);
        self.metadata.edit(metadata_edits);
        self.parent_refs.edit(parent_ref_edits);
    }

    fn fix_conflicts<R: ReplicaContext>(
        &mut self,
        ref_id: ParentRefId,
        moved_dir: bool,
        ctx: &R,
    ) -> Vec<Operation> {
        use btree::KeyedItem;

        let mut fixup_ops = Vec::new();
        let mut reverted_moves: HashMap<ParentRefId, time::Lamport> = HashMap::new();

        // If the child was moved and is a directory, check for cycles.
        if moved_dir {
            let mut visited = HashSet::new();
            let mut latest_move: Option<ParentRefValue> = None;
            let mut cursor = self.parent_refs.cursor();
            cursor.seek(&ref_id, SeekBias::Left);

            loop {
                let mut parent_ref = cursor.item().unwrap();
                if visited.contains(&parent_ref.ref_id.child_id) {
                    // Cycle detected. Revert the most recent move contributing to the cycle.
                    cursor.seek(&latest_move.as_ref().unwrap().key(), SeekBias::Right);

                    // Find the previous value for this parent ref that isn't a deletion and store
                    // its timestamp in our reverted_moves map.
                    loop {
                        let parent_ref = cursor.item().unwrap();
                        if parent_ref.parent.is_some() {
                            reverted_moves.insert(parent_ref.ref_id, parent_ref.timestamp);
                            break;
                        } else {
                            cursor.next();
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
                            cursor.next();
                            parent_ref = cursor.item().unwrap();
                        }
                    }

                    // Check if this parent ref is a move and has the latest timestamp of any move
                    // we have seen so far. If so, it is a candidate to be reverted.
                    if latest_move
                        .as_ref()
                        .map_or(true, |m| parent_ref.timestamp > m.timestamp)
                    {
                        cursor.next();
                        if cursor.item().map_or(false, |next_parent_ref| {
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
                            cursor.seek(&parent_id, SeekBias::Left);
                        }
                    } else {
                        break;
                    }
                }
            }

            // Convert the reverted moves into new move operations.
            let mut moved_ref_ids = Vec::new();
            for (ref_id, timestamp) in &reverted_moves {
                cursor.seek(ref_id, SeekBias::Left);
                let prev_timestamp = cursor.item().unwrap().timestamp;
                cursor.seek_forward(
                    &ParentRefValueId {
                        ref_id: *ref_id,
                        timestamp: *timestamp,
                    },
                    SeekBias::Left,
                );
                let new_parent = cursor.item().unwrap().parent;
                fixup_ops.push(Operation::UpdateParent {
                    op_id: ctx.local_time(),
                    ref_id: *ref_id,
                    timestamp: ctx.lamport_time(),
                    prev_timestamp,
                    new_parent,
                });
                moved_ref_ids.push(*ref_id);
            }

            for op in &fixup_ops {
                self.integrate_op(op.clone(), ctx);
            }
            for ref_id in moved_ref_ids {
                fixup_ops.extend(self.fix_name_conflicts(ref_id, ctx));
            }
        }

        if !reverted_moves.contains_key(&ref_id) {
            fixup_ops.extend(self.fix_name_conflicts(ref_id, ctx));
        }

        fixup_ops
    }

    fn fix_name_conflicts<R: ReplicaContext>(
        &mut self,
        ref_id: ParentRefId,
        ctx: &R,
    ) -> Vec<Operation> {
        let mut fixup_ops = Vec::new();

        let parent_ref = self.cur_parent_ref_value(ref_id).unwrap();
        if let Some((parent_id, name)) = parent_ref.parent {
            let mut cursor_1 = self.child_refs.cursor();
            cursor_1.seek(
                &ChildRefId {
                    parent_id,
                    name: name.clone(),
                },
                SeekBias::Left,
            );
            cursor_1.next();

            let mut cursor_2 = cursor_1.clone();
            let mut unique_name = name.clone();

            while let Some(child_ref) = cursor_1.item() {
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
                        );
                        if let Some(conflicting_child_ref) = cursor_2.item() {
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
                        op_id: ctx.local_time(),
                        ref_id: child_ref.parent_ref_id,
                        timestamp: ctx.lamport_time(),
                        prev_timestamp: child_ref.timestamp,
                        new_parent: Some((parent_id, unique_name.clone())),
                    };
                    self.integrate_op(fixup_op.clone(), ctx);
                    fixup_ops.push(fixup_op);

                    let visible_index = cursor_1.end::<usize>();
                    cursor_1.seek_forward(&visible_index, SeekBias::Right);
                } else {
                    break;
                }
            }
        }

        fixup_ops
    }

    fn id_for_path<P>(&self, path: P) -> Option<time::Local>
    where
        P: Into<PathBuf>,
    {
        self.ref_id_for_path(path).map(|ref_id| ref_id.child_id)
    }

    fn ref_id_for_path<P>(&self, path: P) -> Option<ParentRefId>
    where
        P: Into<PathBuf>,
    {
        let path = path.into();

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
            if cursor.seek(&key, SeekBias::Left) {
                let child_ref = cursor.item().unwrap();
                if child_ref.is_visible() {
                    ref_id = child_ref.parent_ref_id;
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }

        Some(ref_id)
    }

    pub fn resolve_paths(&self, file_id: time::Local) -> SmallVec<[PathBuf; 1]> {
        let mut paths = SmallVec::new();

        if file_id == ROOT_ID {
            paths.push(PathBuf::new());
        } else {
            for parent_ref in self.cur_parent_ref_values(file_id) {
                paths.extend(self.resolve_path(parent_ref.ref_id));
            }
        }
        paths
    }

    fn resolve_path(&self, ref_id: ParentRefId) -> Option<PathBuf> {
        let mut path_components = Vec::new();
        if self.visit_ancestors(ref_id, |name| path_components.push(name)) {
            let mut path = PathBuf::new();
            for component in path_components.into_iter().rev() {
                path.push(component.as_ref());
            }
            Some(path)
        } else {
            None
        }
    }

    fn resolve_depth(&self, ref_id: ParentRefId) -> Option<usize> {
        let mut depth = 0;
        if self.visit_ancestors(ref_id, |_| depth += 1) {
            Some(depth)
        } else {
            None
        }
    }

    fn visit_ancestors<F>(&self, ref_id: ParentRefId, mut f: F) -> bool
    where
        F: FnMut(Arc<OsString>),
    {
        let mut visited = HashSet::new();
        let mut cursor = self.parent_refs.cursor();
        if ref_id.child_id == ROOT_ID {
            true
        } else if cursor.seek(&ref_id, SeekBias::Left) {
            loop {
                if let Some((parent_id, name)) = cursor.item().and_then(|r| r.parent) {
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
                        cursor.seek(&parent_id, SeekBias::Left);
                    }
                } else {
                    return false;
                }
            }

            true
        } else {
            false
        }
    }

    fn resolve_name(&self, ref_id: ParentRefId) -> Option<Arc<OsString>> {
        self.cur_parent_ref_value(ref_id)
            .and_then(|parent_ref| parent_ref.parent)
            .map(|(_, name)| name)
    }

    fn inode_for_id(&self, child_id: time::Local) -> Option<Inode> {
        self.metadata(child_id).and_then(|metadata| metadata.inode)
    }

    fn set_inode_for_id(&mut self, child_id: time::Local, inode: Inode) -> bool {
        let mut cursor = self.metadata.cursor();
        if cursor.seek(&child_id, SeekBias::Left) {
            let mut metadata = cursor.item().unwrap();
            if let Some(inode) = metadata.inode {
                self.inodes_to_file_ids.remove(&inode);
            }

            metadata.inode = Some(inode);
            self.metadata.edit(vec![btree::Edit::Insert(metadata)]);
            self.inodes_to_file_ids.insert(inode, child_id);
            true
        } else {
            false
        }
    }

    fn metadata(&self, child_id: time::Local) -> Option<Metadata> {
        let mut cursor = self.metadata.cursor();
        if cursor.seek(&child_id, SeekBias::Left) {
            cursor.item()
        } else {
            None
        }
    }

    fn cur_parent_ref_values(&self, child_id: time::Local) -> Vec<ParentRefValue> {
        let mut cursor = self.parent_refs.cursor();
        cursor.seek(&child_id, SeekBias::Left);
        let mut parent_ref_values = Vec::new();
        while let Some(parent_ref) = cursor.item() {
            if parent_ref.ref_id.child_id == child_id {
                cursor.seek(&parent_ref.ref_id, SeekBias::Right);
                parent_ref_values.push(parent_ref);
            } else {
                break;
            }
        }
        parent_ref_values
    }

    fn cur_parent_ref_value(&self, ref_id: ParentRefId) -> Option<ParentRefValue> {
        let mut cursor = self.parent_refs.cursor();
        if cursor.seek(&ref_id, SeekBias::Left) {
            cursor.item()
        } else {
            None
        }
    }

    fn child_ref_value(&self, ref_id: ChildRefValueId) -> Option<ChildRefValue> {
        let mut cursor = self.child_refs.cursor();
        if cursor.seek(&ref_id, SeekBias::Left) {
            cursor.item()
        } else {
            None
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

    pub fn name(&self) -> Option<Arc<OsString>> {
        if self.stack.is_empty() {
            None
        } else {
            Some(self.stack.last().unwrap().item().unwrap().name.clone())
        }
    }

    pub fn is_dir(&self) -> Option<bool> {
        self.metadata().map(|metadata| metadata.is_dir)
    }

    pub fn file_id(&self) -> Option<time::Local> {
        self.metadata().map(|metadata| metadata.file_id)
    }

    pub fn ref_id(&self) -> Option<ParentRefId> {
        if self.stack.is_empty() {
            None
        } else {
            Some(self.stack.last().unwrap().item().unwrap().parent_ref_id)
        }
    }

    fn metadata(&self) -> Option<Metadata> {
        if self.stack.is_empty() {
            None
        } else {
            self.metadata_cursor.item()
        }
    }

    pub fn next(&mut self) {
        if !self.stack.is_empty() {
            let metadata = self.metadata_cursor.item().unwrap();
            if !metadata.is_dir || !self.descend() {
                self.next_sibling_or_cousin();
            }
        }
    }

    pub fn next_sibling_or_cousin(&mut self) {
        while !self.stack.is_empty() && !self.next_sibling() {
            self.path.pop();
            self.stack.pop();
        }
    }

    fn descend(&mut self) -> bool {
        let cursor = self.stack.last().unwrap().clone();
        let dir_id = cursor.item().unwrap().parent_ref_id.child_id;
        self.descend_into(cursor, dir_id)
    }

    fn descend_into(
        &mut self,
        mut child_ref_cursor: btree::Cursor<ChildRefValue>,
        dir_id: time::Local,
    ) -> bool {
        child_ref_cursor.seek(&dir_id, SeekBias::Left);
        if let Some(child_ref) = child_ref_cursor.item() {
            if child_ref.parent_id == dir_id {
                self.path.push(child_ref.name.as_os_str());
                self.stack.push(child_ref_cursor.clone());

                let child_id = child_ref.parent_ref_id.child_id;
                if child_ref.is_visible() {
                    self.metadata_cursor.seek(&child_id, SeekBias::Left);
                    true
                } else if self.next_sibling() {
                    true
                } else {
                    self.path.pop();
                    self.stack.pop();
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    fn next_sibling(&mut self) -> bool {
        let cursor = self.stack.last_mut().unwrap();
        let parent_id = cursor.item().unwrap().parent_id;
        let next_visible_index: usize = cursor.end();
        cursor.seek(&next_visible_index, SeekBias::Right);
        while let Some(child_ref) = cursor.item() {
            if child_ref.parent_id == parent_id {
                self.path.pop();
                self.path.push(child_ref.name.as_os_str());
                self.metadata_cursor
                    .seek(&child_ref.parent_ref_id.child_id, SeekBias::Left);
                return true;
            } else {
                break;
            }
        }

        false
    }
}

impl btree::Item for Metadata {
    type Summary = time::Local;

    fn summarize(&self) -> Self::Summary {
        self.file_id
    }
}

impl btree::KeyedItem for Metadata {
    type Key = time::Local;

    fn key(&self) -> Self::Key {
        self.file_id
    }
}

impl btree::Dimension<time::Local> for time::Local {
    fn from_summary(summary: &time::Local) -> Self {
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

impl btree::Dimension<ParentRefValueId> for time::Local {
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

impl btree::Dimension<ChildRefValueSummary> for time::Local {
    fn from_summary(summary: &ChildRefValueSummary) -> Self {
        summary.parent_id
    }
}

impl btree::Dimension<ChildRefValueSummary> for VisibleCount {
    fn from_summary(summary: &ChildRefValueSummary) -> Self {
        summary.visible_count
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
        let ctx = TestContext::new(1);
        let mut timeline = Timeline::new();
        timeline.create_dir_all("a/b2/", &ctx).unwrap();
        assert_eq!(timeline.paths(), ["a/", "a/b2/"]);

        timeline.create_dir_all("a/b1/c", &ctx).unwrap();
        assert_eq!(timeline.paths(), ["a/", "a/b1/", "a/b1/c/", "a/b2/"]);

        timeline.create_dir_all("a/b1/d", &ctx).unwrap();
        assert_eq!(
            timeline.paths(),
            ["a/", "a/b1/", "a/b1/c/", "a/b1/d/", "a/b2/"]
        );

        timeline.remove("a/b1/c", &ctx).unwrap();
        assert_eq!(timeline.paths(), ["a/", "a/b1/", "a/b1/d/", "a/b2/"]);

        timeline.remove("a/b1", &ctx).unwrap();
        assert_eq!(timeline.paths(), ["a/", "a/b2/"]);

        timeline.create_dir_all("a/b1/c", &ctx).unwrap();
        timeline.create_dir_all("a/b1/d", &ctx).unwrap();
        assert_eq!(
            timeline.paths(),
            ["a/", "a/b1/", "a/b1/c/", "a/b1/d/", "a/b2/"]
        );

        let moved_id = timeline.id_for_path("a/b1").unwrap();
        timeline.rename("a/b1", "b", &ctx).unwrap();
        assert_eq!(timeline.paths(), ["a/", "a/b2/", "b/", "b/c/", "b/d/"]);
        assert_eq!(timeline.id_for_path("b",).unwrap(), moved_id);

        let moved_id = timeline.id_for_path("b/d").unwrap();
        timeline.rename("b/d", "a/b2/d", &ctx).unwrap();
        assert_eq!(timeline.paths(), ["a/", "a/b2/", "a/b2/d/", "b/", "b/c/"]);
        assert_eq!(timeline.id_for_path("a/b2/d",).unwrap(), moved_id);
    }

    #[test]
    fn test_fs_sync_random() {
        for seed in 0..100 {
            // let seed = 210774;
            println!("SEED: {:?}", seed);

            let mut rng = StdRng::from_seed(&[seed]);
            let ctx = TestContext::new(1);

            let mut fs_1 = FakeFileSystem::new(&ctx, rng.clone());
            fs_1.mutate(5);

            let mut fs_2 = fs_1.clone();
            let mut prev_fs_1_version = fs_1.version();
            let mut prev_fs_2_version = fs_2.version();
            let mut index_1 = fs_1.timeline();
            let mut index_2 = index_1.clone();
            let mut ops_1 = Vec::new();
            let mut ops_2 = Vec::new();

            // println!("mutate fs 1");

            fs_1.mutate(5);

            loop {
                if fs_1.version() > prev_fs_1_version && rng.gen() {
                    // println!("scanning from fs 1");
                    prev_fs_1_version = fs_1.version();
                    ops_1.extend(index_1.read_from_fs(fs_1.entries(), &ctx));
                }

                if fs_2.version() > prev_fs_2_version && rng.gen() {
                    // println!("scanning from fs 2");
                    prev_fs_2_version = fs_2.version();
                    ops_2.extend(index_2.read_from_fs(fs_2.entries(), &ctx));
                }

                if !ops_2.is_empty() && rng.gen() {
                    // println!("integrating into index 1");
                    ops_1.extend(index_1.integrate_ops(
                        &ops_2.drain(..).collect::<Vec<_>>(),
                        Some(&mut fs_1),
                        &ctx,
                    ));
                }

                if !ops_1.is_empty() && rng.gen() {
                    // println!("integrating into index 2");
                    ops_2.extend(index_2.integrate_ops(
                        &ops_1.drain(..).collect::<Vec<_>>(),
                        Some(&mut fs_2),
                        &ctx,
                    ));
                }

                if ops_1.is_empty()
                    && ops_2.is_empty()
                    && fs_1.version() == prev_fs_1_version
                    && fs_2.version() == prev_fs_2_version
                {
                    break;
                }
            }

            assert_eq!(index_2.paths_with_ids(), index_1.paths_with_ids());
            assert_eq!(fs_2.paths(), fs_1.paths());
            assert_eq!(index_1.paths(), fs_1.paths());
        }
    }

    #[test]
    fn test_name_conflict_fixups() {
        let ctx_1 = TestContext::new(1);
        let mut timeline_1 = Timeline::new();
        let mut timeline_1_ops = Vec::new();

        let ctx_2 = TestContext::new(2);
        let mut timeline_2 = Timeline::new();
        let mut timeline_2_ops = Vec::new();

        timeline_1_ops.extend(timeline_1.create_dir_all("a", &ctx_1).unwrap());
        let id_1 = timeline_1.id_for_path("a").unwrap();

        timeline_2_ops.extend(timeline_2.create_dir_all("a", &ctx_2).unwrap());
        timeline_2_ops.extend(timeline_2.create_dir_all("a~", &ctx_2).unwrap());
        let id_2 = timeline_2.id_for_path("a").unwrap();
        let id_3 = timeline_2.id_for_path("a~").unwrap();

        while !timeline_1_ops.is_empty() || !timeline_2_ops.is_empty() {
            let ops_from_timeline_2_to_timeline_1 = timeline_2_ops.drain(..).collect::<Vec<_>>();
            let ops_from_timeline_1_to_timeline_2 = timeline_1_ops.drain(..).collect::<Vec<_>>();
            timeline_1_ops.extend(timeline_1.integrate_ops::<FakeFileSystem<StdRng>, _, _>(
                &ops_from_timeline_2_to_timeline_1,
                None,
                &ctx_1,
            ));
            timeline_2_ops.extend(timeline_2.integrate_ops::<FakeFileSystem<StdRng>, _, _>(
                &ops_from_timeline_1_to_timeline_2,
                None,
                &ctx_2,
            ));
        }

        assert_eq!(timeline_1.paths_with_ids(), timeline_2.paths_with_ids());
        assert_eq!(timeline_1.paths(), ["a/", "a~/", "a~~/"]);
        assert_eq!(timeline_1.id_for_path("a").unwrap(), id_2);
        assert_eq!(timeline_1.id_for_path("a~").unwrap(), id_3);
        assert_eq!(timeline_1.id_for_path("a~~").unwrap(), id_1);
    }

    #[test]
    fn test_cycle_fixups() {
        let ctx_1 = TestContext::new(1);
        let mut timeline_1 = Timeline::new();
        timeline_1.create_dir_all("a", &ctx_1).unwrap();
        timeline_1.create_dir_all("b", &ctx_1).unwrap();
        let mut timeline_1_ops = Vec::new();

        let ctx_2 = TestContext::new(2);
        let mut timeline_2 = timeline_1.clone();
        let mut timeline_2_ops = Vec::new();

        timeline_1_ops.push(timeline_1.rename("a", "b/a", &ctx_1).unwrap());
        timeline_2_ops.push(timeline_2.rename("b", "a/b", &ctx_1).unwrap());
        while !timeline_1_ops.is_empty() || !timeline_2_ops.is_empty() {
            let ops_from_timeline_2_to_timeline_1 = timeline_2_ops.drain(..).collect::<Vec<_>>();
            let ops_from_timeline_1_to_timeline_2 = timeline_1_ops.drain(..).collect::<Vec<_>>();
            timeline_1_ops.extend(timeline_1.integrate_ops::<FakeFileSystem<StdRng>, _, _>(
                &ops_from_timeline_2_to_timeline_1,
                None,
                &ctx_1,
            ));
            timeline_2_ops.extend(timeline_2.integrate_ops::<FakeFileSystem<StdRng>, _, _>(
                &ops_from_timeline_1_to_timeline_2,
                None,
                &ctx_2,
            ));
        }

        assert_eq!(timeline_1.paths_with_ids(), timeline_2.paths_with_ids());
        assert_eq!(timeline_1.paths(), ["b/", "b/a/"]);
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

            let ctx = Vec::from_iter((0..PEERS).map(|i| TestContext::new(i as u64 + 1)));
            let mut fs = Vec::from_iter((0..PEERS).map(|i| FakeFileSystem::new(&ctx[i], rng)));
            let mut prev_fs_versions = Vec::from_iter((0..PEERS).map(|_| 0));
            let mut timelines = Vec::from_iter((0..PEERS).map(|_| Timeline::new()));
            let mut inboxes = Vec::from_iter((0..PEERS).map(|_| Vec::new()));

            // Generate and deliver random mutations
            for _ in 0..5 {
                let replica_index = rng.gen_range(0, PEERS);
                let ctx = &ctx[replica_index];
                let fs = &mut fs[replica_index];
                let timeline = &mut timelines[replica_index];

                if !inboxes[replica_index].is_empty() && rng.gen() {
                    let ops = mem::replace(&mut inboxes[replica_index], Vec::new());
                    let fixup_ops = timeline.integrate_ops(&ops, Some(fs), ctx);
                    deliver_ops(replica_index, &mut inboxes, fixup_ops);
                } else {
                    if prev_fs_versions[replica_index] == fs.version() || rng.gen() {
                        fs.mutate(rng.gen_range(1, 5));
                    }

                    prev_fs_versions[replica_index] = fs.version();
                    let ops = timeline.read_from_fs(fs.entries(), ctx);
                    deliver_ops(replica_index, &mut inboxes, ops);
                }
            }

            // Allow system to quiesce
            loop {
                let mut done = true;
                for replica_index in 0..PEERS {
                    let ctx = &ctx[replica_index];
                    let fs = &mut fs[replica_index];
                    let timeline = &mut timelines[replica_index];

                    if prev_fs_versions[replica_index] < fs.version() {
                        prev_fs_versions[replica_index] = fs.version();
                        let ops = timeline.read_from_fs(fs.entries(), ctx);
                        deliver_ops(replica_index, &mut inboxes, ops);
                        done = false;
                    }

                    let ops = mem::replace(&mut inboxes[replica_index], Vec::new());
                    if !ops.is_empty() {
                        let fixup_ops = timeline.integrate_ops(&ops, Some(fs), ctx);
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
                    timelines[i].paths_with_ids(),
                    timelines[i + 1].paths_with_ids()
                );
            }

            // Ensure all timelines match their underlying file system
            for i in 0..PEERS {
                assert_eq!(timelines[i].paths(), fs[i].paths());
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
        ctx: &'a TestContext,
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

    struct TestContext {
        next_id: Cell<time::Local>,
        lamport_clock: Cell<time::Lamport>,
    }

    impl<'a, T: Rng + Clone> FakeFileSystem<'a, T> {
        fn new(ctx: &'a TestContext, rng: T) -> Self {
            FakeFileSystem(Rc::new(RefCell::new(FakeFileSystemState::new(ctx, rng))))
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

        fn create_file(&mut self, path: &Path) -> bool {
            self.0.borrow_mut().create_file(path)
        }

        fn create_dir(&mut self, path: &Path) -> bool {
            self.0.borrow_mut().create_dir(path)
        }

        fn hard_link(&mut self, src: &Path, dst: &Path) -> bool {
            self.0.borrow_mut().hard_link(src, dst)
        }

        fn remove(&mut self, path: &Path, is_dir: bool) -> bool {
            self.0.borrow_mut().remove(path, is_dir)
        }

        fn rename(&mut self, from: &Path, to: &Path) -> bool {
            self.0.borrow_mut().rename(from, to)
        }

        fn inode(&self, path: &Path) -> Option<Inode> {
            self.0.borrow_mut().inode(path)
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
        fn new(ctx: &'a TestContext, rng: T) -> Self {
            let timeline = Timeline::new();
            Self {
                timeline,
                next_inode: 0,
                ctx,
                rng,
                version: 0,
            }
        }

        fn mutate(&mut self, count: usize) {
            self.version += 1;
            self.timeline
                .mutate(&mut self.rng, count, &mut self.next_inode, self.ctx);
        }

        fn paths(&self) -> Vec<String> {
            self.timeline.paths()
        }

        fn create_file(&mut self, path: &Path) -> bool {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before create_file");
                self.mutate(1);
            } else {
                self.version += 1;
            }

            // println!("FileSystem: create file {:?}", path);
            let inode = self.next_inode;
            self.next_inode += 1;
            self.timeline
                .create_file_internal(path, false, Some(inode), self.ctx)
                .is_ok()
        }

        fn create_dir(&mut self, path: &Path) -> bool {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before create_dir");
                self.mutate(1);
            } else {
                self.version += 1;
            }

            // println!("FileSystem: create dir {:?}", path);
            let inode = self.next_inode;
            self.next_inode += 1;
            self.timeline
                .create_file_internal(path, true, Some(inode), self.ctx)
                .is_ok()
        }

        fn hard_link(&mut self, src: &Path, dst: &Path) -> bool {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before hard_link");
                self.mutate(1);
            } else {
                self.version += 1;
            }

            // println!("FileSystem: hard link {:?} to {:?}", src, dst);
            self.timeline.hard_link(src, dst, self.ctx).is_ok()
        }

        fn remove(&mut self, path: &Path, is_dir: bool) -> bool {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before remove");
                self.mutate(1);
            } else {
                self.version += 1;
            }

            // println!("FileSystem: remove {:?}", path);
            if let Some(child_id) = self.timeline.id_for_path(path) {
                let metadata = self.timeline.metadata(child_id).unwrap();
                is_dir == metadata.is_dir && self.timeline.remove(path, self.ctx).is_ok()
            } else {
                false
            }
        }

        fn rename(&mut self, from: &Path, to: &Path) -> bool {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before rename");
                self.mutate(1);
            } else {
                self.version += 1;
            }

            // println!("FileSystem: move from {:?} to {:?}", from, to);
            !to.starts_with(from) && self.timeline.rename(from, to, self.ctx).is_ok()
        }

        fn inode(&mut self, path: &Path) -> Option<Inode> {
            if self.rng.gen_weighted_bool(10) {
                // println!("mutate before inode");
                self.mutate(1);
            }

            self.timeline.id_for_path(path).map(|id| {
                let mut cursor = self.timeline.metadata.cursor();
                cursor.seek(&id, SeekBias::Left);
                cursor.item().unwrap().inode.unwrap()
            })
        }
    }

    impl<'a, T: Rng + Clone> FakeFileSystemIter<'a, T> {
        fn build_cursor(&self) -> Cursor {
            let state = self.state.borrow();
            let mut new_cursor = state.timeline.cursor();
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
                        new_cursor.next();
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
                self.cursor.as_mut().unwrap().next();
            }

            let cursor = self.cursor.as_mut().unwrap();

            let depth = cursor.depth();
            if depth == 0 {
                None
            } else {
                let name = cursor.name().unwrap();
                let Metadata { is_dir, inode, .. } = cursor.metadata().unwrap();
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

    impl TestContext {
        fn new(replica_id: ReplicaId) -> Self {
            Self {
                next_id: Cell::new(time::Local::new(replica_id)),
                lamport_clock: Cell::new(time::Lamport::new(replica_id)),
            }
        }
    }

    impl ReplicaContext for TestContext {
        fn replica_id(&self) -> ReplicaId {
            self.lamport_clock.get().replica_id
        }

        fn local_time(&self) -> time::Local {
            let next_id = self.next_id.get();
            self.next_id.replace(next_id.next());
            next_id
        }

        fn lamport_time(&self) -> time::Lamport {
            self.lamport_clock.replace(self.lamport_clock.get().inc());
            self.lamport_clock.get()
        }

        fn observe_lamport_timestamp(&self, timestamp: time::Lamport) {
            self.lamport_clock
                .set(self.lamport_clock.get().update(timestamp));
        }
    }

    impl Timeline {
        fn mutate<R, T: Rng>(
            &mut self,
            rng: &mut T,
            count: usize,
            next_inode: &mut Inode,
            ctx: &R,
        ) -> Vec<Operation>
        where
            R: ReplicaContext,
        {
            let mut ops = Vec::new();
            for _ in 0..count {
                let k = rng.gen_range(0, 3);
                if self.is_empty() || k == 0 {
                    let subtree_depth = rng.gen_range(1, 5);
                    let path = self.gen_path(rng, subtree_depth);

                    if rng.gen() {
                        // println!("Random mutation: Inserting dirs {:?}", path);
                        ops.extend(
                            self.create_dir_all_internal(&path, &mut Some(next_inode), ctx)
                                .unwrap(),
                        );
                    } else {
                        if let Some(parent_path) = path.parent() {
                            ops.extend(
                                self.create_dir_all_internal(
                                    parent_path,
                                    &mut Some(next_inode),
                                    ctx,
                                ).unwrap(),
                            );
                        }

                        // TODO: Maybe use a more efficient way to select a random file.
                        let mut existing_file_paths = Vec::new();
                        let mut cursor = self.cursor();
                        while cursor.path().is_some() {
                            if !cursor.is_dir().unwrap() {
                                existing_file_paths.push(cursor.path().unwrap().to_path_buf());
                            }
                            cursor.next();
                        }

                        if rng.gen() && !existing_file_paths.is_empty() {
                            let src = rng.choose(&existing_file_paths).unwrap();
                            // println!(
                            //     "Random mutation: Inserting hard link {:?} <-- {:?}",
                            //     src, path
                            // );
                            ops.push(self.hard_link(src, &path, ctx).unwrap());
                        } else {
                            // println!("Random mutation: Create file {:?}", path);
                            let inode = *next_inode;
                            *next_inode += 1;
                            ops.extend(
                                self.create_file_internal(&path, false, Some(inode), ctx)
                                    .unwrap(),
                            );
                        }
                    }
                } else if k == 1 {
                    let path = self.select_path(rng, false).unwrap();
                    // println!("Random mutation: Removing {:?}", path);
                    ops.push(self.remove(&path, ctx).unwrap());
                } else {
                    let (old_path, new_path) = loop {
                        let old_path = self.select_path(rng, false).unwrap();
                        let new_path = self.gen_path(rng, 1);
                        if !new_path.starts_with(&old_path) {
                            break (old_path, new_path);
                        }
                    };

                    // println!("Random mutation: Moving {:?} to {:?}", old_path, new_path);
                    ops.push(self.rename(&old_path, &new_path, ctx).unwrap());
                }
            }
            ops
        }

        fn gen_path<T: Rng>(&self, rng: &mut T, depth: usize) -> PathBuf {
            loop {
                let mut new_path = PathBuf::new();
                for _ in 0..depth {
                    new_path.push(gen_name(rng));
                }

                let path = if self.is_empty() || rng.gen_weighted_bool(8) {
                    new_path
                } else {
                    let mut prefix = self.select_path(rng, true).unwrap();
                    prefix.push(new_path);
                    prefix
                };

                if self.id_for_path(&path).is_none() {
                    return path;
                }
            }
        }

        fn select_path<T: Rng>(&self, rng: &mut T, select_dir: bool) -> Option<PathBuf> {
            if self.is_empty() {
                None
            } else {
                let mut depth = 0;
                let mut path = PathBuf::new();
                let mut cursor = self.child_refs.cursor();
                let mut parent_id = ROOT_ID;

                loop {
                    cursor.seek(&parent_id, SeekBias::Left);
                    let mut child_refs = Vec::new();
                    while let Some(child_ref) = cursor.item() {
                        if child_ref.parent_id == parent_id {
                            let child_id = child_ref.parent_ref_id.child_id;
                            if child_ref.is_visible()
                                && (!select_dir || self.metadata(child_id).unwrap().is_dir)
                            {
                                child_refs.push(child_ref);
                            }
                            let next_visible_index = cursor.end::<usize>() + 1;
                            cursor.seek_forward(&next_visible_index, SeekBias::Left);
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
