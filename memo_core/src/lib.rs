mod btree;
mod buffer;
mod epoch;
#[allow(non_snake_case, unused_imports)]
mod message;
mod operation_queue;
pub mod time;
mod work_tree;

pub use crate::buffer::{Buffer, Change, Point};
pub use crate::epoch::{Cursor, DirEntry, Epoch, FileStatus, FileType, ROOT_FILE_ID};
pub use crate::work_tree::{BufferId, ChangeObserver, GitProvider, Operation, WorkTree};
use std::borrow::Cow;
use std::fmt;
use std::io;
use uuid::Uuid;

pub type ReplicaId = Uuid;
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

trait ReplicaIdExt {
    fn to_message(&self) -> message::ReplicaId;
}

impl ReplicaIdExt for ReplicaId {
    fn to_message(&self) -> message::ReplicaId {
        message::ReplicaId {
            uuid: Some(Cow::Borrowed(self.as_bytes())),
        }
    }
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

#[cfg(test)]
mod tests {
    use crate::ReplicaId;
    use rand::Rng;
    use std::collections::BTreeMap;

    #[derive(Clone)]
    struct Envelope<T: Clone> {
        message: T,
        sender: ReplicaId,
    }

    pub(crate) struct Network<T: Clone> {
        inboxes: BTreeMap<ReplicaId, Vec<Envelope<T>>>,
        all_messages: Vec<T>,
    }

    impl<T: Clone> Network<T> {
        pub fn new() -> Self {
            Network {
                inboxes: BTreeMap::new(),
                all_messages: Vec::new(),
            }
        }

        pub fn add_peer(&mut self, id: ReplicaId) {
            self.inboxes.insert(id, Vec::new());
        }

        pub fn is_idle(&self) -> bool {
            self.inboxes.values().all(|i| i.is_empty())
        }

        pub fn all_messages(&self) -> &Vec<T> {
            &self.all_messages
        }

        pub fn broadcast<R>(&mut self, sender: ReplicaId, messages: Vec<T>, rng: &mut R)
        where
            R: Rng,
        {
            for (replica, inbox) in self.inboxes.iter_mut() {
                if *replica != sender {
                    for message in &messages {
                        let min_index = inbox
                            .iter()
                            .enumerate()
                            .rev()
                            .find_map(|(index, envelope)| {
                                if sender == envelope.sender {
                                    Some(index + 1)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);

                        // Insert one or more duplicates of this message *after* the previous
                        // message delivered by this replica.
                        for _ in 0..rng.gen_range(1, 4) {
                            let insertion_index = rng.gen_range(min_index, inbox.len() + 1);
                            inbox.insert(
                                insertion_index,
                                Envelope {
                                    message: message.clone(),
                                    sender,
                                },
                            );
                        }
                    }
                }
            }
            self.all_messages.extend(messages);
        }

        pub fn has_unreceived(&self, receiver: ReplicaId) -> bool {
            !self.inboxes[&receiver].is_empty()
        }

        pub fn receive<R>(&mut self, receiver: ReplicaId, rng: &mut R) -> Vec<T>
        where
            R: Rng,
        {
            let inbox = self.inboxes.get_mut(&receiver).unwrap();
            let count = rng.gen_range(0, inbox.len() + 1);
            inbox
                .drain(0..count)
                .map(|envelope| envelope.message)
                .collect()
        }

        pub fn clear_unreceived(&mut self, receiver: ReplicaId) {
            self.inboxes.get_mut(&receiver).unwrap().clear();
        }
    }
}
