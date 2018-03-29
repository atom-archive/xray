use parking_lot::RwLock;
use std::ffi::{OsString, OsStr};
use std::path::Path;
use std::result;
use std::sync::Arc;
use std::iter::Iterator;
use futures::Stream;

pub type EntryId = u32;
pub type Result<T> = result::Result<T, ()>;

pub trait Tree {
    fn path(&self) -> &Path;
    fn root(&self) -> &Entry;
    fn updates(&self) -> Box<Stream<Item = (), Error = ()>>;
}

#[derive(Clone, Debug)]
pub enum Entry {
    Dir(Arc<DirInner>),
    File(Arc<FileInner>),
}

#[derive(Debug)]
pub struct DirInner {
    id: EntryId,
    name: OsString,
    name_chars: Vec<char>,
    children: RwLock<Arc<Vec<Entry>>>,
    symlink: bool,
    ignored: bool
}

#[derive(Clone, Debug)]
pub struct FileInner {
    id: EntryId,
    name: OsString,
    name_chars: Vec<char>,
    symlink: bool,
    ignored: bool
}

impl Entry {
    pub fn file(id: EntryId, name: OsString, symlink: bool, ignored: bool) -> Self {
        Entry::File(Arc::new(FileInner {
            id,
            name_chars: name.to_string_lossy().chars().collect(),
            name,
            symlink,
            ignored
        }))
    }

    pub fn dir(id: EntryId, name: OsString, symlink: bool, ignored: bool) -> Self {
        Entry::Dir(Arc::new(DirInner {
            id,
            name_chars: name.to_string_lossy().chars().collect(),
            name,
            children: RwLock::new(Arc::new(Vec::new())),
            symlink,
            ignored
        }))
    }

    pub fn is_dir(&self) -> bool {
        match self {
            &Entry::Dir(_) => true,
            &Entry::File(_) => false
        }
    }

    pub fn id(&self) -> EntryId {
        match self {
            &Entry::Dir(ref inner) => inner.id,
            &Entry::File(ref inner) => inner.id
        }
    }

    pub fn name(&self) -> &OsStr {
        match self {
            &Entry::Dir(ref inner) => &inner.name,
            &Entry::File(ref inner) => &inner.name
        }
    }

    pub fn name_chars(&self) -> &[char] {
        match self {
            &Entry::Dir(ref inner) => &inner.name_chars,
            &Entry::File(ref inner) => &inner.name_chars
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
            &Entry::File(..) => None
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
                            Err(index) => index
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
    use serde_json;

    impl Entry {
        fn entry_names(&self) -> Vec<String> {
            match self {
                &Entry::Dir(ref inner) => inner
                    .read()
                    .children
                    .iter()
                    .map(|&(ref name, _)| name.clone().into_string().unwrap())
                    .collect(),
                _ => panic!(),
            }
        }
    }

    #[test]
    fn test_insert() {
        let root = Entry::dir(0, false, false);
        assert_eq!(root.insert("a", Entry::file(1, false, false)), Ok(()));
        assert_eq!(root.insert("c", Entry::file(2, false, false)), Ok(()));
        assert_eq!(root.insert("b", Entry::file(3, false, false)), Ok(()));
        assert_eq!(root.insert("a", Entry::file(4, false, false)), Err(()));
        assert_eq!(root.entry_names(), vec!["a", "b", "c"]);
    }

    fn build_directory(json: &serde_json::Value) -> Entry {
        let object = json.as_object().unwrap();
        let result = Entry::dir(false, false);
        for (key, value) in object {
            let child_entry = if value.is_object() {
                build_directory(value)
            } else {
                Entry::file(false, false)
            };
            assert_eq!(result.insert(key, child_entry), Ok(()));
        }
        result
    }
}
