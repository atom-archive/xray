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
mod operation_queue;
pub mod time;
mod work_tree;

pub use crate::buffer::{Buffer, Point};
pub use crate::epoch::{Cursor, DirEntry, Epoch, FileStatus, FileType, ROOT_FILE_ID};
pub use crate::work_tree::{BufferId, GitProvider, Operation, WorkTree};
use std::borrow::Cow;
use std::fmt;
use std::io;

pub type ReplicaId = u32;
pub type UserId = u64;
pub type Oid = [u8; 20];

#[derive(Debug)]
pub enum Error {
    IoError(io::Error),
    InvalidPath(Cow<'static, str>),
    InvalidOperations,
    InvalidFileId(Cow<'static, str>),
    InvalidBufferId,
    InvalidDirEntry,
    InvalidOperation,
    CursorExhausted,
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        format!("{:?}", error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::IoError(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
