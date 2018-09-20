extern crate bincode;
extern crate js_sys;
extern crate memo_core;
extern crate wasm_bindgen;

use memo_core::time;
use std::ffi::OsString;
use std::iter;
use std::rc::Rc;
use std::vec;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct GlobalTime(time::Global);

#[wasm_bindgen]
pub struct WorkTree {
    tree: memo_core::WorkTree,
    base_entries_to_append: Vec<memo_core::DirEntry>,
}

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct FileId(memo_core::FileId);

#[wasm_bindgen]
#[derive(Clone)]
pub struct Operation(Rc<memo_core::Operation>);

#[wasm_bindgen]
pub struct OperationIter(iter::Peekable<vec::IntoIter<memo_core::Operation>>);

#[wasm_bindgen]
pub struct NewFileResult {
    file_id: FileId,
    operation: Operation,
}

#[wasm_bindgen]
impl NewFileResult {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn operation(&mut self) -> Operation {
        self.operation.clone()
    }
}

#[wasm_bindgen]
pub enum FileType {
    Directory,
    Text,
}

#[wasm_bindgen]
impl WorkTree {
    pub fn new(replica_id: memo_core::ReplicaId) -> Self {
        WorkTree {
            tree: memo_core::WorkTree::new(replica_id),
            base_entries_to_append: Vec::new(),
        }
    }

    pub fn version(&self) -> GlobalTime {
        GlobalTime(self.tree.version())
    }

    pub fn append_base_entry(&mut self, depth: usize, name: String, file_type: FileType) {
        self.base_entries_to_append.push(memo_core::DirEntry {
            depth,
            name: OsString::from(name),
            file_type: file_type.into(),
        });
    }

    pub fn flush_base_entries(&mut self) -> OperationIter {
        // TODO: return an error instead of unwrapping once wasm_bindgen supports Result.
        let fixup_ops = self
            .tree
            .append_base_entries(self.base_entries_to_append.drain(..))
            .unwrap();
        OperationIter(fixup_ops.into_iter().peekable())
    }

    pub fn apply_ops(&mut self, ops: Vec<u8>) -> OperationIter {
        // TODO: return an error instead of unwrapping once wasm_bindgen supports Result.
        let fixup_ops = self
            .tree
            .apply_ops(bincode::deserialize::<Vec<_>>(&ops).unwrap())
            .unwrap();
        OperationIter(fixup_ops.into_iter().peekable())
    }

    pub fn new_text_file(&mut self) -> NewFileResult {
        let (file_id, op) = self.tree.new_text_file();
        NewFileResult {
            file_id: FileId(file_id),
            operation: Operation(Rc::new(op)),
        }
    }

    pub fn new_dir(&mut self, parent_id: FileId, name: String) -> NewFileResult {
        // TODO: return an error instead of unwrapping once wasm_bindgen supports Result.
        let (file_id, op) = self
            .tree
            .new_dir(parent_id.0, OsString::from(name))
            .unwrap();
        NewFileResult {
            file_id: FileId(file_id),
            operation: Operation(Rc::new(op)),
        }
    }
    //             pub fn open_text_file(&mut self, file_id: FileId, base_text: Text) -> Result<BufferId, Error> {
    //                 pub fn rename<N>(
    //                     pub fn remove(&mut self, file_id: FileId) -> Result<Operation, Error> {
    //                         pub fn edit<'a, I, T>(
    //                             pub fn file_id<P>(&self, path: P) -> Result<FileId, Error>
    //                             pub fn path(&self, file_id: FileId) -> Result<PathBuf, Error> {
    //                                 pub fn text(&self, buffer_id: BufferId) -> Result<buffer::Iter, Error> {
    //                                     pub fn changes_since(
}

#[wasm_bindgen]
impl OperationIter {
    pub fn has_next(&mut self) -> bool {
        self.0.peek().is_some()
    }

    pub fn next(&mut self) -> Operation {
        Operation(Rc::new(self.0.next().unwrap()))
    }
}

impl Into<memo_core::FileType> for FileType {
    fn into(self) -> memo_core::FileType {
        match self {
            FileType::Text => memo_core::FileType::Text,
            FileType::Directory => memo_core::FileType::Directory,
        }
    }
}
