use btree::{self, SeekBias};
use buffer::{self, Buffer, Point, Text};
use operation_queue::{self, OperationQueue};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::{Add, AddAssign, Range};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use time;
use ReplicaId;

pub const ROOT_FILE_ID: FileId = FileId::Base(0);

#[derive(Clone)]
pub struct WorkTree {
    base_entries_next_id: u64,
    base_entries_stack: Vec<FileId>,
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRefValue>,
    child_refs: btree::Tree<ChildRefValue>,
    version: time::Global,
    local_clock: time::Local,
    lamport_clock: time::Lamport,
    text_files: HashMap<FileId, TextFile>,
    deferred_ops: OperationQueue<Operation>,
}

pub struct Cursor {
    metadata_cursor: btree::Cursor<Metadata>,
    parent_ref_cursor: btree::Cursor<ParentRefValue>,
    child_ref_cursor: btree::Cursor<ChildRefValue>,
    stack: Vec<CursorStackEntry>,
    path: PathBuf,
}

struct CursorStackEntry {
    cursor: btree::Cursor<ChildRefValue>,
    visible: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct CursorEntry {
    pub file_id: FileId,
    pub file_type: FileType,
    pub depth: usize,
    pub name: Arc<OsString>,
    pub status: FileStatus,
    pub visible: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DirEntry {
    pub depth: usize,
    #[serde(
        serialize_with = "serialize_os_string",
        deserialize_with = "deserialize_os_string"
    )]
    pub name: OsString,
    #[serde(rename = "type")]
    pub file_type: FileType,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Operation {
    InsertMetadata {
        file_id: FileId,
        file_type: FileType,
        #[serde(
            serialize_with = "serialize_parent",
            deserialize_with = "deserialize_parent"
        )]
        parent: Option<(FileId, Arc<OsString>)>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    },
    UpdateParent {
        child_id: FileId,
        #[serde(
            serialize_with = "serialize_parent",
            deserialize_with = "deserialize_parent"
        )]
        new_parent: Option<(FileId, Arc<OsString>)>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    },
    EditText {
        file_id: FileId,
        edits: Vec<buffer::Operation>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidPath,
    InvalidFileId,
    InvalidBufferId,
    InvalidDirEntry,
    InvalidOperation,
    CursorExhausted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum FileId {
    Base(u64),
    New(time::Local),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BufferId(FileId);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum FileStatus {
    New,
    Renamed,
    Removed,
    Modified,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FileType {
    Directory,
    Text,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Metadata {
    file_id: FileId,
    file_type: FileType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParentRefValue {
    child_id: FileId,
    timestamp: time::Lamport,
    parent: Option<(FileId, Arc<OsString>)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ParentRefValueKey {
    child_id: FileId,
    timestamp: time::Lamport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChildRefValue {
    parent_id: FileId,
    name: Arc<OsString>,
    timestamp: time::Lamport,
    child_id: FileId,
    visible: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefValueSummary {
    parent_id: FileId,
    name: Arc<OsString>,
    visible: bool,
    timestamp: time::Lamport,
    visible_count: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ChildRefValueKey {
    parent_id: FileId,
    name: Arc<OsString>,
    visible: bool,
    timestamp: time::Lamport,
}

#[derive(Clone, Debug, Default, Ord, Eq, PartialEq, PartialOrd)]
pub struct ChildRefKey {
    parent_id: FileId,
    name: Arc<OsString>,
}

#[derive(Clone)]
enum TextFile {
    Deferred(Vec<buffer::Operation>),
    Buffered(Buffer),
}

impl WorkTree {
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            base_entries_next_id: 1,
            base_entries_stack: Vec::new(),
            metadata: btree::Tree::new(),
            parent_refs: btree::Tree::new(),
            child_refs: btree::Tree::new(),
            version: time::Global::new(),
            local_clock: time::Local::new(replica_id),
            lamport_clock: time::Lamport::new(replica_id),
            text_files: HashMap::new(),
            deferred_ops: OperationQueue::new(),
        }
    }

    pub fn version(&self) -> time::Global {
        self.version.clone()
    }

    pub fn cursor(&self) -> Option<Cursor> {
        let metadata_cursor = self.metadata.cursor();
        let parent_ref_cursor = self.parent_refs.cursor();
        let child_ref_cursor = self.child_refs.cursor();
        let mut cursor = Cursor {
            metadata_cursor,
            parent_ref_cursor,
            child_ref_cursor,
            stack: Vec::new(),
            path: PathBuf::new(),
        };
        if cursor.descend_into(true, ROOT_FILE_ID) {
            Some(cursor)
        } else {
            None
        }
    }

    pub fn append_base_entries<I>(&mut self, entries: I) -> Result<Vec<Operation>, Error>
    where
        I: IntoIterator<Item = DirEntry>,
    {
        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        let mut child_ref_cursor = self.child_refs.cursor();
        let mut name_conflicts = HashSet::new();

        for entry in entries {
            let stack_depth = self.base_entries_stack.len();
            if entry.depth == 0 || entry.depth > stack_depth + 1 {
                return Err(Error::InvalidDirEntry);
            }
            self.base_entries_stack.truncate(entry.depth - 1);

            let parent_id = self
                .base_entries_stack
                .last()
                .cloned()
                .unwrap_or(ROOT_FILE_ID);
            let name = Arc::new(entry.name);
            let file_id = FileId::Base(self.base_entries_next_id);
            metadata_edits.push(btree::Edit::Insert(Metadata {
                file_id,
                file_type: entry.file_type,
            }));
            parent_ref_edits.push(btree::Edit::Insert(ParentRefValue {
                child_id: file_id,
                timestamp: time::Lamport::min_value(),
                parent: Some((parent_id, name.clone())),
            }));
            child_ref_edits.push(btree::Edit::Insert(ChildRefValue {
                parent_id,
                name: name.clone(),
                timestamp: time::Lamport::min_value(),
                child_id: file_id,
                visible: true,
            }));

            // In the rare case we already have a child ref with this name, remember to fix the
            // name conflict later.
            if child_ref_cursor.seek(&ChildRefKey { parent_id, name }, SeekBias::Left) {
                name_conflicts.insert(file_id);
            }

            self.base_entries_next_id += 1;
            if entry.file_type == FileType::Directory {
                self.base_entries_stack.push(file_id);
            }
        }

        self.metadata.edit(&mut metadata_edits);
        self.parent_refs.edit(&mut parent_ref_edits);
        self.child_refs.edit(&mut child_ref_edits);

        let mut fixup_ops = Vec::new();
        for file_id in name_conflicts {
            fixup_ops.extend(self.fix_name_conflicts(file_id));
        }
        let deferred_ops = self.deferred_ops.drain();
        fixup_ops.extend(self.apply_ops_internal(deferred_ops)?);

        Ok(fixup_ops)
    }

    pub fn apply_ops<I>(&mut self, ops: I) -> Result<Vec<Operation>, Error>
    where
        I: IntoIterator<Item = Operation>,
    {
        let mut fixup_ops = Vec::new();
        fixup_ops.extend(self.apply_ops_internal(ops)?);
        let deferred_ops = self.deferred_ops.drain();
        fixup_ops.extend(self.apply_ops_internal(deferred_ops)?);
        Ok(fixup_ops)
    }

    fn apply_ops_internal<I>(&mut self, ops: I) -> Result<Vec<Operation>, Error>
    where
        I: IntoIterator<Item = Operation>,
    {
        let mut ops = ops.into_iter().peekable();
        if ops.peek().is_none() {
            return Ok(Vec::new());
        }

        let mut new_tree = self.clone();
        let mut deferred_ops = Vec::new();
        let mut potential_conflicts = HashSet::new();

        for op in ops {
            if new_tree.can_apply_op(&op) {
                match &op {
                    Operation::InsertMetadata {
                        file_id, parent, ..
                    } => {
                        if parent.is_some() {
                            potential_conflicts.insert(*file_id);
                        }
                    }
                    Operation::UpdateParent { child_id, .. } => {
                        potential_conflicts.insert(*child_id);
                    }
                    _ => {}
                }
                new_tree.apply_op(op)?;
            } else {
                deferred_ops.push(op);
            }
        }
        new_tree.deferred_ops.insert(deferred_ops);

        let mut fixup_ops = Vec::new();
        for file_id in &potential_conflicts {
            fixup_ops.extend(new_tree.fix_conflicts(*file_id));
        }

        *self = new_tree;
        Ok(fixup_ops)
    }

    pub fn apply_op(&mut self, op: Operation) -> Result<(), Error> {
        self.version.observe(op.local_timestamp());
        self.lamport_clock.observe(op.lamport_timestamp());

        match op {
            Operation::InsertMetadata {
                file_id,
                file_type,
                parent,
                lamport_timestamp,
                ..
            } => {
                self.metadata.insert(Metadata { file_id, file_type });
                if let Some((parent_id, name)) = parent {
                    self.parent_refs.insert(ParentRefValue {
                        child_id: file_id,
                        parent: Some((parent_id, name.clone())),
                        timestamp: lamport_timestamp,
                    });
                    self.child_refs.insert(ChildRefValue {
                        parent_id,
                        name,
                        timestamp: lamport_timestamp,
                        child_id: file_id,
                        visible: true,
                    });
                }
            }
            Operation::UpdateParent {
                child_id,
                new_parent,
                lamport_timestamp,
                ..
            } => {
                let mut child_ref_edits: SmallVec<[_; 3]> = SmallVec::new();

                let mut parent_ref_cursor = self.parent_refs.cursor();
                if parent_ref_cursor.seek(&child_id, SeekBias::Left) {
                    let latest_parent_ref = parent_ref_cursor.item().unwrap();
                    let mut latest_visible_parent_ref = None;
                    while let Some(parent_ref) = parent_ref_cursor.item() {
                        if parent_ref.child_id != child_id {
                            break;
                        } else if parent_ref.parent.is_some() {
                            latest_visible_parent_ref = Some(parent_ref);
                            break;
                        } else {
                            parent_ref_cursor.next();
                        }
                    }

                    let mut child_ref = None;
                    if let Some(ref latest_visible_parent_ref) = latest_visible_parent_ref {
                        let mut child_ref_cursor = self.child_refs.cursor();
                        let (parent_id, name) = latest_visible_parent_ref.parent.clone().unwrap();
                        child_ref_cursor.seek(
                            &ChildRefValueKey {
                                parent_id,
                                name,
                                visible: latest_parent_ref.parent.is_some(),
                                timestamp: latest_visible_parent_ref.timestamp,
                            },
                            SeekBias::Left,
                        );
                        child_ref = child_ref_cursor.item();
                    }

                    if lamport_timestamp > latest_parent_ref.timestamp {
                        if let Some(ref child_ref) = child_ref {
                            child_ref_edits.push(btree::Edit::Remove(child_ref.clone()));
                        }

                        if let Some((parent_id, name)) = new_parent.clone() {
                            child_ref_edits.push(btree::Edit::Insert(ChildRefValue {
                                parent_id,
                                name,
                                timestamp: lamport_timestamp,
                                child_id,
                                visible: true,
                            }));
                        } else if let Some(mut child_ref) = child_ref {
                            child_ref.visible = false;
                            child_ref_edits.push(btree::Edit::Insert(child_ref));
                        }
                    } else if latest_visible_parent_ref
                        .map_or(true, |r| lamport_timestamp > r.timestamp)
                        && latest_parent_ref.parent.is_none()
                        && new_parent.is_some()
                    {
                        let (parent_id, name) = new_parent.clone().unwrap();
                        if let Some(child_ref) = child_ref {
                            child_ref_edits.push(btree::Edit::Remove(child_ref.clone()));
                        }
                        child_ref_edits.push(btree::Edit::Insert(ChildRefValue {
                            parent_id,
                            name,
                            timestamp: lamport_timestamp,
                            child_id,
                            visible: false,
                        }));
                    }
                } else if let Some((parent_id, name)) = new_parent.clone() {
                    child_ref_edits.push(btree::Edit::Insert(ChildRefValue {
                        parent_id,
                        name,
                        timestamp: lamport_timestamp,
                        child_id,
                        visible: true,
                    }));
                }

                self.parent_refs.insert(ParentRefValue {
                    child_id,
                    timestamp: lamport_timestamp,
                    parent: new_parent,
                });
                self.child_refs.edit(&mut child_ref_edits);
            }
            Operation::EditText { file_id, edits, .. } => match self
                .text_files
                .entry(file_id)
                .or_insert_with(|| TextFile::Deferred(Vec::new()))
            {
                TextFile::Deferred(operations) => {
                    operations.extend(edits);
                }
                TextFile::Buffered(buffer) => {
                    buffer
                        .apply_ops(edits, &mut self.lamport_clock)
                        .map_err(|_| Error::InvalidOperation)?;
                }
            },
        }

        Ok(())
    }

    fn can_apply_op(&self, op: &Operation) -> bool {
        match op {
            Operation::InsertMetadata { .. } => true,
            Operation::UpdateParent { child_id, .. } => self.metadata(*child_id).is_ok(),
            Operation::EditText { file_id, .. } => self.metadata(*file_id).is_ok(),
        }
    }

    pub fn new_text_file(&mut self) -> (FileId, Operation) {
        let file_id = FileId::New(self.local_clock.tick());
        let operation = Operation::InsertMetadata {
            file_id,
            file_type: FileType::Text,
            parent: None,
            local_timestamp: self.local_clock.tick(),
            lamport_timestamp: self.lamport_clock.tick(),
        };
        self.apply_op(operation.clone()).unwrap();
        (file_id, operation)
    }

    pub fn create_dir<N>(
        &mut self,
        parent_id: FileId,
        name: N,
    ) -> Result<(FileId, Operation), Error>
    where
        N: AsRef<OsStr>,
    {
        self.check_file_id(parent_id, Some(FileType::Directory))?;

        let mut new_tree = self.clone();
        let file_id = FileId::New(new_tree.local_clock.tick());
        let operation = Operation::InsertMetadata {
            file_id,
            file_type: FileType::Directory,
            parent: Some((parent_id, Arc::new(name.as_ref().into()))),
            local_timestamp: new_tree.local_clock.tick(),
            lamport_timestamp: new_tree.lamport_clock.tick(),
        };
        let fixup_ops = new_tree
            .apply_ops_internal(Some(operation.clone()))
            .unwrap();
        if fixup_ops.is_empty() {
            *self = new_tree;
            Ok((file_id, operation))
        } else {
            Err(Error::InvalidOperation)
        }
    }

    pub fn open_text_file<T>(&mut self, file_id: FileId, base_text: T) -> Result<BufferId, Error>
    where
        T: Into<Text>,
    {
        self.check_file_id(file_id, Some(FileType::Text))?;

        match self.text_files.remove(&file_id) {
            Some(TextFile::Deferred(operations)) => {
                let mut buffer = Buffer::new(base_text);
                buffer
                    .apply_ops(operations, &mut self.lamport_clock)
                    .map_err(|_| Error::InvalidOperation)?;
                self.text_files.insert(file_id, TextFile::Buffered(buffer));
            }
            Some(text_file) => {
                self.text_files.insert(file_id, text_file);
            }
            None => {
                self.text_files
                    .insert(file_id, TextFile::Buffered(Buffer::new(base_text)));
            }
        }

        Ok(BufferId(file_id))
    }

    pub fn rename<N>(
        &mut self,
        file_id: FileId,
        new_parent_id: FileId,
        new_name: N,
    ) -> Result<Operation, Error>
    where
        N: AsRef<OsStr>,
    {
        self.check_file_id(file_id, None)?;
        self.check_file_id(new_parent_id, Some(FileType::Directory))?;

        let mut new_tree = self.clone();
        let operation = Operation::UpdateParent {
            child_id: file_id,
            new_parent: Some((new_parent_id, Arc::new(new_name.as_ref().into()))),
            local_timestamp: new_tree.local_clock.tick(),
            lamport_timestamp: new_tree.lamport_clock.tick(),
        };
        let fixup_ops = new_tree
            .apply_ops_internal(Some(operation.clone()))
            .unwrap();
        if fixup_ops.is_empty() {
            *self = new_tree;
            Ok(operation)
        } else {
            Err(Error::InvalidOperation)
        }
    }

    pub fn remove(&mut self, file_id: FileId) -> Result<Operation, Error> {
        self.check_file_id(file_id, None)?;

        let operation = Operation::UpdateParent {
            child_id: file_id,
            new_parent: None,
            local_timestamp: self.local_clock.tick(),
            lamport_timestamp: self.lamport_clock.tick(),
        };
        self.apply_op(operation.clone()).unwrap();
        Ok(operation)
    }

    pub fn edit<I, T>(
        &mut self,
        buffer_id: BufferId,
        old_ranges: I,
        new_text: T,
    ) -> Result<Operation, Error>
    where
        I: IntoIterator<Item = Range<usize>>,
        T: Into<Text>,
    {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get_mut(&buffer_id.0) {
            let edits = buffer.edit(
                old_ranges,
                new_text,
                &mut self.local_clock,
                &mut self.lamport_clock,
            );
            let local_timestamp = self.local_clock.tick();
            self.version.observe(local_timestamp);
            Ok(Operation::EditText {
                file_id: buffer_id.0,
                edits,
                local_timestamp,
                lamport_timestamp: self.lamport_clock.tick(),
            })
        } else {
            Err(Error::InvalidBufferId)
        }
    }

    pub fn edit_2d<I, T>(
        &mut self,
        buffer_id: BufferId,
        old_ranges: I,
        new_text: T,
    ) -> Result<Operation, Error>
    where
        I: IntoIterator<Item = Range<Point>>,
        T: Into<Text>,
    {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get_mut(&buffer_id.0) {
            let edits = buffer.edit_2d(
                old_ranges,
                new_text,
                &mut self.local_clock,
                &mut self.lamport_clock,
            );
            let local_timestamp = self.local_clock.tick();
            self.version.observe(local_timestamp);
            Ok(Operation::EditText {
                file_id: buffer_id.0,
                edits,
                local_timestamp,
                lamport_timestamp: self.lamport_clock.tick(),
            })
        } else {
            Err(Error::InvalidBufferId)
        }
    }

    pub fn file_id<P>(&self, path: P) -> Result<FileId, Error>
    where
        P: AsRef<Path>,
    {
        let mut cursor = self.child_refs.cursor();
        let mut parent_id = ROOT_FILE_ID;
        for component in path.as_ref().components() {
            match component {
                Component::Normal(name) => {
                    let name = Arc::new(name.into());
                    if cursor.seek(&ChildRefKey { parent_id, name }, SeekBias::Left) {
                        let child_ref = cursor.item().unwrap();
                        if child_ref.visible {
                            parent_id = child_ref.child_id;
                        } else {
                            return Err(Error::InvalidPath);
                        }
                    } else {
                        return Err(Error::InvalidPath);
                    }
                }
                _ => return Err(Error::InvalidPath),
            }
        }

        Ok(parent_id)
    }

    pub fn path(&self, file_id: FileId) -> Result<PathBuf, Error> {
        let mut path_components = Vec::new();
        if self.visit_ancestors(file_id, |name| path_components.push(name)) {
            let mut path = PathBuf::new();
            for component in path_components.into_iter().rev() {
                path.push(component.as_ref());
            }
            Ok(path)
        } else {
            Err(Error::InvalidPath)
        }
    }

    pub fn text(&self, buffer_id: BufferId) -> Result<buffer::Iter, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&buffer_id.0) {
            Ok(buffer.iter())
        } else {
            Err(Error::InvalidBufferId)
        }
    }

    pub fn changes_since(
        &self,
        buffer_id: BufferId,
        version: time::Global,
    ) -> Result<impl Iterator<Item = buffer::Change>, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&buffer_id.0) {
            Ok(buffer.changes_since(version))
        } else {
            Err(Error::InvalidBufferId)
        }
    }

    fn metadata(&self, file_id: FileId) -> Result<Metadata, Error> {
        if file_id == ROOT_FILE_ID {
            Ok(Metadata {
                file_id: ROOT_FILE_ID,
                file_type: FileType::Directory,
            })
        } else {
            let mut cursor = self.metadata.cursor();
            if cursor.seek(&file_id, SeekBias::Left) {
                Ok(cursor.item().unwrap())
            } else {
                Err(Error::InvalidFileId)
            }
        }
    }

    fn check_file_id(&self, file_id: FileId, expected_type: Option<FileType>) -> Result<(), Error> {
        let metadata = self.metadata(file_id)?;
        if expected_type.map_or(true, |expected_type| expected_type == metadata.file_type) {
            Ok(())
        } else {
            Err(Error::InvalidFileId)
        }
    }

    fn visit_ancestors<F>(&self, file_id: FileId, mut f: F) -> bool
    where
        F: FnMut(Arc<OsString>),
    {
        let mut visited = HashSet::new();
        let mut cursor = self.parent_refs.cursor();
        if file_id == ROOT_FILE_ID {
            true
        } else if cursor.seek(&file_id, SeekBias::Left) {
            loop {
                if let Some((parent_id, name)) = cursor.item().and_then(|r| r.parent) {
                    // TODO: Only check for cycles in debug mode
                    if visited.contains(&parent_id) {
                        panic!("Cycle detected when visiting ancestors");
                    } else {
                        visited.insert(parent_id);
                    }

                    f(name);
                    if parent_id == ROOT_FILE_ID {
                        break;
                    } else if !cursor.seek(&parent_id, SeekBias::Left) {
                        return false;
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

    fn fix_conflicts(&mut self, file_id: FileId) -> Vec<Operation> {
        use btree::KeyedItem;

        let mut fixup_ops = Vec::new();
        let mut reverted_moves: HashMap<FileId, time::Lamport> = HashMap::new();

        // TODO: Only check for cycles if the child was moved and is a directory.
        let mut visited = HashSet::new();
        let mut latest_move: Option<ParentRefValue> = None;
        let mut cursor = self.parent_refs.cursor();
        cursor.seek(&file_id, SeekBias::Left);

        loop {
            let mut parent_ref = cursor.item().unwrap();
            if visited.contains(&parent_ref.child_id) {
                // Cycle detected. Revert the most recent move contributing to the cycle.
                cursor.seek(&latest_move.as_ref().unwrap().key(), SeekBias::Right);

                // Find the previous value for this parent ref that isn't a deletion and store
                // its timestamp in our reverted_moves map.
                loop {
                    let parent_ref = cursor.item().unwrap();
                    if parent_ref.parent.is_some() {
                        reverted_moves.insert(parent_ref.child_id, parent_ref.timestamp);
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
                visited.insert(parent_ref.child_id);

                // If we have already reverted this parent ref to a previous value, interpret
                // it as having the value we reverted to.
                if let Some(prev_timestamp) = reverted_moves.get(&parent_ref.child_id) {
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
                        next_parent_ref.child_id == parent_ref.child_id
                    }) {
                        latest_move = Some(parent_ref.clone());
                    }
                }

                // Walk up to the next parent or break if none exists or the parent is the root
                if let Some((parent_id, _)) = parent_ref.parent {
                    if parent_id == ROOT_FILE_ID {
                        break;
                    } else {
                        if !cursor.seek(&parent_id, SeekBias::Left) {
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        }

        // Convert the reverted moves into new move operations.
        let mut moved_file_ids = Vec::new();
        for (child_id, timestamp) in &reverted_moves {
            cursor.seek(
                &ParentRefValueKey {
                    child_id: *child_id,
                    timestamp: *timestamp,
                },
                SeekBias::Left,
            );
            fixup_ops.push(Operation::UpdateParent {
                child_id: *child_id,
                new_parent: cursor.item().unwrap().parent,
                local_timestamp: self.local_clock.tick(),
                lamport_timestamp: self.lamport_clock.tick(),
            });
            moved_file_ids.push(*child_id);
        }

        for op in &fixup_ops {
            self.apply_op(op.clone()).unwrap();
        }
        for file_id in moved_file_ids {
            fixup_ops.extend(self.fix_name_conflicts(file_id));
        }

        if !reverted_moves.contains_key(&file_id) {
            fixup_ops.extend(self.fix_name_conflicts(file_id));
        }

        fixup_ops
    }

    fn fix_name_conflicts(&mut self, file_id: FileId) -> Vec<Operation> {
        let mut fixup_ops = Vec::new();

        let mut parent_ref_cursor = self.parent_refs.cursor();
        parent_ref_cursor.seek(&file_id, SeekBias::Left);
        if let Some((parent_id, name)) = parent_ref_cursor.item().unwrap().parent {
            let mut cursor_1 = self.child_refs.cursor();
            cursor_1.seek(
                &ChildRefKey {
                    parent_id,
                    name: name.clone(),
                },
                SeekBias::Left,
            );
            cursor_1.next();

            let mut cursor_2 = cursor_1.clone();
            let mut unique_name = name.clone();

            while let Some(child_ref) = cursor_1.item() {
                if child_ref.visible && child_ref.parent_id == parent_id && child_ref.name == name {
                    loop {
                        Arc::make_mut(&mut unique_name).push("~");
                        cursor_2.seek_forward(
                            &ChildRefKey {
                                parent_id,
                                name: unique_name.clone(),
                            },
                            SeekBias::Left,
                        );
                        if let Some(conflicting_child_ref) = cursor_2.item() {
                            if !conflicting_child_ref.visible
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
                        child_id: file_id,
                        new_parent: Some((parent_id, unique_name.clone())),
                        local_timestamp: self.local_clock.tick(),
                        lamport_timestamp: self.lamport_clock.tick(),
                    };
                    self.apply_op(fixup_op.clone()).unwrap();
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
}

impl Cursor {
    pub fn next(&mut self, can_descend: bool) -> bool {
        if !self.stack.is_empty() {
            let entry = self.entry().unwrap();
            if !can_descend
                || entry.file_type != FileType::Directory
                || !self.descend_into(entry.visible, entry.file_id)
            {
                while !self.stack.is_empty() && !self.next_sibling() {
                    self.stack.pop();
                    self.path.pop();
                }
            }
        }

        !self.stack.is_empty()
    }

    pub fn entry(&self) -> Result<CursorEntry, Error> {
        let CursorStackEntry {
            cursor: child_ref_cursor,
            visible: parent_visible,
        } = self.stack.last().ok_or(Error::CursorExhausted)?;
        let metadata = self.metadata_cursor.item().unwrap();
        let child_ref = child_ref_cursor.item().unwrap();

        let mut parent_ref_cursor = self.parent_ref_cursor.clone();
        parent_ref_cursor.seek(&metadata.file_id, SeekBias::Left);
        let newest_parent_ref_value = parent_ref_cursor.item().unwrap();
        parent_ref_cursor.seek(&metadata.file_id, SeekBias::Right);
        parent_ref_cursor.prev();
        let oldest_parent_ref_value = parent_ref_cursor.item().unwrap();
        let (status, visible) = match metadata.file_id {
            FileId::Base(_) => {
                if newest_parent_ref_value.parent == oldest_parent_ref_value.parent {
                    (FileStatus::Unchanged, true)
                } else if newest_parent_ref_value.parent.is_some() {
                    (FileStatus::Renamed, true)
                } else {
                    (FileStatus::Removed, false)
                }
            }
            FileId::New(_) => (FileStatus::New, newest_parent_ref_value.parent.is_some()),
        };

        Ok(CursorEntry {
            file_id: metadata.file_id,
            file_type: metadata.file_type,
            name: child_ref.name,
            depth: self.stack.len(),
            status,
            visible: *parent_visible && visible,
        })
    }

    pub fn path(&self) -> Result<&Path, Error> {
        if self.stack.is_empty() {
            Err(Error::CursorExhausted)
        } else {
            Ok(&self.path)
        }
    }

    fn descend_into(&mut self, parent_visible: bool, dir_id: FileId) -> bool {
        let mut child_ref_cursor = self.child_ref_cursor.clone();
        child_ref_cursor.seek(&dir_id, SeekBias::Left);
        if let Some(child_ref) = child_ref_cursor.item() {
            if child_ref.parent_id == dir_id {
                self.stack.push(CursorStackEntry {
                    cursor: child_ref_cursor,
                    visible: parent_visible,
                });
                self.path.push(child_ref.name.as_ref());
                self.metadata_cursor
                    .seek(&child_ref.child_id, SeekBias::Left);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn next_sibling(&mut self) -> bool {
        let CursorStackEntry { cursor, .. } = self.stack.last_mut().unwrap();
        let parent_id = cursor.item().unwrap().parent_id;
        cursor.next();
        if let Some(child_ref) = cursor.item() {
            if child_ref.parent_id == parent_id {
                self.metadata_cursor
                    .seek(&child_ref.child_id, SeekBias::Left);
                self.path.pop();
                self.path.push(child_ref.name.as_ref());
                return true;
            }
        }

        false
    }
}

impl Operation {
    fn local_timestamp(&self) -> time::Local {
        match self {
            Operation::InsertMetadata {
                local_timestamp, ..
            } => *local_timestamp,
            Operation::UpdateParent {
                local_timestamp, ..
            } => *local_timestamp,
            Operation::EditText {
                local_timestamp, ..
            } => *local_timestamp,
        }
    }

    fn lamport_timestamp(&self) -> time::Lamport {
        match self {
            Operation::InsertMetadata {
                lamport_timestamp, ..
            } => *lamport_timestamp,
            Operation::UpdateParent {
                lamport_timestamp, ..
            } => *lamport_timestamp,
            Operation::EditText {
                lamport_timestamp, ..
            } => *lamport_timestamp,
        }
    }
}

impl operation_queue::Operation for Operation {
    fn timestamp(&self) -> time::Lamport {
        self.lamport_timestamp()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl btree::Dimension<FileId> for FileId {
    fn from_summary(summary: &Self) -> Self {
        *summary
    }
}

impl Default for FileId {
    fn default() -> Self {
        FileId::Base(0)
    }
}

impl<'a> AddAssign<&'a Self> for FileId {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self <= *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for FileId {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self <= *other);
        other.clone()
    }
}

impl btree::Item for Metadata {
    type Summary = FileId;

    fn summarize(&self) -> Self::Summary {
        use btree::KeyedItem;
        self.key()
    }
}

impl btree::KeyedItem for Metadata {
    type Key = FileId;

    fn key(&self) -> Self::Key {
        self.file_id
    }
}

impl btree::Item for ParentRefValue {
    type Summary = ParentRefValueKey;

    fn summarize(&self) -> Self::Summary {
        use btree::KeyedItem;
        self.key()
    }
}

impl btree::KeyedItem for ParentRefValue {
    type Key = ParentRefValueKey;

    fn key(&self) -> Self::Key {
        ParentRefValueKey {
            child_id: self.child_id,
            timestamp: self.timestamp,
        }
    }
}

impl btree::Dimension<ParentRefValueKey> for ParentRefValueKey {
    fn from_summary(summary: &ParentRefValueKey) -> ParentRefValueKey {
        summary.clone()
    }
}

impl Ord for ParentRefValueKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.child_id
            .cmp(&other.child_id)
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

impl PartialOrd for ParentRefValueKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> AddAssign<&'a Self> for ParentRefValueKey {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ParentRefValueKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self < *other);
        other.clone()
    }
}

impl btree::Dimension<ParentRefValueKey> for FileId {
    fn from_summary(summary: &ParentRefValueKey) -> Self {
        summary.child_id
    }
}

impl btree::Item for ChildRefValue {
    type Summary = ChildRefValueSummary;

    fn summarize(&self) -> Self::Summary {
        ChildRefValueSummary {
            parent_id: self.parent_id,
            name: self.name.clone(),
            visible: self.visible,
            timestamp: self.timestamp,
            visible_count: if self.visible { 1 } else { 0 },
        }
    }
}

impl btree::KeyedItem for ChildRefValue {
    type Key = ChildRefValueKey;

    fn key(&self) -> Self::Key {
        ChildRefValueKey {
            parent_id: self.parent_id,
            name: self.name.clone(),
            visible: self.visible,
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

impl btree::Dimension<ChildRefValueSummary> for FileId {
    fn from_summary(summary: &ChildRefValueSummary) -> Self {
        summary.parent_id
    }
}

impl btree::Dimension<ChildRefValueSummary> for ChildRefValueKey {
    fn from_summary(summary: &ChildRefValueSummary) -> ChildRefValueKey {
        ChildRefValueKey {
            parent_id: summary.parent_id,
            name: summary.name.clone(),
            visible: summary.visible,
            timestamp: summary.timestamp,
        }
    }
}

impl Ord for ChildRefValueKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parent_id
            .cmp(&other.parent_id)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.visible.cmp(&other.visible).reverse())
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

impl PartialOrd for ChildRefValueKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> AddAssign<&'a Self> for ChildRefValueKey {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ChildRefValueKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self < *other);
        other.clone()
    }
}

impl btree::Dimension<ChildRefValueSummary> for ChildRefKey {
    fn from_summary(summary: &ChildRefValueSummary) -> Self {
        ChildRefKey {
            parent_id: summary.parent_id,
            name: summary.name.clone(),
        }
    }
}

impl<'a> AddAssign<&'a Self> for ChildRefKey {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self <= *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for ChildRefKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self <= *other);
        other.clone()
    }
}

impl btree::Dimension<ChildRefValueSummary> for usize {
    fn from_summary(summary: &ChildRefValueSummary) -> Self {
        summary.visible_count
    }
}

fn serialize_os_string<S>(os_string: &OsString, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    os_string.to_string_lossy().serialize(serializer)
}

fn deserialize_os_string<'de, D>(deserializer: D) -> Result<OsString, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(OsString::from(String::deserialize(deserializer)?))
}

fn serialize_parent<S>(
    parent: &Option<(FileId, Arc<OsString>)>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    parent
        .as_ref()
        .map(|(parent_id, name)| (parent_id, name.to_string_lossy()))
        .serialize(serializer)
}

fn deserialize_parent<'de, D>(deserializer: D) -> Result<Option<(FileId, Arc<OsString>)>, D::Error>
where
    D: Deserializer<'de>,
{
    let parent = <Option<(FileId, String)>>::deserialize(deserializer)?;
    Ok(parent.map(|(parent_id, name)| (parent_id, Arc::new(OsString::from(name)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffer::Point;
    use rand::{Rng, SeedableRng, StdRng};
    use std::iter::FromIterator;

    #[test]
    fn test_append_base_entries() {
        let mut tree = WorkTree::new(1);
        assert!(tree.paths().is_empty());

        let fixup_ops = tree
            .append_base_entries(vec![
                DirEntry {
                    depth: 1,
                    name: OsString::from("a"),
                    file_type: FileType::Directory,
                },
                DirEntry {
                    depth: 2,
                    name: OsString::from("b"),
                    file_type: FileType::Directory,
                },
                DirEntry {
                    depth: 3,
                    name: OsString::from("c"),
                    file_type: FileType::Text,
                },
                DirEntry {
                    depth: 2,
                    name: OsString::from("d"),
                    file_type: FileType::Directory,
                },
            ]).unwrap();
        assert_eq!(tree.paths(), vec!["a", "a/b", "a/b/c", "a/d"]);
        assert_eq!(fixup_ops.len(), 0);

        let a = tree.file_id("a").unwrap();
        let (file_1, _) = tree.new_text_file();
        tree.rename(file_1, a, "e").unwrap();
        tree.create_dir(a, "z").unwrap();

        let fixup_ops = tree
            .append_base_entries(vec![
                DirEntry {
                    depth: 2,
                    name: OsString::from("e"),
                    file_type: FileType::Directory,
                },
                DirEntry {
                    depth: 1,
                    name: OsString::from("f"),
                    file_type: FileType::Text,
                },
            ]).unwrap();
        assert_eq!(
            tree.paths(),
            vec!["a", "a/b", "a/b/c", "a/d", "a/e", "a/e~", "a/z", "f"]
        );
        assert_eq!(fixup_ops.len(), 1);
    }

    #[test]
    fn test_cursor() {
        let mut tree = WorkTree::new(1);
        tree.append_base_entries(vec![
            DirEntry {
                depth: 1,
                name: OsString::from("a"),
                file_type: FileType::Directory,
            },
            DirEntry {
                depth: 2,
                name: OsString::from("b"),
                file_type: FileType::Directory,
            },
            DirEntry {
                depth: 3,
                name: OsString::from("c"),
                file_type: FileType::Text,
            },
            DirEntry {
                depth: 2,
                name: OsString::from("d"),
                file_type: FileType::Directory,
            },
            DirEntry {
                depth: 2,
                name: OsString::from("e"),
                file_type: FileType::Directory,
            },
            DirEntry {
                depth: 1,
                name: OsString::from("f"),
                file_type: FileType::Directory,
            },
        ]).unwrap();

        let a = tree.file_id("a").unwrap();
        let b = tree.file_id("a/b").unwrap();
        let c = tree.file_id("a/b/c").unwrap();
        let d = tree.file_id("a/d").unwrap();
        let e = tree.file_id("a/e").unwrap();
        let f = tree.file_id("f").unwrap();

        tree.remove(b).unwrap();

        let (new_file, _) = tree.new_text_file();
        tree.rename(new_file, a, "x").unwrap();

        let (new_file_that_got_removed, _) = tree.new_text_file();
        tree.rename(new_file_that_got_removed, e, "y").unwrap();
        tree.remove(new_file_that_got_removed).unwrap();

        tree.rename(e, a, "z").unwrap();

        let mut cursor = tree.cursor().unwrap();
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: a,
                file_type: FileType::Directory,
                depth: 1,
                name: Arc::new(OsString::from("a")),
                status: FileStatus::Unchanged,
                visible: true,
            }
        );

        assert!(cursor.next(true));
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: b,
                file_type: FileType::Directory,
                depth: 2,
                name: Arc::new(OsString::from("b")),
                status: FileStatus::Removed,
                visible: false,
            }
        );

        assert!(cursor.next(true));
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: c,
                file_type: FileType::Text,
                depth: 3,
                name: Arc::new(OsString::from("c")),
                status: FileStatus::Unchanged,
                visible: false,
            }
        );

        assert!(cursor.next(true));
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: d,
                file_type: FileType::Directory,
                depth: 2,
                name: Arc::new(OsString::from("d")),
                status: FileStatus::Unchanged,
                visible: true,
            }
        );

        assert!(cursor.next(true));
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: new_file,
                file_type: FileType::Text,
                depth: 2,
                name: Arc::new(OsString::from("x")),
                status: FileStatus::New,
                visible: true,
            }
        );

        assert!(cursor.next(true));
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: e,
                file_type: FileType::Directory,
                depth: 2,
                name: Arc::new(OsString::from("z")),
                status: FileStatus::Renamed,
                visible: true,
            }
        );

        assert!(cursor.next(true));
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: new_file_that_got_removed,
                file_type: FileType::Text,
                depth: 3,
                name: Arc::new(OsString::from("y")),
                status: FileStatus::New,
                visible: false,
            }
        );

        assert!(cursor.next(true));
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: f,
                file_type: FileType::Directory,
                depth: 1,
                name: Arc::new(OsString::from("f")),
                status: FileStatus::Unchanged,
                visible: true,
            }
        );

        assert!(!cursor.next(true));
        assert_eq!(cursor.entry(), Err(Error::CursorExhausted));
    }

    #[test]
    fn test_buffers() {
        let base_entries = vec![
            DirEntry {
                depth: 1,
                name: OsString::from("dir"),
                file_type: FileType::Directory,
            },
            DirEntry {
                depth: 1,
                name: OsString::from("file"),
                file_type: FileType::Text,
            },
        ];
        let base_text = Text::from("abc");

        let mut tree_1 = WorkTree::new(1);
        tree_1.append_base_entries(base_entries.clone()).unwrap();
        let mut tree_2 = WorkTree::new(2);
        tree_2.append_base_entries(base_entries).unwrap();

        let file_id = tree_1.file_id("file").unwrap();
        let buffer_id = tree_2.open_text_file(file_id, base_text.clone()).unwrap();
        let ops = tree_2.edit(buffer_id, vec![1..2, 3..3], "x");
        tree_1.apply_ops(ops).unwrap();

        // Must call open_text_file on any given replica first before interacting with a buffer.
        assert_eq!(tree_1.text(buffer_id).err(), Some(Error::InvalidBufferId));
        tree_1.open_text_file(file_id, base_text).unwrap();
        assert_eq!(tree_1.text(buffer_id).unwrap().into_string(), "axcx");
        assert_eq!(tree_2.text(buffer_id).unwrap().into_string(), "axcx");

        let ops = tree_1.edit(buffer_id, vec![1..2, 4..4], "y");
        let base_version = tree_2.version();

        tree_2.apply_ops(ops).unwrap();

        assert_eq!(tree_1.text(buffer_id).unwrap().into_string(), "aycxy");
        assert_eq!(tree_2.text(buffer_id).unwrap().into_string(), "aycxy");

        let changes = tree_2
            .changes_since(buffer_id, base_version.clone())
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].range, Point::new(0, 1)..Point::new(0, 2));
        assert_eq!(changes[0].code_units, [b'y' as u16]);
        assert_eq!(changes[1].range, Point::new(0, 4)..Point::new(0, 4));
        assert_eq!(changes[1].code_units, [b'y' as u16]);

        let dir_id = tree_1.file_id("dir").unwrap();
        assert_eq!(
            tree_1.open_text_file(dir_id, Text::from("")),
            Err(Error::InvalidFileId)
        );
    }

    #[test]
    fn test_replication_random() {
        const PEERS: usize = 5;

        for seed in 0..100 {
            println!("SEED: {:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut base_tree = WorkTree::new(999);
            base_tree.mutate(&mut rng, 20);
            let base_entries = base_tree.entries();
            let base_entries = base_entries
                .iter()
                .filter(|entry| entry.visible)
                .map(|entry| DirEntry {
                    depth: entry.depth,
                    name: entry.name.as_ref().clone(),
                    file_type: entry.file_type,
                }).collect::<Vec<_>>();

            let mut trees = Vec::from_iter((0..PEERS).map(|i| WorkTree::new(i as u64 + 1)));
            let mut base_entries_to_append =
                Vec::from_iter((0..PEERS).map(|_| base_entries.clone()));
            let mut inboxes = Vec::from_iter((0..PEERS).map(|_| Vec::new()));

            // Generate and deliver random mutations
            for _ in 0..10 {
                let k = rng.gen_range(0, 3);
                let replica_index = rng.gen_range(0, PEERS);
                let tree = &mut trees[replica_index];
                let base_entries_to_append = &mut base_entries_to_append[replica_index];

                if k == 0 && !base_entries_to_append.is_empty() {
                    let count = rng.gen_range(0, base_entries_to_append.len());
                    let fixup_ops = tree
                        .append_base_entries(base_entries_to_append.drain(0..count))
                        .unwrap();
                    deliver_ops(&mut rng, replica_index, &mut inboxes, fixup_ops);
                } else if k == 1 && !inboxes[replica_index].is_empty() {
                    let count = rng.gen_range(1, inboxes[replica_index].len() + 1);
                    let fixup_ops = tree
                        .apply_ops(inboxes[replica_index].drain(0..count))
                        .unwrap();
                    deliver_ops(&mut rng, replica_index, &mut inboxes, fixup_ops);
                } else {
                    let ops = tree.mutate(&mut rng, 5);
                    deliver_ops(&mut rng, replica_index, &mut inboxes, ops);
                }
            }

            // Allow system to quiesce
            loop {
                let mut done = true;
                for replica_index in 0..PEERS {
                    let tree = &mut trees[replica_index];
                    let base_entries_to_append = &mut base_entries_to_append[replica_index];
                    if !base_entries_to_append.is_empty() {
                        let fixup_ops = tree
                            .append_base_entries(base_entries_to_append.drain(..))
                            .unwrap();
                        deliver_ops(&mut rng, replica_index, &mut inboxes, fixup_ops);
                    }

                    if !inboxes[replica_index].is_empty() {
                        let count = rng.gen_range(1, inboxes[replica_index].len() + 1);
                        let fixup_ops = tree
                            .apply_ops(inboxes[replica_index].drain(0..count))
                            .unwrap();
                        deliver_ops(&mut rng, replica_index, &mut inboxes, fixup_ops);
                        done = false;
                    }
                }

                if done {
                    break;
                }
            }

            for i in 0..PEERS - 1 {
                assert_eq!(trees[i].entries(), trees[i + 1].entries());
            }

            fn deliver_ops<T: Rng>(
                rng: &mut T,
                sender: usize,
                inboxes: &mut Vec<Vec<Operation>>,
                ops: Vec<Operation>,
            ) {
                for (i, inbox) in inboxes.iter_mut().enumerate() {
                    if i != sender {
                        for op in &ops {
                            let min_index = inbox
                                .iter()
                                .enumerate()
                                .rev()
                                .find_map(|(index, existing_op)| {
                                    if existing_op.lamport_timestamp().replica_id
                                        == op.lamport_timestamp().replica_id
                                    {
                                        Some(index + 1)
                                    } else {
                                        None
                                    }
                                }).unwrap_or(0);
                            let insertion_index = rng.gen_range(min_index, inbox.len() + 1);
                            inbox.insert(insertion_index, op.clone());
                        }
                    }
                }
            }
        }
    }

    impl WorkTree {
        fn entries(&self) -> Vec<CursorEntry> {
            let mut entries = Vec::new();
            if let Some(mut cursor) = self.cursor() {
                loop {
                    entries.push(cursor.entry().unwrap());
                    if !cursor.next(true) {
                        break;
                    }
                }
            }
            entries
        }

        fn paths(&self) -> Vec<String> {
            let mut paths = Vec::new();
            if let Some(mut cursor) = self.cursor() {
                loop {
                    paths.push(cursor.path().unwrap().to_string_lossy().into_owned());
                    if !cursor.next(true) {
                        break;
                    }
                }
            }
            paths
        }

        fn mutate<T: Rng>(&mut self, rng: &mut T, count: usize) -> Vec<Operation> {
            let mut ops = Vec::new();
            for _ in 0..count {
                let k = rng.gen_range(0, 3);
                if self.child_refs.is_empty() || k == 0 {
                    // println!("Random mutation: Creating file");
                    let parent_id = self
                        .select_file(rng, Some(FileType::Directory), true)
                        .unwrap();

                    if rng.gen() {
                        loop {
                            match self.create_dir(parent_id, gen_name(rng)) {
                                Ok((_, op)) => {
                                    ops.push(op);
                                    break;
                                }
                                Err(error) => assert_eq!(error, Error::InvalidOperation),
                            }
                        }
                    } else {
                        let (file_id, op) = self.new_text_file();
                        ops.push(op);
                        loop {
                            match self.rename(file_id, parent_id, gen_name(rng)) {
                                Ok(op) => {
                                    ops.push(op);
                                    break;
                                }
                                Err(error) => assert_eq!(error, Error::InvalidOperation),
                            }
                        }
                    }
                } else if k == 1 {
                    let file_id = self.select_file(rng, None, false).unwrap();
                    // println!("Random mutation: Removing {:?}", file_id);
                    ops.push(self.remove(file_id).unwrap());
                } else {
                    let file_id = self.select_file(rng, None, false).unwrap();
                    loop {
                        let new_parent_id = self
                            .select_file(rng, Some(FileType::Directory), true)
                            .unwrap();
                        let new_name = gen_name(rng);
                        // println!(
                        //     "Random mutation: Attempting to move {:?} to ({:?}, {:?})",
                        //     file_id, new_parent_id, new_name
                        // );
                        match self.rename(file_id, new_parent_id, new_name) {
                            Ok(op) => {
                                ops.push(op);
                                break;
                            }
                            Err(error) => assert_eq!(error, Error::InvalidOperation),
                        }
                    }
                }
            }
            ops
        }

        fn select_file<T: Rng>(
            &self,
            rng: &mut T,
            file_type: Option<FileType>,
            allow_root: bool,
        ) -> Option<FileId> {
            let metadata = self
                .metadata
                .cursor()
                .filter(|metadata| file_type.is_none() || file_type.unwrap() == metadata.file_type)
                .collect::<Vec<_>>();
            if allow_root
                && file_type.map_or(true, |file_type| file_type == FileType::Directory)
                && rng.gen_weighted_bool(metadata.len() as u32 + 1)
            {
                Some(ROOT_FILE_ID)
            } else {
                rng.choose(&metadata).map(|metadata| metadata.file_id)
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
