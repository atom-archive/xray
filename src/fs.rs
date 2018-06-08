use btree::{self, SeekBias};
use id;
use smallvec::SmallVec;
use std::cmp::{Ord, Ordering};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::{Add, AddAssign};
use std::sync::Arc;

pub trait Store {
    type ReadError: fmt::Debug;
    type EntryStore: btree::NodeStore<Entry, ReadError = Self::ReadError>;
    type PositionStore: btree::NodeStore<EntryIdToPosition, ReadError = Self::ReadError>;

    fn gen_id(&self) -> id::Unique;
    fn entry_store(&self) -> &Self::EntryStore;
    fn position_store(&self) -> &Self::PositionStore;
}

pub struct Tree {
    entries: btree::Tree<Entry>,
    positions_by_entry_id: btree::Tree<EntryIdToPosition>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Entry {
    id: id::Unique,
    position: id::Ordered,
    component: Arc<PathComponent>,
}

#[derive(Clone, Debug, Eq, PartialEq, PartialOrd)]
pub enum PathComponent {
    File { name: OsString, inode: Option<u64> },
    Dir { name: OsString, inode: Option<u64> },
    ParentDir,
}

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct EntrySummary {
    position: id::Ordered,
    path: RelativePath,
}

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct RelativePath(SmallVec<[Arc<PathComponent>; 1]>);

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
    fn new<S: Store>(root: PathComponent, db: &S) -> Result<Self, Error<S>> {
        let entry_db = db.entry_store();
        let position_db = db.position_store();
        let mut tree = Self {
            entries: btree::Tree::new(),
            positions_by_entry_id: btree::Tree::new(),
        };

        let root_entry_id = db.gen_id();
        let root_entry = Entry {
            id: root_entry_id,
            position: id::Ordered::min_value(),
            component: Arc::new(root),
        };
        tree.entries
            .push(root_entry, entry_db)
            .map_err(|err| Error::StoreReadError(err))?;
        let root_parent_entry_id = db.gen_id();
        let root_parent_entry = Entry {
            id: root_parent_entry_id,
            position: id::Ordered::max_value(),
            component: Arc::new(PathComponent::ParentDir),
        };
        tree.entries
            .push(root_parent_entry, entry_db)
            .map_err(|err| Error::StoreReadError(err))?;

        tree.positions_by_entry_id
            .push(
                EntryIdToPosition {
                    entry_id: root_entry_id,
                    position: id::Ordered::min_value(),
                },
                position_db,
            )
            .map_err(|err| Error::StoreReadError(err))?;
        tree.positions_by_entry_id
            .push(
                EntryIdToPosition {
                    entry_id: root_parent_entry_id,
                    position: id::Ordered::max_value(),
                },
                position_db,
            )
            .map_err(|err| Error::StoreReadError(err))?;

        Ok(tree)
    }

    fn insert<I, S>(&mut self, path: RelativePath, iter: I, db: &S) -> Result<(), Error<S>>
    where
        I: IntoIterator<Item = PathComponent>,
        S: Store,
    {
        self.validate_path(&path, db)?;

        let entry_db = db.entry_store();
        let mut cursor = self.entries.cursor();
        let mut new_entries = cursor
            .slice(&path, SeekBias::Left, entry_db)
            .map_err(|error| Error::StoreReadError(error))?;

        if cursor.start::<RelativePath>() == path {
            Err(Error::DuplicatePath)
        } else {
            let prev_entry = new_entries
                .last(entry_db)
                .map_err(|error| Error::StoreReadError(error))?;
            let next_entry = cursor
                .item(entry_db)
                .map_err(|error| Error::StoreReadError(error))?;

            println!("{:?} {:?}", prev_entry, next_entry);

            let positions =
                id::Ordered::between(&prev_entry.unwrap().position, &next_entry.unwrap().position);
            let mut entry_ids_to_positions = Vec::new();

            new_entries
                .extend(
                    iter.into_iter()
                        .zip(positions)
                        .map(|(component, position)| {
                            let id = db.gen_id();
                            entry_ids_to_positions.push(EntryIdToPosition {
                                entry_id: id,
                                position: position.clone(),
                            });
                            Entry {
                                id,
                                position,
                                component: Arc::new(component),
                            }
                        }),
                    entry_db,
                )
                .map_err(|error| Error::StoreReadError(error))?;

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

            for mapping in entry_ids_to_positions {
                let entry_id = mapping.entry_id;
                self.positions_by_entry_id
                    .insert(&entry_id, SeekBias::Left, mapping, db.position_store())
                    .map_err(|error| Error::StoreReadError(error))?;
            }

            Ok(())
        }
    }

    fn validate_path<S>(&self, path: &RelativePath, db: &S) -> Result<(), Error<S>>
    where
        S: Store,
    {
        let root_entry = self.entries
            .first(db.entry_store())
            .map_err(|err| Error::StoreReadError(err))?
            .unwrap();
        if path.0.is_empty() || path.0.first().unwrap().name() != root_entry.component.name() {
            Err(Error::InvalidPath)
        } else {
            Ok(())
        }
    }
}

