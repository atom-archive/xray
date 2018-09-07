extern crate futures;
#[macro_use]
extern crate lazy_static;
extern crate parking_lot;
extern crate smallvec;
extern crate uuid;

mod btree;
pub mod buffer;
mod notify_cell;
mod patch;
mod replica_context;
mod time;
pub mod timeline;

pub type ReplicaId = u64;
pub type UserId = u64;
