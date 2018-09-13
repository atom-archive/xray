extern crate futures;
#[macro_use]
extern crate lazy_static;
extern crate parking_lot;
#[cfg(test)]
extern crate rand;
extern crate smallvec;
extern crate uuid;

mod btree;
// pub mod buffer;
// mod notify_cell;
// mod patch;
// mod replica_context;
// mod index;
mod time;
mod work_tree;
// mod working_copy;
// pub mod timeline;

pub type ReplicaId = u64;
pub type UserId = u64;
