use btree::{self, SeekBias};
use buffer::Text;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::ops::{Add, AddAssign, Range};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use time;
use ReplicaId;

type FileId = time::Local;

pub const ROOT_FILE_ID: time::Local = time::Local::DEFAULT;

pub struct Patch {
    local_clock: time::Local,
    lamport_clock: time::Lamport,
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRefValue>,
    child_refs: btree::Tree<ChildRefValue>,
    file_aliases: HashMap<FileId, FileId>,
}

#[derive(Clone, Debug)]
pub enum Operation {
    RegisterBasePath {
        parent_id: FileId,
        components: SmallVec<[(Arc<OsString>, FileId); 32]>,
        timestamp: time::Lamport,
    },
}

pub struct Changes {
    inserted: HashSet<FileId>,
    renamed: HashSet<FileId>,
    removed: HashSet<FileId>,
    edited: HashSet<FileId>,
}

#[derive(Debug)]
pub enum Error {
    InvalidPath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Metadata {
    id: FileId,
    is_dir: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParentRefValue {
    child_id: FileId,
    timestamp: time::Lamport,
    prev_timestamp: time::Lamport,
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
    deletions: SmallVec<[time::Local; 1]>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildRefValueSummary {
    parent_id: time::Local,
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
    parent_id: time::Local,
    name: Arc<OsString>,
}

impl Patch {
    pub fn new(replica_id: ReplicaId) -> Self {
        Patch {
            local_clock: time::Local::new(replica_id),
            lamport_clock: time::Lamport::new(replica_id),
            metadata: btree::Tree::new(),
            parent_refs: btree::Tree::new(),
            child_refs: btree::Tree::new(),
            file_aliases: HashMap::new(),
        }
    }

    pub fn file_id(&mut self, path: &Path) -> Result<(FileId, Option<Operation>), Error> {
        let mut cursor = self.child_refs.cursor();
        let mut parent_id = ROOT_FILE_ID;
        let mut operation_parent_id = None;
        let mut components = SmallVec::new();

        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    let name = Arc::new(OsString::from(name));

                    if operation_parent_id.is_none() {
                        let name = name.clone();
                        if cursor.seek(&ChildRefKey { parent_id, name }, SeekBias::Left) {
                            let child_ref = cursor.item().unwrap();
                            if child_ref.is_visible() {
                                parent_id = child_ref.child_id;
                            } else {
                                return Err(Error::InvalidPath);
                            }
                        } else {
                            operation_parent_id = Some(parent_id);
                        }
                    }

                    if operation_parent_id.is_some() {
                        let file_id = self.local_time();
                        components.push((name.clone(), file_id));
                        parent_id = file_id;
                    }
                }
                _ => return Err(Error::InvalidPath),
            }
        }

        let operation = operation_parent_id.map(|parent_id| {
            let operation = Operation::RegisterBasePath {
                parent_id,
                components,
                timestamp: self.lamport_time(),
            };
            self.integrate_ops(Some(operation.clone()));
            operation
        });

        Ok((parent_id, operation))
    }

    pub fn new_directory<T>(&mut self) -> (FileId, Operation)
    where
        T: Into<Text>,
    {
        unimplemented!()
    }

    pub fn new_text_file<T>(&mut self, text: T) -> (FileId, Operation)
    where
        T: Into<Text>,
    {
        unimplemented!()
    }

    fn rename(&mut self, file_id: FileId, new_parent_id: FileId, new_name: &OsStr) -> Operation {
        unimplemented!()
    }

    fn remove(&mut self, file_id: FileId) -> Operation {
        unimplemented!()
    }

    fn edit<'a, I, T>(&mut self, file_id: FileId, old_ranges: I, new_text: T)
    where
        I: IntoIterator<Item = &'a Range<usize>>,
        T: Into<Text>,
    {
        unimplemented!()
    }

