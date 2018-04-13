use fs;
use futures::unsync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::{Async, Future, Stream};
use notify_cell::{NotifyCell, NotifyCellObserver};
use project::LocalProject;
use rpc::client;
use rpc::server;
use serde_json;
use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::io;
use std::rc::Rc;
use window::{ViewId, Window, WindowUpdateStream};
use workspace::{Workspace, WorkspaceView};
use BackgroundExecutor;
use ForegroundExecutor;
use IntoShared;

pub type WindowId = usize;
pub type PeerName = String;
type WorkspaceId = usize;

pub struct App {
    headless: bool,
    background: BackgroundExecutor,
    io: Rc<fs::IoProvider>,
    commands_tx: UnboundedSender<Command>,
    commands_rx: Option<UnboundedReceiver<Command>>,
    peer_list: Rc<RefCell<PeerList>>,
    next_workspace_id: WorkspaceId,
    workspaces: HashMap<WorkspaceId, Rc<RefCell<Workspace>>>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
    updates: NotifyCell<()>,
}

pub enum Command {
    OpenWindow(WindowId),
}

pub struct PeerList {
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
    app: Rc<RefCell<App>>,
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
    pub fn new<T: 'static + fs::IoProvider>(
        headless: bool,
        foreground: ForegroundExecutor,
        background: BackgroundExecutor,
        io: T,
    ) -> Self {
        let (commands_tx, commands_rx) = mpsc::unbounded();
        App {
            headless,
            background,
            io: Rc::new(io),
            commands_tx,
            commands_rx: Some(commands_rx),
            peer_list: PeerList::new(foreground).into_shared(),
            next_workspace_id: 0,
            workspaces: HashMap::new(),
            next_window_id: 1,
            windows: HashMap::new(),
            updates: NotifyCell::new(()),
        }
    }

    pub fn commands(&mut self) -> Option<UnboundedReceiver<Command>> {
        self.commands_rx.take()
    }

    pub fn headless(&self) -> bool {
        self.headless
    }

    pub fn open_workspace<T: 'static + fs::LocalTree>(&mut self, roots: Vec<T>) {
        let io = self.io.clone();
        self.add_workspace(Workspace::new(LocalProject::new(io, roots)));
    }

    fn add_workspace(&mut self, workspace: Workspace) {
        let workspace = workspace.into_shared();
        if !self.headless {
            let mut window = Window::new(Some(self.background.clone()), 0.0);
            let workspace_view_handle = window.add_view(WorkspaceView::new(workspace.clone()));
            window.set_root_view(workspace_view_handle);
            let window_id = self.next_window_id;
            self.next_window_id += 1;
            self.windows.insert(window_id, window);
            if self.commands_tx
                .unbounded_send(Command::OpenWindow(window_id))
                .is_err()
            {
                let (commands_tx, commands_rx) = mpsc::unbounded();
                commands_tx
                    .unbounded_send(Command::OpenWindow(window_id))
                    .unwrap();
                self.commands_tx = commands_tx;
                self.commands_rx = Some(commands_rx);
            }
        }

        let id = self.next_workspace_id;
        self.next_workspace_id += 1;
        self.workspaces.insert(id, workspace);
        self.updates.set(());
    }

    pub fn start_window(&mut self, id: &WindowId, height: f64) -> Result<WindowUpdateStream, ()> {
        let window = self.windows.get_mut(id).ok_or(())?;
        window.set_height(height);
        Ok(window.updates())
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

    pub fn connect_to_client<S>(app: Rc<RefCell<App>>, incoming: S) -> server::Connection
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
    {
        server::Connection::new(incoming, AppService::new(app.clone()))
    }

    pub fn connect_to_server<S>(
        &self,
        name: PeerName,
        incoming: S,
    ) -> Box<Future<Item = client::Connection, Error = String>>
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
    {
        PeerList::connect_to_server(self.peer_list.clone(), name, incoming)
    }

    #[cfg(test)]
    pub fn peer_list(&self) -> Ref<PeerList> {
        self.peer_list.borrow()
    }
}

impl PeerList {
    fn new(foreground: ForegroundExecutor) -> Self {
        PeerList {
            foreground,
            peers: HashMap::new(),
            updates: NotifyCell::new(()),
        }
    }

    #[cfg(test)]
    fn state(&self) -> Vec<PeerState> {
        self.peers
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
        self.updates.observe()
    }

    fn connect_to_server<S>(
        peer_list: Rc<RefCell<PeerList>>,
        name: PeerName,
        incoming: S,
    ) -> Box<Future<Item = client::Connection, Error = String>>
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
    {
        Box::new(
            client::Connection::new(incoming).map(move |(connection, peer)| {
                let peer = client::FullUpdateService::new(peer);

                let mut peer_list = peer_list.borrow_mut();
                let updates = peer_list.updates.clone();
                peer_list
                    .foreground
                    .execute(Box::new(peer.updates().unwrap().for_each(move |_| {
                        updates.set(());
                        Ok(())
                    })))
                    .unwrap();
                peer_list.peers.insert(name, peer);
                peer_list.updates.set(());
                connection
            }),
        )
    }
}

impl AppService {
    fn new(app: Rc<RefCell<App>>) -> Self {
        let updates = app.borrow().updates.observe();
        Self { app, updates }
    }

    fn state(&self) -> RemoteState {
        RemoteState {
            workspace_ids: self.app.borrow().workspaces.keys().cloned().collect(),
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

    fn poll_update(&mut self, _: &server::Connection) -> Async<Option<Self::Update>> {
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
    use fs::tests::{TestIoProvider, TestTree};
    use futures::{unsync, Future, Sink};
    use stream_ext::StreamExt;
    use tokio_core::reactor;
    use IntoShared;

    #[test]
    fn test_remote_workspaces() {
        let mut reactor = reactor::Core::new().unwrap();
        let executor = Rc::new(reactor.handle());
        let server = App::new(
            true,
            executor.clone(),
            executor.clone(),
            TestIoProvider::new(),
        ).into_shared();
        let mut client = App::new(
            false,
            executor.clone(),
            executor.clone(),
            TestIoProvider::new(),
        );

        let mut peer_list_updates = client.peer_list().updates();
        assert_eq!(client.peer_list().state(), vec![]);

        connect("server", &mut reactor, server.clone(), &mut client);
        peer_list_updates.wait_next(&mut reactor);
        assert_eq!(
            client.peer_list().state(),
            vec![PeerState {
                name: String::from("server"),
                workspaces: vec![],
            }]
        );

        server.borrow_mut().open_workspace(Vec::<TestTree>::new());
        peer_list_updates.wait_next(&mut reactor);
        assert_eq!(
            client.peer_list().state(),
            vec![PeerState {
                name: String::from("server"),
                workspaces: vec![WorkspaceDescriptor { id: 0 }],
            }]
        );
    }

    fn connect(
        name: &str,
        reactor: &mut reactor::Core,
        server: Rc<RefCell<App>>,
        client: &mut App,
    ) {
        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let server_to_client_rx = server_to_client_rx.map_err(|_| unreachable!());
        let (client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();
        let client_to_server_rx = client_to_server_rx.map_err(|_| unreachable!());

        let server_outgoing = App::connect_to_client(server, client_to_server_rx);
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
