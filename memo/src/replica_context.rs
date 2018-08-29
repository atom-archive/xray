use time;
use ReplicaId;

pub struct ReplicaContext {
    replica_id: ReplicaId,
    local_clock: time::Local,
    lamport_clock: time::Lamport,
}

impl ReplicaContext {
    pub fn new(replica_id: ReplicaId) -> Self {
        assert_ne!(replica_id, 0);
        Self {
            replica_id,
            local_clock: time::Local::new(replica_id),
            lamport_clock: time::Lamport::new(replica_id),
        }
    }

    pub fn replica_id(&self) -> ReplicaId {
        self.replica_id
    }

    pub fn local_time(&mut self) -> time::Local {
        self.local_clock.tick();
        self.local_clock
    }

    pub fn lamport_time(&mut self) -> time::Lamport {
        self.lamport_clock.tick();
        self.lamport_clock
    }

    pub fn observe_lamport_timestamp(&mut self, timestamp: time::Lamport) {
        self.lamport_clock.observe(timestamp);
    }
}
