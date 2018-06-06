use btree;
use cross_platform::PathComponent;
use smallvec::SmallVec;
use std::cmp::{Ord, Ordering};
use std::ops::{Add, AddAssign};
use std::sync::Arc;

pub struct Tree {
    entries: btree::Tree<Entry>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Entry(Arc<EntryState>);

#[derive(Debug, Eq, PartialEq, PartialOrd)]
enum EntryState {
    File { name: PathComponent },
    Dir { name: PathComponent },
    ParentDir,
}

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct RelativePath(SmallVec<[Entry; 1]>);

impl Entry {
    fn is_file(&self) -> bool {
        match *self.0 {
            EntryState::File { .. } => true,
            _ => false,
        }
    }

    fn is_dir(&self) -> bool {
        match *self.0 {
            EntryState::Dir { .. } => true,
            _ => false,
        }
    }
}

impl btree::Item for Entry {
    type Summary = RelativePath;

    fn summarize(&self) -> Self::Summary {
        RelativePath(SmallVec::from_vec(vec![self.clone()]))
    }
}

impl btree::Dimension for RelativePath {
    type Summary = Self;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.clone()
    }
}

impl<'a> AddAssign<&'a Self> for RelativePath {
    fn add_assign(&mut self, other: &Self) {
        for other_entry in &other.0 {
            match *other_entry.0 {
                EntryState::File { .. } | EntryState::Dir { .. } => {
                    if self.0.last().map(|e| e.is_file()).unwrap_or(false) {
                        self.0.pop();
                    }
                    self.0.push(other_entry.clone());
                }
                EntryState::ParentDir => {
                    if self
                        .0
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

impl Ord for EntryState {
    fn cmp(&self, other: &Self) -> Ordering {
        match self {
            EntryState::File {
                name: self_name, ..
            } => match other {
                EntryState::File {
                    name: other_name, ..
                } => self_name.cmp(other_name),
                EntryState::Dir { .. } => Ordering::Greater,
                EntryState::ParentDir { .. } => panic!("Can't compare paths with parent entries"),
            },
            EntryState::Dir {
                name: self_name, ..
            } => match other {
                EntryState::File { .. } => Ordering::Less,
                EntryState::Dir {
                    name: other_name, ..
                } => self_name.cmp(other_name),
                EntryState::ParentDir => panic!("Can't compare paths with parent entries"),
            },
            EntryState::ParentDir => panic!("Can't compare paths with parent entries"),
        }
    }
}
