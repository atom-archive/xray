use buffer::{self, Buffer, BufferId};
use cross_platform;
use fs;
use futures::{future, prelude::*};
use fuzzy;
use notify_cell::{NotifyCell, NotifyCellObserver, WeakNotifyCell};
use rpc;
use std::cell::{Cell, RefCell};
use std::cmp;
use std::collections::{BinaryHeap, HashMap};
use std::error::Error;
use std::io;
use std::rc::Rc;
use std::sync::Arc;
use Executor;
use IntoShared;

pub type TreeId = usize;

pub trait Project {
    fn open_path(
        &self,
        tree_id: TreeId,
        relative_path: &cross_platform::Path,
    ) -> Box<Future<Item = Rc<RefCell<Buffer>>, Error = OpenError>>;
    fn open_buffer(
        &self,
        buffer_id: BufferId,
    ) -> Box<Future<Item = Rc<RefCell<Buffer>>, Error = OpenError>>;
    fn search_paths(
        &self,
        needle: &str,
        max_results: usize,
        include_ignored: bool,
    ) -> (PathSearch, NotifyCellObserver<PathSearchStatus>);
}

pub struct LocalProject {
    file_provider: Rc<fs::FileProvider>,
    next_tree_id: TreeId,
    next_buffer_id: Rc<Cell<BufferId>>,
    trees: HashMap<TreeId, Rc<fs::LocalTree>>,
    buffers: Rc<RefCell<HashMap<BufferId, Rc<RefCell<Buffer>>>>>,
    buffers_by_file: Rc<RefCell<HashMap<fs::FileId, Rc<RefCell<Buffer>>>>>,
}

pub struct RemoteProject {
    executor: Rc<Executor>,
    service: Rc<RefCell<rpc::client::Service<ProjectService>>>,
    trees: HashMap<TreeId, Box<fs::Tree>>,
}

pub struct ProjectService {
    project: Rc<RefCell<LocalProject>>,
    tree_services: HashMap<TreeId, rpc::server::ServiceHandle>,
}

#[derive(Deserialize, Serialize)]
pub struct RpcState {
    trees: HashMap<TreeId, rpc::ServiceId>,
}

#[derive(Deserialize, Serialize)]
pub enum RpcRequest {
    OpenPath {
        tree_id: TreeId,
        relative_path: cross_platform::Path,
    },
    OpenBuffer {
        buffer_id: BufferId,
    },
}

#[derive(Deserialize, Serialize)]
pub enum RpcResponse {
    OpenedBuffer(Result<rpc::ServiceId, OpenError>),
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
    pub relative_path: cross_platform::Path,
    pub display_path: String,
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

#[derive(Debug, Serialize, Deserialize)]
pub enum OpenError {
    BufferNotFound,
    TreeNotFound,
    IoError(String),
    RpcError(rpc::Error),
}

impl LocalProject {
    pub fn new<T>(file_provider: Rc<fs::FileProvider>, trees: Vec<T>) -> Self
    where
        T: 'static + fs::LocalTree,
    {
        let mut project = LocalProject {
            file_provider,
            next_tree_id: 0,
            next_buffer_id: Rc::new(Cell::new(0)),
            trees: HashMap::new(),
            buffers: Rc::new(RefCell::new(HashMap::new())),
            buffers_by_file: Rc::new(RefCell::new(HashMap::new())),
        };
        for tree in trees {
            project.add_tree(tree);
        }
        project
    }

    fn add_tree<T: 'static + fs::LocalTree>(&mut self, tree: T) {
        let id = self.next_tree_id;
        self.next_tree_id += 1;
        self.trees.insert(id, Rc::new(tree));
    }

    fn resolve_path(
        &self,
        tree_id: TreeId,
        relative_path: &cross_platform::Path,
    ) -> Option<cross_platform::Path> {
        self.trees.get(&tree_id).map(|tree| {
            let mut absolute_path = tree.path().clone();
            absolute_path.push_path(relative_path);
            absolute_path
        })
    }
}

