extern crate futures;
#[macro_use]
extern crate lazy_static;
#[cfg(test)]
extern crate rand;
#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate smallvec;

mod btree;
mod buffer;
mod epoch;
mod notify_cell;
mod operation_queue;
pub mod time;
pub mod work_tree;

pub use buffer::{Buffer, Point};
pub use epoch::{
    BufferId, Cursor, DirEntry, Epoch, Error, FileId, FileStatus, FileType, Operation, ROOT_FILE_ID,
};
pub type ReplicaId = u64;
pub type UserId = u64;
pub type Oid = [u8; 20];
