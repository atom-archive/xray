use btree::{Dimension, Edit, Item, KeyedItem, SeekBias, Tree};
use std::fmt::Debug;
use std::ops::{Add, AddAssign};

#[derive(Clone)]
pub struct TreeMap<K, V>(Tree<Entry<K, V>>)
where
    K: Clone + Debug + Default + Eq + Ord,
    V: Clone + Debug + Eq + PartialEq;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry<K, V>
where
    K: Clone + Debug + Default + Eq + Ord,
    V: Clone + Debug + Eq + PartialEq,
{
    key: Key<K>,
    value: V,
}

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct Key<K>(K)
where
    K: Clone + Debug + Default + Eq + Ord;

impl<K, V> TreeMap<K, V>
where
    K: Clone + Debug + Default + Eq + Ord,
    V: Clone + Debug + Eq + PartialEq,
{
    pub fn new() -> Self {
        TreeMap(Tree::new())
    }

    pub fn insert(&mut self, key: K, value: V) {
        self.0.edit(&mut [Edit::Insert(Entry {
            key: Key(key),
            value: value,
        })]);
    }

    pub fn get(&self, key: K) -> Option<V> {
        let mut cursor = self.0.cursor();
        if cursor.seek(&Key(key), SeekBias::Left) {
            Some(cursor.item().unwrap().value)
        } else {
            None
        }
    }
}

impl<K, V> Item for Entry<K, V>
where
    K: Clone + Debug + Default + Eq + Ord,
    V: Clone + Debug + Eq + PartialEq,
{
    type Summary = Key<K>;

    fn summarize(&self) -> Self::Summary {
        self.key.clone()
    }
}

impl<K, V> KeyedItem for Entry<K, V>
where
    K: Clone + Debug + Default + Eq + Ord,
    V: Clone + Debug + Eq + PartialEq,
{
    type Key = Key<K>;

    fn key(&self) -> Self::Key {
        self.key.clone()
    }
}

impl<K> Dimension<Key<K>> for Key<K>
where
    K: Clone + Debug + Default + Eq + Ord,
{
    fn from_summary(summary: &Key<K>) -> Self {
        summary.clone()
    }
}

impl<'a, K> AddAssign<&'a Self> for Key<K>
where
    K: Clone + Debug + Default + Eq + Ord,
{
    fn add_assign(&mut self, other: &Self) {
        assert!(*self <= *other);
        *self = other.clone();
    }
}

impl<'a, K> Add<&'a Self> for Key<K>
where
    K: Clone + Debug + Default + Eq + Ord,
{
    type Output = Self;

    fn add(self, other: &Self) -> Self {
        assert!(self <= *other);
        other.clone()
    }
}
