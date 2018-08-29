use std::cmp;
use std::collections::HashMap;
use std::ops::{Add, AddAssign};
use std::sync::Arc;
use ReplicaId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct Local {
    pub replica_id: ReplicaId,
    pub seq: u64,
}

#[derive(Clone, Debug)]
pub struct Global(Arc<HashMap<ReplicaId, u64>>);

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

    pub fn tick(&mut self) {
        self.seq += 1;
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

impl Global {
    pub fn new() -> Self {
        Global(Arc::new(HashMap::new()))
    }

    pub fn include(&mut self, timestamp: &Local) {
        let map = Arc::make_mut(&mut self.0);
        let seq = map.entry(timestamp.replica_id).or_insert(0);
        *seq = cmp::max(*seq, timestamp.seq);
    }

    pub fn includes(&self, timestamp: &Local) -> bool {
        if let Some(seq) = self.0.get(&timestamp.replica_id) {
            *seq >= timestamp.seq
        } else {
            false
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

    pub fn tick(&mut self) {
        self.value += 1;
    }

    pub fn observe(&mut self, timestamp: Self) {
        if timestamp.replica_id != self.replica_id {
            self.value = cmp::max(self.value, timestamp.value) + 1;
        }
    }
}
