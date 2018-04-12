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
use BackgroundExecutor;
use ForegroundExecutor;

pub type WindowId = usize;
pub type PeerName = String;
type WorkspaceId = usize;

#[derive(Clone)]
pub struct App(Rc<RefCell<AppState>>);

struct AppState {
    headless: bool,
    background: BackgroundExecutor,
    commands_tx: UnboundedSender<Command>,
    commands_rx: Option<UnboundedReceiver<Command>>,
    peer_list: PeerList,
    next_workspace_id: WorkspaceId,
    workspaces: HashMap<WorkspaceId, Box<Workspace>>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
    updates: NotifyCell<()>,
}

pub enum Command {
    OpenWindow(WindowId),
}

#[derive(Clone)]
pub struct PeerList(Rc<RefCell<PeerListState>>);

struct PeerListState {
    foreground: ForegroundExecutor,
    peers: HashMap<PeerName, client::FullUpdateService<AppService>>,
    updates: NotifyCell<()>,
}

#[derive(Debug, PartialEq)]
struct PeerState {
    name: String,
    workspaces: Vec<WorkspaceDescriptor>,
}

#[derive(Debug, PartialEq)]
struct WorkspaceDescriptor {
    id: WorkspaceId,
}

struct AppService {
    app: App,
    updates: NotifyCellObserver<()>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteState {
    workspace_ids: Vec<WorkspaceId>,
}

#[derive(Serialize, Deserialize)]
pub enum RemoteRequest {}

#[derive(Serialize, Deserialize)]
pub enum RemoteResponse {}

impl App {
    pub fn new(
        headless: bool,
        foreground: ForegroundExecutor,
        background: BackgroundExecutor,
    ) -> Self {
        let (commands_tx, commands_rx) = mpsc::unbounded();
        App(Rc::new(RefCell::new(AppState {
            headless,
            background,
            commands_tx,
            commands_rx: Some(commands_rx),
            peer_list: PeerList::new(foreground),
            next_workspace_id: 0,
            workspaces: HashMap::new(),
            next_window_id: 1,
            windows: HashMap::new(),
            updates: NotifyCell::new(()),
        })))
    }

    pub fn commands(&self) -> Option<UnboundedReceiver<Command>> {
        self.0.borrow_mut().commands_rx.take()
    }

    pub fn headless(&self) -> bool {
        self.0.borrow().headless
    }

    pub fn open_workspace<T: 'static + fs::LocalTree>(&self, roots: Vec<T>) {
        self.add_workspace(Workspace::new(roots));
    }

    fn add_workspace(&self, workspace: Workspace) {
        let mut state = self.0.borrow_mut();
        if !state.headless {
            let mut window = Window::new(Some(state.background.clone()), 0.0);
            let workspace_view_handle = window.add_view(WorkspaceView::new(workspace.clone()));
            window.set_root_view(workspace_view_handle);
            let window_id = state.next_window_id;
            state.next_window_id += 1;
            state.windows.insert(window_id, window);
            if state
                .commands_tx
                .unbounded_send(Command::OpenWindow(window_id))
                .is_err()
            {
                let (commands_tx, commands_rx) = mpsc::unbounded();
                commands_tx
                    .unbounded_send(Command::OpenWindow(window_id))
                    .unwrap();
                state.commands_tx = commands_tx;
                state.commands_rx = Some(commands_rx);
            }
        }

        let id = state.next_workspace_id;
        state.next_workspace_id += 1;
        state.workspaces.insert(id, Box::new(workspace));
        state.updates.set(());
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
        server::Connection::new(incoming, AppService::new(self.clone()))
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

    pub fn peer_list(&self) -> PeerList {
        self.0.borrow().peer_list.clone()
    }
}

impl PeerList {
    fn new(foreground: ForegroundExecutor) -> Self {
        PeerList(Rc::new(RefCell::new(PeerListState {
            foreground,
            peers: HashMap::new(),
            updates: NotifyCell::new(()),
        })))
    }

    #[cfg(test)]
    fn state(&self) -> Vec<PeerState> {
        self.0
            .borrow()
            .peers
            .iter()
            .filter_map(|(name, peer)| {
                peer.latest_state().map(|state| PeerState {
                    name: name.clone(),
                    workspaces: state
                        .workspace_ids
                        .iter()
                        .map(|id| WorkspaceDescriptor { id: *id })
                        .collect(),
                })
            })
            .collect()
    }

    #[cfg(test)]
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
                let peer = client::FullUpdateService::new(peer);

                let mut state = state.borrow_mut();
                let updates = state.updates.clone();
                state
                    .foreground
                    .execute(Box::new(peer.updates().unwrap().for_each(move |_| {
                        updates.set(());
                        Ok(())
                    })))
                    .unwrap();
                state.peers.insert(name, peer);
                state.updates.set(());
                connection
            }),
        )
    }
}

impl AppService {
    fn new(app: App) -> Self {
        let updates = app.0.borrow().updates.observe();
        Self { app, updates }
    }

    fn state(&self) -> RemoteState {
        RemoteState {
            workspace_ids: self.app.0.borrow().workspaces.keys().cloned().collect(),
        }
    }
}

impl server::Service for AppService {
    type State = RemoteState;
    type Update = RemoteState;
    type Request = RemoteRequest;
    type Response = RemoteResponse;

    fn init(&mut self, _connection: &server::Connection) -> Self::State {
        self.state()
    }

    fn poll_update(&mut self, connection: &server::Connection) -> Async<Option<Self::Update>> {
        match self.updates.poll() {
            Ok(Async::Ready(Some(()))) => Async::Ready(Some(self.state())),
            Ok(Async::Ready(None)) | Err(_) => Async::Ready(None),
            Ok(Async::NotReady) => Async::NotReady,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs::tests::TestTree;
    use futures::{unsync, Future, Sink};
    use stream_ext::StreamExt;
    use tokio_core::reactor;

    #[test]
    fn test_remote_workspaces() {
        let mut reactor = reactor::Core::new().unwrap();
        let executor = Rc::new(reactor.handle());
        let mut server = App::new(true, executor.clone(), executor.clone());
        let mut client = App::new(false, executor.clone(), executor.clone());

        let peer_list = client.peer_list();
        let mut peer_list_updates = peer_list.updates();
        assert_eq!(peer_list.state(), vec![]);

        connect("server", &mut reactor, &mut server, &mut client);
        peer_list_updates.wait_next(&mut reactor);
        assert_eq!(
            peer_list.state(),
            vec![PeerState {
                name: String::from("server"),
                workspaces: vec![],
            }]
        );

        server.open_workspace(Vec::<TestTree>::new());
        peer_list_updates.wait_next(&mut reactor);
        assert_eq!(
            peer_list.state(),
            vec![PeerState {
                name: String::from("server"),
                workspaces: vec![WorkspaceDescriptor { id: 0 }],
            }]
        );
    }

    fn connect(name: &str, reactor: &mut reactor::Core, server: &mut App, client: &mut App) {
        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let server_to_client_rx = server_to_client_rx.map_err(|_| unreachable!());
        let (client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();
        let client_to_server_rx = client_to_server_rx.map_err(|_| unreachable!());

        let server_outgoing = server.connect_to_client(client_to_server_rx);
        reactor.handle().spawn(
            server_to_client_tx
                .send_all(server_outgoing.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );

        let client_future = client.connect_to_server(name.to_string(), server_to_client_rx);
        let client_outgoing = reactor.run(client_future).unwrap();
        reactor.handle().spawn(
            client_to_server_tx
                .send_all(client_outgoing.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );
    }
}
