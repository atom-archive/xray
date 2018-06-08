use btree::{self, SeekBias};
use id;
use std::cmp::{Ord, Ordering};
use std::ffi::OsString;
use std::fmt;
use std::ops::{Add, AddAssign};
use std::path::{self, Path};
use std::sync::Arc;

pub trait Store {
    type ReadError: fmt::Debug;
    type EntryStore: btree::NodeStore<TreeEntry, ReadError = Self::ReadError>;
    type PositionStore: btree::NodeStore<EntryIdToPosition, ReadError = Self::ReadError>;

    fn gen_id(&self) -> id::Unique;
    fn entry_store(&self) -> &Self::EntryStore;
    fn position_store(&self) -> &Self::PositionStore;
}

pub struct Entry {
    name: OsString,
    depth: usize,
    inode: u64,
    kind: EntryKind,
}

pub enum EntryKind {
    Dir,
    File,
}

pub struct Tree {
    entries: btree::Tree<TreeEntry>,
    positions_by_entry_id: btree::Tree<EntryIdToPosition>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Walk(Vec<Step>);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Step {
    VisitFile(Arc<OsString>),
    VisitDir(Arc<OsString>),
    VisitParent,
}

#[derive(Clone, Debug, Eq, PartialEq, PartialOrd)]
pub enum TreeEntry {
    File {
        id: id::Unique,
        name: Arc<OsString>,
        inode: Option<u64>,
        position: id::Ordered,
    },
    Dir {
        id: id::Unique,
        name: Arc<OsString>,
        inode: Option<u64>,
        position: id::Ordered,
    },
    ParentDir {
        position: id::Ordered,
    },
}

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct EntrySummary {
    position: id::Ordered,
    walk: Walk,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntryIdToPosition {
    entry_id: id::Unique,
    position: id::Ordered,
}

#[derive(Debug)]
enum Error<S: Store> {
    InvalidPath,
    DuplicatePath,
    StoreReadError(S::ReadError),
}

impl Tree {
    fn new<N: Into<OsString>, S: Store>(root_name: N, db: &S) -> Result<Self, Error<S>> {
        let entry_db = db.entry_store();
        let mut tree = Self {
            entries: btree::Tree::new(),
            positions_by_entry_id: btree::Tree::new(),
        };

        let root_entry_id = db.gen_id();
        tree.entries
            .push(
                TreeEntry::Dir {
                    id: root_entry_id,
                    name: Arc::new(root_name.into()),
                    inode: None,
                    position: id::Ordered::min_value(),
                },
                entry_db,
            )
            .map_err(|err| Error::StoreReadError(err))?;
        tree.entries
            .push(
                TreeEntry::ParentDir {
                    position: id::Ordered::max_value(),
                },
                entry_db,
            )
            .map_err(|err| Error::StoreReadError(err))?;
        tree.positions_by_entry_id
            .push(
                EntryIdToPosition {
                    entry_id: root_entry_id,
                    position: id::Ordered::min_value(),
                },
                db.position_store(),
            )
            .map_err(|err| Error::StoreReadError(err))?;

        Ok(tree)
    }

    fn insert_dir<'a, I, P, S>(&mut self, path: P, iter: I, db: &S) -> Result<(), Error<S>>
    where
        P: Into<&'a Path>,
        I: IntoIterator<Item = Entry>,
        S: Store,
    {
        let path = path.into();

        let entry_db = db.entry_store();
        let root = self.entries
            .first(entry_db)
            .map_err(|error| Error::StoreReadError(error))?
            .unwrap();

        let mut walk = Walk(Vec::new());
        walk.0.push(root.into());
        for component in path.components() {
            match component {
                path::Component::Normal(name) => {
                    walk.0.push(Step::VisitDir(Arc::new(name.to_owned())));
                }
                _ => return Err(Error::InvalidPath),
            }
        }

        let mut cursor = self.entries.cursor();
        let mut new_entries = cursor
            .slice(&walk, SeekBias::Left, entry_db)
            .map_err(|error| Error::StoreReadError(error))?;

        if cursor.start::<Walk>() == walk {
            Err(Error::DuplicatePath)
        } else {
            let prev_entry = new_entries
                .last(entry_db)
                .map_err(|error| Error::StoreReadError(error))?;
            let next_entry = cursor
                .item(entry_db)
                .map_err(|error| Error::StoreReadError(error))?;
            let mut position_generator = id::Ordered::between(
                prev_entry.unwrap().position(),
                next_entry.unwrap().position(),
            );
            let mut prev_depth = None;
            for entry in iter {
                if prev_depth.map_or(false, |prev_depth| prev_depth != entry.depth) {
                    let mut prev_depth = prev_depth.unwrap();
                    while prev_depth != entry.depth {
                        let position = position_generator.next().unwrap();
                        new_entries
                            .push(TreeEntry::ParentDir { position }, entry_db)
                            .map_err(|error| Error::StoreReadError(error))?;
                        prev_depth -= 1;
                    }
                }
                prev_depth = Some(entry.depth);

                let entry_id = db.gen_id();
                let entry_position = position_generator.next().unwrap();
                let new_entry = match entry.kind {
                    EntryKind::File => TreeEntry::File {
                        id: entry_id,
                        name: Arc::new(entry.name),
                        inode: Some(entry.inode),
                        position: entry_position.clone(),
                    },
                    EntryKind::Dir => TreeEntry::Dir {
                        id: entry_id,
                        name: Arc::new(entry.name),
                        inode: Some(entry.inode),
                        position: entry_position.clone(),
                    },
                };
                new_entries
                    .push(new_entry, entry_db)
                    .map_err(|error| Error::StoreReadError(error))?;

                let mapping = EntryIdToPosition {
                    entry_id,
                    position: entry_position,
                };
                self.positions_by_entry_id
                    .insert(&entry_id, SeekBias::Left, mapping, db.position_store())
                    .map_err(|error| Error::StoreReadError(error))?;
            }

            let old_tree_extent = self.entries
                .extent::<id::Ordered, _>(entry_db)
                .map_err(|error| Error::StoreReadError(error))?;
            let suffix = cursor
                .slice(&old_tree_extent, SeekBias::Right, entry_db)
                .map_err(|error| Error::StoreReadError(error))?;
            new_entries
                .push_tree(suffix, entry_db)
                .map_err(|error| Error::StoreReadError(error))?;
            self.entries = new_entries;

            Ok(())
        }
    }
}

impl btree::Dimension for Walk {
    type Summary = EntrySummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.walk.clone()
    }
}

impl<'a> AddAssign<&'a Self> for Walk {
    fn add_assign(&mut self, other: &Self) {
        for other_step in &other.0 {
            match other_step {
                Step::VisitDir(_) | Step::VisitFile(_) => self.0.push(other_step.clone()),
                Step::VisitParent => {
                    if self.0
                        .last()
                        .map(|step| step.is_parent_visit())
                        .unwrap_or(true)
                    {
                        self.0.push(other_step.clone());
                    } else {
                        self.0.pop();
                    }
                }
            }
        }
    }
}

impl<'a> Add<&'a Self> for Walk {
    type Output = Self;

