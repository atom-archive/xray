use btree::{self, SeekBias};
use buffer::{Buffer, Text};
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use time;

type FileId = time::Local;

pub const ROOT_FILE_ID: time::Local = time::Local::DEFAULT;

enum Operation {
    RegisterBasePath {
        parent_id: FileId,
        components: Vec<(OsString, FileId)>
    }
}

struct Patch {
    parent_refs: btree::Tree<ParentRefValue>,
    child_refs: btree::Tree<ChildRefValue>,
    file_id_aliases: HashMap<FileId, FileId>,
}

struct Changes {
    inserted: HashSet<FileId>,
    renamed: HashSet<FileId>,
    removed: HashSet<FileId>,
    edited: HashSet<FileId>,
}

enum Error {
    InvalidPath,
}

impl Patch {
    fn file_id(&mut self, path: &Path) -> Result<(FileId, Option<Operation>), Error> {
        let path = path.into();

        let mut cursor = self.child_refs.cursor();
        let mut parent_id = ROOT_FILE_ID;
        let mut unregistered_ancestor = true;
        let mut op_components = Vec::new();

        for name in path.components() {
            let name = Arc::new(OsString::from(name.as_os_str()));
            if parent_exists && cursor.seek(&ChildRefId { parent_id, name }, SeekBias::Left) {
                let child_ref = cursor.item().unwrap();
                if child_ref.is_visible() {
                    parent_id = child_ref.parent_ref_id;
                } else {
                    return Err(Error::InvalidPath);
                }
            } else {
                parent_exists = false;

                self.local_clock.tick();
                let file_id = self.local_clock;
                registrations.push({parent_id, name, })

            }
        }

        Some(ref_id)
    }

    fn insert<T>(&mut self, parent_id: FileId, name: &OsStr, text: T) -> (FileId, Operation)
    where
        T: Into<Text>,
    {
        unimplemented!()
    }

    fn rename(&mut self, file_id: FileId, new_parent_id: FileId, new_name: &OsStr) -> Operation {
        unimplemented!()
    }

    fn remove(&mut self, file_id: FileId) -> Operation {
        unimplemented!()
    }

    fn edit<'a, I, T>(&mut self, file_id: FileId, old_ranges: I, new_text: T)
    where
        I: IntoIterator<Item = &'a Range<usize>>,
        T: Into<Text>,
    {
        unimplemented!()
    }

    fn integrate_ops<I>(&mut self, ops: I) -> (Changes, Vec<Operation>)
    where
        I: IntoIterator<Item = Operation>,
    {
        unimplemented!()
    }

    fn path(&self, file_id: FileId) -> Option<PathBuf> {
        unimplemented!()
    }

    fn base_path(&self, file_id: FileId) -> Option<PathBuf> {
        unimplemented!()
    }
}
