use btree::{self, SeekBias};
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::ops::{Add, AddAssign};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use time;
use ReplicaId;

const ROOT_FILE_ID: FileId = FileId::Base(0);

#[derive(Clone)]
pub struct WorkTree {
    base_entries_next_id: u64,
    base_entries_stack: Vec<FileId>,
    metadata: btree::Tree<Metadata>,
    parent_refs: btree::Tree<ParentRefValue>,
    child_refs: btree::Tree<ChildRefValue>,
    local_clock: time::Local,
    lamport_clock: time::Lamport,
}

pub struct Cursor {
    metadata_cursor: btree::Cursor<Metadata>,
    stack: Vec<btree::Cursor<ChildRefValue>>,
    work_tree: WorkTree,
}

pub struct CursorEntry {
    file_id: FileId,
    file_type: FileType,
    depth: usize,
    name: Arc<OsString>,
    status: FileStatus,
}

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
    InvalidCursor,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            base_entries_next_id: 1,
            base_entries_stack: Vec::new(),
            metadata: btree::Tree::new(),
            parent_refs: btree::Tree::new(),
            child_refs: btree::Tree::new(),
            local_clock: time::Local::new(replica_id),
            lamport_clock: time::Lamport::new(replica_id),
        }
    }

    pub fn cursor(&self) -> Option<Cursor> {
        let mut cursor = Cursor {
            stack: Vec::new(),
            metadata_cursor: self.metadata.cursor(),
            work_tree: self.clone(),
        };
        if cursor.descend_into(self.child_refs.cursor(), ROOT_FILE_ID) {
            Some(cursor)
        } else {
            None
        }
    }

    pub fn append_base_entries<I>(&mut self, entries: I) -> Vec<Operation>
    where
        I: IntoIterator<Item = DirEntry>,
    {
        let mut metadata_edits = Vec::new();
        let mut parent_ref_edits = Vec::new();
        let mut child_ref_edits = Vec::new();

        let mut child_ref_cursor = self.child_refs.cursor();
        let mut name_conflicts = HashSet::new();

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
        fixup_ops
    }

    pub fn apply_ops<I>(&mut self, ops: I) -> Vec<Operation>
    where
        I: IntoIterator<Item = Operation>,
    {
        let mut changed_file_ids = HashSet::new();
        for op in ops {
            match &op {
                Operation::UpdateParent { child_id, .. } => {
                    changed_file_ids.insert(*child_id);
                }
                _ => {}
            }
            self.apply_op(op);
        }

        let mut fixup_ops = Vec::new();
        for file_id in changed_file_ids {
            fixup_ops.extend(self.fix_conflicts(file_id));
        }
        fixup_ops
    }

    pub fn apply_op(&mut self, op: Operation) {
        match op {
            Operation::InsertMetadata { file_id, file_type } => {
                self.metadata.insert(Metadata { file_id, file_type });
            }
            Operation::UpdateParent {
                child_id,
                timestamp,
                new_parent,
            } => {
                self.lamport_clock.observe(timestamp);

                let mut child_ref_edits: SmallVec<[_; 3]> = SmallVec::new();

                let mut parent_ref_cursor = self.parent_refs.cursor();
                if parent_ref_cursor.seek(&child_id, SeekBias::Left) {
                    let parent_ref = parent_ref_cursor.item().unwrap();
                    if timestamp > parent_ref.timestamp {
                        if let Some((parent_id, name)) = parent_ref.parent {
                            let seek_key = ChildRefValueKey {
                                parent_id,
                                name,
                                visible: true,
                                timestamp: parent_ref.timestamp,
                            };
                            let mut child_ref_cursor = self.child_refs.cursor();
                            child_ref_cursor.seek(&seek_key, SeekBias::Left);
                            let mut child_ref = child_ref_cursor.item().unwrap();
                            child_ref_edits.push(btree::Edit::Remove(child_ref.clone()));
                            if new_parent.is_none() {
                                child_ref.visible = false;
                                child_ref_edits.push(btree::Edit::Insert(child_ref));
                            }
                        }
                    } else {
                        return;
                    }
                }

                self.parent_refs.insert(ParentRefValue {
                    child_id,
                    timestamp,
                    parent: new_parent.clone(),
                });
                if let Some((parent_id, name)) = new_parent {
                    child_ref_edits.push(btree::Edit::Insert(ChildRefValue {
                        parent_id,
                        name,
                        timestamp,
                        child_id,
                        visible: true,
                    }));
                }
                self.child_refs.edit(&mut child_ref_edits);
            }
        }
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
    ) -> Result<SmallVec<[Operation; 1]>, Error> {
        self.check_file_id(file_id, None)?;
        self.check_file_id(file_id, Some(FileType::Directory))?;

        let operation = Operation::UpdateParent {
            child_id: file_id,
            timestamp: self.lamport_time(),
            new_parent: Some((new_parent_id, Arc::new(new_name.into()))),
        };
        let fixup_ops = self.apply_ops(Some(operation.clone()));
        let mut operations = SmallVec::new();
        operations.push(operation);
        operations.extend(fixup_ops);
        Ok(operations)
    }

    pub fn remove(&mut self, file_id: FileId) -> Result<Operation, Error> {
        self.check_file_id(file_id, None)?;

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

    pub fn status(&self, file_id: FileId) -> Result<FileStatus, Error> {
        match file_id {
            FileId::Base(_) => {
                let mut cursor = self.parent_refs.cursor();
                if cursor.seek(&file_id, SeekBias::Left) {
                    let newest_parent_ref_value = cursor.item().unwrap();
                    cursor.seek(&file_id, SeekBias::Right);
                    cursor.prev();
                    let oldest_parent_ref_value = cursor.item().unwrap();
                    if newest_parent_ref_value.parent == oldest_parent_ref_value.parent {
                        Ok(FileStatus::Unchanged)
                    } else if newest_parent_ref_value.parent.is_some() {
                        Ok(FileStatus::Renamed)
                    } else {
                        Ok(FileStatus::Removed)
                    }
                } else {
                    Err(Error::InvalidFileId)
                }
            }
            FileId::New(_) => Ok(FileStatus::New),
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

    fn check_file_id(
        &self,
        file_id: FileId,
        expected_file_type: Option<FileType>,
    ) -> Result<(), Error> {
        let mut cursor = self.metadata.cursor();
        if cursor.seek(&file_id, SeekBias::Left) {
            if let Some(expected_file_type) = expected_file_type {
                let metadata = cursor.item().unwrap();
                if metadata.file_type == expected_file_type {
                    Ok(())
                } else {
                    Err(Error::InvalidFileId)
                }
            } else {
                Ok(())
            }
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
                        cursor.seek(&parent_id, SeekBias::Left);
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
                timestamp: self.lamport_time(),
                new_parent: cursor.item().unwrap().parent,
            });
            moved_file_ids.push(*child_id);
        }

        for op in &fixup_ops {
            self.apply_op(op.clone());
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
                        timestamp: self.lamport_time(),
                        new_parent: Some((parent_id, unique_name.clone())),
                    };
                    self.apply_op(fixup_op.clone());
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
    fn next(&mut self, can_descend: bool) -> bool {
        if !self.stack.is_empty() {
            let metadata = self.metadata_cursor.item().unwrap();
            if !can_descend || metadata.file_type != FileType::Directory || !self.descend() {
                while !self.stack.is_empty() && !self.next_sibling() {
                    self.stack.pop();
                }
            }
        }

        !self.stack.is_empty()
    }

    fn entry(&self) -> Result<CursorEntry, Error> {
        let metadata = self.metadata_cursor.item().unwrap();
        let child_ref_cursor = self.stack.last().ok_or(Error::InvalidCursor)?;
        let child_ref = child_ref_cursor.item().unwrap();
        Ok(CursorEntry {
            file_id: metadata.file_id,
            file_type: metadata.file_type,
            name: child_ref.name,
            depth: self.stack.len(),
            status: self.work_tree.status(metadata.file_id)?,
        })
    }

    fn descend(&mut self) -> bool {
        let mut cursor = self.stack.last().unwrap().clone();
        let dir_id = cursor.item().unwrap().child_id;
        self.descend_into(cursor, dir_id)
    }

    fn descend_into(
        &mut self,
        mut child_ref_cursor: btree::Cursor<ChildRefValue>,
        dir_id: FileId,
    ) -> bool {
        child_ref_cursor.seek(&dir_id, SeekBias::Left);
        if let Some(child_ref) = child_ref_cursor.item() {
            if child_ref.parent_id == dir_id {
                self.stack.push(child_ref_cursor.clone());

                let child_id = child_ref.child_id;
                if child_ref.visible {
                    self.metadata_cursor.seek(&child_id, SeekBias::Left);
                    true
                } else if self.next_sibling() {
                    true
                } else {
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
                self.metadata_cursor
                    .seek(&child_ref.child_id, SeekBias::Left);
                return true;
            } else {
                break;
            }
        }

        false
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
