use xray_core::notify_cell::NotifyCell;
use serde_json;
use std::cell::{RefCell, Ref, RefMut};
use std::path::PathBuf;
use std::rc::Rc;
use window::{View, ViewUpdateStream, WindowHandle, ViewHandle};

#[derive(Clone)]
pub struct WorkspaceHandle(Rc<RefCell<Workspace>>);

pub struct Workspace {
    paths: Vec<PathBuf>
}

pub struct WorkspaceView {
    workspace: WorkspaceHandle,
    window_handle: Option<WindowHandle>,
    modal_panel: Option<ViewHandle>,
    updates: NotifyCell<()>
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Action {
    ToggleFileFinder,
}

struct FileFinderView {
    updates: NotifyCell<()>
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

impl View for WorkspaceView {
    fn component_name(&self) -> &'static str {
        "Workspace"
    }

    fn render(&self) -> serde_json::Value {
        let ref window_handle = self.window_handle.as_ref().unwrap();

        json!({
            "modal": self.modal_panel.as_ref().map(|view_handle| view_handle.view_id)
        })
    }

    fn updates(&self) -> ViewUpdateStream {
        Box::new(self.updates.observe())
    }

    fn set_window_handle(&mut self, window_handle: WindowHandle) {
        self.window_handle = Some(window_handle);
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {
        match serde_json::from_value(action) {
            Ok(Action::ToggleFileFinder) => self.toggle_file_finder(),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl WorkspaceView {
    pub fn new(workspace: WorkspaceHandle) -> Self {
        WorkspaceView {
            workspace,
            modal_panel: None,
            window_handle: None,
            updates: NotifyCell::new(())
        }
    }

    fn toggle_file_finder(&mut self) {
        let ref mut window_handle = self.window_handle.as_mut().unwrap();
        if self.modal_panel.is_some() {
            self.modal_panel = None;
        } else {
            self.modal_panel = Some(window_handle.add_view(FileFinderView { updates: NotifyCell::new(()) }));
        }
        self.updates.set(());
    }
}

impl View for FileFinderView {
    fn component_name(&self) -> &'static str { "FileFinder" }

    fn render(&self) -> serde_json::Value {
        json!({
            "files": ""
        })
    }

    fn updates(&self) -> ViewUpdateStream {
        Box::new(self.updates.observe())
    }

    fn dispatch_action(&mut self, action: serde_json::Value) {}
}
