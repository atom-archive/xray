use crate::btree::{self, SeekBias};
use crate::buffer::{self, Buffer, Point, Selection, SelectionSetId, Text};
use crate::operation_queue::{self, OperationQueue};
use crate::serialization;
use crate::time;
use crate::Error;
use crate::Oid;
use crate::ReplicaId;
use flatbuffers::{FlatBufferBuilder, UnionWIPOffset, WIPOffset};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_derive::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::ops::{Add, AddAssign, Range};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub const ROOT_FILE_ID: FileId = FileId::Base(0);

pub type Id = time::Lamport;

#[derive(Clone)]
pub struct Epoch {
    pub id: Id,
    pub head: Option<Oid>,
    base_entries_next_id: u64,
    base_entries_stack: Vec<FileId>,
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRefValue>,
    child_refs: btree::Tree<ChildRefValue>,
    version: time::Global,
    local_clock: time::Local,
    text_files: HashMap<FileId, TextFile>,
    deferred_ops: OperationQueue<Operation>,
}

pub struct Cursor<'a> {
    text_files: &'a HashMap<FileId, TextFile>,
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

#[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    InsertMetadata {
        file_id: FileId,
        file_type: FileType,
        parent: Option<(FileId, Arc<OsString>)>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    },
    UpdateParent {
        child_id: FileId,
        new_parent: Option<(FileId, Arc<OsString>)>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    },
    BufferOperation {
        file_id: FileId,
        operations: Vec<buffer::Operation>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FileId {
    Base(u64),
    New(time::Local),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum FileStatus {
    New,
    Renamed,
    Removed,
    Modified,
    RenamedAndModified,
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

impl Epoch {
    pub fn new(replica_id: ReplicaId, id: Id, head: Option<Oid>) -> Self {
        Self {
            id,
            head,
            base_entries_next_id: 1,
            base_entries_stack: Vec::new(),
            metadata: btree::Tree::new(),
            parent_refs: btree::Tree::new(),
            child_refs: btree::Tree::new(),
            version: time::Global::new(),
            local_clock: time::Local::new(replica_id),
            text_files: HashMap::new(),
            deferred_ops: OperationQueue::new(),
        }
    }

    pub fn buffer_version(&self, file_id: FileId) -> Result<time::Global, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            Ok(buffer.version.clone())
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    pub fn buffer_selections_last_update(&self, file_id: FileId) -> Result<time::Lamport, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            Ok(buffer.selections_last_update)
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
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
            text_files: &self.text_files,
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

    pub fn append_base_entries<I>(
        &mut self,
        entries: I,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Vec<Operation>, Error>
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
                timestamp: time::Lamport::default(),
                parent: Some((parent_id, name.clone())),
            }));
            child_ref_edits.push(btree::Edit::Insert(ChildRefValue {
                parent_id,
                name: name.clone(),
                timestamp: time::Lamport::default(),
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
            fixup_ops.extend(self.fix_name_conflicts(file_id, lamport_clock));
        }
        let deferred_ops = self.deferred_ops.drain();
        fixup_ops.extend(self.apply_ops_internal(deferred_ops, lamport_clock)?);

        Ok(fixup_ops)
    }

    pub fn apply_ops<I>(
        &mut self,
        ops: I,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Vec<Operation>, Error>
    where
        I: IntoIterator<Item = Operation>,
    {
        let mut fixup_ops = Vec::new();
        fixup_ops.extend(self.apply_ops_internal(ops, lamport_clock)?);
        let deferred_ops = self.deferred_ops.drain();
        fixup_ops.extend(self.apply_ops_internal(deferred_ops, lamport_clock)?);
        Ok(fixup_ops)
    }

    fn apply_ops_internal<I>(
        &mut self,
        ops: I,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Vec<Operation>, Error>
    where
        I: IntoIterator<Item = Operation>,
    {
        let mut ops = ops.into_iter().peekable();
        if ops.peek().is_none() {
            return Ok(Vec::new());
        }

        let mut new_epoch = self.clone();
        let mut deferred_ops = Vec::new();
        let mut potential_conflicts = HashSet::new();

        for op in ops {
            if new_epoch.can_apply_op(&op) {
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
                new_epoch.apply_op(op, lamport_clock)?;
            } else {
                deferred_ops.push(op);
            }
        }
        new_epoch.deferred_ops.insert(deferred_ops);

        let mut fixup_ops = Vec::new();
        for file_id in &potential_conflicts {
            fixup_ops.extend(new_epoch.fix_conflicts(*file_id, lamport_clock));
        }

        *self = new_epoch;
        Ok(fixup_ops)
    }

    pub fn apply_op(
        &mut self,
        op: Operation,
        lamport_clock: &mut time::Lamport,
    ) -> Result<(), Error> {
        self.version.observe(op.local_timestamp());
        self.local_clock.observe(op.local_timestamp());
        lamport_clock.observe(op.lamport_timestamp());

        match op {
            Operation::InsertMetadata {
                file_id,
                file_type,
                parent,
                lamport_timestamp,
                ..
            } => {
                if !self.metadata.cursor().seek(&file_id, SeekBias::Left) {
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

                self.parent_refs
                    .edit(&mut [btree::Edit::Insert(ParentRefValue {
                        child_id,
                        timestamp: lamport_timestamp,
                        parent: new_parent,
                    })]);
                self.child_refs.edit(&mut child_ref_edits);
            }
            Operation::BufferOperation {
                file_id,
                operations,
                ..
            } => match self
                .text_files
                .entry(file_id)
                .or_insert_with(|| TextFile::Deferred(Vec::new()))
            {
                TextFile::Deferred(deferred_operations) => {
                    deferred_operations.extend(operations);
                }
                TextFile::Buffered(buffer) => {
                    buffer
                        .apply_ops(operations, &mut self.local_clock, lamport_clock)
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
            Operation::BufferOperation { file_id, .. } => self.metadata(*file_id).is_ok(),
        }
    }

    pub fn create_file<N>(
        &mut self,
        parent_id: FileId,
        name: N,
        file_type: FileType,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Operation, Error>
    where
        N: AsRef<OsStr>,
    {
        self.check_file_id(parent_id, Some(FileType::Directory))?;

        let mut new_lamport_clock = *lamport_clock;
        let mut new_epoch = self.clone();
        let file_id = FileId::New(new_epoch.local_clock.tick());
        let operation = Operation::InsertMetadata {
            file_id,
            file_type,
            parent: Some((parent_id, Arc::new(name.as_ref().into()))),
            local_timestamp: new_epoch.local_clock.tick(),
            lamport_timestamp: new_lamport_clock.tick(),
        };
        let fixup_ops = new_epoch
            .apply_ops_internal(Some(operation.clone()), &mut new_lamport_clock)
            .unwrap();
        if fixup_ops.is_empty() {
            *lamport_clock = new_lamport_clock;
            *self = new_epoch;
            Ok(operation)
        } else {
            Err(Error::InvalidOperation)
        }
    }

    pub fn new_text_file(&mut self, lamport_clock: &mut time::Lamport) -> (FileId, Operation) {
        let file_id = FileId::New(self.local_clock.tick());
        let operation = Operation::InsertMetadata {
            file_id,
            file_type: FileType::Text,
            parent: None,
            local_timestamp: self.local_clock.tick(),
            lamport_timestamp: lamport_clock.tick(),
        };
        self.apply_op(operation.clone(), lamport_clock).unwrap();
        (file_id, operation)
    }

    pub fn open_text_file<T>(
        &mut self,
        file_id: FileId,
        base_text: T,
        lamport_clock: &mut time::Lamport,
    ) -> Result<(), Error>
    where
        T: Into<Text>,
    {
        self.check_file_id(file_id, Some(FileType::Text))?;

        match self.text_files.remove(&file_id) {
            Some(TextFile::Deferred(operations)) => {
                let mut buffer = Buffer::new(base_text);
                buffer
                    .apply_ops(operations, &mut self.local_clock, lamport_clock)
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

        Ok(())
    }

    pub fn rename<N>(
        &mut self,
        file_id: FileId,
        new_parent_id: FileId,
        new_name: N,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Operation, Error>
    where
        N: AsRef<OsStr>,
    {
        self.check_file_id(file_id, None)?;
        self.check_file_id(new_parent_id, Some(FileType::Directory))?;

        let mut new_lamport_clock = *lamport_clock;
        let mut new_epoch = self.clone();
        let operation = Operation::UpdateParent {
            child_id: file_id,
            new_parent: Some((new_parent_id, Arc::new(new_name.as_ref().into()))),
            local_timestamp: new_epoch.local_clock.tick(),
            lamport_timestamp: new_lamport_clock.tick(),
        };
        let fixup_ops = new_epoch
            .apply_ops_internal(Some(operation.clone()), &mut new_lamport_clock)
            .unwrap();
        if fixup_ops.is_empty() {
            *lamport_clock = new_lamport_clock;
            *self = new_epoch;
            Ok(operation)
        } else {
            Err(Error::InvalidOperation)
        }
    }

    pub fn remove(
        &mut self,
        file_id: FileId,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Operation, Error> {
        self.check_file_id(file_id, None)?;

        let operation = Operation::UpdateParent {
            child_id: file_id,
            new_parent: None,
            local_timestamp: self.local_clock.tick(),
            lamport_timestamp: lamport_clock.tick(),
        };
        self.apply_op(operation.clone(), lamport_clock).unwrap();
        Ok(operation)
    }

    pub fn edit<I, T>(
        &mut self,
        file_id: FileId,
        old_ranges: I,
        new_text: T,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Operation, Error>
    where
        I: IntoIterator<Item = Range<usize>>,
        T: Into<Text>,
    {
        self.mutate_buffer(
            file_id,
            lamport_clock,
            |buffer, local_clock, lamport_clock| {
                Ok(buffer.edit(old_ranges, new_text, local_clock, lamport_clock))
            },
        )
    }

    pub fn edit_2d<I, T>(
        &mut self,
        file_id: FileId,
        old_ranges: I,
        new_text: T,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Operation, Error>
    where
        I: IntoIterator<Item = Range<Point>>,
        T: Into<Text>,
    {
        self.mutate_buffer(
            file_id,
            lamport_clock,
            |buffer, local_clock, lamport_clock| {
                Ok(buffer.edit_2d(old_ranges, new_text, local_clock, lamport_clock))
            },
        )
    }

    pub fn add_selection_set<I>(
        &mut self,
        file_id: FileId,
        ranges: I,
        lamport_clock: &mut time::Lamport,
    ) -> Result<(SelectionSetId, Operation), Error>
    where
        I: IntoIterator<Item = Range<Point>>,
    {
        let mut new_set_id = None;
        let operation = self.mutate_buffer(
            file_id,
            lamport_clock,
            |buffer, _local_clock, lamport_clock| {
                let (set_id, operation) = buffer.add_selection_set(ranges, lamport_clock)?;
                new_set_id = Some(set_id);
                Ok(vec![operation])
            },
        )?;
        Ok((new_set_id.unwrap(), operation))
    }

    pub fn replace_selection_set<I>(
        &mut self,
        file_id: FileId,
        set_id: SelectionSetId,
        ranges: I,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Operation, Error>
    where
        I: IntoIterator<Item = Range<Point>>,
    {
        self.mutate_buffer(
            file_id,
            lamport_clock,
            |buffer, _local_clock, lamport_clock| {
                let operation = buffer.replace_selection_set(set_id, ranges, lamport_clock)?;
                Ok(vec![operation])
            },
        )
    }

    pub fn remove_selection_set(
        &mut self,
        file_id: FileId,
        set_id: SelectionSetId,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Operation, Error> {
        self.mutate_buffer(
            file_id,
            lamport_clock,
            |buffer, _local_clock, lamport_clock| {
                let operation = buffer.remove_selection_set(set_id, lamport_clock)?;
                Ok(vec![operation])
            },
        )
    }

    pub fn all_selections(
        &self,
        file_id: FileId,
    ) -> Result<Vec<(SelectionSetId, Vec<Selection>)>, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            Ok(buffer
                .all_selections()
                .map(|(set_id, selections)| (*set_id, selections.clone()))
                .collect())
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    pub fn selection_ranges<'a>(
        &'a self,
        file_id: FileId,
        set_id: SelectionSetId,
    ) -> Result<impl Iterator<Item = Range<Point>> + 'a, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            buffer.selection_ranges(set_id)
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    pub fn all_selection_ranges<'a>(
        &'a self,
        file_id: FileId,
    ) -> Result<impl Iterator<Item = (SelectionSetId, Vec<Range<Point>>)> + 'a, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            Ok(buffer.all_selection_ranges())
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    fn mutate_buffer<F>(
        &mut self,
        file_id: FileId,
        lamport_clock: &mut time::Lamport,
        mutate: F,
    ) -> Result<Operation, Error>
    where
        F: FnOnce(
            &mut Buffer,
            &mut time::Local,
            &mut time::Lamport,
        ) -> Result<Vec<buffer::Operation>, Error>,
    {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get_mut(&file_id) {
            let operations = mutate(buffer, &mut self.local_clock, lamport_clock)?;
            let local_timestamp = self.local_clock.tick();
            self.version.observe(local_timestamp);
            Ok(Operation::BufferOperation {
                file_id,
                operations,
                local_timestamp,
                lamport_timestamp: lamport_clock.tick(),
            })
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    pub fn file_id<P>(&self, path: P) -> Result<FileId, Error>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        let mut cursor = self.child_refs.cursor();
        let mut parent_id = ROOT_FILE_ID;
        for component in path.components() {
            match component {
                Component::Normal(name) => {
                    let name = Arc::new(name.into());
                    if cursor.seek(&ChildRefKey { parent_id, name }, SeekBias::Left) {
                        let child_ref = cursor.item().unwrap();
                        if child_ref.visible {
                            parent_id = child_ref.child_id;
                        } else {
                            return Err(Error::InvalidPath(
                                format!("file not found for path {:?}", path).into(),
                            ));
                        }
                    } else {
                        return Err(Error::InvalidPath(
                            format!("file not found for path {:?}", path).into(),
                        ));
                    }
                }
                _ => {
                    return Err(Error::InvalidPath(
                        format!("path {:?} contains unrecognized components", path).into(),
                    ));
                }
            }
        }

        Ok(parent_id)
    }

    pub fn base_path(&self, mut file_id: FileId) -> Option<PathBuf> {
        let mut cursor = self.parent_refs.cursor();
        let mut path_components = Vec::new();

        loop {
            if file_id == ROOT_FILE_ID {
                break;
            } else if file_id.is_base() {
                cursor.seek(
                    &ParentRefValueKey {
                        child_id: file_id,
                        timestamp: time::Lamport::default(),
                    },
                    SeekBias::Left,
                );
                let (parent_id, name) = cursor.item().unwrap().parent.unwrap();
                file_id = parent_id;
                path_components.push(name);
            } else {
                return None;
            }
        }

        let mut path = PathBuf::new();
        for component in path_components.into_iter().rev() {
            path.push(component.as_ref());
        }
        Some(path)
    }

    pub fn path(&self, file_id: FileId) -> Option<PathBuf> {
        let mut path_components = Vec::new();
        if self.visit_ancestors(file_id, |name| path_components.push(name)) {
            let mut path = PathBuf::new();
            for component in path_components.into_iter().rev() {
                path.push(component.as_ref());
            }
            Some(path)
        } else {
            None
        }
    }

    pub fn text(&self, file_id: FileId) -> Result<buffer::Iter, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            Ok(buffer.iter())
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    pub fn selections_changed_since(
        &self,
        file_id: FileId,
        last_selection_update: time::Lamport,
    ) -> Result<bool, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            Ok(buffer.selections_changed_since(last_selection_update))
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    pub fn changes_since(
        &self,
        file_id: FileId,
        version: &time::Global,
    ) -> Result<impl Iterator<Item = buffer::Change>, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            Ok(buffer.changes_since(version))
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    pub fn buffer_deferred_ops_len(&self, file_id: FileId) -> Result<usize, Error> {
        if let Some(TextFile::Buffered(buffer)) = self.text_files.get(&file_id) {
            Ok(buffer.deferred_ops_len())
        } else {
            Err(Error::InvalidFileId("file has not been opened".into()))
        }
    }

    pub fn file_type(&self, file_id: FileId) -> Result<FileType, Error> {
        Ok(self.metadata(file_id)?.file_type)
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
                Err(Error::InvalidFileId("file does not exist".into()))
            }
        }
    }

    fn check_file_id(&self, file_id: FileId, expected_type: Option<FileType>) -> Result<(), Error> {
        let metadata = self.metadata(file_id)?;
        if expected_type.map_or(true, |expected_type| expected_type == metadata.file_type) {
            Ok(())
        } else {
            Err(Error::InvalidFileId(
                format!("expected file to have type {:?}", expected_type).into(),
            ))
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

    fn fix_conflicts(
        &mut self,
        file_id: FileId,
        lamport_clock: &mut time::Lamport,
    ) -> Vec<Operation> {
        use crate::btree::KeyedItem;

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
                lamport_timestamp: lamport_clock.tick(),
            });
            moved_file_ids.push(*child_id);
        }

        for op in &fixup_ops {
            self.apply_op(op.clone(), lamport_clock).unwrap();
        }
        for file_id in moved_file_ids {
            fixup_ops.extend(self.fix_name_conflicts(file_id, lamport_clock));
        }

        if !reverted_moves.contains_key(&file_id) {
            fixup_ops.extend(self.fix_name_conflicts(file_id, lamport_clock));
        }

        fixup_ops
    }

    fn fix_name_conflicts(
        &mut self,
        file_id: FileId,
        lamport_clock: &mut time::Lamport,
    ) -> Vec<Operation> {
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
                        lamport_timestamp: lamport_clock.tick(),
                    };
                    self.apply_op(fixup_op.clone(), lamport_clock).unwrap();
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

impl<'a> Cursor<'a> {
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
                    if self.is_modified_file(metadata.file_id) {
                        (FileStatus::Modified, true)
                    } else {
                        (FileStatus::Unchanged, true)
                    }
                } else if newest_parent_ref_value.parent.is_some() {
                    if self.is_modified_file(metadata.file_id) {
                        (FileStatus::RenamedAndModified, true)
                    } else {
                        (FileStatus::Renamed, true)
                    }
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

    fn is_modified_file(&self, file_id: FileId) -> bool {
        self.text_files
            .get(&file_id)
            .map_or(false, |f| f.is_modified())
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
            Operation::BufferOperation {
                local_timestamp, ..
            } => *local_timestamp,
        }
    }

    pub fn lamport_timestamp(&self) -> time::Lamport {
        match self {
            Operation::InsertMetadata {
                lamport_timestamp, ..
            } => *lamport_timestamp,
            Operation::UpdateParent {
                lamport_timestamp, ..
            } => *lamport_timestamp,
            Operation::BufferOperation {
                lamport_timestamp, ..
            } => *lamport_timestamp,
        }
    }

    pub fn to_flatbuf<'fbb>(
        &self,
        builder: &mut FlatBufferBuilder<'fbb>,
    ) -> (serialization::epoch::Operation, WIPOffset<UnionWIPOffset>) {
        use crate::serialization::epoch::{
            BufferOperation, BufferOperationArgs, FileId as FileIdType, InsertMetadata,
            InsertMetadataArgs, Operation as OperationType, UpdateParent, UpdateParentArgs,
        };

        fn parent_to_flatbuf<'a, 'fbb>(
            parent: &'a Option<(FileId, Arc<OsString>)>,
            builder: &mut FlatBufferBuilder<'fbb>,
        ) -> (
            FileIdType,
            Option<WIPOffset<UnionWIPOffset>>,
            Option<flatbuffers::WIPOffset<&'fbb str>>,
        ) {
            if let Some((file_id, name)) = parent.as_ref() {
                let (file_id_type, file_id) = file_id.to_flatbuf(builder);
                (
                    file_id_type,
                    Some(file_id),
                    Some(builder.create_string(name.to_string_lossy().as_ref())),
                )
            } else {
                (FileIdType::NONE, None, None)
            }
        }

        match self {
            Operation::InsertMetadata {
                file_id,
                file_type,
                parent,
                local_timestamp,
                lamport_timestamp,
            } => {
                let (file_id_type, file_id) = file_id.to_flatbuf(builder);
                let (parent_id_type, parent_id, name_in_parent) =
                    parent_to_flatbuf(parent, builder);

                (
                    OperationType::InsertMetadata,
                    InsertMetadata::create(
                        builder,
                        &InsertMetadataArgs {
                            file_id_type,
                            file_id: Some(file_id),
                            file_type: file_type.to_flatbuf(),
                            parent_id_type,
                            parent_id,
                            name_in_parent,
                            local_timestamp: Some(&local_timestamp.to_flatbuf()),
                            lamport_timestamp: Some(&lamport_timestamp.to_flatbuf()),
                        },
                    )
                    .as_union_value(),
                )
            }
            Operation::UpdateParent {
                child_id,
                new_parent,
                local_timestamp,
                lamport_timestamp,
            } => {
                let (child_id_type, child_id) = child_id.to_flatbuf(builder);
                let (new_parent_id_type, new_parent_id, new_name_in_parent) =
                    parent_to_flatbuf(new_parent, builder);
                (
                    OperationType::UpdateParent,
                    UpdateParent::create(
                        builder,
                        &UpdateParentArgs {
                            child_id_type,
                            child_id: Some(child_id),
                            new_parent_id_type,
                            new_parent_id,
                            new_name_in_parent,
                            local_timestamp: Some(&local_timestamp.to_flatbuf()),
                            lamport_timestamp: Some(&lamport_timestamp.to_flatbuf()),
                        },
                    )
                    .as_union_value(),
                )
            }
            Operation::BufferOperation {
                file_id,
                operations,
                local_timestamp,
                lamport_timestamp,
            } => {
                let (file_id_type, file_id) = file_id.to_flatbuf(builder);
                let op_flatbufs = &operations
                    .iter()
                    .map(|e| e.to_flatbuf(builder))
                    .collect::<Vec<_>>();
                let operations = builder.create_vector(op_flatbufs);

                (
                    OperationType::BufferOperation,
                    BufferOperation::create(
                        builder,
                        &BufferOperationArgs {
                            file_id_type,
                            file_id: Some(file_id),
                            operations: Some(operations),
                            local_timestamp: Some(&local_timestamp.to_flatbuf()),
                            lamport_timestamp: Some(&lamport_timestamp.to_flatbuf()),
                        },
                    )
                    .as_union_value(),
                )
            }
        }
    }

    pub fn from_flatbuf<'a>(
        operation_type: serialization::epoch::Operation,
        message: flatbuffers::Table<'a>,
    ) -> Result<Option<Self>, Error> {
        fn parent_from_flatbuf<'a>(
            parent_id_type: serialization::epoch::FileId,
            parent_id_message: Option<flatbuffers::Table<'a>>,
            name: Option<&'a str>,
        ) -> Option<(FileId, Arc<OsString>)> {
            parent_id_message.map(|parent_id_message| {
                let file_id = FileId::from_flatbuf(parent_id_type, parent_id_message);
                let name = Arc::new(OsString::from(name.unwrap()));
                (file_id, name)
            })
        }

        match operation_type {
            serialization::epoch::Operation::InsertMetadata => {
                let message = serialization::epoch::InsertMetadata::init_from_table(message);
                Ok(Some(Operation::InsertMetadata {
                    file_id: FileId::from_flatbuf(
                        message.file_id_type(),
                        message.file_id().ok_or(Error::DeserializeError)?,
                    ),
                    file_type: FileType::from_flatbuf(&message.file_type()),
                    parent: parent_from_flatbuf(
                        message.parent_id_type(),
                        message.parent_id(),
                        message.name_in_parent(),
                    ),
                    local_timestamp: time::Local::from_flatbuf(&message.local_timestamp().unwrap()),
                    lamport_timestamp: time::Lamport::from_flatbuf(
                        message.lamport_timestamp().ok_or(Error::DeserializeError)?,
                    ),
                }))
            }
            serialization::epoch::Operation::UpdateParent => {
                let message = serialization::epoch::UpdateParent::init_from_table(message);
                Ok(Some(Operation::UpdateParent {
                    child_id: FileId::from_flatbuf(
                        message.child_id_type(),
                        message.child_id().ok_or(Error::DeserializeError)?,
                    ),
                    new_parent: parent_from_flatbuf(
                        message.new_parent_id_type(),
                        message.new_parent_id(),
                        message.new_name_in_parent(),
                    ),
                    local_timestamp: time::Local::from_flatbuf(
                        message.local_timestamp().ok_or(Error::DeserializeError)?,
                    ),
                    lamport_timestamp: time::Lamport::from_flatbuf(
                        message.lamport_timestamp().ok_or(Error::DeserializeError)?,
                    ),
                }))
            }
            serialization::epoch::Operation::BufferOperation => {
                let message = serialization::epoch::BufferOperation::init_from_table(message);
                let op_messages = message.operations().ok_or(Error::DeserializeError)?;
                let mut operations = Vec::with_capacity(op_messages.len());
                for i in 0..op_messages.len() {
                    if let Some(op) = buffer::Operation::from_flatbuf(&op_messages.get(i))? {
                        operations.push(op);
                    }
                }

                Ok(Some(Operation::BufferOperation {
                    file_id: FileId::from_flatbuf(
                        message.file_id_type(),
                        message.file_id().ok_or(Error::DeserializeError)?,
                    ),
                    operations,
                    local_timestamp: time::Local::from_flatbuf(
                        message.local_timestamp().ok_or(Error::DeserializeError)?,
                    ),
                    lamport_timestamp: time::Lamport::from_flatbuf(
                        message.lamport_timestamp().ok_or(Error::DeserializeError)?,
                    ),
                }))
            }
            serialization::epoch::Operation::NONE => Ok(None),
        }
    }
}

impl operation_queue::Operation for Operation {
    fn timestamp(&self) -> time::Lamport {
        self.lamport_timestamp()
    }
}

impl FileId {
    pub fn is_base(&self) -> bool {
        if let FileId::Base(_) = self {
            true
        } else {
            false
        }
    }

    fn to_flatbuf<'fbb>(
        &self,
        builder: &mut FlatBufferBuilder<'fbb>,
    ) -> (serialization::epoch::FileId, WIPOffset<UnionWIPOffset>) {
        use crate::serialization::epoch::{
            BaseFileId, BaseFileIdArgs, FileId as FileIdType, NewFileId, NewFileIdArgs,
        };

        match self {
            FileId::Base(index) => (
                FileIdType::BaseFileId,
                BaseFileId::create(builder, &BaseFileIdArgs { index: *index }).as_union_value(),
            ),
            FileId::New(id) => (
                FileIdType::NewFileId,
                NewFileId::create(
                    builder,
                    &NewFileIdArgs {
                        id: Some(&id.to_flatbuf()),
                    },
                )
                .as_union_value(),
            ),
        }
    }

    fn from_flatbuf<'a>(
        file_id_type: serialization::epoch::FileId,
        message: flatbuffers::Table<'a>,
    ) -> Self {
        match file_id_type {
            serialization::epoch::FileId::BaseFileId => {
                let message = serialization::epoch::BaseFileId::init_from_table(message);
                FileId::Base(message.index())
            }
            serialization::epoch::FileId::NewFileId => {
                let message = serialization::epoch::NewFileId::init_from_table(message);
                FileId::New(time::Local::from_flatbuf(&message.id().unwrap()))
            }
            serialization::epoch::FileId::NONE => unreachable!(),
        }
    }
}

impl FileType {
    fn to_flatbuf(&self) -> serialization::epoch::FileType {
        match self {
            FileType::Directory => serialization::epoch::FileType::Directory,
            FileType::Text => serialization::epoch::FileType::Text,
        }
    }

    fn from_flatbuf(message: &serialization::epoch::FileType) -> Self {
        match message {
            serialization::epoch::FileType::Directory => FileType::Directory,
            serialization::epoch::FileType::Text => FileType::Text,
        }
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
        use crate::btree::KeyedItem;
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
        use crate::btree::KeyedItem;
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

impl TextFile {
    fn is_modified(&self) -> bool {
        match self {
            TextFile::Deferred(ops) => ops.iter().any(|op| op.is_edit()),
            TextFile::Buffered(buffer) => buffer.is_modified(),
        }
    }

    #[cfg(test)]
    fn is_buffered(&self) -> bool {
        match self {
            TextFile::Buffered(_) => true,
            _ => false,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Point;
    use rand::{Rng, SeedableRng, StdRng};
    use uuid::Uuid;

    #[test]
    fn test_append_base_entries() {
        let replica_id = Uuid::nil();
        let mut epoch = Epoch::with_replica_id(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);
        assert!(epoch.paths().is_empty());

        let fixup_ops = epoch
            .append_base_entries(
                vec![
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
                ],
                &mut lamport_clock,
            )
            .unwrap();
        assert_eq!(epoch.paths(), vec!["a", "a/b", "a/b/c", "a/d"]);
        assert_eq!(fixup_ops.len(), 0);

        let a = epoch.file_id("a").unwrap();
        let (file_1, _) = epoch.new_text_file(&mut lamport_clock);
        epoch.rename(file_1, a, "e", &mut lamport_clock).unwrap();
        epoch
            .create_file(a, "z", FileType::Directory, &mut lamport_clock)
            .unwrap();

        let fixup_ops = epoch
            .append_base_entries(
                vec![
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
                ],
                &mut lamport_clock,
            )
            .unwrap();
        assert_eq!(
            epoch.paths(),
            vec!["a", "a/b", "a/b/c", "a/d", "a/e", "a/e~", "a/z", "f"]
        );
        assert_eq!(fixup_ops.len(), 1);
    }

    #[test]
    fn test_cursor() {
        let replica_id = Uuid::nil();
        let mut epoch = Epoch::with_replica_id(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);

        epoch
            .append_base_entries(
                vec![
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
                    DirEntry {
                        depth: 2,
                        name: OsString::from("g"),
                        file_type: FileType::Text,
                    },
                ],
                &mut lamport_clock,
            )
            .unwrap();

        let a = epoch.file_id("a").unwrap();
        let b = epoch.file_id("a/b").unwrap();
        let c = epoch.file_id("a/b/c").unwrap();
        let d = epoch.file_id("a/d").unwrap();
        let e = epoch.file_id("a/e").unwrap();
        let f = epoch.file_id("f").unwrap();
        let g = epoch.file_id("f/g").unwrap();

        epoch.remove(b, &mut lamport_clock).unwrap();

        let (new_file, _) = epoch.new_text_file(&mut lamport_clock);
        epoch.rename(new_file, a, "x", &mut lamport_clock).unwrap();

        let (new_file_that_got_removed, _) = epoch.new_text_file(&mut lamport_clock);
        epoch
            .rename(new_file_that_got_removed, e, "y", &mut lamport_clock)
            .unwrap();
        epoch
            .remove(new_file_that_got_removed, &mut lamport_clock)
            .unwrap();

        epoch.rename(e, a, "z", &mut lamport_clock).unwrap();

        epoch.open_text_file(c, "123", &mut lamport_clock).unwrap();
        epoch.edit(c, Some(0..0), "x", &mut lamport_clock).unwrap();

        epoch
            .rename(g, ROOT_FILE_ID, "g", &mut lamport_clock)
            .unwrap();
        epoch.open_text_file(g, "456", &mut lamport_clock).unwrap();
        epoch.edit(g, Some(0..0), "y", &mut lamport_clock).unwrap();

        let mut cursor = epoch.cursor().unwrap();
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
                status: FileStatus::Modified,
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

        assert!(cursor.next(true));
        assert_eq!(
            cursor.entry().unwrap(),
            CursorEntry {
                file_id: g,
                file_type: FileType::Text,
                depth: 1,
                name: Arc::new(OsString::from("g")),
                status: FileStatus::RenamedAndModified,
                visible: true,
            }
        );

        assert!(!cursor.next(true));
        assert!(cursor.entry().is_err());
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

        let replica_id_1 = Uuid::from_u128(1);
        let mut epoch_1 = Epoch::with_replica_id(replica_id_1);
        let mut lamport_clock_1 = time::Lamport::new(replica_id_1);
        epoch_1
            .append_base_entries(base_entries.clone(), &mut lamport_clock_1)
            .unwrap();
        let replica_id_2 = Uuid::from_u128(2);
        let mut epoch_2 = Epoch::with_replica_id(replica_id_2);
        let mut lamport_clock_2 = time::Lamport::new(replica_id_2);
        epoch_2
            .append_base_entries(base_entries, &mut lamport_clock_2)
            .unwrap();

        let file_id = epoch_1.file_id("file").unwrap();
        epoch_2
            .open_text_file(file_id, base_text.clone(), &mut lamport_clock_2)
            .unwrap();
        let ops = epoch_2.edit(file_id, vec![1..2, 3..3], "x", &mut lamport_clock_2);
        epoch_1.apply_ops(ops, &mut lamport_clock_1).unwrap();

        // Must call open_text_file on any given replica first before interacting with a buffer.
        assert!(epoch_1.text(file_id).is_err());
        epoch_1
            .open_text_file(file_id, base_text, &mut lamport_clock_1)
            .unwrap();
        assert_eq!(epoch_1.text(file_id).unwrap().into_string(), "axcx");
        assert_eq!(epoch_2.text(file_id).unwrap().into_string(), "axcx");

        let ops = epoch_1.edit(file_id, vec![1..2, 4..4], "y", &mut lamport_clock_1);
        let base_version = epoch_2.version();

        epoch_2.apply_ops(ops, &mut lamport_clock_2).unwrap();

        assert_eq!(epoch_1.text(file_id).unwrap().into_string(), "aycxy");
        assert_eq!(epoch_2.text(file_id).unwrap().into_string(), "aycxy");

        let changes = epoch_2
            .changes_since(file_id, &base_version)
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].range, Point::new(0, 1)..Point::new(0, 2));
        assert_eq!(changes[0].code_units, [b'y' as u16]);
        assert_eq!(changes[1].range, Point::new(0, 4)..Point::new(0, 4));
        assert_eq!(changes[1].code_units, [b'y' as u16]);

        let dir_id = epoch_1.file_id("dir").unwrap();
        assert!(epoch_1
            .open_text_file(dir_id, Text::from(""), &mut lamport_clock_1)
            .is_err());
    }

    #[test]
    fn test_buffer_deferred_ops_len() -> Result<(), Error> {
        let replica_1_id = Uuid::from_u128(1);
        let mut epoch_1 = Epoch::with_replica_id(replica_1_id);
        let mut clock_1 = time::Lamport::new(replica_1_id);

        let (file_id, new_file_op) = epoch_1.new_text_file(&mut clock_1);
        epoch_1.open_text_file(file_id, "", &mut clock_1).unwrap();
        let edit_1_op = epoch_1.edit(file_id, Some(0..0), "135", &mut clock_1)?;
        let edit_2_op = epoch_1.edit(file_id, Some(1..1), "2", &mut clock_1)?;
        let edit_3_op = epoch_1.edit(file_id, Some(3..3), "4", &mut clock_1)?;

        let replica_2_id = Uuid::from_u128(2);
        let mut epoch_2 = Epoch::with_replica_id(replica_2_id);
        let mut clock_2 = time::Lamport::new(replica_2_id);
        epoch_2.apply_ops(Some(new_file_op.clone()), &mut clock_2)?;

        epoch_2.open_text_file(file_id, "", &mut clock_2)?;
        assert_eq!(epoch_2.buffer_deferred_ops_len(file_id)?, 0);

        epoch_2.apply_ops(Some(edit_3_op.clone()), &mut clock_2)?;
        assert_eq!(epoch_2.buffer_deferred_ops_len(file_id)?, 1);

        epoch_2.apply_ops(Some(edit_2_op.clone()), &mut clock_2)?;
        assert_eq!(epoch_2.buffer_deferred_ops_len(file_id)?, 2);

        epoch_2.apply_ops(Some(edit_1_op.clone()), &mut clock_2)?;
        assert_eq!(epoch_2.buffer_deferred_ops_len(file_id)?, 0);

        // If the buffer has never been opened, we can't determine how many operations are deferred.
        let replica_3_id = Uuid::from_u128(3);
        let mut epoch_3 = Epoch::with_replica_id(replica_3_id);
        let mut clock_3 = time::Lamport::new(replica_3_id);
        epoch_3.apply_ops(Some(new_file_op), &mut clock_3)?;
        epoch_3.apply_ops(Some(edit_3_op), &mut clock_3)?;
        assert!(epoch_3.buffer_deferred_ops_len(file_id).is_err());

        Ok(())
    }

    #[test]
    fn test_replication_random() {
        use crate::tests::Network;

        const PEERS: usize = 5;

        for seed in 0..100 {
            println!("SEED: {:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut base_epoch = Epoch::with_replica_id(Uuid::nil());
            base_epoch.randomly_mutate(&mut rng, &mut time::Lamport::new(Uuid::nil()), 20);
            let base_entries = base_epoch.entries();
            let base_entries = base_entries
                .iter()
                .filter(|entry| entry.visible)
                .map(|entry| DirEntry {
                    depth: entry.depth,
                    name: entry.name.as_ref().clone(),
                    file_type: entry.file_type,
                })
                .collect::<Vec<_>>();

            let mut base_epoch = Epoch::with_replica_id(Uuid::nil());
            base_epoch
                .append_base_entries(base_entries.clone(), &mut time::Lamport::new(Uuid::nil()))
                .unwrap();

            let mut replica_ids = Vec::new();
            let mut epochs = Vec::new();
            let mut lamport_clocks = Vec::new();
            let mut base_entries_to_append = Vec::new();
            let mut network = Network::new();
            for i in 0..PEERS {
                let replica_id = Uuid::from_u128((i + 1) as u128);
                replica_ids.push(replica_id);
                epochs.push(Epoch::with_replica_id(replica_id));
                lamport_clocks.push(time::Lamport::new(replica_id));
                base_entries_to_append.push(base_entries.clone());
                network.add_peer(replica_id);
            }

            // Generate and deliver random mutations
            for _ in 0..10 {
                let k = rng.gen_range(0, 10);
                let replica_index = rng.gen_range(0, PEERS);
                let replica_id = replica_ids[replica_index];
                let epoch = &mut epochs[replica_index];
                let lamport_clock = &mut lamport_clocks[replica_index];
                let base_entries_to_append = &mut base_entries_to_append[replica_index];

                if k < 3 && !base_entries_to_append.is_empty() {
                    let count = rng.gen_range(0, base_entries_to_append.len());
                    let fixup_ops = epoch
                        .append_base_entries(base_entries_to_append.drain(0..count), lamport_clock)
                        .unwrap();
                    network.broadcast(replica_id, fixup_ops, &mut rng);
                } else if k < 6 && network.has_unreceived(replica_id) {
                    let fixup_ops = epoch
                        .apply_ops(network.receive(replica_id, &mut rng), lamport_clock)
                        .unwrap();
                    network.broadcast(replica_id, fixup_ops, &mut rng);
                } else if k < 7 && !network.all_messages().is_empty() {
                    network.clear_unreceived(replica_id);
                    *base_entries_to_append = base_entries.clone();
                    *epoch = Epoch::with_replica_id(epoch.local_clock.replica_id);
                    let fixup_ops = epoch
                        .apply_ops(network.all_messages().iter().cloned(), lamport_clock)
                        .unwrap();
                    network.broadcast(replica_id, fixup_ops, &mut rng);
                } else {
                    let ops = epoch.randomly_mutate(&mut rng, lamport_clock, 5);
                    network.broadcast(replica_id, ops, &mut rng);
                }
            }

            // Allow system to quiesce
            loop {
                let mut done = true;
                for replica_index in 0..PEERS {
                    let replica_id = replica_ids[replica_index];
                    let epoch = &mut epochs[replica_index];
                    let lamport_clock = &mut lamport_clocks[replica_index];
                    let base_entries_to_append = &mut base_entries_to_append[replica_index];
                    if !base_entries_to_append.is_empty() {
                        let fixup_ops = epoch
                            .append_base_entries(base_entries_to_append.drain(..), lamport_clock)
                            .unwrap();
                        network.broadcast(replica_id, fixup_ops, &mut rng);
                    }

                    if network.has_unreceived(replica_id) {
                        let fixup_ops = epoch
                            .apply_ops(network.receive(replica_id, &mut rng), lamport_clock)
                            .unwrap();
                        network.broadcast(replica_id, fixup_ops, &mut rng);
                        done = false;
                    }
                }

                if done {
                    break;
                }
            }

            for i in 0..PEERS {
                assert!(epochs[i].deferred_ops.is_empty());
            }

            for i in 0..PEERS - 1 {
                assert_eq!(epochs[i].entries(), epochs[i + 1].entries());
            }

            for i in 0..PEERS {
                for _ in 0..rng.gen_range(0, 5) {
                    let base_file_id =
                        FileId::Base(rng.gen_range(0, base_entries.len() as u64 + 1));
                    assert_eq!(
                        epochs[i].base_path(base_file_id).unwrap(),
                        base_epoch.path(base_file_id).unwrap()
                    );
                }
            }
        }
    }

    impl Epoch {
        pub fn with_replica_id(replica_id: ReplicaId) -> Self {
            Epoch::new(replica_id, Id::default(), None)
        }

        pub fn entries(&self) -> Vec<CursorEntry> {
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

        pub fn dir_entries(&self) -> Vec<DirEntry> {
            let mut entries = Vec::new();
            if let Some(mut cursor) = self.cursor() {
                loop {
                    let entry = cursor.entry().unwrap();
                    let advanced = if entry.visible {
                        entries.push(entry.into());
                        cursor.next(true)
                    } else {
                        cursor.next(false)
                    };

                    if !advanced {
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

        pub fn randomly_mutate<T: Rng>(
            &mut self,
            rng: &mut T,
            lamport_clock: &mut time::Lamport,
            count: usize,
        ) -> Vec<Operation> {
            let mut ops = Vec::new();
            for _ in 0..count {
                let k = rng.gen_range(0, 4);
                if self.child_refs.is_empty() || k == 0 {
                    // println!("Random mutation: Creating file");
                    let parent_id = self
                        .select_file(rng, Some(FileType::Directory), true)
                        .unwrap();

                    loop {
                        let name = gen_name(rng);
                        let file_type = if rng.gen() {
                            FileType::Directory
                        } else {
                            FileType::Text
                        };

                        match self.create_file(parent_id, name, file_type, lamport_clock) {
                            Ok(op) => {
                                ops.push(op);
                                break;
                            }
                            Err(_) => {}
                        }
                    }
                } else if k == 1 {
                    let file_id = self.select_file(rng, None, false).unwrap();
                    // println!("Random mutation: Removing {:?}", file_id);
                    ops.push(self.remove(file_id, lamport_clock).unwrap());
                } else if k == 2 {
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
                        match self.rename(file_id, new_parent_id, new_name, lamport_clock) {
                            Ok(op) => {
                                ops.push(op);
                                break;
                            }
                            Err(_error) => {}
                        }
                    }
                } else if k == 3 && self.text_files.values().any(|f| f.is_buffered()) {
                    let buffered_file_ids = self
                        .text_files
                        .iter()
                        .filter_map(|(file_id, file)| {
                            if file.is_buffered() {
                                Some(*file_id)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    let file_id = *rng.choose(&buffered_file_ids).unwrap();
                    let op = self
                        .mutate_buffer(
                            file_id,
                            lamport_clock,
                            |buffer, local_clock, lamport_clock| {
                                let (_, _, ops) =
                                    buffer.randomly_mutate(rng, local_clock, lamport_clock);
                                Ok(ops)
                            },
                        )
                        .unwrap();
                    ops.push(op);
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

    impl From<CursorEntry> for DirEntry {
        fn from(entry: CursorEntry) -> Self {
            Self {
                depth: entry.depth,
                name: entry.name.as_ref().clone(),
                file_type: entry.file_type,
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
