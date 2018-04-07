use BackgroundExecutor;
use ForegroundExecutor;
use fs;
use futures::unsync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::{Async, Future, Stream};
use notify_cell::{NotifyCell, NotifyCellObserver};
use rpc::{ConnectionToServer, ConnectionToClient, Service, ServiceClient};
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::rc::Rc;
use window::{ViewId, Window, WindowUpdateStream};
use workspace::{WorkspaceHandle, WorkspaceView};

pub type WindowId = usize;
pub type PeerName = String;

#[derive(Clone)]
pub struct App(Rc<RefCell<AppState>>);

struct AppState {
    headless: bool,
    foreground: ForegroundExecutor,
    background: BackgroundExecutor,
    updates_tx: UnboundedSender<AppUpdate>,
    updates_rx: Option<UnboundedReceiver<AppUpdate>>,
    peer_list: PeerList,
    workspaces: Vec<WorkspaceHandle>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
}

pub enum AppUpdate {
    OpenWindow(WindowId),
}

struct PeerList(Rc<RefCell<PeerListState>>);

struct PeerListState {
    peers: HashMap<PeerName, ServiceClient<App>>,
    updates: NotifyCell<()>
}

impl PeerList {
    fn new() -> Self {
        PeerList(Rc::new(RefCell::new(PeerListState {
            peers: HashMap::new(),
            updates: NotifyCell::new(())
        })))
    }

    fn updates(&self) -> NotifyCellObserver<()> {
        self.0.borrow().updates.observe()
    }

    fn connect_to_server<S>(&self, name: PeerName, incoming: S) -> Box<Future<Item = ConnectionToServer, Error = String>>
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>
    {
        let state = self.0.clone();
        Box::new(ConnectionToServer::new(incoming).map(move |(connection, peer)| {
            let mut state = state.borrow_mut();
            state.peers.insert(name, peer);
            state.updates.set(());
            connection
        }))
    }
}

impl App {
    pub fn new(
        headless: bool,
        foreground: ForegroundExecutor,
        background: BackgroundExecutor,
    ) -> Self {
        let (updates_tx, updates_rx) = mpsc::unbounded();
        App(Rc::new(RefCell::new(AppState {
            headless,
            foreground,
            background,
            updates_tx,
            updates_rx: Some(updates_rx),
            peer_list: PeerList::new(),
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
            let mut window = Window::new(Some(state.background.clone()), 0.0);
            let workspace_view_handle = window.add_view(WorkspaceView::new(workspace.clone()));
            window.set_root_view(workspace_view_handle);
            let window_id = state.next_window_id;
            state.next_window_id += 1;
            state.windows.insert(window_id, window);
            if state
                .updates_tx
                .unbounded_send(AppUpdate::OpenWindow(window_id))
                .is_err()
            {
                let (updates_tx, updates_rx) = mpsc::unbounded();
                updates_tx
                    .unbounded_send(AppUpdate::OpenWindow(window_id))
                    .unwrap();
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

    pub fn connect_to_client<S>(&self, incoming: S) -> ConnectionToClient
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
    {
        ConnectionToClient::new(incoming, self.clone())
    }

    pub fn connect_to_server<S>(&self, name: PeerName, incoming: S) -> Box<Future<Item = ConnectionToServer, Error = String>>
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

#[derive(Deserialize)]
pub enum RemoteRequest {}

#[derive(Serialize)]
pub enum RemoteResponse {}

impl Service for App {
    type State = RemoteState;
    type Update = RemoteState;
    type Request = RemoteRequest;
    type Response = RemoteResponse;
    type Error = ();

    fn state(&self, _connection: &mut ConnectionToClient) -> Self::State {
        RemoteState {
            workspace_count: self.0.borrow().workspaces.len(),
        }
    }

    fn poll_update(&mut self, _connection: &mut ConnectionToClient) -> Async<Option<Self::Update>> {
        Async::NotReady
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
        S2: 'static + Stream<Item = I>
    {
        Box::new(sink.send_all(stream.map_err(|_| unreachable!())).then(|_| Ok(())))
    }
}
