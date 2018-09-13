use btree::{self, SeekBias};
use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::ops::{Add, AddAssign};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use time;

const ROOT_FILE_ID: FileId = FileId::New(time::Local::DEFAULT);

pub struct WorkTree {
    base_entries_index: u64,
    base_entries_stack: Vec<FileId>,
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRefValue>,
    child_refs: btree::Tree<ChildRefValue>,
    local_clock: time::Local,
    lamport_clock: time::Lamport,
}

pub struct Cursor {}

pub struct DirEntry {
    depth: usize,
    name: OsString,
    file_type: FileType,
}

#[derive(Clone)]
pub enum Operation {
    InsertMetadata {
        file_id: FileId,
        file_type: FileType,
    },
    UpdateParent {
        child_id: FileId,
        timestamp: time::Lamport,
        new_parent: Option<(FileId, Arc<OsString>)>,
    },
}

#[derive(Debug)]
pub enum Error {
    InvalidPath,
    InvalidFileId,
}

#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum FileId {
    Base(u64),
    New(time::Local),
}

pub enum FileStatus {
    New,
    Renamed,
    Removed,
    Modified,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

impl WorkTree {
    pub fn append_base_entries<I>(&mut self, entries: I)
    where
        I: IntoIterator<Item = DirEntry>,
    {
        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        for entry in entries {
            let depth = self.base_entries_stack.len();
            assert!(entry.depth <= depth || entry.depth == depth + 1);
            self.base_entries_stack.truncate(entry.depth);

            let parent_id = self
                .base_entries_stack
                .last()
                .cloned()
                .unwrap_or(ROOT_FILE_ID);
            let name = Arc::new(entry.name);
            let file_id = FileId::Base(self.base_entries_index);
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
                name,
                timestamp: time::Lamport::min_value(),
                child_id: file_id,
                visible: true,
            }));

            self.base_entries_index += 1;
            if entry.file_type == FileType::Directory {
                self.base_entries_stack.push(file_id);
            }
        }

        self.metadata.edit(&mut metadata_edits);
        self.parent_refs.edit(&mut parent_ref_edits);
        self.child_refs.edit(&mut child_ref_edits);
    }

    pub fn apply_ops<I>(&mut self, ops: I) -> Vec<Operation>
    where
        I: IntoIterator<Item = Operation>,
    {
        unimplemented!()
    }

    pub fn apply_op(&mut self, op: Operation) {
        unimplemented!()
    }

    pub fn new_text_file(&mut self) -> (FileId, Operation) {
        let file_id = FileId::New(self.local_time());
        let operation = Operation::InsertMetadata {
            file_id,
            file_type: FileType::Text,
        };
        self.apply_op(operation.clone());
        (file_id, operation)
    }

    pub fn new_dir(&mut self) -> (FileId, Operation) {
        let file_id = FileId::New(self.local_time());
        let operation = Operation::InsertMetadata {
            file_id,
            file_type: FileType::Directory,
        };
        self.apply_op(operation.clone());
        (file_id, operation)
    }

    pub fn rename(
        &mut self,
        file_id: FileId,
        new_parent_id: FileId,
        new_name: &OsStr,
    ) -> Result<Operation, Error> {
        self.check_file_id(file_id)?;

        let operation = Operation::UpdateParent {
            child_id: file_id,
            timestamp: self.lamport_time(),
            new_parent: Some((new_parent_id, Arc::new(new_name.into()))),
        };
        self.apply_op(operation.clone());
        Ok(operation)
    }

    pub fn remove(&mut self, file_id: FileId) -> Result<Operation, Error> {
        self.check_file_id(file_id)?;

        let operation = Operation::UpdateParent {
            child_id: file_id,
            timestamp: self.lamport_time(),
            new_parent: None,
        };
        self.apply_op(operation.clone());
        Ok(operation)
    }

    pub fn file_id(&self, path: &Path) -> Result<FileId, Error> {
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

    pub fn status(&self, file_id: FileId) -> Result<FileId, Error> {
        unimplemented!()
    }

    fn local_time(&mut self) -> time::Local {
        self.local_clock.tick();
        self.local_clock
    }

    fn lamport_time(&mut self) -> time::Lamport {
        self.lamport_clock.tick();
        self.lamport_clock
    }

    fn check_file_id(&self, file_id: FileId) -> Result<(), Error> {
        let mut cursor = self.metadata.cursor();
        if cursor.seek(&file_id, SeekBias::Left) {
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
}

impl Cursor {
    fn next(&mut self, descend: bool) -> bool {
        unimplemented!()
    }

    fn id(&self) -> FileId {
        unimplemented!()
    }

    fn name(&self) -> Arc<OsString> {
        unimplemented!()
    }

    fn depth(&self) -> usize {
        unimplemented!()
    }

    fn status(&self) -> FileStatus {
        unimplemented!()
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
        assert!(*self < *other);
        *self = other.clone();
    }
}

impl<'a> Add<&'a Self> for FileId {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self < *other);
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
