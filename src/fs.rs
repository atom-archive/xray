use btree::{self, SeekBias};
use id;
use std::cmp::{Ord, Ordering};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::{Add, AddAssign};
use std::path::{self, Path, PathBuf};
use std::sync::Arc;

pub trait Store {
    type ReadError: fmt::Debug;
    type EntryStore: btree::NodeStore<TreeEntry, ReadError = Self::ReadError>;
    type PositionStore: btree::NodeStore<EntryIdToPosition, ReadError = Self::ReadError>;

    fn gen_id(&self) -> id::Unique;
    fn entry_store(&self) -> &Self::EntryStore;
    fn position_store(&self) -> &Self::PositionStore;
}

#[derive(Clone)]
pub struct Tree {
    entries: btree::Tree<TreeEntry>,
    positions_by_entry_id: btree::Tree<EntryIdToPosition>,
}

pub struct Cursor {
    tree_cursor: btree::Cursor<TreeEntry>,
    path: PathBuf,
}

// The builder is used to update an existing tree. You create a builder with a tree and a path to
// an existing directory within that tree. You need to include the root as part of that path. For
// an empty tree, you'll just specify a path to its root.
//
// Once you have a builder, you'll update it by calling `push_dir`, `pop_dir`, and `push_file`.
// This will add entries inside the existing directory you specified. It is assumed that you'll
// call methods representing every current directory, so any directories you don't mention that
// were present in the previous tree are implicitly deleted.
pub struct Builder {
    walk: Walk,
    position: id::Ordered,
    old_entries: btree::Cursor<TreeEntry>,
    new_prefix: btree::Tree<TreeEntry>,
    new_entries: Vec<TreeEntry>,
    positions_by_entry_id: btree::Tree<EntryIdToPosition>,
    open_dir_count: usize,
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

pub enum Error<S: Store> {
    InvalidPath,
    DuplicatePath,
    StoreReadError(S::ReadError),
}

impl Tree {
    pub fn new<N: Into<OsString>, S: Store>(root_name: N, db: &S) -> Result<Self, Error<S>> {
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

    pub fn cursor<S: Store>(&self, db: &S) -> Result<Cursor, Error<S>> {
        let entry_store = db.entry_store();
        let mut tree_cursor = self.entries.cursor();
        tree_cursor
            .seek(&id::Ordered::min_value(), SeekBias::Left, entry_store)
            .map_err(|error| Error::StoreReadError(error))?;

        let mut walk: Walk = tree_cursor.start();
        walk.0.push(
            tree_cursor
                .item(entry_store)
                .map_err(|error| Error::StoreReadError(error))?
                .unwrap()
                .into(),
        );

        let mut path = PathBuf::new();
        path.extend(walk.0.iter().filter_map(|step| step.name()));
        Ok(Cursor { tree_cursor, path })
    }

    #[cfg(test)]
    fn paths<S: Store>(&self, store: &S) -> Vec<String> {
        let mut paths = Vec::new();
        let mut cursor = self.cursor(store).unwrap();
        loop {
            paths.push(cursor.path().to_string_lossy().into_owned());
            if !cursor.next(store).unwrap() {
                return paths;
            }
        }
    }
}

impl Cursor {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn next<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        let db = db.entry_store();

        let prev_position = self.tree_cursor.start::<id::Ordered>();
        let mut pop_count = match self.tree_cursor.item(db)?.unwrap() {
            TreeEntry::File { .. } => 1,
            _ => 0,
        };
        loop {
            self.tree_cursor.next(db)?;
            match self.tree_cursor.item(db)? {
                Some(entry) => match entry {
                    TreeEntry::Dir { name, .. } | TreeEntry::File { name, .. } => {
                        for _ in 0..pop_count {
                            self.path.pop();
                        }
                        self.path.push(name.as_ref());
                        return Ok(true);
                    }
                    TreeEntry::ParentDir { .. } => pop_count += 1,
                },
                None => {
                    self.tree_cursor.seek(&prev_position, SeekBias::Right, db)?;
                    return Ok(false);
                }
            }
        }
    }

