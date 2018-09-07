use buffer::{Buffer, Text};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::ops::Range;
use std::path::{Path, PathBuf};
use time;

type FileId = time::Local;

enum Operation {}

struct Patch {
    
}

struct Changes {
    inserted: HashSet<FileId>,
    renamed: HashSet<FileId>,
    removed: HashSet<FileId>,
    edited: HashSet<FileId>,
}

impl Patch {
    fn file_id(&mut self, path: &Path) -> (FileId, Option<Operation>) {
        unimplemented!()
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
