use btree;
use id;
use std::ffi::OsString;
use std::ops::{Add, AddAssign};
use std::sync::Arc;

struct Tree {
    items: btree::Tree<Item>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Item {
    ParentRef {
        file_id: id::Unique,
        parent_id: id::Unique,
    },
    ChildRef {
        file_id: id::Unique,
        name: Arc<OsString>,
        child_id: id::Unique,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Key {
    ParentRef {
        file_id: id::Unique,
    },
    ChildRef {
        file_id: id::Unique,
        name: Arc<OsString>,
    },
}

impl Tree {
    
}

impl btree::Item for Item {
    type Summary = Key;

    fn summarize(&self) -> Key {
        match self {
            Item::ParentRef { file_id, .. } => Key::ParentRef { file_id: *file_id },
            Item::ChildRef { file_id, name, .. } => Key::ChildRef {
                file_id: *file_id,
                name: name.clone(),
            },
        }
    }
}

impl Default for Key {
    fn default() -> Self {
        Key::ParentRef {
            file_id: id::Unique::default(),
        }
    }
}

impl<'a> AddAssign<&'a Self> for Key {
    fn add_assign(&mut self, other: &Self) {
        if *self < *other {
            *self = other.clone();
        }
    }
}

impl<'a> Add<&'a Self> for Key {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        if self < *other {
            other.clone()
        } else {
            self
        }
    }
}