    pub fn next_sibling<S: Store>(&mut self, db: &S) -> Result<bool, S::ReadError> {
        let db = db.entry_store();

        let prev_position = self.tree_cursor.start::<id::Ordered>();
        match self.tree_cursor.item(db)?.unwrap() {
            TreeEntry::File { .. } => self.tree_cursor.next(db)?,
            TreeEntry::Dir { name, .. } => {
                let mut dir_end = self.tree_cursor.start::<Walk>();
                dir_end.push(Step::VisitDir(name.clone()));
                dir_end.push(Step::VisitParent);
                self.tree_cursor
                    .seek_forward(&dir_end, SeekBias::Right, db)?;
            }
            TreeEntry::ParentDir { .. } => unreachable!("Cursor must be parked at Dir or File"),
        }

        let sibling = self.tree_cursor.item(db)?;
        if sibling.as_ref().map_or(true, |s| s.is_parent_dir()) {
            self.tree_cursor.seek(&prev_position, SeekBias::Right, db)?;
            Ok(false)
        } else {
            self.path.pop();
            self.path.push(sibling.unwrap().name());
            Ok(true)
        }
    }
}

impl Builder {
    pub fn new<S: Store>(old_tree: Tree, path: &Path, store: &S) -> Result<Self, Error<S>> {
        let entry_store = store.entry_store();

        let mut walk = Walk(Vec::new());
        for component in path.components() {
            match component {
                path::Component::Normal(name) => {
                    walk.push(Step::VisitDir(Arc::new(name.to_owned())));
                }
                _ => return Err(Error::InvalidPath),
            }
        }

        let mut old_entries = old_tree.entries.cursor();
        let new_prefix = old_entries
            .slice(&walk, SeekBias::Left, entry_store)
            .map_err(|error| Error::StoreReadError(error))?;
        let position = new_prefix
            .extent(entry_store)
            .map_err(|error| Error::StoreReadError(error))?;

        walk.0.pop();
        if walk == old_entries.start() {
            Ok(Builder {
                walk,
                position,
                old_entries,
                new_prefix,
                new_entries: Vec::new(),
                positions_by_entry_id: old_tree.positions_by_entry_id.clone(),
                open_dir_count: 0,
            })
        } else {
            Err(Error::InvalidPath)
        }
    }

    pub fn push_dir<N: Into<OsString>, S: Store>(
        &mut self,
        name: N,
        inode: u64,
        store: &S,
    ) -> Result<(), S::ReadError> {
        let entry_store = store.entry_store();
        let id = store.gen_id();
        let name = Arc::new(name.into());
        let inode = Some(inode);

        self.walk.push(Step::VisitDir(name.clone()));

        self.old_entries
            .seek_forward(&self.walk, SeekBias::Left, entry_store)?;

        match self
            .old_entries
            .end::<Walk, _>(entry_store)?
            .cmp(&self.walk)
        {
            Ordering::Less => {
                self.old_entries.next(entry_store)?;
                self.open_dir_count += 1;
            }
            Ordering::Equal => {
                self.old_entries.next(entry_store)?;
            }
            Ordering::Greater => {
                self.open_dir_count += 1;
            }
        }

        let position = id::Ordered::between(
            &self.position,
            &self.old_entries.item(entry_store)?.unwrap().position(),
        );
        self.position = position.clone();
        self.new_entries.push(TreeEntry::Dir {
            id,
            name,
            inode,
            position,
        });

        Ok(())
    }

    pub fn pop_dir<S: Store>(&mut self, store: &S) -> Result<(), S::ReadError> {
        let entry_store = store.entry_store();

        let position = id::Ordered::between(
            &self.position,
            &self.old_entries.item(entry_store)?.unwrap().position(),
        );
        self.walk.push(Step::VisitParent);
        self.position = position.clone();
        self.old_entries
            .seek_forward(&self.walk, SeekBias::Right, entry_store)?;
        if self.open_dir_count != 0 {
            self.open_dir_count -= 1;
        }
        self.new_entries.push(TreeEntry::ParentDir { position });

        Ok(())
    }

    pub fn tree<S: Store>(&mut self, store: &S) -> Result<Tree, S::ReadError> {
        let entry_store = store.entry_store();
        self.new_prefix
            .extend(self.new_entries.drain(..), entry_store)?;
        let mut entries = self.new_prefix.clone();

        if self.open_dir_count > 0 {
            let next_entry = self.old_entries.item(entry_store)?.unwrap();
            let mut last_position = self.position.clone();
            let mut parent_entries = Vec::with_capacity(self.open_dir_count);
            for _ in 0..self.open_dir_count {
                last_position = id::Ordered::between(&last_position, next_entry.position());
                parent_entries.push(TreeEntry::ParentDir {
                    position: last_position.clone(),
                });
            }
            entries.extend(parent_entries, entry_store)?;
        }

        entries.push_tree(
            self.old_entries
                .clone()
                .suffix::<id::Ordered, _>(entry_store)?,
            entry_store,
        )?;

        Ok(Tree {
            entries,
            positions_by_entry_id: self.positions_by_entry_id.clone(),
        })
    }
}

impl Walk {
    fn push(&mut self, step: Step) {
        if self.0.last().map_or(false, |last| last.is_file_visit()) {
            self.0.pop();
        }

        if self.0.len() >= 2
            && self.0[self.0.len() - 2].is_dir_visit()
            && self.0[self.0.len() - 1].is_parent_visit()
        {
            self.0.pop();
            self.0.pop();
        }

        self.0.push(step);
    }
}

impl btree::Dimension for Walk {
    type Summary = EntrySummary;

