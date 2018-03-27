use futures::{Poll, Stream};
use serde_json;
use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;
use window::{View, ViewHandle, WindowHandle};
use buffer::Buffer;
use buffer_view::BufferView;
use notify_cell::NotifyCell;
use fs;
use file_finder::{FileFinderView, FileFinderViewDelegate};

pub struct WorkspaceView(Rc<RefCell<WorkspaceViewState>>);

struct WorkspaceViewState {
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

impl WorkspaceView {
    pub fn new(roots: Vec<Box<fs::Tree>>) -> Self {
        WorkspaceView(Rc::new(RefCell::new(WorkspaceViewState {
            roots: Rc::new(roots),
            modal_panel: None,
            center_pane: None,
            window_handle: None,
            updates: NotifyCell::new(()),
        })))
    }
}

impl View for WorkspaceView {
    fn component_name(&self) -> &'static str {
        "Workspace"
    }

    fn render(&self) -> serde_json::Value {
        let state = self.0.borrow();
        json!({
            "modal": state.modal_panel.as_ref().map(|view_handle| view_handle.view_id),
            "center_pane": state.center_pane.as_ref().map(|view_handle| view_handle.view_id)
        })
    }

    fn will_mount(&mut self, window_handle: WindowHandle) {
        let mut state = self.0.borrow_mut();

        let src_path: PathBuf = env::var("XRAY_SRC_PATH")
            .expect("Missing XRAY_SRC_PATH environment variable")
            .into();

        let react_js_path =
            src_path.join("xray_electron/node_modules/react/cjs/react.development.js");

        state.center_pane = Some(window_handle.add_view(state.open_path(react_js_path)));
        state.window_handle = Some(window_handle);
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {
        let delegate = Rc::downgrade(&self.0);
        let mut state = self.0.borrow_mut();
        match serde_json::from_value(action) {
            Ok(WorkspaceViewAction::ToggleFileFinder) => state.toggle_file_finder(delegate),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl WorkspaceViewState {
    fn toggle_file_finder(&mut self, delegate: Weak<RefCell<WorkspaceViewState>>) {
        let ref mut window_handle = self.window_handle.as_mut().unwrap();
        if self.modal_panel.is_some() {
            self.modal_panel = None;
        } else {
            self.modal_panel = Some(window_handle.add_view(FileFinderView::new(delegate)));
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

impl FileFinderViewDelegate for WorkspaceViewState {
    fn trees(&self) -> &Vec<Box<fs::Tree>> {
        &self.roots
    }

    fn did_close(&mut self) {
        self.modal_panel = None;
        self.updates.set(());
    }

    fn did_confirm(&mut self, path: PathBuf) {
        self.center_pane = Some(self.window_handle.as_ref().unwrap().add_view(self.open_path(path)));
        self.modal_panel = None;
        self.updates.set(());
    }
}

impl Stream for WorkspaceView {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut state = self.0.borrow_mut();
        state.updates.poll()
    }
}
