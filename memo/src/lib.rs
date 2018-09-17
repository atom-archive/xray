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
mod time;
mod work_tree;

pub type ReplicaId = u64;
pub type UserId = u64;
