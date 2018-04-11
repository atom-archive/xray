use serde::de::SeqAccess;
use serde::de::Visitor;
use futures::{Async, Future, Stream};
use notify_cell::NotifyCellObserver;
use parking_lot::RwLock;
use rpc::{client, server};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de;
use serde::ser::{SerializeSeq, SerializeTupleVariant};
use std::ffi::{OsStr, OsString};
use std::iter::Iterator;
use std::path::Path;
use std::rc::Rc;
use std::result;
use std::sync::Arc;
use std::fmt;

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

    // Returns an iterator that
    // fn iter(&self) -> TreeIter {}
}

struct TreeIter {}

struct TreeService {
    tree: Rc<Tree>,
    populated: Option<Box<Future<Item = (), Error = ()>>>,
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

impl TreeService {
    fn new(tree: Rc<Tree>) -> Self {
        Self {
            tree,
            populated: Some(tree.populated()),
        }
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
        if let Some(populated) = self.populated {
            if let Ok(Async::NotReady) = populated.poll() {
                return Async::NotReady;
            }
        }

        self.populated.take();
        Async::Ready(Some(self.tree.root().clone()))
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

impl Serialize for Entry {
    fn serialize<S>(&self, serializer: S) -> result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Entry::Dir(dir) => {
                let mut variant = serializer.serialize_tuple_variant("Entry", 0, "Dir", 4)?;
                variant.serialize_field(&dir.name)?;
                variant.serialize_field(&dir.ignored)?;
                variant.serialize_field(&dir.symlink)?;
                variant.serialize_field(&**dir.children.read())?;
                variant.end()
            },
            Entry::File(file) => {
                let mut variant = serializer.serialize_tuple_variant("Entry", 1, "File", 3)?;
                variant.serialize_field(&file.name)?;
                variant.serialize_field(&file.ignored)?;
                variant.serialize_field(&file.symlink)?;
                variant.end()
            }
        }
    }
}

impl<'a> Deserialize<'a> for Entry {
    fn deserialize<D>(deserializer: D) -> result::Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = Entry;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Duration")
            }

            fn visit_seq<V>(self, mut seq: V) -> result::Result<Entry, V::Error>
                where V: SeqAccess<'de>
            {
                let is_dir = seq.next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;

                if is_dir {
                    let name = seq.next_element()?
                        .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                    let ignored = seq.next_element()?
                        .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                    let symlink = seq.next_element()?
                        .ok_or_else(|| de::Error::invalid_length(3, &self))?;
                    let child_count: u64 = seq.next_element()?
                        .ok_or_else(|| de::Error::invalid_length(3, &self))?;
                    Ok(Entry::dir(name, ignored, symlink))
                } else {
                    let name = seq.next_element()?
                        .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                    let ignored = seq.next_element()?
                        .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                    let symlink = seq.next_element()?
                        .ok_or_else(|| de::Error::invalid_length(3, &self))?;
                    Ok(Entry::file(name, ignored, symlink))
                }
            }
        }

        const VARIANTS: &'static [&'static str] = &["Dir", "File"];
        deserializer.deserialize_enum("Entry", VARIANTS, EntryVisitor)
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