    fn path(&self, file_id: FileId) -> Option<PathBuf> {
        let file_id = self.resolve_file_alias(file_id);
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

    fn base_path(&self, file_id: FileId) -> Option<PathBuf> {
        unimplemented!()
    }

    fn integrate_ops<I>(&mut self, ops: I) -> (Changes, Vec<Operation>)
    where
        I: IntoIterator<Item = Operation>,
    {
        let mut changes = Changes {
            inserted: HashSet::new(),
            renamed: HashSet::new(),
            removed: HashSet::new(),
            edited: HashSet::new(),
        };

        for op in ops {
            self.integrate_op(op, &mut changes);
        }

        (changes, Vec::new())
    }

    fn integrate_op(&mut self, op: Operation, changes: &mut Changes) {
        match op {
            Operation::RegisterBasePath {
                mut parent_id,
                components,
                timestamp,
            } => {
                let mut metadata_edits = Vec::new();
                let mut parent_ref_edits = Vec::new();
                let mut child_ref_edits = Vec::new();
                let mut cursor = self.child_refs.cursor();
                let mut parent_exists = true;

                for (name, file_id) in components {
                    if parent_exists && cursor.seek(
                        &ChildRefKey {
                            parent_id,
                            name: name.clone(),
                        },
                        SeekBias::Right,
                    ) {
                        cursor.prev();
                        let existing_file_id = cursor.item().unwrap().child_id;
                        self.file_aliases.insert(file_id, existing_file_id);
                        parent_id = existing_file_id;
                    } else {
                        parent_exists = false;
                        metadata_edits.push(btree::Edit::Insert(Metadata {
                            id: file_id,
                            is_dir: None,
                        }));
                        parent_ref_edits.push(btree::Edit::Insert(ParentRefValue {
                            child_id: file_id,
                            timestamp,
                            prev_timestamp: timestamp,
                            parent: Some((parent_id, name.clone())),
                        }));
                        child_ref_edits.push(btree::Edit::Insert(ChildRefValue {
                            parent_id,
                            name,
                            timestamp,
                            child_id: file_id,
                            deletions: SmallVec::new(),
                        }));
                        parent_id = file_id;
                    }
                }

                self.metadata.edit(metadata_edits);
                self.parent_refs.edit(parent_ref_edits);
                self.child_refs.edit(child_ref_edits);
            }
        }
    }

    fn local_time(&mut self) -> time::Local {
        self.local_clock.tick();
        self.local_clock
    }

    fn lamport_time(&mut self) -> time::Lamport {
        self.lamport_clock.tick();
        self.lamport_clock
    }

    fn resolve_file_alias(&self, file_id: FileId) -> FileId {
        *self.file_aliases.get(&file_id).unwrap_or(&file_id)
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

impl btree::Item for Metadata {
    type Summary = time::Local;

    fn summarize(&self) -> Self::Summary {
        use btree::KeyedItem;
        self.key()
    }
}

impl btree::KeyedItem for Metadata {
    type Key = time::Local;

    fn key(&self) -> Self::Key {
        self.id
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

impl btree::Dimension<ParentRefValueKey> for time::Local {
    fn from_summary(summary: &ParentRefValueKey) -> Self {
        summary.child_id
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
    type Key = ChildRefValueKey;

    fn key(&self) -> Self::Key {
        ChildRefValueKey {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aliases() {
        let mut patch_1 = Patch::new(1);
        let mut patch_2 = Patch::new(2);

        let (id_1, op_1) = patch_1.file_id(&PathBuf::from("/a/b")).unwrap();
        assert!(op_1.is_some());
        let (id_2, op_2) = patch_2.file_id(&PathBuf::from("a/b/c")).unwrap();
        assert!(op_2.is_some());

        patch_1.integrate_ops(op_2);
        patch_2.integrate_ops(op_1);

        assert_eq!(patch_1.path(id_1), Some(PathBuf::from("a/b")));
        assert_eq!(patch_1.path(id_2), Some(PathBuf::from("a/b/c")));
        assert_eq!(patch_2.path(id_1), Some(PathBuf::from("a/b")));
        assert_eq!(patch_2.path(id_2), Some(PathBuf::from("a/b/c")));
    }
}