    fn add(mut self, other: &Self) -> Self {
        self += other;
        self
    }
}

impl PartialOrd for Walk {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Walk {
    fn cmp(&self, other: &Self) -> Ordering {
        let mut self_iter = self.0.iter();
        let mut other_iter = other.0.iter();
        loop {
            if let Some(self_step) = self_iter.next() {
                if let Some(other_step) = other_iter.next() {
                    let ordering = match self_step {
                        Step::VisitFile(self_name) => match other_step {
                            Step::VisitFile(other_name) => self_name.cmp(other_name),
                            Step::VisitDir(_) => Ordering::Greater,
                            Step::VisitParent => panic!("Can't compare walks with parent entries"),
                        },
                        Step::VisitDir(self_name) => match other_step {
                            Step::VisitFile(_) => Ordering::Less,
                            Step::VisitDir(other_name) => self_name.cmp(other_name),
                            Step::VisitParent => panic!("Can't compare walks with parent entries"),
                        },
                        Step::VisitParent => panic!("Can't compare walks with parent entries"),
                    };

                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                } else {
                    return Ordering::Greater;
                }
            } else {
                if other_iter.next().is_some() {
                    return Ordering::Less;
                } else {
                    return Ordering::Equal;
                }
            }
        }
    }
}

impl Step {
    fn is_parent_visit(&self) -> bool {
        match self {
            Step::VisitParent => true,
            _ => false,
        }
    }
}

impl From<TreeEntry> for Step {
    fn from(entry: TreeEntry) -> Self {
        match entry {
            TreeEntry::Dir { name, .. } => Step::VisitDir(name),
            TreeEntry::File { name, .. } => Step::VisitFile(name),
            TreeEntry::ParentDir { .. } => Step::VisitParent,
        }
    }
}

impl<'a> From<&'a TreeEntry> for Step {
    fn from(entry: &'a TreeEntry) -> Self {
        match entry {
            TreeEntry::Dir { name, .. } => Step::VisitDir(name.clone()),
            TreeEntry::File { name, .. } => Step::VisitFile(name.clone()),
            TreeEntry::ParentDir { .. } => Step::VisitParent,
        }
    }
}

impl TreeEntry {
    fn position(&self) -> &id::Ordered {
        match self {
            TreeEntry::Dir { position, .. } => position,
            TreeEntry::File { position, .. } => position,
            TreeEntry::ParentDir { position, .. } => position,
        }
    }
}

impl btree::Item for TreeEntry {
    type Summary = EntrySummary;

