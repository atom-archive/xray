use std::cmp;
use std::iter;
use std::ops::{Add, AddAssign};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Unique {
    replica_id: Uuid,
    seq: u64,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
pub struct Ordered(Arc<Vec<u16>>);

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
    static ref ORDERED_ID_MIN_VALUE: Ordered = Ordered(Arc::new(vec![0 as u16]));
    static ref ORDERED_ID_MAX_VALUE: Ordered = Ordered(Arc::new(vec![u16::max_value()]));
}

impl Ordered {
    pub fn min_value() -> Self {
        ORDERED_ID_MIN_VALUE.clone()
    }

    pub fn max_value() -> Self {
        ORDERED_ID_MAX_VALUE.clone()
    }

    pub fn between(left: &Self, right: &Self) -> Self {
        Self::between_with_max(left, right, u16::max_value())
    }

    fn between_with_max(left: &Self, right: &Self, max_value: u16) -> Self {
        let mut new_entries = Vec::new();

        let left_entries = left.0.iter().cloned().chain(iter::repeat(0));
        let right_entries = right.0.iter().cloned().chain(iter::repeat(max_value));
        for (l, r) in left_entries.zip(right_entries) {
            let interval = r - l;
            if interval > 1 {
                new_entries.push(l + interval / 2);
                break;
            } else {
                new_entries.push(l);
            }
        }

        Ordered(Arc::new(new_entries))
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
