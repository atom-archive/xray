use BackgroundExecutor;
use ForegroundExecutor;
use fs;
use futures::unsync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::{Async, Future, Stream};
use notify_cell::{NotifyCell, NotifyCellObserver};
use rpc::client;
use rpc::server;
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::rc::Rc;
use window::{ViewId, Window, WindowUpdateStream};
use workspace::{Workspace, WorkspaceView};

pub type WindowId = usize;
pub type PeerName = String;
type WorkspaceId = usize;

#[derive(Clone)]
pub struct App(Rc<RefCell<AppState>>);

struct AppState {
    headless: bool,
    foreground: ForegroundExecutor,
    background: BackgroundExecutor,
    commands_tx: UnboundedSender<AppCommand>,
    commands_rx: Option<UnboundedReceiver<AppCommand>>,
    peer_list: PeerList,
    next_workspace_id: WorkspaceId,
    workspaces: HashMap<WorkspaceId, Workspace>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
}

pub enum AppCommand {
    OpenWindow(WindowId),
}

struct PeerList(Rc<RefCell<PeerListState>>);

struct PeerListState {
    peers: HashMap<PeerName, client::Service<App>>,
    updates: NotifyCell<()>,
}

impl PeerList {
    fn new() -> Self {
        PeerList(Rc::new(RefCell::new(PeerListState {
            peers: HashMap::new(),
            updates: NotifyCell::new(()),
        })))
    }

    fn updates(&self) -> NotifyCellObserver<()> {
        self.0.borrow().updates.observe()
    }

    fn connect_to_server<S>(
        &self,
        name: PeerName,
        incoming: S,
    ) -> Box<Future<Item = client::Connection, Error = String>>
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
    {
        let state = self.0.clone();
        Box::new(
            client::Connection::new(incoming).map(move |(connection, peer)| {
                let mut state = state.borrow_mut();
                state.peers.insert(name, peer);
                state.updates.set(());
                connection
            }),
        )
    }
}

impl App {
    pub fn new(
        headless: bool,
        foreground: ForegroundExecutor,
        background: BackgroundExecutor,
    ) -> Self {
        let (commands_tx, commands_rx) = mpsc::unbounded();
        App(Rc::new(RefCell::new(AppState {
            headless,
            foreground,
            background,
            peer_list: PeerList::new(),
            commands_tx,
            commands_rx: Some(commands_rx),
            next_workspace_id: 0,
            workspaces: HashMap::new(),
            next_window_id: 1,
            windows: HashMap::new(),
        })))
    }

    pub fn updates(&self) -> Option<UnboundedReceiver<AppUpdate>> {
        self.0.borrow_mut().updates_rx.take()
    pub fn commands(&self) -> Option<UnboundedReceiver<AppCommand>> {
        self.0.borrow_mut().commands_rx.take()
    }

    pub fn headless(&self) -> bool {
        self.0.borrow().headless
    }

    pub fn open_workspace(&self, roots: Vec<Box<fs::Tree>>) {
        let mut state = self.0.borrow_mut();
        let workspace = Workspace::new(roots);
        if !state.headless {
            let mut window = Window::new(Some(state.background.clone()), 0.0);
            let workspace_view_handle = window.add_view(WorkspaceView::new(workspace.clone()));
            window.set_root_view(workspace_view_handle);
            let window_id = state.next_window_id;
            state.next_window_id += 1;
            state.windows.insert(window_id, window);
            if state
                .commands_tx
                .unbounded_send(AppCommand::OpenWindow(window_id))
                .is_err()
            {
                let (commands_tx, commands_rx) = mpsc::unbounded();
                commands_tx
                    .unbounded_send(AppCommand::OpenWindow(window_id))
                    .unwrap();
                state.commands_tx = commands_tx;
                state.commands_rx = Some(commands_rx);
            }
        };

        let id = state.next_workspace_id;
        state.next_window_id += 1;
        state.workspaces.insert(id, workspace);
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

    pub fn connect_to_client<S>(&self, incoming: S) -> server::Connection
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
    {
        server::Connection::new(incoming, self.clone())
    }

    pub fn connect_to_server<S>(
        &self,
        name: PeerName,
        incoming: S,
    ) -> Box<Future<Item = client::Connection, Error = String>>
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
    {
        self.0.borrow().peer_list.connect_to_server(name, incoming)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteState {
    workspace_count: usize,
}

#[derive(Serialize, Deserialize)]
pub enum RemoteRequest {}

#[derive(Serialize, Deserialize)]
pub enum RemoteResponse {}

impl server::Service for App {
    type State = RemoteState;
    type Update = RemoteState;
    type Request = RemoteRequest;
    type Response = RemoteResponse;
    type Error = ();

    fn state(&self, _connection: &server::Connection) -> Self::State {
        RemoteState {
            workspace_count: self.0.borrow().workspaces.len(),
        }
    }

    fn poll_update(&mut self, _: &server::Connection) -> Async<Option<Self::Update>> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    extern crate tokio_core;

    use super::*;
    use futures::{Future, Sink};

    #[test]
    fn test_rpc() {
        // let mut reactor = tokio_core::reactor::Core::new().unwrap();
        // let executor = Rc::new(reactor.handle());
        //
        // let server = App::new(true, executor.clone(), executor.clone());
        // let client = App::new(false, executor.clone(), executor.clone());
        // let (server_to_client_tx, server_to_client_rx) = mpsc::unbounded();
        // let (client_to_server_tx, client_to_server_rx) = mpsc::unbounded();
        //
        // let server_updates = server.connect_to_client(
        //     client_to_server_rx.map_err(|_| unreachable!())
        // );
        // executor.spawn(send_all(server_to_client_tx, server_updates));
        //
        // let client_updates = reactor.run(
        //     client.connect_to_server(String::from("server"), server_to_client_rx.map_err(|_| unreachable!()))
        // ).unwrap();
        // executor.spawn(send_all(client_to_server_tx, client_updates));
        //
        // let peer_list = client.peer_list();
        // assert_eq!(peer_list.state(), vec![
        //     PeerState { name: String::from("") }
        // ]);
    }

    fn send_all<I, S1, S2>(sink: S1, stream: S2) -> Box<Future<Item = (), Error = ()>>
    where
        S1: 'static + Sink<SinkItem = I>,
        S2: 'static + Stream<Item = I>,
    {
        Box::new(
            sink.send_all(stream.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        )
    }
}
