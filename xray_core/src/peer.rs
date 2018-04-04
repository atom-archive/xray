use fs;
use serde_json;
use std::collections::HashMap;
use window::{Executor, ViewId, Window};
use workspace::{WorkspaceHandle, WorkspaceView};

pub type WindowId = usize;

pub struct Peer {
    headless: bool,
    executor: Executor,
    workspaces: Vec<WorkspaceHandle>,
    next_window_id: WindowId,
    pub windows: HashMap<WindowId, Window>,
}

impl Peer {
    pub fn new(headless: bool, executor: Executor) -> Self {
        Self {
            headless,
            executor: executor,
            workspaces: Vec::new(),
            next_window_id: 1,
            windows: HashMap::new(),
        }
    }

    pub fn headless(&self) -> bool {
        self.headless
    }

    pub fn open_workspace(&mut self, roots: Vec<Box<fs::Tree>>) -> Option<WindowId> {
        let workspace = WorkspaceHandle::new(roots);
        let window_id = if self.headless {
            None
        } else {
            let mut window = Window::new(Some(self.executor.clone()), 0.0);
            let workspace_view_handle = window.add_view(WorkspaceView::new(workspace.clone()));
            window.set_root_view(workspace_view_handle);
            let window_id = self.next_window_id;
            self.next_window_id += 1;
            self.windows.insert(window_id, window);
            Some(window_id)
        };
        self.workspaces.push(workspace);
        window_id
    }

    pub fn dispatch_action(
        &mut self,
        window_id: WindowId,
        view_id: ViewId,
        action: serde_json::Value,
    ) {
        match self.windows.get_mut(&window_id) {
            Some(ref mut window) => window.dispatch_action(view_id, action),
            None => unimplemented!(),
        };
    }
}
