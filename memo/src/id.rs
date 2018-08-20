use std::cmp;
use std::ops::{Add, AddAssign};

pub type ReplicaId = u64;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct Unique {
    pub replica_id: ReplicaId,
    pub seq: u64,
}

impl Unique {
    pub const DEFAULT: Unique = Unique {
        replica_id: 0,
        seq: 0,
    };

    pub fn new(replica_id: u64) -> Self {
        Self { replica_id, seq: 0 }
    }

    pub fn next(mut self) -> Self {
        self.seq += 1;
        self
    }
}

impl Default for Unique {
    fn default() -> Self {
        Unique::DEFAULT
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
