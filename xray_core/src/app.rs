use fs;
use schema_capnp;
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use window::{Executor, ViewId, Window, WindowUpdateStream};
use workspace::{WorkspaceHandle, WorkspaceView};

pub type WindowId = usize;

#[derive(Clone)]
pub struct App(Rc<RefCell<AppState>>);

struct AppState {
    headless: bool,
    executor: Executor,
    workspaces: Vec<WorkspaceHandle>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
}

impl App {
    pub fn new(headless: bool, executor: Executor) -> Self {
        App(Rc::new(RefCell::new(AppState {
            headless,
            executor: executor,
            workspaces: Vec::new(),
            next_window_id: 1,
            windows: HashMap::new(),
        })))
    }

    pub fn headless(&self) -> bool {
        self.0.borrow().headless
    }

    pub fn open_workspace(&self, roots: Vec<Box<fs::Tree>>) -> Option<WindowId> {
        let mut state = self.0.borrow_mut();
        let workspace = WorkspaceHandle::new(roots);
        let window_id = if state.headless {
            None
        } else {
            let mut window = Window::new(Some(state.executor.clone()), 0.0);
            let workspace_view_handle = window.add_view(WorkspaceView::new(workspace.clone()));
            window.set_root_view(workspace_view_handle);
            let window_id = state.next_window_id;
            state.next_window_id += 1;
            state.windows.insert(window_id, window);
            Some(window_id)
        };
        state.workspaces.push(workspace);
        window_id
    }

    pub fn dispatch_action(&self, window_id: WindowId, view_id: ViewId, action: serde_json::Value) {
        let mut state = self.0.borrow_mut();
        match state.windows.get_mut(&window_id) {
            Some(ref mut window) => window.dispatch_action(view_id, action),
            None => unimplemented!(),
        };
    }

    pub fn window_updates(&self, id: &WindowId, height: f64) -> WindowUpdateStream {
        let mut state = self.0.borrow_mut();
        let window = state.windows.get_mut(id).unwrap();
        window.set_height(height);
        window.updates()
    }
}

impl schema_capnp::peer::Server for App {}
