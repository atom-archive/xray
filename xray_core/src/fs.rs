use futures::{Async, Future, Stream};
use parking_lot::RwLock;
use rpc::server;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(test)]
use serde_json;
use std::ffi::{OsStr, OsString};
use std::iter::Iterator;
use std::path::Path;
use std::rc::Rc;
use std::result;
use std::sync::Arc;

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
    fn populated(&self) -> Box<Future<Item = (), Error = ()>>;
}

struct TreeService {
    tree: Rc<Tree>,
    populated: Option<Box<Future<Item = (), Error = ()>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Entry {
    #[serde(serialize_with = "serialize_dir", deserialize_with = "deserialize_dir")]
    Dir(Arc<DirInner>),
    #[serde(serialize_with = "serialize_file", deserialize_with = "deserialize_file")]
    File(Arc<FileInner>),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DirInner {
    name: OsString,
    #[serde(skip_serializing, skip_deserializing)]
    name_chars: Vec<char>,
    #[serde(serialize_with = "serialize_dir_children")]
    #[serde(deserialize_with = "deserialize_dir_children")]
    children: RwLock<Arc<Vec<Entry>>>,
    symlink: bool,
    ignored: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileInner {
    name: OsString,
    name_chars: Vec<char>,
    symlink: bool,
    ignored: bool,
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

    #[cfg(test)]
    pub(crate) fn from_json(name: &str, json: &serde_json::Value) -> Self {
        if json.is_object() {
            let object = json.as_object().unwrap();
            let dir = Entry::dir(OsString::from(name), false, false);
            for (key, value) in object {
                let child_entry = Self::from_json(key, value);
                assert_eq!(dir.insert(child_entry), Ok(()));
            }
            dir
        } else {
            Entry::file(OsString::from(name), false, false)
        }
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

fn serialize_dir<S: Serializer>(
    dir: &Arc<DirInner>,
    serializer: S,
) -> result::Result<S::Ok, S::Error> {
    dir.serialize(serializer)
}

fn deserialize_dir<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> result::Result<Arc<DirInner>, D::Error> {
    let mut inner = DirInner::deserialize(deserializer)?;

    let mut name_chars: Vec<char> = inner.name.to_string_lossy().chars().collect();
    name_chars.push('/');
    inner.name_chars = name_chars;

    Ok(Arc::new(inner))
}

fn serialize_file<S: Serializer>(
    file: &Arc<FileInner>,
    serializer: S,
) -> result::Result<S::Ok, S::Error> {
    file.serialize(serializer)
}

fn deserialize_file<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> result::Result<Arc<FileInner>, D::Error> {
    let mut inner = FileInner::deserialize(deserializer)?;
    inner.name_chars = inner.name.to_string_lossy().chars().collect();
    Ok(Arc::new(inner))
}

fn serialize_dir_children<S: Serializer>(
    children: &RwLock<Arc<Vec<Entry>>>,
    serializer: S,
) -> result::Result<S::Ok, S::Error> {
    children.read().serialize(serializer)
}

fn deserialize_dir_children<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> result::Result<RwLock<Arc<Vec<Entry>>>, D::Error> {
    Ok(RwLock::new(Arc::new(Vec::deserialize(deserializer)?)))
}

impl TreeService {
    fn new(tree: Rc<Tree>) -> Self {
        let populated = Some(tree.populated());
        Self { tree, populated }
    }
}

impl server::Service for TreeService {
    type State = ();
    type Update = Entry;
    type Request = ();
    type Response = ();

    fn state(&self, _: &server::Connection) -> Self::State {
        ()
    }

    fn poll_update(&mut self, _: &server::Connection) -> Async<Option<Self::Update>> {
        if let Some(populated) = self.populated.as_mut().map(|p| p.poll().unwrap()) {
            if let Async::Ready(_) = populated {
                self.populated.take();
                Async::Ready(Some(self.tree.root().clone()))
            } else {
                Async::NotReady
            }
        } else {
            Async::NotReady
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bincode::{deserialize, serialize};
    use serde_json;

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

    #[test]
    fn test_serialize_deserialize() {
        let root = Entry::from_json(
            "root",
            &json!({
                "child-1": {
                    "subchild-1-1": null
                },
                "child-2": null,
                "child-3": {
                    "subchild-3-1": {
                        "subchild-3-1-1": null,
                        "subchild-3-1-2": null,
                    }
                }
            }),
        );
        assert_eq!(
            deserialize::<Entry>(&serialize(&root).unwrap()).unwrap(),
            root
        );
    }

            }
        }

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

    impl PartialEq for Entry {
        fn eq(&self, other: &Self) -> bool {
            self.name() == other.name() && self.name_chars() == other.name_chars()
                && self.is_dir() == other.is_dir()
                && self.is_ignored() == other.is_ignored()
                && self.children() == other.children()
        }
    }
}
