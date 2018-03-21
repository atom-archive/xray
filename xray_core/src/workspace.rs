use serde_json;
use std::cell::{RefCell, Ref, RefMut};
use std::env;
use std::path::PathBuf;
use std::rc::Rc;
use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;
use window::{View, ViewUpdateStream, WindowHandle, ViewHandle};
use buffer::Buffer;
use buffer_view::{BufferView, Measurements};
use notify_cell::NotifyCell;

pub struct Workspace {
    paths: Vec<PathBuf>
}

#[derive(Clone)]
pub struct WorkspaceHandle(Rc<RefCell<Workspace>>);

pub struct WorkspaceView {
    workspace: WorkspaceHandle,
    window_handle: Option<WindowHandle>,
    modal_panel: Option<ViewHandle>,
    center_pane: Option<ViewHandle>,
    updates: NotifyCell<()>
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WorkspaceViewAction {
    ToggleFileFinder,
}

struct FileFinderView {
    query: String,
    updates: NotifyCell<()>
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum FileFinderAction {
    UpdateQuery { query: String }
}

impl Workspace {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl WorkspaceHandle {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        WorkspaceHandle(Rc::new(RefCell::new(Workspace::new(paths))))
    }

    pub fn borrow(&self) -> Ref<Workspace> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<Workspace> {
        self.0.borrow_mut()
    }
}

impl WorkspaceView {
    pub fn new(workspace: WorkspaceHandle) -> Self {
        WorkspaceView {
            workspace,
            modal_panel: None,
            center_pane: None,
            window_handle: None,
            updates: NotifyCell::new(())
        }
    }

    fn toggle_file_finder(&mut self) {
        let ref mut window_handle = self.window_handle.as_mut().unwrap();
        if self.modal_panel.is_some() {
            self.modal_panel = None;
        } else {
            self.modal_panel = Some(window_handle.add_view(FileFinderView::new()));
        }
        self.updates.set(());
    }

    fn build_example_buffer_view(&self) -> BufferView {
        let src_path : PathBuf = env::var("XRAY_SRC_PATH")
            .expect("Missing XRAY_SRC_PATH environment variable")
            .into();

        let react_js_path = src_path.join("xray_electron/node_modules/react/cjs/react.development.js");
        let file = File::open(react_js_path).unwrap();
        let mut buf_reader = BufReader::new(file);
        let mut contents = String::new();
        buf_reader.read_to_string(&mut contents).unwrap();

        let mut buffer = Buffer::new(1);
        buffer.splice(0..0, contents.as_str());

        BufferView::new(Rc::new(RefCell::new(buffer)), Measurements{
            height: 100.0,
            line_height: 12.0,
            scroll_top: 0.0,
        })
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

    fn did_mount(&mut self, window_handle: WindowHandle) {
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
    fn component_name(&self) -> &'static str { "FileFinder" }

    fn render(&self) -> serde_json::Value {
        json!({
            "query": self.query.as_str()
        })
    }

    fn updates(&self) -> ViewUpdateStream {
        Box::new(self.updates.observe())
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {
        match serde_json::from_value(action) {
            Ok(FileFinderAction::UpdateQuery { query }) => self.update_query(query),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl FileFinderView {
    fn new() -> Self {
        Self {
            query: String::new(),
            updates: NotifyCell::new(())
        }
    }

    fn update_query(&mut self, query: String) {
        if self.query != query {
            self.query = query;
            self.updates.set(());
        }
    }
}