impl Project for LocalProject {
    fn open_path(
        &self,
        tree_id: TreeId,
        relative_path: &cross_platform::Path,
    ) -> Box<Future<Item = Rc<RefCell<Buffer>>, Error = OpenError>> {
        if let Some(absolute_path) = self.resolve_path(tree_id, relative_path) {
            let next_buffer_id_cell = self.next_buffer_id.clone();
            let buffers_by_file = self.buffers_by_file.clone();
            let buffers = self.buffers.clone();
            Box::new(
                self.file_provider
                    .open(&absolute_path)
                    .and_then(move |file| {
                        let buffer = buffers_by_file.borrow().get(&file.id()).cloned();
                        if let Some(buffer) = buffer {
                            Box::new(future::ok(buffer))
                                as Box<Future<Item = Rc<RefCell<Buffer>>, Error = io::Error>>
                        } else {
                            Box::new(file.read().and_then(move |content| {
                                let mut buffers_by_file = buffers_by_file.borrow_mut();
                                Ok(buffers_by_file
                                    .entry(file.id())
                                    .or_insert_with(|| {
                                        let buffer_id = next_buffer_id_cell.get();
                                        next_buffer_id_cell.set(next_buffer_id_cell.get() + 1);
                                        let mut buffer = Buffer::new(buffer_id);
                                        buffer.edit(0..0, content.as_str());
                                        let buffer = buffer.into_shared();
                                        buffers.borrow_mut().insert(buffer_id, buffer.clone());
                                        buffer
                                    })
                                    .clone())
                            }))
                        }
                    })
                    .map_err(|error| error.into()),
            )
        } else {
            Box::new(future::err(OpenError::TreeNotFound))
        }
    }

