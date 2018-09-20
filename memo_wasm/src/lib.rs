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
    ops_to_apply: Vec<memo_core::Operation>,
}

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct Error(memo_core::Error);

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct FileId(memo_core::FileId);

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct BufferId(memo_core::BufferId);

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
pub struct OpenTextFileResult {
    buffer_id: Option<BufferId>,
    error: Option<Error>,
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
            ops_to_apply: Vec::new(),
        }
    }

    pub fn version(&self) -> GlobalTime {
        GlobalTime(self.tree.version())
    }

    pub fn push_base_entry(&mut self, depth: usize, name: String, file_type: FileType) {
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

    pub fn push_op(&mut self, op: &Operation) {
        self.ops_to_apply.push(op.0.as_ref().clone());
    }

    pub fn flush_ops(&mut self) -> OperationIter {
        // TODO: return an error instead of unwrapping once wasm_bindgen supports Result.
        let fixup_ops = self.tree.apply_ops(self.ops_to_apply.drain(..)).unwrap();
        OperationIter(fixup_ops.into_iter().peekable())
    }

    pub fn new_text_file(&mut self) -> NewFileResult {
        let (file_id, op) = self.tree.new_text_file();
        NewFileResult {
            file_id: FileId(file_id),
            operation: Operation(Rc::new(op)),
        }
    }

    pub fn new_dir(&mut self, parent_id: &FileId, name: String) -> NewFileResult {
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

    pub fn open_text_file(&mut self, file_id: &FileId, base_text: String) -> OpenTextFileResult {
        let result = self
            .tree
            .open_text_file(file_id.0, base_text.as_str().into());

        OpenTextFileResult {
            buffer_id: result.ok().map(|buffer_id| BufferId(buffer_id)),
            error: result.err().map(|err| Error(err)),
        }
    }

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
impl OpenTextFileResult {
    pub fn is_ok(&self) -> bool {
        self.buffer_id.is_some()
    }

    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id.unwrap()
    }

    pub fn error(&self) -> Error {
        self.error.unwrap()
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
