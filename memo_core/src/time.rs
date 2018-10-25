use crate::ReplicaId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_derive::{Deserialize, Serialize};
use std::cmp::{self, Ordering};
use std::collections::HashMap;
use std::ops::{Add, AddAssign};
use std::sync::Arc;

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Ord, PartialOrd, Serialize,
)]
pub struct Local {
    pub replica_id: ReplicaId,
    pub seq: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Global(
    #[serde(
        serialize_with = "Global::serialize_inner",
        deserialize_with = "Global::deserialize_inner"
    )]
    Arc<HashMap<ReplicaId, u64>>,
);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct Lamport {
    pub value: u64,
    pub replica_id: ReplicaId,
}

impl Local {
    pub fn new(replica_id: ReplicaId) -> Self {
        Self { replica_id, seq: 1 }
    }

    pub fn tick(&mut self) -> Self {
        let timestamp = *self;
        self.seq += 1;
        timestamp
    }

    pub fn observe(&mut self, timestamp: Self) {
        if timestamp.replica_id == self.replica_id {
            self.seq = cmp::max(self.seq, timestamp.seq + 1);
        }
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

    pub fn get(&self, replica_id: ReplicaId) -> u64 {
        *self.0.get(&replica_id).unwrap_or(&0)
    }

    pub fn observe(&mut self, timestamp: Local) {
        let map = Arc::make_mut(&mut self.0);
        let seq = map.entry(timestamp.replica_id).or_insert(0);
        *seq = cmp::max(*seq, timestamp.seq);
    }

    pub fn observe_all(&mut self, other: &Self) {
        for (replica_id, seq) in other.0.as_ref() {
            self.observe(Local {
                replica_id: *replica_id,
                seq: *seq,
            });
        }
    }

    pub fn observed(&self, timestamp: Local) -> bool {
        self.get(timestamp.replica_id) >= timestamp.seq
    }

    pub fn changed_since(&self, other: &Self) -> bool {
        self.0
            .iter()
            .any(|(replica_id, seq)| *seq > other.get(*replica_id))
    }

    fn serialize_inner<S>(
        inner: &Arc<HashMap<ReplicaId, u64>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        inner.serialize(serializer)
    }

    fn deserialize_inner<'de, D>(deserializer: D) -> Result<Arc<HashMap<ReplicaId, u64>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Arc::new(HashMap::deserialize(deserializer)?))
    }
}

impl PartialOrd for Global {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let mut global_ordering = Ordering::Equal;

        for replica_id in self.0.keys().chain(other.0.keys()) {
            let ordering = self.get(*replica_id).cmp(&other.get(*replica_id));
            if ordering != Ordering::Equal {
                if global_ordering == Ordering::Equal {
                    global_ordering = ordering;
                } else if ordering != global_ordering {
                    return None;
                }
            }
        }

        Some(global_ordering)
    }
}

impl Lamport {
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            value: 1,
            replica_id,
        }
    }

    pub fn tick(&mut self) -> Self {
        let timestamp = *self;
        self.value += 1;
        timestamp
    }

    pub fn observe(&mut self, timestamp: Self) {
        self.value = cmp::max(self.value, timestamp.value) + 1;
    }
}