impl PathComponent {
    fn is_file(&self) -> bool {
        match self {
            PathComponent::File { .. } => true,
            _ => false,
        }
    }

    fn is_dir(&self) -> bool {
        match self {
            PathComponent::Dir { .. } => true,
            _ => false,
        }
    }

    fn name(&self) -> &OsStr {
        match self {
            PathComponent::Dir { ref name, .. } => name,
            PathComponent::File { ref name, .. } => name,
            PathComponent::ParentDir => panic!("ParentDir does not have a name"),
        }
    }
}

impl btree::Item for Entry {
    type Summary = EntrySummary;

    fn summarize(&self) -> Self::Summary {
        EntrySummary {
            position: self.position.clone(),
            path: RelativePath(SmallVec::from_vec(vec![self.component.clone()])),
        }
    }
}

impl<'a> AddAssign<&'a Self> for EntrySummary {
    fn add_assign(&mut self, other: &Self) {
        self.position += &other.position;
        self.path += &other.path;
    }
}

impl<'a> Add<&'a Self> for EntrySummary {
    type Output = Self;

    fn add(mut self, other: &Self) -> Self {
        self += other;
        self
    }
}

impl btree::Dimension for RelativePath {
    type Summary = EntrySummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.path.clone()
    }
}

impl<'a> AddAssign<&'a Self> for RelativePath {
    fn add_assign(&mut self, other: &Self) {
        for other_entry in &other.0 {
            match other_entry.as_ref() {
                PathComponent::File { .. } | PathComponent::Dir { .. } => {
                    if self.0.last().map(|e| e.is_file()).unwrap_or(false) {
                        self.0.pop();
                    }
                    self.0.push(other_entry.clone());
                }
                PathComponent::ParentDir => {
                    if self.0
                        .last()
                        .map(|e| e.is_file() || e.is_dir())
                        .unwrap_or(false)
                    {
                        self.0.pop();
                    } else {
                        self.0.push(other_entry.clone());
                    }
                }
            }
        }
    }
}

impl<'a> Add<&'a Self> for RelativePath {
    type Output = Self;

    fn add(mut self, other: &Self) -> Self {
        self += other;
        self
    }
}

impl Ord for PathComponent {
    fn cmp(&self, other: &Self) -> Ordering {
        match self {
            PathComponent::File {
                name: self_name, ..
            } => match other {
                PathComponent::File {
                    name: other_name, ..
                } => self_name.cmp(other_name),
                PathComponent::Dir { .. } => Ordering::Greater,
                PathComponent::ParentDir { .. } => {
                    panic!("Can't compare paths with parent entries")
                }
            },
            PathComponent::Dir {
                name: self_name, ..
            } => match other {
                PathComponent::File { .. } => Ordering::Less,
                PathComponent::Dir {
                    name: other_name, ..
                } => self_name.cmp(other_name),
                PathComponent::ParentDir => panic!("Can't compare paths with parent entries"),
            },
            PathComponent::ParentDir => panic!("Can't compare paths with parent entries"),
        }
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
        let mut tree = Tree::new(PathComponent::dir("root", None), &db).unwrap();

        assert!(
            tree.insert(RelativePath::from("not-root/foo"), None, &db)
                .is_err()
        );
        tree.insert(
            RelativePath::from("root/foo"),
            Some(PathComponent::file("foo", None)),
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

    impl btree::NodeStore<Entry> for NullStore {
        type ReadError = ();

        fn get(&self, id: btree::NodeId) -> Result<Arc<btree::Node<Entry>>, Self::ReadError> {
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

    impl PathComponent {
        fn dir<P: Into<OsString>>(name: P, inode: Option<u64>) -> Self {
            PathComponent::Dir {
                name: name.into(),
                inode,
            }
        }

        fn file<P: Into<OsString>>(name: P, inode: Option<u64>) -> Self {
            PathComponent::File {
                name: name.into(),
                inode,
            }
        }

        fn parent() -> Self {
            PathComponent::ParentDir
        }
    }

    impl<'a> From<&'a str> for RelativePath {
        fn from(path: &'a str) -> Self {
            let path = PathBuf::from(path.to_string());
            let mut iter = path.components().peekable();
            let mut components = SmallVec::new();

            while let Some(component) = iter.next() {
                match component {
                    Component::Normal(name) => if iter.peek().is_some() {
                        components.push(Arc::new(PathComponent::Dir {
                            name: name.to_owned(),
                            inode: None,
                        }));
                    } else {
                        components.push(Arc::new(PathComponent::File {
                            name: name.to_owned(),
                            inode: None,
                        }));
                    },
                    _ => panic!("Unexpected component: {:?}", component),
                }
            }

            RelativePath(components)
        }
    }
}
