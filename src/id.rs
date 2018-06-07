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

pub struct OrderedGenerator {
    prefix: Vec<u16>,
    step: u16,
    index: usize,
    count: usize,
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

    pub fn between(left: &Self, right: &Self, count: usize) -> OrderedGenerator {
        Self::between_with_max(left, right, count, u16::max_value())
    }

    fn between_with_max(
        left: &Self,
        right: &Self,
        count: usize,
        max_value: u16,
    ) -> OrderedGenerator {
        OrderedGenerator::new(left, right, count, max_value)
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
    fn new(left: &Ordered, right: &Ordered, count: usize, max_value: u16) -> Self {
        let step;
        let prefix = Vec::new();
        let left_entries = left.0.iter().cloned().chain(iter::repeat(0));
        let right_entries = right.0.iter().cloned().chain(iter::repeat(max_value));
        for (left, right) in left_entries.zip(right_entries) {
            let interval = right - left;
            if interval as u16 > count {
                step = interval / count;
                prefix.push(left);
                break;
            } else {
                prefix.push(left);
            }
        }

        OrderedGenerator {
            prefix,
            index: 0,
            count,
            step,
        }
    }
}

impl<'a> Iterator for OrderedGenerator<'a> {
    type Item = Ordered;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.count {
            self.index += 1;
            let entries = self.prefix.clone();
            *entries.last_mut().unwrap() += self.index * self.step;
            Some(Ordered(Arc::new(entries)))
        } else {
            None
        }
    }
}
