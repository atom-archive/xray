use fs;
use futures::{Async, Future, Poll};
use fuzzy;
use notify_cell::{NotifyCell, NotifyCellObserver, WeakNotifyCell};
use std::cmp;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

pub struct Project {
    pub trees: Vec<Box<fs::Tree>>,
}

pub struct PathSearch {
    roots: Arc<Vec<fs::Entry>>,
    needle: Vec<char>,
    max_results: usize,
    include_ignored: bool,
    stack: Vec<StackEntry>,
    updates: WeakNotifyCell<PathSearchStatus>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PathSearchStatus {
    Pending,
    Ready(Vec<PathSearchResult>),
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PathSearchResult {
    pub score: fuzzy::Score,
    pub positions: Vec<usize>,
    pub path: PathBuf,
}

struct StackEntry {
    children: Arc<Vec<fs::Entry>>,
    child_index: usize,
    found_match: bool,
}

#[derive(Debug)]
enum MatchMarker {
    ContainsMatch,
    IsMatch,
}

impl Project {
    pub fn new(trees: Vec<Box<fs::Tree>>) -> Self {
        Project { trees }
    }

    pub fn search_paths(
        &self,
        needle: &str,
        max_results: usize,
        include_ignored: bool,
    ) -> (PathSearch, NotifyCellObserver<PathSearchStatus>) {
        let (updates, updates_observer) = NotifyCell::weak(PathSearchStatus::Pending);
        let search = PathSearch {
            roots: Arc::new(self.trees.iter().map(|tree| tree.root().clone()).collect()),
            needle: needle.chars().collect(),
            max_results,
            include_ignored,
            stack: Vec::new(),
            updates,
        };

        (search, updates_observer)
    }
}

impl Future for PathSearch {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if self.needle.is_empty() {
            let _ = self.updates.try_set(PathSearchStatus::Ready(Vec::new()));
        } else {
            let matches = self.find_matches()?;
            let results = self.rank_matches(matches)?;
            let _ = self.updates.try_set(PathSearchStatus::Ready(results));
        }
        Ok(Async::Ready(()))
    }
}

impl PathSearch {
    fn find_matches(&mut self) -> Result<HashMap<fs::EntryId, MatchMarker>, ()> {
        let mut results = HashMap::new();
        let mut matcher = fuzzy::Matcher::new(&self.needle);

        let mut steps_since_last_check = 0;
        let mut children = self.roots.clone();
        let mut child_index = 0;
        let mut found_match = false;

        loop {
            self.check_cancellation(&mut steps_since_last_check, 10000)?;
            let stack = &mut self.stack;

            if child_index < children.len() {
                if children[child_index].is_ignored() {
                    child_index += 1;
                    continue;
                }

                if matcher.push(&children[child_index].name_chars()) {
                    matcher.pop();
                    results.insert(children[child_index].id(), MatchMarker::IsMatch);
                    found_match = true;
                    child_index += 1;
                } else if children[child_index].is_dir() {
                    let next_children = children[child_index].children().unwrap();
                    stack.push(StackEntry {
                        children: children,
                        child_index,
                        found_match,
                    });
                    children = next_children;
                    child_index = 0;
                    found_match = false;
                } else {
                    matcher.pop();
                    child_index += 1;
                }
            } else if stack.len() > 0 {
                matcher.pop();
                let entry = stack.pop().unwrap();
                children = entry.children;
                child_index = entry.child_index;
                if found_match {
                    results.insert(children[child_index].id(), MatchMarker::ContainsMatch);
                } else {
                    found_match = entry.found_match;
                }
                child_index += 1;
            } else {
                break;
            }
        }

        Ok(results)
    }

