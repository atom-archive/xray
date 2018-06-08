use std::cmp;
use std::iter;
use std::ops::{Add, AddAssign};
use std::sync::Arc;
use uuid::{Uuid, UuidVersion};

type OrderedEntry = u16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Unique {
    replica_id: Uuid,
    seq: u64,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
pub struct Ordered(Arc<Vec<OrderedEntry>>);

pub struct OrderedGenerator {
    prev_entries: Vec<OrderedEntry>,
    next: Ordered,
    max_entry: OrderedEntry,
}

impl Unique {
    pub fn random() -> Self {
        Self {
            replica_id: Uuid::new(UuidVersion::Random).unwrap(),
            seq: 0,
        }
    }

    pub fn inc(&mut self) {
        self.seq += 1;
    }
}

impl Default for Unique {
    fn default() -> Self {
        Self {
            replica_id: Uuid::nil(),
            seq: 0,
        }
    }
}

impl<'a> Add<&'a Self> for Unique {
    type Output = Unique;

    fn add(self, other: &'a Self) -> Self::Output {
        cmp::max(&self, other).clone()
    }
}

impl<'a> AddAssign<&'a Unique> for Unique {
    fn add_assign(&mut self, other: &Self) {
        if *self < *other {
            *self = other.clone();
        }
    }
}

lazy_static! {
    static ref ORDERED_ID_MIN_VALUE: Ordered = Ordered(Arc::new(vec![0 as OrderedEntry]));
    static ref ORDERED_ID_MAX_VALUE: Ordered = Ordered(Arc::new(vec![OrderedEntry::max_value()]));
}

impl Ordered {
    pub fn min_value() -> Self {
        ORDERED_ID_MIN_VALUE.clone()
    }

    pub fn max_value() -> Self {
        ORDERED_ID_MAX_VALUE.clone()
    }

    pub fn between(prev: &Self, next: &Self) -> OrderedGenerator {
        Self::between_with_max(prev, next, OrderedEntry::max_value())
    }

    fn between_with_max(prev: &Self, next: &Self, max_value: OrderedEntry) -> OrderedGenerator {
        OrderedGenerator::new(prev, next, max_value)
    }
}

impl Default for Ordered {
    fn default() -> Self {
        Self::min_value()
    }
}

impl<'a> Add<&'a Self> for Ordered {
    type Output = Ordered;

    fn add(self, other: &'a Self) -> Self::Output {
        cmp::max(&self, other).clone()
    }
}

impl<'a> AddAssign<&'a Self> for Ordered {
    fn add_assign(&mut self, other: &Self) {
        if *self < *other {
            *self = other.clone();
        }
    }
}

impl OrderedGenerator {
    fn new(prev: &Ordered, next: &Ordered, max_entry: OrderedEntry) -> Self {
        let prev_iter = prev.0.iter().cloned().chain(iter::repeat(0));
        let next_iter = next.0.iter().cloned().chain(iter::repeat(max_entry));
        let mut iter = prev_iter.zip(next_iter);
        let mut prev_entries = Vec::new();
        loop {
            let (prev, next) = iter.next().unwrap();
            prev_entries.push(prev);
            if next - prev >= 2 {
                break;
            }
        }

        OrderedGenerator {
            max_entry,
            prev_entries,
            next: next.clone(),
        }
    }
}

impl Iterator for OrderedGenerator {
    type Item = Ordered;

    fn next(&mut self) -> Option<Self::Item> {
        let prev_entry = *self.prev_entries.last().unwrap();
        let next_entry = *self.next
            .0
            .get(self.prev_entries.len() - 1)
            .unwrap_or(&self.max_entry);

        let interval = next_entry - prev_entry;

        if interval >= 2 {
            const MAX_STEP: OrderedEntry = 32;
            *self.prev_entries.last_mut().unwrap() += cmp::min(interval / 2, MAX_STEP);
            Some(Ordered(Arc::new(self.prev_entries.clone())))
        } else {
            self.prev_entries.push(0);
            self.next()
        }
    }
}