    fn open_buffer(
        &self,
        buffer_id: BufferId,
    ) -> Box<Future<Item = Rc<RefCell<Buffer>>, Error = OpenError>> {
        use futures::prelude::*;
        Box::new(
            self.buffers
                .borrow()
                .get(&buffer_id)
                .cloned()
                .ok_or(OpenError::BufferNotFound)
                .into_future(),
        )
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

impl RemoteProject {
    pub fn new(
        executor: Rc<Executor>,
        service: rpc::client::Service<ProjectService>,
    ) -> Result<Self, rpc::Error> {
        let state = service.state()?;
        let mut trees = HashMap::new();
        for (tree_id, service_id) in &state.trees {
            let tree_service = service
                .take_service(*service_id)
                .expect("The server should create services for each tree in our project state.");
            let remote_tree = fs::RemoteTree::new(executor.clone(), tree_service);
            trees.insert(*tree_id, Box::new(remote_tree) as Box<fs::Tree>);
        }
        Ok(Self {
            executor,
            service: service.into_shared(),
            trees,
        })
    }
}

impl Project for RemoteProject {
    fn open_path(
        &self,
        tree_id: TreeId,
        relative_path: &cross_platform::Path,
    ) -> Box<Future<Item = Rc<RefCell<Buffer>>, Error = OpenError>> {
        let executor = self.executor.clone();
        let service = self.service.clone();

        Box::new(
            self.service
                .borrow()
                .request(RpcRequest::OpenPath {
                    tree_id,
                    relative_path: relative_path.clone(),
                })
                .then(move |response| {
                    response
                        .map_err(|error| error.into())
                        .and_then(|response| match response {
                            RpcResponse::OpenedBuffer(result) => result.and_then(|service_id| {
                                service
                                    .borrow()
                                    .take_service(service_id)
                                    .map_err(|error| error.into())
                                    .and_then(|buffer_service| {
                                        Buffer::remote(executor, buffer_service)
                                            .map_err(|error| error.into())
                                    })
                            }),
                        })
                }),
        )
    }

    fn open_buffer(
        &self,
        buffer_id: BufferId,
    ) -> Box<Future<Item = Rc<RefCell<Buffer>>, Error = OpenError>> {
        let executor = self.executor.clone();
        let service = self.service.clone();
        Box::new(
            self.service
                .borrow()
                .request(RpcRequest::OpenBuffer { buffer_id })
                .then(move |response| {
                    response
                        .map_err(|error| error.into())
                        .and_then(|response| match response {
                            RpcResponse::OpenedBuffer(result) => result.and_then(|service_id| {
                                service
                                    .borrow()
                                    .take_service(service_id)
                                    .map_err(|error| error.into())
                                    .and_then(|buffer_service| {
                                        Buffer::remote(executor, buffer_service)
                                            .map_err(|error| error.into())
                                    })
                            }),
                        })
                }),
        )
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

impl ProjectService {
    pub fn new(project: Rc<RefCell<LocalProject>>) -> Self {
        Self {
            project,
            tree_services: HashMap::new(),
        }
    }
}

impl rpc::server::Service for ProjectService {
    type State = RpcState;
    type Update = RpcState;
    type Request = RpcRequest;
    type Response = RpcResponse;

    fn init(
        &mut self,
        _cx: &mut task::Context,
        connection: &rpc::server::Connection,
    ) -> Self::State {
        let mut state = RpcState {
            trees: HashMap::new(),
        };
        for (tree_id, tree) in &self.project.borrow().trees {
            let handle = connection.add_service(fs::TreeService::new(tree.clone()));
            state.trees.insert(*tree_id, handle.service_id());
            self.tree_services.insert(*tree_id, handle);
        }

        state
    }

    fn poll_update(
        &mut self,
        _cx: &mut task::Context,
        _connection: &rpc::server::Connection,
    ) -> Async<Option<Self::Update>> {
        Async::Pending
    }

    fn request(
        &mut self,
        request: Self::Request,
        connection: &rpc::server::Connection,
    ) -> Option<Box<Future<Item = Self::Response, Error = Never>>> {
        match request {
            RpcRequest::OpenPath {
                tree_id,
                relative_path,
            } => {
                let connection = connection.clone();
                Some(Box::new(
                    self.project
                        .borrow()
                        .open_path(tree_id, &relative_path)
                        .then(move |result| {
                            Ok(RpcResponse::OpenedBuffer(result.map(|buffer| {
                                connection
                                    .add_service(buffer::rpc::Service::new(buffer))
                                    .service_id()
                            })))
                        }),
                ))
            }
            RpcRequest::OpenBuffer { buffer_id } => {
                let connection = connection.clone();
                Some(Box::new(self.project.borrow().open_buffer(buffer_id).then(
                    move |result| {
                        Ok(RpcResponse::OpenedBuffer(result.map(|buffer| {
                            connection
                                .add_service(buffer::rpc::Service::new(buffer))
                                .service_id()
                        })))
                    },
                )))
            }
        }
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

                            let mut relative_path = cross_platform::Path::new();
                            let mut display_path = String::new();
                            for (i, entry) in stack.iter().enumerate() {
                                let child = &entry.children[entry.child_index];
                                if self.roots.len() == 1 || i != 0 {
                                    relative_path.push(child.name());
                                }
                                display_path.extend(child.name_chars());
                            }
                            let child = &children[child_index];
                            relative_path.push(child.name());
                            display_path.extend(child.name_chars());
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

impl Future for PathSearch {
    type Item = ();
    type Error = Never;

    fn poll(&mut self, _cx: &mut task::Context) -> Poll<Self::Item, Self::Error> {
        if self.needle.is_empty() {
            let _ = self.updates.try_set(PathSearchStatus::Ready(Vec::new()));
        } else {
            let _ = self.find_matches()
                .and_then(|matches| self.rank_matches(matches))
                .and_then(|results| {
                    let _ = self.updates.try_set(PathSearchStatus::Ready(results));
                    Ok(())
                });
        }
        Ok(Async::Ready(()))
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

impl From<io::Error> for OpenError {
    fn from(error: io::Error) -> Self {
        OpenError::IoError(error.description().to_owned())
    }
}

impl From<rpc::Error> for OpenError {
    fn from(error: rpc::Error) -> Self {
        OpenError::RpcError(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs::tests::{TestFileProvider, TestTree};
    use tokio_core::reactor;
    use IntoShared;

    #[test]
    fn test_open_same_path_concurrently() {
        let file_provider = Rc::new(TestFileProvider::new());
        let project = build_project(file_provider.clone());

        let tree_id = 0;
        let relative_path = cross_platform::Path::from("subdir-a/subdir-1/bar");
        file_provider.write_sync(
            project.resolve_path(tree_id, &relative_path).unwrap(),
            "abc",
        );

        let buffer_future_1 = project.open_path(tree_id, &relative_path);
        let buffer_future_2 = project.open_path(tree_id, &relative_path);
        let (buffer_1, buffer_2) = buffer_future_1.join(buffer_future_2).wait().unwrap();
        assert!(Rc::ptr_eq(&buffer_1, &buffer_2));
    }

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
        let project = LocalProject::new(Rc::new(TestFileProvider::new()), vec![tree]);
        let (mut search, observer) = project.search_paths("sub2", 10, true);

        assert_eq!(search.poll(), Ok(Async::Ready(())));
        assert_eq!(
            summarize_results(&observer.get()),
            Some(vec![
                (
                    0,
                    "root-2/subdir-2/file-3".to_string(),
                    "root-2/subdir-2/file-3".to_string(),
                    vec![7, 8, 9, 14],
                ),
                (
                    0,
                    "root-2/subdir-2/file-4".to_string(),
                    "root-2/subdir-2/file-4".to_string(),
                    vec![7, 8, 9, 14],
                ),
                (
                    0,
                    "root-1/subdir-1/file-2".to_string(),
                    "root-1/subdir-1/file-2".to_string(),
                    vec![7, 8, 9, 21],
                ),
            ])
        );
    }

    #[test]
    fn test_search_many_trees() {
        let project = build_project(Rc::new(TestFileProvider::new()));

        let (mut search, observer) = project.search_paths("bar", 10, true);
        assert_eq!(search.poll(), Ok(Async::Ready(())));
        assert_eq!(
            summarize_results(&observer.get()),
            Some(vec![
                (
                    1,
                    "subdir-b/subdir-2/foo".to_string(),
                    "bar/subdir-b/subdir-2/foo".to_string(),
                    vec![0, 1, 2],
                ),
                (
                    0,
                    "subdir-a/subdir-1/bar".to_string(),
                    "foo/subdir-a/subdir-1/bar".to_string(),
                    vec![22, 23, 24],
                ),
                (
                    1,
                    "subdir-b/subdir-2/file-3".to_string(),
                    "bar/subdir-b/subdir-2/file-3".to_string(),
                    vec![0, 1, 2],
                ),
                (
                    0,
                    "subdir-a/subdir-1/file-1".to_string(),
                    "foo/subdir-a/subdir-1/file-1".to_string(),
                    vec![6, 11, 18],
                ),
            ])
        );
    }

    #[test]
    fn test_replication() {
        let mut reactor = reactor::Core::new().unwrap();
        let handle = Rc::new(reactor.handle());
        let file_provider = Rc::new(TestFileProvider::new());

        let local_project = build_project(file_provider.clone()).into_shared();
        let remote_project = RemoteProject::new(
            handle,
            rpc::tests::connect(&mut reactor, ProjectService::new(local_project.clone())),
        ).unwrap();

        let (mut local_search, local_observer) =
            local_project.borrow().search_paths("bar", 10, true);
        let (mut remote_search, remote_observer) = remote_project.search_paths("bar", 10, true);
        assert_eq!(local_search.poll(), Ok(Async::Ready(())));
        assert_eq!(remote_search.poll(), Ok(Async::Ready(())));
        assert_eq!(
            summarize_results(&remote_observer.get()),
            summarize_results(&local_observer.get())
        );

        let PathSearchResult {
            tree_id,
            ref relative_path,
            ..
        } = remote_observer.get().unwrap()[0];

        let absolute_path = local_project
            .borrow()
            .resolve_path(tree_id, relative_path)
            .unwrap();
        file_provider.write_sync(absolute_path, "abc");

        let remote_buffer = reactor
            .run(remote_project.open_path(tree_id, &relative_path))
            .unwrap();
        let local_buffer = reactor
            .run(
                local_project
                    .borrow_mut()
                    .open_path(tree_id, &relative_path),
            )
            .unwrap();

        assert_eq!(
            remote_buffer.borrow().to_string(),
            local_buffer.borrow().to_string()
        );
    }

    fn build_project(file_provider: Rc<TestFileProvider>) -> LocalProject {
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
        tree_1.populated.set(true);

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
        tree_2.populated.set(true);

        LocalProject::new(file_provider, vec![tree_1, tree_2])
    }

    fn summarize_results(
        results: &PathSearchStatus,
    ) -> Option<Vec<(TreeId, String, String, Vec<usize>)>> {
        match results {
            &PathSearchStatus::Pending => None,
            &PathSearchStatus::Ready(ref results) => {
                let summary = results
                    .iter()
                    .map(|result| {
                        let tree_id = result.tree_id;
                        let relative_path = result.relative_path.to_string_lossy();
                        let display_path = result.display_path.clone();
                        let positions = result.positions.clone();
                        (tree_id, relative_path, display_path, positions)
                    })
                    .collect();
                Some(summary)
            }
        }
    }

    impl PathSearchStatus {
        fn unwrap(self) -> Vec<PathSearchResult> {
            match self {
                PathSearchStatus::Ready(results) => results,
                _ => panic!(),
            }
        }
    }
}
