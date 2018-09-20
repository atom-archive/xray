#[macro_use]
extern crate lazy_static;
#[cfg(test)]
extern crate rand;
extern crate smallvec;

mod btree;
pub mod buffer;
mod operation_queue;
mod time;
pub mod work_tree;

pub type ReplicaId = u64;
pub type UserId = u64;
