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

#[derive(Clone)]
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
        let matches = self.find_matches();
        let results = self.rank_matches(matches);
        let _ = self.updates.try_set(PathSearchStatus::Ready(results));
        Ok(Async::Ready(()))
    }
}

impl PathSearch {
    fn find_matches(&mut self) -> HashMap<fs::EntryId, MatchMarker> {
        let mut results = HashMap::new();
        let mut matcher = fuzzy::Matcher::new(&self.needle);

        let stack = &mut self.stack;
        let mut children = self.roots.clone();
        let mut child_index = 0;
        let mut found_match = false;

        loop {
            if child_index < children.len() {
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
                    child_index += 1;
                }
            } else if stack.len() > 0 {
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

        results
    }

    fn rank_matches(
        &mut self,
        matches: HashMap<fs::EntryId, MatchMarker>,
    ) -> Vec<PathSearchResult> {
        let mut results: BinaryHeap<PathSearchResult> = BinaryHeap::new();
        let mut positions = Vec::with_capacity(self.needle.len());
        let mut scorer = fuzzy::Scorer::new(&self.needle);

        let stack = &mut self.stack;
        let mut children = self.roots.clone();
        let mut child_index = 0;
        let mut found_match = false;

        loop {
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
                        let score = scorer.push(children[child_index].name_chars(), Some(&mut positions));
                        scorer.pop();
                        if results.len() < self.max_results
                            || score > results.peek().map(|r| r.score).unwrap()
                        {
                            let mut path = PathBuf::new();
                            for entry in stack.iter() {
                                path.push(entry.children[entry.child_index].name());
                            }

                            results.pop();
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

        let mut sorted_results = results.into_sorted_vec();
        let sorted_results_len = sorted_results.len();
        for i in 0..(sorted_results_len / 2) {
            sorted_results.swap(i, sorted_results_len - i - 1);
        }
        sorted_results
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
