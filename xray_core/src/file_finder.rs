use cross_platform;
use futures::{Async, Poll, Stream};
use notify_cell::{NotifyCell, NotifyCellObserver};
use project::{PathSearch, PathSearchResult, PathSearchStatus, TreeId};
use serde_json;
use window::{View, WeakViewHandle, Window};

pub trait FileFinderViewDelegate {
    fn search_paths(
        &self,
        needle: &str,
        max_results: usize,
        include_ignored: bool,
    ) -> (PathSearch, NotifyCellObserver<PathSearchStatus>);
    fn did_close(&mut self);
    fn did_confirm(
        &mut self,
        tree_id: TreeId,
        relative_path: &cross_platform::Path,
        window: &mut Window,
    );
}

pub struct FileFinderView<T: FileFinderViewDelegate> {
    delegate: WeakViewHandle<T>,
    query: String,
    include_ignored: bool,
    selected_index: usize,
    search_results: Vec<PathSearchResult>,
    search_updates: Option<NotifyCellObserver<PathSearchStatus>>,
    updates: NotifyCell<()>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum FileFinderAction {
    UpdateQuery { query: String },
    UpdateIncludeIgnored { include_ignored: bool },
    SelectPrevious,
    SelectNext,
    Confirm,
    Close,
}

impl<T: FileFinderViewDelegate> View for FileFinderView<T> {
    fn component_name(&self) -> &'static str {
        "FileFinder"
    }

    fn render(&self) -> serde_json::Value {
        json!({
            "selected_index": self.selected_index,
            "query": self.query.as_str(),
            "results": self.search_results,
        })
    }

    fn dispatch_action(&mut self, action: serde_json::Value, window: &mut Window) {
        match serde_json::from_value(action) {
            Ok(FileFinderAction::UpdateQuery { query }) => self.update_query(query, window),
            Ok(FileFinderAction::UpdateIncludeIgnored { include_ignored }) => {
                self.update_include_ignored(include_ignored, window)
            }
            Ok(FileFinderAction::SelectPrevious) => self.select_previous(),
            Ok(FileFinderAction::SelectNext) => self.select_next(),
            Ok(FileFinderAction::Confirm) => self.confirm(window),
            Ok(FileFinderAction::Close) => self.close(),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl<T: FileFinderViewDelegate> Stream for FileFinderView<T> {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let search_poll = self.search_updates
            .as_mut()
            .map(|s| s.poll())
            .unwrap_or(Ok(Async::NotReady))?;
        let updates_poll = self.updates.poll()?;
        match (search_poll, updates_poll) {
            (Async::NotReady, Async::NotReady) => Ok(Async::NotReady),
            (Async::Ready(Some(search_status)), _) => {
                match search_status {
                    PathSearchStatus::Pending => {}
                    PathSearchStatus::Ready(results) => {
                        self.search_results = results;
                    }
                }

                Ok(Async::Ready(Some(())))
            }
            _ => Ok(Async::Ready(Some(()))),
        }
    }
}

impl<T: FileFinderViewDelegate> FileFinderView<T> {
    pub fn new(delegate: WeakViewHandle<T>) -> Self {
        Self {
            delegate,
            query: String::new(),
            include_ignored: false,
            selected_index: 0,
            search_results: Vec::new(),
            search_updates: None,
            updates: NotifyCell::new(()),
        }
    }

    fn update_query(&mut self, query: String, window: &mut Window) {
        if self.query != query {
            self.query = query;
            self.search(window);
            self.updates.set(());
        }
    }

    fn update_include_ignored(&mut self, include_ignored: bool, window: &mut Window) {
        if self.include_ignored != include_ignored {
            self.include_ignored = include_ignored;
            self.search(window);
            self.updates.set(());
        }
    }

    fn select_previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.updates.set(());
        }
    }

    fn select_next(&mut self) {
        if self.selected_index + 1 < self.search_results.len() {
            self.selected_index += 1;
            self.updates.set(());
        }
    }

    fn confirm(&mut self, window: &mut Window) {
        if let Some(search_result) = self.search_results.get(self.selected_index) {
            self.delegate.map(|delegate| {
                delegate.did_confirm(search_result.tree_id, &search_result.relative_path, window)
            });
        }
    }

    fn close(&mut self) {
        self.delegate.map(|delegate| delegate.did_close());
    }

    fn search(&mut self, window: &mut Window) {
        let search = self.delegate
            .map(|delegate| delegate.search_paths(&self.query, 10, self.include_ignored));

        if let Some((search, search_updates)) = search {
            self.search_updates = Some(search_updates);
            window.spawn(search);
            self.updates.set(());
        }
    }
}
