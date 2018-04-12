use buffer::Buffer;
use fs;
use futures::{Async, Future, Poll};
use fuzzy;
use notify_cell::{NotifyCell, NotifyCellObserver, WeakNotifyCell};
use std::cmp;
use std::collections::{BinaryHeap, HashMap};
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type TreeId = usize;

pub trait Project {
    fn open_buffer(&self, tree_id: TreeId, relative_path: &Path) -> Result<Buffer, OpenError>;
    fn search_paths(
        &self,
        needle: &str,
        max_results: usize,
        include_ignored: bool,
    ) -> (PathSearch, NotifyCellObserver<PathSearchStatus>);
}

pub struct LocalProject {
    next_tree_id: TreeId,
    trees: HashMap<TreeId, Box<fs::LocalTree>>,
}

pub struct PathSearch {
    tree_ids: Vec<TreeId>,
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
    pub tree_id: TreeId,
    pub relative_path: PathBuf,
    pub display_path: PathBuf,
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

#[derive(Debug)]
pub enum OpenError {
    TreeNotFound,
    IOError(io::Error),
}

impl LocalProject {
    pub fn new<T: 'static + fs::LocalTree>(trees: Vec<T>) -> Self {
        let mut project = Self {
            next_tree_id: 0,
            trees: HashMap::new(),
        };
        for tree in trees {
            project.add_tree(tree);
        }
        project
    }

    fn add_tree<T: 'static + fs::LocalTree>(&mut self, tree: T) {
        let id = self.next_tree_id;
        self.next_tree_id += 1;
        self.trees.insert(id, Box::new(tree));
    }
}

impl Project for LocalProject {
    fn open_buffer(&self, tree_id: TreeId, relative_path: &Path) -> Result<Buffer, OpenError> {
        let tree = self.trees.get(&tree_id).ok_or(OpenError::TreeNotFound)?;
        let mut absolute_path = tree.path().to_owned();
        absolute_path.push(relative_path);

        let file = File::open(absolute_path).map_err(|error| OpenError::IOError(error))?;
        let mut buf_reader = BufReader::new(file);
        let mut contents = String::new();
        buf_reader
            .read_to_string(&mut contents)
            .map_err(|error| OpenError::IOError(error))?;

        let mut buffer = Buffer::new(1);
        buffer.splice(0..0, contents.as_str());

        Ok(buffer)
    }

