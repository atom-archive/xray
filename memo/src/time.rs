use std::cmp;
use std::ops::{Add, AddAssign};

use ReplicaId;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Local {
    pub replica_id: ReplicaId,
    pub seq: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Lamport {
    pub value: u64,
    pub replica_id: ReplicaId,
}

impl Local {
    pub const DEFAULT: Local = Local {
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

impl Default for Local {
    fn default() -> Self {
        Local::DEFAULT
    }
}

impl<'a> Add<&'a Self> for Local {
    type Output = Local;

    fn add(self, other: &'a Self) -> Self::Output {
        cmp::max(&self, other).clone()
    }
}

impl<'a> AddAssign<&'a Local> for Local {
    fn add_assign(&mut self, other: &Self) {
        if *self < *other {
            *self = other.clone();
        }
    }
}

impl Lamport {
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            value: 0,
            replica_id,
        }
    }

    pub fn max_value() -> Self {
        Self {
            value: u64::max_value(),
            replica_id: ReplicaId::max_value(),
        }
    }

    pub fn inc(self) -> Self {
        Self {
            value: self.value + 1,
            replica_id: self.replica_id,
        }
    }

    pub fn update(self, other: Self) -> Self {
        Self {
            value: cmp::max(self.value, other.value) + 1,
            replica_id: self.replica_id,
        }
    }
}
