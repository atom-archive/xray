use fs;
use futures::sync::mpsc;
use futures::{stream, Future, Sink, Stream};
use futures_cpupool::CpuPool;
use messages::{IncomingMessage, OutgoingMessage};
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use tokio_core::reactor;
use xray_core::window::{ViewId, Window};
use xray_core::workspace::WorkspaceView;
use xray_core::{self, Peer};

type OutboundSender = mpsc::UnboundedSender<OutgoingMessage>;
pub type WindowId = usize;

pub struct App {
    state: Rc<RefCell<AppState>>,
}

struct AppState {
    peer: Peer,
    app_channel: Option<OutboundSender>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
    reactor: reactor::Handle,
}

impl App {
    pub fn new(reactor: reactor::Handle) -> Self {
        Self {
            state: Rc::new(RefCell::new(AppState {
                peer: Peer::new(),
                next_window_id: 1,
                app_channel: None,
                windows: HashMap::new(),
                reactor,
            })),
        }
    }

    pub fn add_connection<'a, S>(&mut self, socket: S)
    where
        S: 'static
            + Stream<Item = IncomingMessage, Error = io::Error>
            + Sink<SinkItem = OutgoingMessage>,
    {
        let (outgoing, incoming) = socket.split();
        let app_state = self.state.clone();
        self.state.borrow_mut().reactor.spawn(
            incoming
                .into_future()
                .map(|(first_message, incoming)| {
                    first_message.map(|first_message| match first_message {
                        IncomingMessage::StartApp => {
                            Self::start_app(app_state, outgoing, incoming);
                        }
                        IncomingMessage::StartCli => {
                            Self::start_cli(app_state, outgoing, incoming);
                        }
                        IncomingMessage::StartWindow { window_id, height } => {
                            Self::start_window(app_state, outgoing, incoming, window_id, height);
                        }
                        _ => eprintln!("Unexpected message {:?}", first_message),
                    });
                })
                .then(|_| Ok(())),
        );
    }

    fn start_app<O, I>(app_state: Rc<RefCell<AppState>>, outgoing: O, incoming: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        let app_state_clone = app_state.clone();
        let mut app_state_borrow = app_state.borrow_mut();
        if app_state_borrow.app_channel.is_some() {
            let responses = stream::once(Ok(OutgoingMessage::Error {
                description: "An application client is already registered".into(),
            }));
            app_state_borrow.reactor.spawn(
                outgoing
                    .send_all(responses.map_err(|_: ()| unreachable!()))
                    .then(|_| Ok(())),
            );
        } else {
            let (tx, rx) = mpsc::unbounded();
            app_state_borrow.app_channel = Some(tx.clone());
            let responses = report_input_errors(
                incoming
                    .map(move |message| app_state_clone.borrow_mut().handle_app_message(message)),
            );
            app_state_borrow.reactor.spawn(
                outgoing
                    .send_all(responses.select(rx).map_err(|_| unreachable!()))
                    .then(|_| Ok(())),
            );
        }
    }

    fn start_cli<O, I>(app_state: Rc<RefCell<AppState>>, outgoing: O, incoming: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        let app_state_clone = app_state.clone();
        let responses = report_input_errors(
            incoming.map(move |message| app_state_clone.borrow_mut().handle_app_message(message)),
        );
        let responses = stream::once(Ok(OutgoingMessage::Ok)).chain(responses);

        app_state.borrow_mut().reactor.spawn(
            outgoing
                .send_all(responses.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        )
    }

    fn start_window<O, I>(
        app_state: Rc<RefCell<AppState>>,
        outgoing: O,
        incoming: I,
        window_id: WindowId,
        height: f64,
    ) where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        let app_state_clone = app_state.clone();
        let mut app_state = app_state.borrow_mut();
        let window_updates = {
            let window = app_state.windows.get_mut(&window_id).unwrap();
            window.set_height(height);
            window.updates()
        };
        let receive_incoming = incoming
            .for_each(move |message| {
                app_state_clone
                    .borrow_mut()
                    .handle_window_message(window_id, message);
                Ok(())
            })
            .then(|_| Ok(()));

        let outgoing_messages = window_updates.map(|update| OutgoingMessage::UpdateWindow(update));
        let send_outgoing = outgoing
            .send_all(outgoing_messages.map_err(|_| unreachable!()))
            .then(|_| Ok(()));

        app_state.reactor.spawn(
            receive_incoming
                .select(send_outgoing)
                .then(|_: Result<((), _), ((), _)>| Ok(())),
        );
    }
}

impl AppState {
    fn handle_app_message(&mut self, message: IncomingMessage) -> OutgoingMessage {
        match message {
            IncomingMessage::OpenWorkspace { paths } => match self.open_workspace(paths) {
                Ok(()) => OutgoingMessage::Ok,
                Err(description) => OutgoingMessage::Error {
                    description: description.to_string(),
                },
            },
            _ => OutgoingMessage::Error {
                description: format!("Unexpected message {:?}", message),
            },
        }
    }

    fn handle_window_message(&mut self, window_id: usize, message: IncomingMessage) {
        match message {
            IncomingMessage::Action { view_id, action } => {
                self.dispatch_action(window_id, view_id, action);
            }
            _ => {
                eprintln!("Unexpected message {:?}", message);
            }
        };
    }

    fn open_workspace(&mut self, paths: Vec<PathBuf>) -> Result<(), &'static str> {
        let window_id = self.next_window_id;
        self.next_window_id += 1;

        let background_executor = Box::new(CpuPool::new_num_cpus());
        let mut window = Window::new(Some(background_executor), 0.0);

        if !paths.iter().all(|path| path.is_absolute()) {
            return Err("All paths must be absolute");
        }

        let roots = paths
            .iter()
            .map(|path| Box::new(fs::Tree::new(path).unwrap()) as Box<xray_core::fs::Tree>)
            .collect();

        let workspace = self.peer.open_workspace(roots);
        let workspace_view_handle = window.add_view(WorkspaceView::new(workspace));
        window.set_root_view(workspace_view_handle);
        self.windows.insert(window_id, window);

        if let Some(ref mut app_channel) = self.app_channel {
            app_channel
                .unbounded_send(OutgoingMessage::OpenWindow { window_id })
                .expect("Tried to open a workspace with no connected app");
        }

        Ok(())
    }

    fn dispatch_action(&mut self, window_id: WindowId, view_id: ViewId, action: serde_json::Value) {
        match self.windows.get_mut(&window_id) {
            Some(ref mut window) => window.dispatch_action(view_id, action),
            None => unimplemented!(),
        };
    }
}

fn report_input_errors<S>(incoming: S) -> Box<Stream<Item = OutgoingMessage, Error = ()>>
where
    S: 'static + Stream<Item = OutgoingMessage, Error = io::Error>,
{
    Box::new(
        incoming
            .then(|value| match value {
                Err(error) => Ok(OutgoingMessage::Error {
                    description: format!("Error reading message on server: {}", error),
                }),
                _ => value,
            })
            .map_err(|_| ()),
    )
}
