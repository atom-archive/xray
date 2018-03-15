use serde_json;
use std::cell::{RefCell, Ref, RefMut};
use std::path::PathBuf;
use std::rc::Rc;
use window::View;

#[derive(Clone)]
pub struct WorkspaceHandle(Rc<RefCell<Workspace>>);

pub struct Workspace {
    paths: Vec<PathBuf>
}

pub struct WorkspaceView {
    workspace: WorkspaceHandle
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

impl Workspace {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl WorkspaceView {
    pub fn new(workspace: WorkspaceHandle) -> Self {
        Self { workspace }
    }
}

impl View for WorkspaceView {
    fn component_name(&self) -> &'static str {
        "Workspace"
    }

    fn render(&self) -> serde_json::Value {
        json!({})
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {

    }
}

// #[derive(Deserialize)]
// #[serde(tag = "type")]
// enum Action {
//     ToggleFileFinder,
// }
//
// struct FileFinderView {
// }
//
// impl View for FileFinderView {
//     fn type_name(&self) -> &str { "FileFinderView" }
//     fn id(&self) -> usize { 0 }
//     fn render(&self) -> serde_json::Value { json!({}) }
//     fn handle_action(&self, action: serde_json::Value) {}
// }
//
//
//
//
// impl WorkspaceView {
//     pub fn new() -> Self {
//         WorkspaceView{ modal_panel: None }
//     }
//
//     pub fn handle_action(&mut self, view_type: String, view_id: usize, action: serde_json::Value) {
//         match serde_json::from_value(action) {
//             Ok(Action::ToggleFileFinder) => self.toggle_file_finder(),
//             _ => eprintln!("Unrecognized action"),
//         }
//     }
//
//     fn toggle_file_finder(&mut self) {
//         if self.modal_panel.is_some() {
//             self.modal_panel = None;
//         } else {
//             self.modal_panel = Some(Box::new(FileFinderView{}));
//         }
//     }
// }
