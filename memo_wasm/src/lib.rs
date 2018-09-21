extern crate bincode;
extern crate js_sys;
extern crate memo_core;
#[macro_use]
extern crate serde_derive;
extern crate base64;
extern crate serde;
extern crate wasm_bindgen;

use memo_core::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

pub type WorkTreeId = u32;

#[wasm_bindgen]
pub struct Server {
    work_trees: HashMap<WorkTreeId, WorkTree>,
    next_work_tree_id: WorkTreeId,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum Request {
    GetRootFileId,
    CreateWorkTree {
        replica_id: ReplicaId,
    },
    AppendBaseEntries {
        tree_id: WorkTreeId,
        entries: Vec<DirEntry>,
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
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum Response {
    Ok,
    Error {
        message: String,
    },
    GetRootFileId {
        file_id: Base64<FileId>,
    },
    CreateWorkTree {
        tree_id: WorkTreeId,
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
}

struct Base64<T>(T);

#[wasm_bindgen]
impl Server {
    pub fn new() -> Self {
        Self {
            work_trees: HashMap::new(),
            next_work_tree_id: 0,
        }
    }

    pub fn request(&mut self, request: JsValue) -> JsValue {
        let response = match request.into_serde::<Request>() {
            Ok(request) => match self.request_internal(request) {
                Ok(response) => response,
                Err(message) => Response::Error {
                    message: message.into(),
                },
            },
            Err(error) => Response::Error {
                message: error.to_string(),
            },
        };
        JsValue::from_serde(&response).unwrap()
    }
}

impl Server {
    fn request_internal(&mut self, request: Request) -> Result<Response, String> {
        match request {
            Request::GetRootFileId => Ok(Response::GetRootFileId {
                file_id: Base64(ROOT_FILE_ID),
            }),
            Request::CreateWorkTree { replica_id } => {
                let tree_id = self.next_work_tree_id;
                self.next_work_tree_id += 1;
                self.work_trees.insert(tree_id, WorkTree::new(replica_id));
                Ok(Response::CreateWorkTree { tree_id })
            }
            Request::AppendBaseEntries { tree_id, entries } => {
                self.get_work_tree(tree_id)?
                    .append_base_entries(entries)
                    .map_err(|e| e.to_string())?;
                Ok(Response::Ok)
            }
            Request::ApplyOperations {
                tree_id,
                operations,
            } => {
                let fixup_ops = self
                    .get_work_tree(tree_id)?
                    .apply_ops(operations.into_iter().map(|op| op.0))
                    .map_err(|e| e.to_string())?;
                Ok(Response::ApplyOperations {
                    operations: fixup_ops
                        .into_iter()
                        .map(|op| Base64(op))
                        .collect::<Vec<_>>(),
                })
            }
            Request::NewTextFile { tree_id } => {
                let (file_id, operation) = self.get_work_tree(tree_id)?.new_text_file();
                Ok(Response::NewTextFile {
                    file_id: Base64(file_id),
                    operation: Base64(operation),
                })
            }
            Request::CreateDirectory {
                tree_id,
                parent_id: Base64(parent_id),
                name,
            } => {
                let (file_id, operation) = self
                    .get_work_tree(tree_id)?
                    .create_dir(parent_id, name)
                    .map_err(|e| e.to_string())?;
                Ok(Response::CreateDirectory {
                    file_id: Base64(file_id),
                    operation: Base64(operation),
                })
            }
            Request::OpenTextFile {
                tree_id,
                file_id: Base64(file_id),
                base_text,
            } => {
                let buffer_id = self
                    .get_work_tree(tree_id)?
                    .open_text_file(file_id, base_text.as_str())
                    .map_err(|e| e.to_string())?;
                Ok(Response::OpenTextFile {
                    buffer_id: Base64(buffer_id),
                })
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
