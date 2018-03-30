use futures::{Async, Poll, Stream};
use std::path::PathBuf;
use project::{PathSearch, PathSearchStatus, PathSearchResult};
use window::{View, WeakViewHandle, WindowHandle};
use notify_cell::{NotifyCell, NotifyCellObserver};
use serde_json;

pub trait FileFinderViewDelegate {
    fn search_paths(&self, needle: &str, max_results: usize, include_ignored: bool) -> (PathSearch, NotifyCellObserver<PathSearchStatus>);
    fn did_close(&mut self);
    fn did_confirm(&mut self, path: PathBuf);
}

pub struct FileFinderView<T: FileFinderViewDelegate> {
    delegate: WeakViewHandle<T>,
    query: String,
    include_ignored: bool,
    selected_index: usize,
    search_results: Vec<PathSearchResult>,
    search_updates: Option<NotifyCellObserver<PathSearchStatus>>,
    window_handle: Option<WindowHandle>,
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

    fn will_mount(&mut self, window_handle: WindowHandle, _: WeakViewHandle<Self>) {
        self.window_handle = Some(window_handle);
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {
        match serde_json::from_value(action) {
            Ok(FileFinderAction::UpdateQuery { query }) => self.update_query(query),
            Ok(FileFinderAction::UpdateIncludeIgnored { include_ignored }) => self.update_include_ignored(include_ignored),
            Ok(FileFinderAction::SelectPrevious) => self.select_previous(),
            Ok(FileFinderAction::SelectNext) => self.select_next(),
            Ok(FileFinderAction::Confirm) => self.confirm(),
            Ok(FileFinderAction::Close) => self.close(),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl<T: FileFinderViewDelegate> Stream for FileFinderView<T> {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let search_poll = self.search_updates.as_mut().map(|s| s.poll()).unwrap_or(Ok(Async::NotReady))?;
        let updates_poll = self.updates.poll()?;
        match (search_poll, updates_poll) {
            (Async::NotReady, Async::NotReady) => Ok(Async::NotReady),
            (Async::Ready(Some(search_status)), _) => {
                match search_status {
                    PathSearchStatus::Pending => {},
                    PathSearchStatus::Ready(results) => {
                        self.search_results = results;
                    },
                }

                Ok(Async::Ready(Some(())))
            },
            _ => Ok(Async::Ready(Some(())))
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
            window_handle: None,
        }
    }

    fn update_query(&mut self, query: String) {
        if self.query != query {
            self.query = query;
            self.search();
            self.updates.set(());
        }
    }

    fn update_include_ignored(&mut self, include_ignored: bool) {
        if self.include_ignored != include_ignored {
            self.include_ignored = include_ignored;
            self.search();
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

    fn confirm(&mut self) {
        if let Some(search_result) = self.search_results.get(self.selected_index) {
            self.delegate.map(|delegate|
                delegate.did_confirm(search_result.absolute_path.clone())
            );
        }
    }

    fn close(&mut self) {
        self.delegate.map(|delegate| delegate.did_close());
    }

    fn search(&mut self) {
        let search = self.delegate.map(|delegate|
            delegate.search_paths(&self.query, 10, self.include_ignored)
        );

        if let Some((search, search_updates)) = search {
            self.search_updates = Some(search_updates);
            self.window_handle.as_ref().unwrap().spawn(search);
            self.updates.set(());
        }
    }
}
