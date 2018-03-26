use futures::{Async, Poll, Stream};
use std::rc::Rc;
use fuzzy_search::SearchResult;
use fs;
use window::{View, WindowHandle};
use notify_cell::{NotifyCell, NotifyCellObserver};
use serde_json;

pub struct FileFinderView {
    roots: Rc<Vec<Box<fs::Tree>>>,
    query: String,
    search_results: Vec<SearchResult>,
    search_updates: Option<NotifyCellObserver<Vec<SearchResult>>>,
    window_handle: Option<WindowHandle>,
    updates: NotifyCell<()>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum FileFinderAction {
    UpdateQuery { query: String },
}

impl View for FileFinderView {
    fn component_name(&self) -> &'static str {
        "FileFinder"
    }

    fn render(&self) -> serde_json::Value {
        json!({
            "query": self.query.as_str(),
            "results": self.search_results,
        })
    }

    fn will_mount(&mut self, window_handle: WindowHandle) {
        self.window_handle = Some(window_handle);
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {
        match serde_json::from_value(action) {
            Ok(FileFinderAction::UpdateQuery { query }) => self.update_query(query),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl Stream for FileFinderView {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let search_poll = self.search_updates.as_mut().map(|s| s.poll()).unwrap_or(Ok(Async::NotReady))?;
        let updates_poll = self.updates.poll()?;
        match (search_poll, updates_poll) {
            (Async::NotReady, Async::NotReady) => Ok(Async::NotReady),
            (Async::Ready(Some(search_results)), _) => {
                self.search_results = search_results;
                Ok(Async::Ready(Some(())))
            },
            _ => Ok(Async::Ready(Some(())))
        }
    }
}

impl FileFinderView {
    pub fn new(roots: Rc<Vec<Box<fs::Tree>>>) -> Self {
        Self {
            roots: roots,
            query: String::new(),
            search_results: Vec::new(),
            search_updates: None,
            updates: NotifyCell::new(()),
            window_handle: None,
        }
    }

    fn update_query(&mut self, query: String) {
        if self.query != query {
            self.query = query;
            if let Ok((search, search_updates)) = self.roots[0].root().search(&self.query, 10) {
                self.search_updates = Some(search_updates);
                self.window_handle.as_ref().unwrap().spawn(search.for_each(|_| Ok(())));
            }
            self.updates.set(());
        }
    }
}
