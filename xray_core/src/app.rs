use fs;
use futures::sync::mpsc::UnboundedSender;
use futures::{future, Future, Sink, Stream};
use messages::{IncomingMessage, OutgoingMessage};
use schema_capnp;
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use window::{self, ViewId, Window};
use workspace::{WorkspaceHandle, WorkspaceView};

pub type WindowId = usize;
pub type Executor = Rc<future::Executor<Box<Future<Item = (), Error = ()>>>>;

#[derive(Clone)]
pub struct App(Rc<RefCell<AppState>>);

struct AppState {
    client_tx: Option<UnboundedSender<OutgoingMessage>>,
    headless: bool,
    fg_executor: Executor,
    bg_executor: window::Executor,
    workspaces: Vec<WorkspaceHandle>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
}

impl App {
    pub fn new(headless: bool, fg_executor: Executor, bg_executor: window::Executor) -> Self {
        App(Rc::new(RefCell::new(AppState {
            headless,
            fg_executor,
            bg_executor,
            client_tx: None,
            workspaces: Vec::new(),
            next_window_id: 1,
            windows: HashMap::new(),
        })))
    }

    pub fn has_client(&self) -> bool {
        self.0.borrow().client_tx.is_some()
    }

    pub fn set_client_tx(&self, tx: UnboundedSender<OutgoingMessage>) {
        self.0.borrow_mut().client_tx = Some(tx);
    }

    pub fn headless(&self) -> bool {
        self.0.borrow().headless
    }

    pub fn open_workspace(&self, roots: Vec<Box<fs::Tree>>) {
        let mut state = self.0.borrow_mut();
        let workspace = WorkspaceHandle::new(roots);
        if !state.headless {
            let mut window = Window::new(Some(state.bg_executor.clone()), 0.0);
            let workspace_view_handle = window.add_view(WorkspaceView::new(workspace.clone()));
            window.set_root_view(workspace_view_handle);
            let window_id = state.next_window_id;
            state.next_window_id += 1;
            state.windows.insert(window_id, window);
            if let Some(ref tx) = state.client_tx {
                tx.unbounded_send(OutgoingMessage::OpenWindow { window_id })
                    .expect("Error sending to app channel");
            }
        };
        state.workspaces.push(workspace);
    }

    pub fn start_window<O, I>(&self, outgoing: O, incoming: I, window_id: WindowId, height: f64)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = ()>,
    {
        let app_clone = self.clone();
        let receive_incoming = incoming
            .for_each(move |message| {
                app_clone.handle_window_message(window_id, message);
                Ok(())
            })
            .then(|_| Ok(()));

        let window_updates = {
            let mut state = self.0.borrow_mut();
            let window = state.windows.get_mut(&window_id).unwrap();
            window.set_height(height);
            window.updates()
        };
        let outgoing_messages = window_updates.map(|update| OutgoingMessage::UpdateWindow(update));
        let send_outgoing = outgoing
            .send_all(outgoing_messages.map_err(|_| unreachable!()))
            .then(|_| Ok(()));

        self.0
            .borrow()
            .fg_executor
            .execute(Box::new(
                receive_incoming
                    .select(send_outgoing)
                    .then(|_: Result<((), _), ((), _)>| Ok(())),
            ))
            .unwrap();
    }

    fn handle_window_message(&self, window_id: WindowId, message: IncomingMessage) {
        match message {
            IncomingMessage::Action { view_id, action } => {
                self.dispatch_action(window_id, view_id, action);
            }
            _ => {
                eprintln!("Unexpected message {:?}", message);
            }
        }
    }

    pub fn dispatch_action(&self, window_id: WindowId, view_id: ViewId, action: serde_json::Value) {
        let mut state = self.0.borrow_mut();
        match state.windows.get_mut(&window_id) {
            Some(ref mut window) => window.dispatch_action(view_id, action),
            None => unimplemented!(),
        };
    }
}

impl schema_capnp::peer::Server for App {}
