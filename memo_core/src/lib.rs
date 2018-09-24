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
mod operation_queue;
pub mod time;
mod work_tree;

pub use buffer::Buffer;
pub use work_tree::{
    BufferId, Cursor, CursorEntry, DirEntry, Error, FileId, FileStatus, FileType, Operation,
    WorkTree, ROOT_FILE_ID
};
pub type ReplicaId = u64;
pub type UserId = u64;
