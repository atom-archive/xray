use std::cmp;
use std::ops::{Add, AddAssign};
use std::sync::Arc;
use uuid::{Uuid, UuidVersion};

type OrderedEntry = u16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Unique {
    replica_id: Uuid,
    pub seq: u64,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
pub struct Ordered(Arc<Vec<OrderedEntry>>);

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

    pub fn between(prev_id: &Self, next_id: &Self) -> Self {
        Self::between_with_max(prev_id, next_id, OrderedEntry::max_value())
    }

    fn between_with_max(prev_id: &Self, next_id: &Self, max_value: OrderedEntry) -> Self {
        const MAX_STEP: OrderedEntry = 32;
        let mut level = 0;
        let mut found_lesser = false;
        let mut new_id = Vec::new();

        loop {
            let prev = *prev_id.0.get(level).unwrap_or(&0);
            let next = if found_lesser {
                max_value
            } else {
                *next_id.0.get(level).unwrap_or(&max_value)
            };

            let interval = next - prev;
            if interval > 1 {
                new_id.push(prev + cmp::min(interval / 2, MAX_STEP));
                return Ordered(Arc::new(new_id));
            } else {
                if interval == 1 {
                    found_lesser = true;
                }
                new_id.push(prev);
                level += 1;
            }
        }
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