    fn from_summary(summary: &Self::Summary) -> &Self {
        &summary.walk
    }
}

impl<'a> AddAssign<&'a Self> for Walk {
    fn add_assign(&mut self, other: &Self) {
        for other_step in &other.0 {
            self.push(other_step.clone());
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
                            Step::VisitParent => Ordering::Less,
                        },
                        Step::VisitDir(self_name) => match other_step {
                            Step::VisitFile(_) => Ordering::Less,
                            Step::VisitDir(other_name) => self_name.cmp(other_name),
                            Step::VisitParent => Ordering::Less,
                        },
                        Step::VisitParent => match other_step {
                            Step::VisitFile(_) => Ordering::Greater,
                            Step::VisitDir(_) => Ordering::Greater,
                            Step::VisitParent => Ordering::Equal,
                        },
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

    fn is_file_visit(&self) -> bool {
        match self {
            Step::VisitFile(_) => true,
            _ => false,
        }
    }

    fn is_dir_visit(&self) -> bool {
        match self {
            Step::VisitDir(_) => true,
            _ => false,
        }
    }

    fn name(&self) -> Option<&OsStr> {
        match self {
            Step::VisitFile(name) => Some(name.as_ref()),
            Step::VisitDir(name) => Some(name.as_ref()),
            Step::VisitParent => None,
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

    fn name(&self) -> &OsStr {
        match self {
            TreeEntry::Dir { name, .. } => name,
            TreeEntry::File { name, .. } => name,
            TreeEntry::ParentDir { .. } => panic!("This method can't be called on ParentDir"),
        }
    }

    fn is_parent_dir(&self) -> bool {
        match self {
            TreeEntry::ParentDir { .. } => true,
            _ => false,
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

    fn from_summary(summary: &Self::Summary) -> &Self {
        &summary
    }
}

impl btree::Dimension for id::Ordered {
    type Summary = EntrySummary;

    fn from_summary(summary: &Self::Summary) -> &Self {
        &summary.position
    }
}

// When we derive this implementation, the compiler thinks that Error<S> does not implement Debug
// in some cases.
impl<S: Store> fmt::Debug for Error<S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            Error::InvalidPath => write!(f, "InvalidPath"),
            Error::DuplicatePath => write!(f, "DuplicatePath"),
            Error::StoreReadError(error) => write!(f, "StoreReadError({:?})", error),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use self::rand::{Rng, SeedableRng, StdRng};
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_builder_basic() {
        let db = NullStore::new();
        let tree = Tree::new("root", &db).unwrap();
        let mut builder = Builder::new(tree, &PathBuf::from("root"), &db).unwrap();

        builder.push_dir("root", 0, &db).unwrap();
        builder.push_dir("a", 1, &db).unwrap();
        builder.push_dir("b", 2, &db).unwrap();
        builder.push_dir("c", 3, &db).unwrap();
        let tree = builder.tree(&db).unwrap();
        assert_eq!(
            tree.paths(&db),
            ["root", "root/a", "root/a/b", "root/a/b/c"]
        );

        builder.pop_dir(&db).unwrap();
        builder.push_dir("d", 4, &db).unwrap();
        builder.push_dir("e", 5, &db).unwrap();
        builder.pop_dir(&db).unwrap();
        builder.push_dir("f", 6, &db).unwrap();
        let tree = builder.tree(&db).unwrap();
        assert_eq!(
            tree.paths(&db),
            [
                "root",
                "root/a",
                "root/a/b",
                "root/a/b/c",
                "root/a/b/d",
                "root/a/b/d/e",
                "root/a/b/d/f",
            ]
        );

        let mut builder = Builder::new(tree, &PathBuf::from("root/a/b"), &db).unwrap();
        builder.push_dir("b", 0, &db).unwrap();
        builder.push_dir("ca", 7, &db).unwrap();
        builder.push_dir("g", 8, &db).unwrap();
        builder.push_dir("h", 9, &db).unwrap();
        let tree = builder.tree(&db).unwrap();
        assert_eq!(
            tree.paths(&db),
            [
                "root",
                "root/a",
                "root/a/b",
                "root/a/b/ca",
                "root/a/b/ca/g",
                "root/a/b/ca/g/h",
                "root/a/b/d",
                "root/a/b/d/e",
                "root/a/b/d/f",
            ]
        );
        builder.pop_dir(&db).unwrap();
        builder.pop_dir(&db).unwrap();
        builder.pop_dir(&db).unwrap();
        builder.push_dir("d", 4, &db).unwrap();
        builder.push_dir("e", 5, &db).unwrap();
        builder.pop_dir(&db).unwrap();
        builder.push_dir("ea", 10, &db).unwrap();
        let tree = builder.tree(&db).unwrap();
        assert_eq!(
            tree.paths(&db),
            [
                "root",
                "root/a",
                "root/a/b",
                "root/a/b/ca",
                "root/a/b/ca/g",
                "root/a/b/ca/g/h",
                "root/a/b/d",
                "root/a/b/d/e",
                "root/a/b/d/ea",
                "root/a/b/d/f",
            ]
        );
    }

    #[test]
    fn test_builder_random() {
        for seed in 0..10 {
            let mut rng = StdRng::from_seed(&[seed]);

            let mut store = NullStore::new();
            let store = &store;

            let mut reference_tree = TestDir::gen(&mut rng, 0);
            let mut tree = Tree::new(reference_tree.name.clone(), store).unwrap();

            for _ in 0..5 {
                // eprintln!("=========================================");
                // eprintln!("existing paths {:#?}", tree.paths(store).len());
                // eprintln!("new tree paths {:#?}", reference_tree.paths().len());
                // eprintln!("=========================================");

                let mut builder =
                    Builder::new(tree.clone(), &PathBuf::from(&reference_tree.name), store)
                        .unwrap();
                reference_tree.build(&mut builder, store);
                tree = builder.tree(store).unwrap();
                assert_eq!(tree.paths(store), reference_tree.paths());
                reference_tree.mutate(&mut rng, 0);
            }
        }
    }

    const MAX_TEST_TREE_DEPTH: usize = 5;

    struct TestDir {
        name: OsString,
        dir_entries: Vec<TestDir>,
    }

    impl TestDir {
        fn gen<T: Rng>(rng: &mut T, depth: usize) -> Self {
            let mut tree = Self {
                name: gen_name(rng),
                dir_entries: (0..rng.gen_range(0, MAX_TEST_TREE_DEPTH - depth + 1))
                    .map(|_| Self::gen(rng, depth + 1))
                    .collect(),
            };
            tree.normalize_entries();
            tree
        }

        fn mutate<T: Rng>(&mut self, rng: &mut T, depth: usize) {
            self.dir_entries.retain(|_| !rng.gen_weighted_bool(5));
            for dir in &mut self.dir_entries {
                if rng.gen_weighted_bool(3) {
                    dir.mutate(rng, depth + 1);
                }
            }
            if depth < MAX_TEST_TREE_DEPTH {
                for _ in 0..rng.gen_range(0, 5) {
                    self.dir_entries.push(Self::gen(rng, depth + 1));
                }
            }
            self.normalize_entries();
        }

        fn normalize_entries(&mut self) {
            let mut existing_names = HashSet::new();
            self.dir_entries.sort_by(|a, b| a.name.cmp(&b.name));
            self.dir_entries.retain(|entry| {
                if existing_names.contains(&entry.name) {
                    false
                } else {
                    existing_names.insert(entry.name.clone());
                    true
                }
            });
        }

        fn paths(&self) -> Vec<String> {
            let mut cur_path = PathBuf::new();
            let mut paths = Vec::new();
            self.paths_recursive(&mut cur_path, &mut paths);
            paths
        }

        fn paths_recursive(&self, cur_path: &mut PathBuf, paths: &mut Vec<String>) {
            cur_path.push(self.name.clone());
            paths.push(cur_path.clone().to_string_lossy().into_owned());
            for dir in &self.dir_entries {
                dir.paths_recursive(cur_path, paths);
            }
            cur_path.pop();
        }

        fn build<S: Store>(&self, builder: &mut Builder, store: &S) {
            builder.push_dir(self.name.clone(), 0, store).unwrap();
            for dir in &self.dir_entries {
                dir.build(builder, store);
            }
            builder.pop_dir(store).unwrap();
        }
    }

    fn gen_name<T: Rng>(rng: &mut T) -> OsString {
        let mut name = String::new();
        for _ in 0..rng.gen_range(1, 4) {
            name.push(rng.gen_range(b'a', b'z' + 1).into());
        }
        name.into()
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

        fn get(&self, _id: btree::NodeId) -> Result<Arc<btree::Node<TreeEntry>>, Self::ReadError> {
            panic!("get should never be called on a null store")
        }
    }

    impl btree::NodeStore<EntryIdToPosition> for NullStore {
        type ReadError = ();

        fn get(
            &self,
            _id: btree::NodeId,
        ) -> Result<Arc<btree::Node<EntryIdToPosition>>, Self::ReadError> {
            panic!("get should never be called on a null store")
        }
    }
}
