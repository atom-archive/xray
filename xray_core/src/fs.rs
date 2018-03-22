use futures::Stream;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::result;
use std::sync::Arc;

pub type Result<T> = result::Result<T, Error>;

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    InvalidPath
}

pub trait Tree {
    fn path(&self) -> &Path;
    fn root(&self) -> &Dir;
    fn updates(&self) -> Box<Stream<Item = (), Error = ()>>;
}

#[derive(Clone, Debug)]
pub struct Dir(Arc<RwLock<DirInner>>);

#[derive(Clone, Debug)]
struct DirInner {
    dirs: BTreeMap<OsString, Dir>,
    files: BTreeMap<OsString, File>,
    is_symlink: bool
}

#[derive(Clone, Debug)]
pub struct File(Arc<FileInner>);

#[derive(Clone, Debug)]
struct FileInner {
    is_symlink: bool
}

impl Dir {
    pub fn new(is_symlink: bool) -> Self {
        Dir(Arc::new(RwLock::new(DirInner {
            dirs: BTreeMap::new(),
            files: BTreeMap::new(),
            is_symlink
        })))
    }

    pub fn add_dir<T: Into<OsString>>(&self, name: T, dir: Dir) {
        self.0.write().dirs.insert(name.into(), dir);
    }

    pub fn add_file<T: Into<OsString>>(&self, name: T, dir: File) {
        self.0.write().files.insert(name.into(), dir);
    }
}

impl File {
    pub fn new(is_symlink: bool) -> Self {
        File(Arc::new(FileInner { is_symlink }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_insertion() {
        let mut root = Entry::dir();
        assert_eq!(root.insert(Path::new("foo"), Entry::dir()), Ok(None));
        assert_eq!(root.insert(Path::new("foo/bar/baz"), Entry::dir()), Err(Error::InvalidPath));
        assert_eq!(root.insert(Path::new("foo/bar"), Entry::dir()), Ok(None));
        assert_eq!(root.insert(Path::new("foo/bar/baz"), Entry::dir()), Ok(None));
        assert_eq!(root.insert(Path::new("foo/bar/baz"), Entry::dir()), Ok(Some(Entry::dir())));
    }
}