    fn search_paths(
        &self,
        needle: &str,
        max_results: usize,
        include_ignored: bool,
    ) -> (PathSearch, NotifyCellObserver<PathSearchStatus>) {
        let (updates, updates_observer) = NotifyCell::weak(PathSearchStatus::Pending);

        let mut tree_ids = Vec::new();
        let mut roots = Vec::new();
        for (id, tree) in &self.trees {
            tree_ids.push(*id);
            roots.push(tree.root().clone());
        }

        let search = PathSearch {
            tree_ids,
            roots: Arc::new(roots),
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
        let mut children = if self.roots.len() == 1 {
            self.roots[0].children().unwrap()
        } else {
            self.roots.clone()
        };
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
        let mut children = if self.roots.len() == 1 {
            self.roots[0].children().unwrap()
        } else {
            self.roots.clone()
        };
        let mut child_index = 0;
        let mut found_match = false;

        loop {
            self.check_cancellation(&mut steps_since_last_check, 1000)?;
            let stack = &mut self.stack;

            if child_index < children.len() {
                if children[child_index].is_ignored() && !self.include_ignored {
                    child_index += 1;
                } else if children[child_index].is_dir() {
                    let descend;
                    let child_is_match;

                    if found_match {
                        child_is_match = true;
                        descend = true;
                    } else {
                        match matches.get(&children[child_index].id()) {
                            Some(&MatchMarker::IsMatch) => {
                                child_is_match = true;
                                descend = true;
                            }
                            Some(&MatchMarker::ContainsMatch) => {
                                child_is_match = false;
                                descend = true;
                            }
                            None => {
                                child_is_match = false;
                                descend = false;
                            }
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
                        found_match = child_is_match;
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
                            let tree_id = if self.roots.len() == 1 {
                                self.tree_ids[0]
                            } else {
                                self.tree_ids[stack[0].child_index]
                            };

                            let mut relative_path = PathBuf::new();
                            let mut display_path = PathBuf::new();
                            for (i, entry) in stack.iter().enumerate() {
                                let name = entry.children[entry.child_index].name();
                                if self.roots.len() == 1 || i != 0 {
                                    relative_path.push(name);
                                }
                                display_path.push(name);
                            }
                            let file_name = children[child_index].name();
                            relative_path.push(file_name);
                            display_path.push(file_name);
                            if results.len() == self.max_results {
                                results.pop();
                            }
                            results.push(PathSearchResult {
                                score,
                                tree_id,
                                relative_path,
                                display_path,
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
    fn check_cancellation(
        &self,
        steps_since_last_check: &mut usize,
        steps_between_checks: usize,
    ) -> Result<(), ()> {
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
    use fs::tests::TestTree;

    #[test]
    fn test_search_one_tree() {
        let tree = TestTree::from_json(
            "/Users/someone/tree",
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
        let project = LocalProject::new(vec![tree]);
        let (mut search, observer) = project.search_paths("sub2", 10, true);

        assert_eq!(search.poll(), Ok(Async::Ready(())));
        assert_eq!(
            summarize_results(&project, &observer.get()),
            Some(vec![
                (
                    "/Users/someone/tree/root-2/subdir-2/file-3".to_string(),
                    "root-2/subdir-2/file-3".to_string(),
                    vec![7, 8, 9, 14],
                ),
                (
                    "/Users/someone/tree/root-2/subdir-2/file-4".to_string(),
                    "root-2/subdir-2/file-4".to_string(),
                    vec![7, 8, 9, 14],
                ),
                (
                    "/Users/someone/tree/root-1/subdir-1/file-2".to_string(),
                    "root-1/subdir-1/file-2".to_string(),
                    vec![7, 8, 9, 21],
                ),
            ])
        );
    }

    #[test]
    fn test_search_many_trees() {
        let tree_1 = TestTree::from_json(
            "/Users/someone/foo",
            json!({
                "subdir-a": {
                    "file-1": null,
                    "subdir-1": {
                        "file-1": null,
                        "bar": null,
                    }
                }
            }),
        );
        let tree_2 = TestTree::from_json(
            "/Users/someone/bar",
            json!({
                "subdir-b": {
                    "subdir-2": {
                        "file-3": null,
                        "foo": null,
                    }
                }
            }),
        );
        let project = LocalProject::new(vec![tree_1, tree_2]);

        let (mut search, observer) = project.search_paths("bar", 10, true);
        assert_eq!(search.poll(), Ok(Async::Ready(())));
        assert_eq!(
            summarize_results(&project, &observer.get()),
            Some(vec![
                (
                    "/Users/someone/bar/subdir-b/subdir-2/foo".to_string(),
                    "bar/subdir-b/subdir-2/foo".to_string(),
                    vec![0, 1, 2],
                ),
                (
                    "/Users/someone/foo/subdir-a/subdir-1/bar".to_string(),
                    "foo/subdir-a/subdir-1/bar".to_string(),
                    vec![22, 23, 24],
                ),
                (
                    "/Users/someone/bar/subdir-b/subdir-2/file-3".to_string(),
                    "bar/subdir-b/subdir-2/file-3".to_string(),
                    vec![0, 1, 2],
                ),
                (
                    "/Users/someone/foo/subdir-a/subdir-1/file-1".to_string(),
                    "foo/subdir-a/subdir-1/file-1".to_string(),
                    vec![6, 11, 18],
                ),
            ])
        );
    }

    fn summarize_results<'a>(
        project: &LocalProject,
        results: &'a PathSearchStatus,
    ) -> Option<Vec<(String, String, Vec<usize>)>> {
        match results {
            &PathSearchStatus::Pending => None,
            &PathSearchStatus::Ready(ref results) => {
                let summary = results
                    .iter()
                    .map(|result| {
                        let mut absolute_path =
                            PathBuf::from(project.trees.get(&result.tree_id).unwrap().path());
                        absolute_path.push(&result.relative_path);
                        let absolute_path = absolute_path.to_str().unwrap().to_string();
                        let display_path = result.display_path.to_str().unwrap().to_string();
                        (absolute_path, display_path, result.positions.clone())
                    })
                    .collect();
                Some(summary)
            }
        }
    }
}
