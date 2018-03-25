use serde_json;
use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;
use window::{View, ViewHandle, ViewUpdateStream, WindowHandle};
use buffer::Buffer;
use buffer_view::BufferView;
use futures::{future, Stream};
use notify_cell::NotifyCell;
use fs;

pub struct WorkspaceView {
    roots: Rc<Vec<Box<fs::Tree>>>,
    window_handle: Option<WindowHandle>,
    modal_panel: Option<ViewHandle>,
    center_pane: Option<ViewHandle>,
    updates: NotifyCell<()>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WorkspaceViewAction {
    ToggleFileFinder,
}

struct FileFinderView {
    roots: Rc<Vec<Box<fs::Tree>>>,
    query: String,
    search_handle: Option<fs::SearchHandle>,
    window_handle: Option<WindowHandle>,
    updates: Arc<NotifyCell<Vec<fs::SearchResult>>>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum FileFinderAction {
    UpdateQuery { query: String },
}

impl WorkspaceView {
    pub fn new(roots: Vec<Box<fs::Tree>>) -> Self {
        WorkspaceView {
            roots: Rc::new(roots),
            modal_panel: None,
            center_pane: None,
            window_handle: None,
            updates: NotifyCell::new(()),
        }
    }

    fn toggle_file_finder(&mut self) {
        let ref mut window_handle = self.window_handle.as_mut().unwrap();
        if self.modal_panel.is_some() {
            self.modal_panel = None;
        } else {
            self.modal_panel = Some(window_handle.add_view(FileFinderView::new(self.roots.clone())));
        }
        self.updates.set(());
    }

    fn build_example_buffer_view(&self) -> BufferView {
        let src_path: PathBuf = env::var("XRAY_SRC_PATH")
            .expect("Missing XRAY_SRC_PATH environment variable")
            .into();

        let react_js_path =
            src_path.join("xray_electron/node_modules/react/cjs/react.development.js");
        let file = File::open(react_js_path).unwrap();
        let mut buf_reader = BufReader::new(file);
        let mut contents = String::new();
        buf_reader.read_to_string(&mut contents).unwrap();

        let mut buffer = Buffer::new(1);
        buffer.splice(0..0, contents.as_str());

        let mut buffer_view = BufferView::new(Rc::new(RefCell::new(buffer)));
        buffer_view.set_line_height(20.0);
        buffer_view
    }
}

impl View for WorkspaceView {
    fn component_name(&self) -> &'static str {
        "Workspace"
    }

    fn render(&self) -> serde_json::Value {
        json!({
            "modal": self.modal_panel.as_ref().map(|view_handle| view_handle.view_id),
            "center_pane": self.center_pane.as_ref().map(|view_handle| view_handle.view_id)
        })
    }

    fn updates(&self) -> ViewUpdateStream {
        Box::new(self.updates.observe())
    }

    fn will_mount(&mut self, window_handle: WindowHandle) {
        self.center_pane = Some(window_handle.add_view(self.build_example_buffer_view()));
        self.window_handle = Some(window_handle);
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {
        match serde_json::from_value(action) {
            Ok(WorkspaceViewAction::ToggleFileFinder) => self.toggle_file_finder(),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl View for FileFinderView {
    fn component_name(&self) -> &'static str {
        "FileFinder"
    }

    fn render(&self) -> serde_json::Value {
        json!({
            "query": self.query.as_str(),
            "results": self.updates.get(),
        })
    }

    fn will_mount(&mut self, window_handle: WindowHandle) {
        self.window_handle = Some(window_handle);
    }

    fn updates(&self) -> ViewUpdateStream {
        Box::new(self.updates.observe().map(|_| ()))
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {
        match serde_json::from_value(action) {
            Ok(FileFinderAction::UpdateQuery { query }) => self.update_query(query),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl FileFinderView {
    fn new(roots: Rc<Vec<Box<fs::Tree>>>) -> Self {
        Self {
            roots: roots,
            query: String::new(),
            updates: Arc::new(NotifyCell::new(Vec::new())),
            search_handle: None,
            window_handle: None,
        }
    }

    fn update_query(&mut self, query: String) {
        if self.query != query {
            self.query = query;
            self.updates.set(Vec::new());
            if let Ok((mut search, search_handle)) = self.roots[0].root().search(&self.query, 10) {
                self.search_handle = Some(search_handle);
                search.set_entry_count_per_poll(1000);
                let mut updates = self.updates.clone();
                self.window_handle.as_ref().unwrap().spawn(search.for_each(move |results| {
                    updates.set(results);
                    Ok(())
                }));
            }
        }
    }
}
