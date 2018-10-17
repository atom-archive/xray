#![feature(macros_in_extern)]

extern crate bincode;
extern crate memo_core;
#[macro_use]
extern crate serde_derive;
extern crate base64;
extern crate serde;
extern crate wasm_bindgen;

use memo_core::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::char;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

type WorkTreeId = u32;
type StreamId = u32;

#[wasm_bindgen(module = "./support")]
extern "C" {
    pub type Client;

    #[wasm_bindgen(method)]
    fn receive(this: &Client, message: JsValue) -> JsValue;
}

#[wasm_bindgen]
pub struct Server {
    client: Rc<Client>,
    channels: Rc<RefCell<HashMap<StreamId, mpsc::UnboundedSender<RequestToServer>>>>,
    work_trees: HashMap<WorkTreeId, WorkTree>,
    next_work_tree_id: WorkTreeId,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum RequestToServer {
    StreamMessage {
        stream_id: StreamId,
        message: MessageToServer,
    },
    StreamEnd {

    }
    GetRootFileId,
    CreateWorkTree {
        replica_id: ReplicaId,
    },
    GetVersion {
        tree_id: WorkTreeId,
    },
    ApplyOperations {
        tree_id: WorkTreeId,
        operations: Vec<Base64<Operation>>,
    },
    NewTextFile {
        tree_id: WorkTreeId,
    },
    CreateDirectory {
        tree_id: WorkTreeId,
        parent_id: Base64<FileId>,
        name: String,
    },
    OpenTextFile {
        tree_id: WorkTreeId,
        file_id: Base64<FileId>,
        base_text: String,
    },
    Rename {
        tree_id: WorkTreeId,
        file_id: Base64<FileId>,
        new_parent_id: Base64<FileId>,
        new_name: String,
    },
    Remove {
        tree_id: WorkTreeId,
        file_id: Base64<FileId>,
    },
    Edit {
        tree_id: WorkTreeId,
        buffer_id: Base64<BufferId>,
        ranges: Vec<EditRange>,
        new_text: String,
    },
    ChangesSince {
        tree_id: WorkTreeId,
        buffer_id: Base64<BufferId>,
        version: Base64<time::Global>,
    },
    GetText {
        tree_id: WorkTreeId,
        buffer_id: Base64<BufferId>,
    },
    FileIdForPath {
        tree_id: WorkTreeId,
        path: String,
    },
    PathForFileId {
        tree_id: WorkTreeId,
        file_id: Base64<FileId>,
    },
    BasePathForFileId {
        tree_id: WorkTreeId,
        file_id: Base64<FileId>,
    },
    Entries {
        tree_id: WorkTreeId,
        show_deleted: bool,
        descend_into: Option<HashSet<Base64<FileId>>>,
    },
}

enum MessageToServer {

}

#[derive(Serialize)]
#[serde(tag = "type")]
enum MessageToClient {
    Error {
        message: String,
    },
    GetRootFileId {
        file_id: Base64<FileId>,
    },
    CreateWorkTree {
        tree_id: WorkTreeId,
        stream_id: StreamId,
    },
    GetVersion {
        version: Base64<time::Global>,
    },
    NewTextFile {
        file_id: Base64<FileId>,
        operation: Base64<Operation>,
    },
    CreateDirectory {
        file_id: Base64<FileId>,
        operation: Base64<Operation>,
    },
    OpenTextFile {
        buffer_id: Base64<BufferId>,
    },
    ApplyOperations {
        operations: Vec<Base64<Operation>>,
    },
    Rename {
        operation: Base64<Operation>,
    },
    Remove {
        operation: Base64<Operation>,
    },
    Edit {
        operation: Base64<Operation>,
    },
    ChangesSince {
        changes: Vec<Change>,
    },
    GetText {
        text: String,
    },
    FileIdForPath {
        file_id: Option<Base64<FileId>>,
    },
    PathForFileId {
        path: Option<String>,
    },
    BasePathForFileId {
        path: Option<String>,
    },
    Entries {
        entries: Vec<Entry>,
    },
    OperationsStream {
        stream_id: u32,
        operations: Vec<Base64<Operation>>,
        done: bool,
    },
}

#[derive(Copy, Clone, Serialize, Deserialize)]
struct EditRange {
    start: Point,
    end: Point,
}

#[derive(Serialize, Deserialize)]
struct Change {
    start: Point,
    end: Point,
    text: String,
}

#[derive(Serialize)]
struct Entry {
    #[serde(rename = "fileId")]
    file_id: Base64<FileId>,
    #[serde(rename = "type")]
    file_type: FileType,
    depth: usize,
    name: String,
    path: String,
    status: FileStatus,
    visible: bool,
}

#[derive(Eq, Hash, PartialEq)]
struct Base64<T>(T);

#[wasm_bindgen]
impl Server {
    pub fn new(client: Client) -> Self {
        Self {
            client: Rc::new(client),
            work_trees: HashMap::new(),
            next_work_tree_id: 0,
        }
    }

