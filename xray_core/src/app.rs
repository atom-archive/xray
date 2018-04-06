use fs;
use futures::unsync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::{future, Future};
use serde_json;
use rpc::{ConnectionToClient, Service};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use window::{self, ViewId, Window, WindowUpdateStream};
use workspace::{WorkspaceHandle, WorkspaceView};

pub type WindowId = usize;
pub type Executor = Rc<future::Executor<Box<Future<Item = (), Error = ()>>>>;

#[derive(Clone)]
pub struct App(Rc<RefCell<AppState>>);

struct AppState {
    headless: bool,
    executor: window::Executor,
    updates_tx: UnboundedSender<AppUpdate>,
    updates_rx: Option<UnboundedReceiver<AppUpdate>>,
    workspaces: Vec<WorkspaceHandle>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
}

pub enum AppUpdate {
    OpenWindow(WindowId)
}

impl App {
    pub fn new(headless: bool, executor: window::Executor) -> Self {
        let (updates_tx, updates_rx) = mpsc::unbounded();
        App(Rc::new(RefCell::new(AppState {
            headless,
            executor,
            updates_tx,
            updates_rx: Some(updates_rx),
            workspaces: Vec::new(),
            next_window_id: 1,
            windows: HashMap::new(),
        })))
    }

    pub fn updates(&self) -> Option<UnboundedReceiver<AppUpdate>> {
        self.0.borrow_mut().updates_rx.take()
    }

    pub fn headless(&self) -> bool {
        self.0.borrow().headless
    }

    pub fn open_workspace(&self, roots: Vec<Box<fs::Tree>>) {
        let mut state = self.0.borrow_mut();
        let workspace = WorkspaceHandle::new(roots);
        if !state.headless {
            let mut window = Window::new(Some(state.executor.clone()), 0.0);
            let workspace_view_handle = window.add_view(WorkspaceView::new(workspace.clone()));
            window.set_root_view(workspace_view_handle);
            let window_id = state.next_window_id;
            state.next_window_id += 1;
            state.windows.insert(window_id, window);
            if state.updates_tx.unbounded_send(AppUpdate::OpenWindow(window_id)).is_err() {
                let (updates_tx, updates_rx) = mpsc::unbounded();
                updates_tx.unbounded_send(AppUpdate::OpenWindow(window_id)).unwrap();
                state.updates_tx = updates_tx;
                state.updates_rx = Some(updates_rx);
            }
        };
        state.workspaces.push(workspace);
    }

    pub fn start_window(&self, id: &WindowId, height: f64) -> Result<WindowUpdateStream, ()> {
        let mut state = self.0.borrow_mut();
        let window = state.windows.get_mut(id).ok_or(())?;
        window.set_height(height);
        Ok(window.updates())
    }

    pub fn dispatch_action(&self, window_id: WindowId, view_id: ViewId, action: serde_json::Value) {
        let mut state = self.0.borrow_mut();
        match state.windows.get_mut(&window_id) {
            Some(ref mut window) => window.dispatch_action(view_id, action),
            None => unimplemented!(),
        };
    }

    // pub fn accept_connection(&self) -> ConnectionToClient {
    //     ConnectionToClient::new(self.clone())
    // }
}

#[derive(Serialize)]
pub struct RemoteState {
    workspace_count: usize
}
#[derive(Deserialize)]
pub enum RemoteRequest {}
#[derive(Serialize)]
pub enum RemoteResponse {}

// impl Service for App {
//     type State = RemoteState;
//     type Update = RemoteState;
//     type Request = RemoteRequest;
//     type Response = RemoteResponse;
//     type Error = ();
//
//     fn state(&self, _connection: &ConnectionToClient) -> Self::State {
//         RemoteState {
//             workspace_count: self.0.borrow().workspaces.len()
//         }
//     }
// }
