use fs;
use workspace::{WorkspaceHandle, WorkspaceView};
use std::collections::HashMap;
use window::{Window, Executor};

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
}