    fn rank_matches(
        &mut self,
        matches: HashMap<fs::EntryId, MatchMarker>,
    ) -> Result<Vec<PathSearchResult>, ()> {
        let mut results: BinaryHeap<PathSearchResult> = BinaryHeap::new();
        let mut positions = Vec::new();
        positions.resize(self.needle.len(), 0);
        let mut scorer = fuzzy::Scorer::new(&self.needle);

        let mut steps_since_last_check = 0;
        let mut children = self.roots.clone();
        let mut child_index = 0;
        let mut found_match = false;

        loop {
            self.check_cancellation(&mut steps_since_last_check, 1000)?;
            let stack = &mut self.stack;

            if child_index < children.len() {
                if children[child_index].is_ignored() && !self.include_ignored {
                    child_index += 1;
                } else if children[child_index].is_dir() {
                    let descend = found_match || {
                        match matches.get(&children[child_index].id()) {
                            Some(&MatchMarker::IsMatch) => {
                                found_match = true;
                                true
                            }
                            Some(&MatchMarker::ContainsMatch) => true,
                            None => false,
                        }
                    };
                    if descend {
                        scorer.push(children[child_index].name_chars(), None);
                        let next_children = children[child_index].children().unwrap();
                        stack.push(StackEntry {
                            child_index,
                            children,
                            found_match,
                        });
                        children = next_children;
                        child_index = 0;
                    } else {
                        child_index += 1;
                    }
                } else {
                    if found_match || matches.contains_key(&children[child_index].id()) {
                        let score =
                            scorer.push(children[child_index].name_chars(), Some(&mut positions));
                        scorer.pop();
                        if results.len() < self.max_results
                            || score > results.peek().map(|r| r.score).unwrap()
                        {
                            let mut path = PathBuf::new();
                            for entry in stack.iter() {
                                path.push(entry.children[entry.child_index].name());
                            }
                            path.push(children[child_index].name());

                            if results.len() == self.max_results {
                                results.pop();
                            }
                            results.push(PathSearchResult {
                                score,
                                path,
                                positions: positions.clone(),
                            });
                        }
                    }
                    child_index += 1;
                }
            } else if stack.len() > 0 {
                scorer.pop();
                let entry = stack.pop().unwrap();
                children = entry.children;
                child_index = entry.child_index;
                found_match = entry.found_match;
                child_index += 1;
            } else {
                break;
            }
        }

        Ok(results.into_sorted_vec())
    }

    #[inline(always)]
    fn check_cancellation(&self, steps_since_last_check: &mut usize, steps_between_checks: usize) -> Result<(), ()> {
        *steps_since_last_check += 1;
        if *steps_since_last_check == steps_between_checks {
            if self.updates.has_observers() {
                *steps_since_last_check = 0;
            } else {
                return Err(());
            }
        }
        Ok(())
    }
}

impl Ord for PathSearchResult {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.partial_cmp(other).unwrap_or(cmp::Ordering::Equal)
    }
}

impl PartialOrd for PathSearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        // Reverse the comparison so results with lower scores sort
        // closer to the top of the results heap.
        other.score.partial_cmp(&self.score)
    }
}

impl Eq for PathSearchResult {}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::Stream;
    use serde_json;
    use std::ffi::OsString;
    use std::path::Path;

    #[test]
    fn test_search_one_tree() {
        let tree = TestTree::from_json(
            "tree",
            json!({
                "root-1": {
                    "file-1": null,
                    "subdir-1": {
                        "file-1": null,
                        "file-2": null,
                    }
                },
                "root-2": {
                    "subdir-2": {
                        "file-3": null,
                        "file-4": null,
                    }
                }
            }),
        );
        let project = Project::new(vec![Box::new(tree)]);
        let (mut search, observer) = project.search_paths("sub2", 10, true);

        assert_eq!(search.poll(), Ok(Async::Ready(())));
        assert_eq!(
            summarize_results(&observer.get()),
            Some(vec![
                ("tree/root-2/subdir-2/file-3", vec![12, 13, 14, 19]),
                ("tree/root-2/subdir-2/file-4", vec![12, 13, 14, 19]),
                ("tree/root-1/subdir-1/file-2", vec![12, 13, 14, 26]),
            ])
        );
    }

    fn summarize_results(results: &PathSearchStatus) -> Option<Vec<(&str, Vec<usize>)>> {
        match results {
            &PathSearchStatus::Pending => None,
            &PathSearchStatus::Ready(ref results) => {
                Some(results.iter().map(|r| (r.path.to_str().unwrap(), r.positions.clone())).collect())
            }
        }
    }

    struct TestTree {
        root: fs::Entry,
        path: PathBuf,
    }

    impl fs::Tree for TestTree {
        fn root(&self) -> &fs::Entry {
            &self.root
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn updates(&self) -> Box<Stream<Item = (), Error = ()>> {
            unimplemented!()
        }
    }

    impl TestTree {
        fn from_json<T: Into<PathBuf>>(path: T, json: serde_json::Value) -> Self {
            fn build_entry(name: &str, json: &serde_json::Value) -> fs::Entry {
                if json.is_object() {
                    let object = json.as_object().unwrap();
                    let dir = fs::Entry::dir(OsString::from(name), false, false);
                    for (key, value) in object {
                        let child_entry = build_entry(key, value);
                        assert_eq!(dir.insert(child_entry), Ok(()));
                    }
                    dir
                } else {
                    fs::Entry::file(OsString::from(name), false, false)
                }
            }

            let path = path.into();
            Self {
                root: build_entry(path.to_str().unwrap(), &json),
                path,
            }
        }
    }
}