    pub fn receive(&mut self, message: JsValue) -> JsValue {
        let response = match message.into_serde::<RequestToServer>() {
            Ok(message) => match self.receive_internal(message) {
                Ok(response) => response,
                Err(message) => MessageToClient::Error {
                    message: message.into(),
                },
            },
            Err(error) => MessageToClient::Error {
                message: error.to_string(),
            },
        };
        JsValue::from_serde(&response).unwrap()
    }
}

impl Server {
    fn receive_internal(&mut self, message: RequestToServer) -> Result<MessageToClient, String> {
        match message {
            RequestToServer::GetRootFileId => Ok(MessageToClient::GetRootFileId {
                file_id: Base64(ROOT_FILE_ID),
            }),
            RequestToServer::CreateWorkTree { replica_id } => {
                let tree_id = self.next_work_tree_id;
                self.next_work_tree_id += 1;
                self.work_trees.insert(tree_id, WorkTree::new(replica_id));
                Ok(MessageToClient::CreateWorkTree { tree_id })
            }
            RequestToServer::GetVersion { tree_id } => Ok(MessageToClient::GetVersion {
                version: Base64(self.get_work_tree(tree_id)?.version()),
            }),
            RequestToServer::ApplyOperations {
                tree_id,
                operations,
            } => {
                let fixup_ops = self
                    .get_work_tree(tree_id)?
                    .apply_ops(operations.into_iter().map(|op| op.0))
                    .map_err(|e| e.to_string())?;
                Ok(MessageToClient::ApplyOperations {
                    operations: fixup_ops.into_iter().map(|op| Base64(op)).collect(),
                })
            }
            RequestToServer::NewTextFile { tree_id } => {
                let (file_id, operation) = self.get_work_tree(tree_id)?.new_text_file();
                Ok(MessageToClient::NewTextFile {
                    file_id: Base64(file_id),
                    operation: Base64(operation),
                })
            }
            RequestToServer::CreateDirectory {
                tree_id,
                parent_id: Base64(parent_id),
                name,
            } => {
                let (file_id, operation) = self
                    .get_work_tree(tree_id)?
                    .create_dir(parent_id, name)
                    .map_err(|e| e.to_string())?;
                Ok(MessageToClient::CreateDirectory {
                    file_id: Base64(file_id),
                    operation: Base64(operation),
                })
            }
            RequestToServer::OpenTextFile {
                tree_id,
                file_id: Base64(file_id),
                base_text,
            } => {
                let buffer_id = self
                    .get_work_tree(tree_id)?
                    .open_text_file(file_id, base_text.as_str())
                    .map_err(|e| e.to_string())?;
                Ok(MessageToClient::OpenTextFile {
                    buffer_id: Base64(buffer_id),
                })
            }
            RequestToServer::Rename {
                tree_id,
                file_id: Base64(file_id),
                new_parent_id: Base64(new_parent_id),
                new_name,
            } => {
                let tree = self.get_work_tree(tree_id)?;
                let op = tree
                    .rename(file_id, new_parent_id, new_name)
                    .map_err(|e| e.to_string())?;
                Ok(MessageToClient::Rename {
                    operation: Base64(op),
                })
            }
            RequestToServer::Remove {
                tree_id,
                file_id: Base64(file_id),
            } => {
                let tree = self.get_work_tree(tree_id)?;
                let op = tree.remove(file_id).map_err(|e| e.to_string())?;
                Ok(MessageToClient::Remove {
                    operation: Base64(op),
                })
            }
            RequestToServer::Edit {
                tree_id,
                buffer_id: Base64(buffer_id),
                ranges,
                new_text,
            } => {
                let tree = self.get_work_tree(tree_id)?;
                let op = tree
                    .edit_2d(
                        buffer_id,
                        ranges.into_iter().map(|range| range.start..range.end),
                        new_text.as_str(),
                    )
                    .map_err(|e| e.to_string())?;
                Ok(MessageToClient::Edit {
                    operation: Base64(op),
                })
            }
            RequestToServer::ChangesSince {
                tree_id,
                buffer_id: Base64(buffer_id),
                version: Base64(version),
            } => {
                let tree = self.get_work_tree(tree_id)?;
                let changes = tree
                    .changes_since(buffer_id, version)
                    .map_err(|e| e.to_string())?
                    .map(|change| Change {
                        start: change.range.start,
                        end: change.range.end,
                        text: String::from_utf16_lossy(&change.code_units),
                    })
                    .collect();
                Ok(MessageToClient::ChangesSince { changes })
            }
            RequestToServer::GetText {
                tree_id,
                buffer_id: Base64(buffer_id),
            } => {
                let tree = self.get_work_tree(tree_id)?;
                let text_iter = tree.text(buffer_id).map_err(|err| err.to_string())?;
                let mut text = String::new();
                for ch in char::decode_utf16(text_iter) {
                    text.push(ch.unwrap_or(char::REPLACEMENT_CHARACTER));
                }
                Ok(MessageToClient::GetText { text })
            }
            RequestToServer::FileIdForPath { tree_id, path } => {
                let tree = self.get_work_tree(tree_id)?;
                let path = Path::new(&path);
                Ok(MessageToClient::FileIdForPath {
                    file_id: tree.file_id(path).ok().map(|id| Base64(id)),
                })
            }
            RequestToServer::PathForFileId {
                tree_id,
                file_id: Base64(file_id),
            } => {
                let tree = self.get_work_tree(tree_id)?;
                let path = tree.path(file_id);
                Ok(MessageToClient::PathForFileId {
                    path: path.map(|p| p.to_string_lossy().into_owned()),
                })
            }
            RequestToServer::BasePathForFileId {
                tree_id,
                file_id: Base64(file_id),
            } => {
                let tree = self.get_work_tree(tree_id)?;
                let path = tree.base_path(file_id);
                Ok(MessageToClient::BasePathForFileId {
                    path: path.map(|p| p.to_string_lossy().into_owned()),
                })
            }
            RequestToServer::Entries {
                tree_id,
                show_deleted,
                descend_into,
            } => {
                let tree = self.get_work_tree(tree_id)?;
                let mut entries = Vec::new();
                tree.with_cursor(|cursor| loop {
                    let entry = cursor.entry().unwrap();
                    let mut descend = false;
                    if show_deleted || entry.status != FileStatus::Removed {
                        entries.push(Entry {
                            file_id: Base64(entry.file_id),
                            file_type: entry.file_type,
                            depth: entry.depth,
                            name: entry.name.to_string_lossy().into_owned(),
                            path: cursor.path().unwrap().to_string_lossy().into_owned(),
                            status: entry.status,
                            visible: entry.visible,
                        });
                        descend = descend_into
                            .as_ref()
                            .map_or(true, |d| d.contains(&Base64(entry.file_id)));
                    }

                    if !cursor.next(descend) {
                        break;
                    }
                });

                Ok(MessageToClient::Entries { entries })
            }
        }
    }

    fn get_work_tree(&mut self, tree_id: WorkTreeId) -> Result<&mut WorkTree, String> {
        self.work_trees
            .get_mut(&tree_id)
            .ok_or_else(|| "WorkTree not found".into())
    }
}

impl<T: Serialize> Serialize for Base64<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::Error;
        base64::encode(&bincode::serialize(&self.0).map_err(Error::custom)?).serialize(serializer)
    }
}

impl<'de1, T: for<'de2> Deserialize<'de2>> Deserialize<'de1> for Base64<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de1>,
    {
        use serde::de::Error;
        let bytes = base64::decode(&String::deserialize(deserializer)?).map_err(Error::custom)?;
        let inner = bincode::deserialize::<T>(&bytes).map_err(D::Error::custom)?;
        Ok(Base64(inner))
    }
}
