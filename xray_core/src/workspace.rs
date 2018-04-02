use project::{Project, PathSearch, PathSearchStatus};
use notify_cell::NotifyCellObserver;
use futures::{Poll, Stream};
use serde_json;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;
use window::{View, ViewHandle, WeakViewHandle, Window};
use buffer::Buffer;
use buffer_view::BufferView;
use notify_cell::NotifyCell;
use fs;
use file_finder::{FileFinderView, FileFinderViewDelegate};

pub struct WorkspaceView {
    project: Project,
    modal_panel: Option<ViewHandle>,
    center_pane: Option<ViewHandle>,
    updates: NotifyCell<()>,
    self_handle: Option<WeakViewHandle<WorkspaceView>>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WorkspaceViewAction {
    ToggleFileFinder,
}

impl WorkspaceView {
    pub fn new(roots: Vec<Box<fs::Tree>>) -> Self {
        WorkspaceView {
            project: Project::new(roots),
            modal_panel: None,
            center_pane: None,
            updates: NotifyCell::new(()),
            self_handle: None,
        }
    }

    fn toggle_file_finder(&mut self, window: &mut Window) {
        if self.modal_panel.is_some() {
            self.modal_panel = None;
        } else {
            let delegate = self.self_handle.as_ref().cloned().unwrap();
            let view = window.add_view(FileFinderView::new(delegate));
            view.focus().unwrap();
            self.modal_panel = Some(view);
        }
        self.updates.set(());
    }

    fn open_path(&self, path: PathBuf) -> BufferView {
        let file = File::open(path).unwrap();
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

    fn will_mount(&mut self, _window: &mut Window, view_handle: WeakViewHandle<Self>) {
        self.self_handle = Some(view_handle);
    }

    fn dispatch_action(&mut self, action: serde_json::Value, window: &mut Window) {
        match serde_json::from_value(action) {
            Ok(WorkspaceViewAction::ToggleFileFinder) => self.toggle_file_finder(window),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl FileFinderViewDelegate for WorkspaceView {
    fn search_paths(&self, needle: &str, max_results: usize, include_ignored: bool) -> (PathSearch, NotifyCellObserver<PathSearchStatus>) {
        self.project.search_paths(needle, max_results, include_ignored)
    }

    fn did_close(&mut self) {
        self.modal_panel = None;
        self.updates.set(());
    }

    fn did_confirm(&mut self, path: PathBuf, window: &mut Window) {
        let buffer_view = window.add_view(self.open_path(path));
        buffer_view.focus().unwrap();
        self.center_pane = Some(buffer_view);
        self.modal_panel = None;
        self.updates.set(());
    }
}

impl Stream for WorkspaceView {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.updates.poll()
    }
}
