use futures::Stream;
use parking_lot::RwLock;
use std::collections::BinaryHeap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::result;
use std::sync::Arc;
use notify_cell::{NotifyCell, NotifyCellObserver};
use futures::{Future, Poll};

pub type Result<T> = result::Result<T, ()>;
type Entries = Vec<(OsString, Entry)>;

pub trait Tree {
    fn path(&self) -> &Path;
    fn root(&self) -> &Entry;
    fn updates(&self) -> Box<Stream<Item = (), Error = ()>>;

    fn search_results() -> NotifyCellObserver<Vec<SearchResult>>;

    fn search(&self, query: &str, max_results: usize) -> Result<Search> {
        match self.root() {
            &Entry::Dir(ref inner) => {
                Ok(Search {
                    query: query.to_owned(),
                    max_results,
                    results: BinaryHeap::new(),
                    stack: vec![(inner.read().entries.clone(), 0)]
                })
            },
            _ => Err(())
        }
    }
}

#[derive(Clone, Debug)]
pub enum Entry {
    Dir(Arc<RwLock<DirInner>>),
    File(Arc<FileInner>),
}

#[derive(Clone, Debug)]
pub struct DirInner {
    entries: Arc<Entries>,
    is_symlink: bool,
}

#[derive(Clone, Debug)]
pub struct FileInner {
    is_symlink: bool,
}

pub struct Search {
    query: String,
    max_results: usize,
    results: BinaryHeap<SearchResult>,
    stack: Vec<(Arc<Entries>, usize)>
}

pub struct SearchResult {
    score: usize,
    path: PathBuf,
    matches: Vec<usize>
}

struct StackEntry {
    entries: Arc<Entries>,
    entries_index: usize,
    match_variants: Vec<MatchVariant>
}

struct MatchVariant {
    query_index: u16,
    score: usize,
}

impl Entry {
    pub fn file(is_symlink: bool) -> Self {
        Entry::File(Arc::new(FileInner { is_symlink }))
    }

    pub fn dir(is_symlink: bool) -> Self {
        Entry::Dir(Arc::new(RwLock::new(DirInner {
            entries: Arc::new(Vec::new()),
            is_symlink,
        })))
    }

    pub fn insert<T: Into<OsString>>(&self, new_name: T, new_entry: Entry) -> Result<()> {
        match self {
            &Entry::Dir(ref inner) => {
                let new_name = new_name.into();

                let mut inner = inner.write();
                let entries = Arc::make_mut(&mut inner.entries);
                if entries
                    .last()
                    .map(|&(ref name, _)| name < &new_name)
                    .unwrap_or(true)
                {
                    entries.push((new_name, new_entry));
                    Ok(())
                } else {
                    match entries.binary_search_by(|&(ref name, _)| name.cmp(&new_name)) {
                        Ok(_) => Err(()), // An entry already exists with this name
                        Err(index) => {
                            entries.insert(index, (new_name, new_entry));
                            Ok(())
                        }
                    }
                }
            }
            &Entry::File(_) => Err(()),
        }
    }
}

impl Search {
    pub fn results(&self) -> NotifyCellObserver<Vec<SearchResult>> {
        self.updates.observe()
    }
}

// bob/axelllllllasdjfklasdfadsfdasf/a

// root: bob
// query: bxx

// (bob, 0, [{query_index: 0, score: 0}, {query_index: 1, score: 5}, {query_index: 1, score: 3}])
// (axel, 0, [{query_index: 0, score: 0}, {query_index: 1, score: 5}, {query_index: 1, score: 3}, {query_index: 2, score: 6}])


impl Future for Search {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        loop {
            let cur_dir = self.stack.last();
            if cur_dir.entries_index == cur_dir.entries.len() {
                self.parent_path.pop();
                if let Some(last) = self.stack.pop() {
                    self.match_variants.truncate(last.prev_variants_len);
                } else {
                    Async::Ready(())
                }
            } else {
                let child = cur_dir.entries[index];
                match child {
                    Entry::Dir(ref inner) => {
                        self.parent_path.push(name);
                        let prev_variants_len = self.match_variants.len();
                        compute_match_variants(&mut self.match_variants, query, inner.name);
                        self.stack.push(StackEntry {
                            entries: inner.entries.clone(),
                            entries_index: 0,
                            prev_variants_len
                        })
                    },
                    Entry::File(ref inner) => {
                        // Fuzzy match on file name.

                        index += 1;
                    }
                }
            }
        }


        // scan tree...
        // every now and then call notifyCell.set(latest_result)
        // Async::NotReady()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    impl Entry {
        fn entry_names(&self) -> Vec<String> {
            match self {
                &Entry::Dir(ref inner) => inner
                    .read()
                    .entries
                    .iter()
                    .map(|&(ref name, _)| name.clone().into_string().unwrap())
                    .collect(),
                _ => panic!(),
            }
        }
    }

    #[test]
    fn test_insert() {
        let root = Entry::dir(false);
        assert_eq!(root.insert("a", Entry::file(false)), Ok(()));
        assert_eq!(root.insert("c", Entry::file(false)), Ok(()));
        assert_eq!(root.insert("b", Entry::file(false)), Ok(()));
        assert_eq!(root.insert("a", Entry::file(false)), Err(()));
        assert_eq!(root.entry_names(), vec!["a", "b", "c"]);
    }
}
