use notify_cell::{NotifyCell, NotifyCellObserver, WeakNotifyCell};
use parking_lot::RwLock;
use std::ffi::{OsString, OsStr};
use std::path::Path;
use std::result;
use std::sync::Arc;
use std::iter::Iterator;
use futures::{Async, Poll, Stream};
use std::os::unix::ffi::OsStrExt;
use std::usize;
use fuzzy_search::{Search as FuzzySearch, SearchResult, Checkpoint};

pub type Result<T> = result::Result<T, ()>;
type Entries = Vec<(OsString, Entry)>;

pub trait Tree {
    fn path(&self) -> &Path;
    fn root(&self) -> &Entry;
    fn updates(&self) -> Box<Stream<Item = (), Error = ()>>;
}

#[derive(Clone, Debug)]
pub enum Entry {
    Dir(Arc<RwLock<DirInner>>),
    File(Arc<FileInner>),
}

#[derive(Clone, Debug)]
pub struct DirInner {
    entries: Arc<Entries>,
    symlink: bool,
    ignored: bool
}

#[derive(Clone, Debug)]
pub struct FileInner {
    symlink: bool,
    ignored: bool
}

pub struct Search {
    search: FuzzySearch,
    max_results: usize,
    include_ignored: bool,
    results: Vec<SearchResult>,
    stack: Vec<StackEntry>,
    entry_count_per_poll: usize,
    done: bool,
    updates: WeakNotifyCell<Vec<SearchResult>>,
}

pub struct SearchHandle(Arc<()>);

struct StackEntry {
    entries: Arc<Entries>,
    entries_index: usize,
    search_checkpoint: Checkpoint,
}

impl Entry {
    pub fn file(symlink: bool, ignored: bool) -> Self {
        Entry::File(Arc::new(FileInner { symlink, ignored }))
    }

    pub fn dir(symlink: bool, ignored: bool) -> Self {
        Entry::Dir(Arc::new(RwLock::new(DirInner {
            entries: Arc::new(Vec::new()),
            symlink,
            ignored
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

    pub fn search(&self, query: &str, max_results: usize, include_ignored: bool) -> Result<(Search, NotifyCellObserver<Vec<SearchResult>>)> {
        match self {
            &Entry::Dir(ref inner) => Ok(Search::new(inner, query, max_results, include_ignored)),
            _ => Err(())
        }
    }
}

impl Stream for Search {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        if self.done {
            return Ok(Async::Ready(None));
        }

        for _ in 0..self.entry_count_per_poll {
            if self.stack.len() > 0 {
                let (entries, entries_index) = {
                    let last = self.stack.last().unwrap();
                    (last.entries.clone(), last.entries_index)
                };

                if entries_index < entries.len() {
                    let child = &entries[entries_index];

                    match child.1 {
                        Entry::Dir(ref inner) => {
                            let inner = inner.read();
                            if inner.ignored && !self.include_ignored {
                                self.stack.last_mut().map(|last| last.entries_index += 1);
                            } else {
                                self.process_entry(&child.0, false);
                                self.stack.push(StackEntry {
                                    entries: inner.entries.clone(),
                                    entries_index: 0,
                                    search_checkpoint: self.search.get_checkpoint(),
                                });
                            }
                        },
                        Entry::File(ref inner) => {
                            if inner.ignored && !self.include_ignored {
                                self.stack.last_mut().map(|last| last.entries_index += 1);
                            } else {
                                self.process_entry(&child.0, true);
                                let mut last = self.stack.last_mut().unwrap();
                                last.entries_index += 1;
                                self.search.restore_checkpoint(last.search_checkpoint.clone());
                            }
                        }
                    }
                } else {
                    self.stack.pop().unwrap();
                    if let Some(last) = self.stack.last_mut() {
                        self.search.restore_checkpoint(last.search_checkpoint.clone());
                        last.entries_index += 1;
                    }
                }
            } else {
                self.done = true;
                break;
            }
        }

        if self.updates.try_set(self.results.clone()).is_ok() {
            Ok(Async::Ready(Some(())))
        } else {
            self.done = true;
            Ok(Async::Ready(None))
        }
    }
}

impl Search {
    const DEFAULT_ENTRY_COUNT_PER_POLL: usize = 100000;

    fn new(dir: &Arc<RwLock<DirInner>>, query: &str, max_results: usize, include_ignored: bool) -> (Self, NotifyCellObserver<Vec<SearchResult>>) {
        let (updates, updates_observer) = NotifyCell::weak(Vec::new());
        let mut search = FuzzySearch::new(query);
        search
            .set_subword_start_bonus(10)
            .set_consecutive_bonus(5);
        let search_checkpoint = search.get_checkpoint();
        let search = Search {
            search,
            max_results,
            include_ignored,
            updates,
            results: Vec::new(),
            stack: vec![StackEntry {
                entries: dir.read().entries.clone(),
                entries_index: 0,
                search_checkpoint,
            }],
            done: false,
            entry_count_per_poll: Self::DEFAULT_ENTRY_COUNT_PER_POLL,
        };

        (search, updates_observer)
    }

    fn process_entry(&mut self, name: &OsStr, is_file: bool) {
        let separator = if self.stack.len() > 1 {
            Some('/')
        } else {
            None
        };

        let characters = separator.iter().cloned().chain(
            name.as_bytes().iter().map(|c| c.to_ascii_lowercase() as char)
        );

        let match_bonus = if is_file {
            10
        } else {
            1
        };

        self.search.process(characters, match_bonus);

        if is_file {
            if let Some(result) = self.search.finish() {
                match self.results.binary_search_by(|r| result.score.cmp(&r.score)) {
                    Ok(index) | Err(index) => {
                        self.results.insert(index, result);
                        self.results.truncate(self.max_results);
                    }
                }
            }
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
        let root = Entry::dir(false, false);
        assert_eq!(root.insert("a", Entry::file(false, false)), Ok(()));
        assert_eq!(root.insert("c", Entry::file(false, false)), Ok(()));
        assert_eq!(root.insert("b", Entry::file(false, false)), Ok(()));
        assert_eq!(root.insert("a", Entry::file(false, false)), Err(()));
        assert_eq!(root.entry_names(), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_search_subword_start_bonus() {
        let root = build_directory(&json!({
            "ace": {
                "identifier": null
            },
            "cats": {
                "dogs": {
                    "eagles": null
                },
                "indent": null
            },
            "accident": {
                "ogre": null
            }
        }));

        let (mut search, results) = root.search("cde", 10, true).unwrap();
        assert_eq!(search.poll(), Ok(Async::Ready(Some(()))));
        assert_eq!(results.get().unwrap()[0].string, "cats/dogs/eagles");

        let (mut search, results) = root.search("og", 10, true).unwrap();
        assert_eq!(search.poll(), Ok(Async::Ready(Some(()))));
        assert_eq!(results.get().unwrap()[0].string, "accident/ogre");
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
