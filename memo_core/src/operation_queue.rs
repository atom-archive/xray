use btree::{Cursor, Dimension, Edit, Item, KeyedItem, Tree};
use std::fmt::Debug;
use std::ops::{Add, AddAssign};
use time;

pub trait Operation: Clone + Debug + Eq {
    fn timestamp(&self) -> time::Lamport;
}

#[derive(Clone, Debug)]
pub struct OperationQueue<T: Operation>(Tree<T>);

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct OperationKey(time::Lamport);

impl<T: Operation> OperationQueue<T> {
    pub fn new() -> Self {
        OperationQueue(Tree::new())
    }

    pub fn insert<I>(&mut self, ops: I)
    where
        I: IntoIterator<Item = T>,
    {
        let mut edits = ops
            .into_iter()
            .map(|op| Edit::Insert(op))
            .collect::<Vec<Edit<T>>>();
        self.0.edit(&mut edits);
    }

    pub fn drain(&mut self) -> Cursor<T> {
        let cursor = self.0.cursor();
        self.0 = Tree::new();
        cursor
    }
}

impl<T: Operation> Item for T {
    type Summary = OperationKey;

    fn summarize(&self) -> Self::Summary {
        OperationKey(self.timestamp())
    }
}

impl<T: Operation> KeyedItem for T {
    type Key = OperationKey;

    fn key(&self) -> Self::Key {
        OperationKey(self.timestamp())
    }
}

impl<'a> AddAssign<&'a Self> for OperationKey {
    fn add_assign(&mut self, other: &Self) {
        assert!(*self < *other);
        *self = *other;
    }
}

impl<'a> Add<&'a Self> for OperationKey {
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self < *other);
        *other
    }
}

impl Dimension<OperationKey> for OperationKey {
    fn from_summary(summary: &OperationKey) -> Self {
        *summary
    }
}
