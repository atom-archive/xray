use futures::{Async, Future, Stream};
use futures::task::{self, Task};
use parking_lot::RwLock;
use std::ffi::{OsStr, OsString};
use std::iter::Iterator;
use std::path::Path;
use std::result;
use std::sync::Arc;
use std::rc::Rc;
use std::rpc::{server, client};
use notify_cell::NotifyCellObserver;

pub type EntryId = usize;
pub type Result<T> = result::Result<T, ()>;

pub trait Tree {
    fn path(&self) -> &Path;
    fn root(&self) -> &Entry;
    fn updates(&self) -> Box<Stream<Item = (), Error = ()>>;

    // Returns a promise that resolves once tree is populated
    // We could potentially implement this promise from an observer for a boolean notify cell
    // to avoid needing to maintain a set of oneshot channels or something similar.
    // cell.observe().skip_while(|resolved| !resolved).into_future().then(Ok(()))
    fn populated() -> Box<Future<Item = (), Error = ()>>;

    // Returns an iterator that
    fn iter() -> TreeIter {

    }
}

struct TreeIter {}

impl Iterator for TreeIter {
    type Item = TreeUpdate;

    fn next(&mut self) -> Option<Self::Item> {

    }
}

struct TreeService {
    tree: Rc<Tree>,
    populated: Option<Box<Future<Item=(), Error = ()>>>,
}

#[derive(Clone, Debug)]
pub enum Entry {
    Dir(Arc<DirInner>),
    File(Arc<FileInner>),
}

#[derive(Debug)]
pub struct DirInner {
    name: OsString,
    name_chars: Vec<char>,
    children: RwLock<Arc<Vec<Entry>>>,
    symlink: bool,
    ignored: bool,
}

#[derive(Clone, Debug)]
pub struct FileInner {
    name: OsString,
    name_chars: Vec<char>,
    symlink: bool,
    ignored: bool,
}

enum TreeUpdate {

}

impl server::Service for TreeService {
    type State = ();
    type Update = TreeUpdate;

    fn new(tree: Rc<Tree>) -> Self {
        Self { tree, populated }
    }

    fn state(&self, _: &server::Connection) -> Self::State {
        ()
    }

    fn poll_update(&mut self, _: &server::Connection) -> Async<Option<Self::Update>> {
        self.updates.poll().unwrap()
    }
}

impl Entry {
    pub fn file(name: OsString, symlink: bool, ignored: bool) -> Self {
        Entry::File(Arc::new(FileInner {
            name_chars: name.to_string_lossy().chars().collect(),
            name,
            symlink,
            ignored,
        }))
    }

    pub fn dir(name: OsString, symlink: bool, ignored: bool) -> Self {
        let mut name_chars: Vec<char> = name.to_string_lossy().chars().collect();
        name_chars.push('/');
        Entry::Dir(Arc::new(DirInner {
            name_chars,
            name,
            children: RwLock::new(Arc::new(Vec::new())),
            symlink,
            ignored,
        }))
    }

    pub fn is_dir(&self) -> bool {
        match self {
            &Entry::Dir(_) => true,
            &Entry::File(_) => false,
        }
    }

    pub fn id(&self) -> EntryId {
        match self {
            &Entry::Dir(ref inner) => inner.as_ref() as *const DirInner as EntryId,
            &Entry::File(ref inner) => inner.as_ref() as *const FileInner as EntryId,
        }
    }

    pub fn name(&self) -> &OsStr {
        match self {
            &Entry::Dir(ref inner) => &inner.name,
            &Entry::File(ref inner) => &inner.name,
        }
    }

    pub fn name_chars(&self) -> &[char] {
        match self {
            &Entry::Dir(ref inner) => &inner.name_chars,
            &Entry::File(ref inner) => &inner.name_chars,
        }
    }

    pub fn is_ignored(&self) -> bool {
        match self {
            &Entry::Dir(ref inner) => inner.ignored,
            &Entry::File(ref inner) => inner.ignored,
        }
    }

    pub fn children(&self) -> Option<Arc<Vec<Entry>>> {
        match self {
            &Entry::Dir(ref inner) => Some(inner.children.read().clone()),
            &Entry::File(..) => None,
        }
    }

    pub fn insert(&self, new_entry: Entry) -> Result<()> {
        match self {
            &Entry::Dir(ref inner) => {
                let mut children = inner.children.write();
                let children = Arc::make_mut(&mut children);
                if children
                    .last()
                    .map(|child| child.name() < new_entry.name())
                    .unwrap_or(true)
                {
                    children.push(new_entry);
                    Ok(())
                } else {
                    let index = {
                        let new_name = new_entry.name();
                        match children.binary_search_by(|child| child.name().cmp(new_name)) {
                            Ok(_) => return Err(()), // An entry already exists with this name
                            Err(index) => index,
                        }
                    };
                    children.insert(index, new_entry);
                    Ok(())
                }
            }
            &Entry::File(_) => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Entry {
        fn entry_names(&self) -> Vec<String> {
            match self {
                &Entry::Dir(ref inner) => inner
                    .children
                    .read()
                    .iter()
                    .map(|ref entry| entry.name().to_string_lossy().into_owned())
                    .collect(),
                _ => panic!(),
            }
        }
    }

    #[test]
    fn test_insert() {
        let root = Entry::dir(OsString::from("root"), false, false);
        assert_eq!(
            root.insert(Entry::file(OsString::from("a"), false, false)),
            Ok(())
        );
        assert_eq!(
            root.insert(Entry::file(OsString::from("c"), false, false)),
            Ok(())
        );
        assert_eq!(
            root.insert(Entry::file(OsString::from("b"), false, false)),
            Ok(())
        );
        assert_eq!(
            root.insert(Entry::file(OsString::from("a"), false, false)),
            Err(())
        );
        assert_eq!(root.entry_names(), vec!["a", "b", "c"]);
    }
}