    fn summarize(&self) -> Self::Summary {
        EntrySummary {
            position: self.position().clone(),
            walk: Walk(vec![self.into()]),
        }
    }
}

impl<'a> AddAssign<&'a Self> for EntrySummary {
    fn add_assign(&mut self, other: &Self) {
        self.position += &other.position;
        self.walk += &other.walk;
    }
}

impl<'a> Add<&'a Self> for EntrySummary {
    type Output = Self;

    fn add(mut self, other: &Self) -> Self {
        self += other;
        self
    }
}

impl btree::Item for EntryIdToPosition {
    type Summary = id::Unique;

    fn summarize(&self) -> Self::Summary {
        self.entry_id.clone()
    }
}

impl btree::Dimension for id::Unique {
    type Summary = Self;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.clone()
    }
}

impl btree::Dimension for id::Ordered {
    type Summary = EntrySummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.position.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::{Component, PathBuf};

    #[test]
    fn test_insert() {
        let db = NullStore::new();
        let mut tree = Tree::new("root", &db).unwrap();

        assert!(tree.insert(Walk::from("not-root/foo"), None, &db).is_err());
        tree.insert(
            Walk::from("root/foo"),
            Some(TreeEntry::file("foo", None)),
            &db,
        ).unwrap();
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
        type EntryStore = NullStore;
        type PositionStore = NullStore;

        fn gen_id(&self) -> id::Unique {
            let next_id = self.next_id.borrow().clone();
            self.next_id.borrow_mut().inc();
            next_id
        }

        fn entry_store(&self) -> &Self::EntryStore {
            self
        }

        fn position_store(&self) -> &Self::PositionStore {
            self
        }
    }

    impl btree::NodeStore<TreeEntry> for NullStore {
        type ReadError = ();

        fn get(&self, id: btree::NodeId) -> Result<Arc<btree::Node<TreeEntry>>, Self::ReadError> {
            panic!("get should never be called on a null store")
        }
    }

    impl btree::NodeStore<EntryIdToPosition> for NullStore {
        type ReadError = ();

        fn get(
            &self,
            id: btree::NodeId,
        ) -> Result<Arc<btree::Node<EntryIdToPosition>>, Self::ReadError> {
            panic!("get should never be called on a null store")
        }
    }

    impl TreeEntry {
        fn dir<P: Into<OsString>>(name: P, inode: Option<u64>) -> Self {
            TreeEntry::Dir {
                name: name.into(),
                inode,
            }
        }

        fn file<P: Into<OsString>>(name: P, inode: Option<u64>) -> Self {
            TreeEntry::File {
                name: name.into(),
                inode,
            }
        }

        fn parent() -> Self {
            TreeEntry::ParentDir
        }
    }

    impl<'a> From<&'a str> for Walk {
        fn from(path: &'a str) -> Self {
            let path = PathBuf::from(path.to_string());
            let mut iter = path.components().peekable();
            let mut components = SmallVec::new();

            while let Some(component) = iter.next() {
                match component {
                    Component::Normal(name) => if iter.peek().is_some() {
                        components.push(Arc::new(TreeEntry::Dir {
                            name: name.to_owned(),
                            inode: None,
                        }));
                    } else {
                        components.push(Arc::new(TreeEntry::File {
                            name: name.to_owned(),
                            inode: None,
                        }));
                    },
                    _ => panic!("Unexpected component: {:?}", component),
                }
            }

            Walk(components)
        }
    }
}
