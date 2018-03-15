extern crate xray_core;

use futures::{Future, Stream, Sink};
use futures::sync::mpsc;
use messages::{IncomingMessage, OutgoingMessage};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use serde_json;
use workspace::WorkspaceView;

type OutboundSender = mpsc::UnboundedSender<OutgoingMessage>;

pub struct App {
    shared: Rc<RefCell<Shared>>,
}

struct Shared {
    next_workspace_id: usize,
    app_channel: Option<OutboundSender>,
    window_channels: HashMap<usize, OutboundSender>,
    workspace_views: HashMap<usize, WorkspaceView>,
}

struct Client {
    channel: OutboundSender,
    state: ClientState,
    app_state: Rc<RefCell<Shared>>
}

enum ClientState {
    Unknown,
    Application,
    Window {
        workspace_id: usize
    },
}

impl App {
    pub fn new() -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared {
                next_workspace_id: 1,
                app_channel: None,
                window_channels: HashMap::new(),
                workspace_views: HashMap::new(),
            })),
        }
    }

    pub fn add_connection<'a, S>(&mut self, socket: S) -> Box<'a + Future<Item = (), Error = ()>>
    where
        S: 'a + Stream<Item = IncomingMessage, Error = io::Error> + Sink<SinkItem = OutgoingMessage>,
    {
        let (outgoing, incoming) = socket.split();
        let (tx, rx) = mpsc::unbounded();

        let mut client = Client::new(tx, self.shared.clone());
        let handle_incoming = incoming.for_each(move |message| {
            client.handle_message(message);
            Ok(())
        });

        let send_outgoing = outgoing.send_all(rx.map_err(|_| unreachable!())).then(|_| Ok(()));

        Box::new(handle_incoming.select(send_outgoing).then(|_| Ok(())))
    }
}

impl Client {
    fn new(channel: OutboundSender, app_state: Rc<RefCell<Shared>>) -> Self {
        Self {
            channel,
            state: ClientState::Unknown,
            app_state
        }
    }

    fn handle_message(&mut self, message: IncomingMessage) {
        match &self.state {
            &ClientState::Unknown => {
                match message {
                    IncomingMessage::StartApplication => {
                        self.start_application();
                        self.state = ClientState::Application;
                    }
                    IncomingMessage::StartWindow { workspace_id } => {
                        self.start_window(workspace_id);
                        self.state = ClientState::Window { workspace_id };
                    }
                    IncomingMessage::OpenWorkspace { paths } => {
                        self.open_workspace(paths);
                    },
                    _ => panic!("Unexpected message"),
                }
            },
            &ClientState::Application => {},
            &ClientState::Window { workspace_id } => {
                match message {
                    IncomingMessage::Action{view_type, view_id, action} => {
                        self.handle_action(
                            workspace_id,
                            view_type,
                            view_id,
                            action,
                        )
                    }
                    _ => panic!("Unexpected message")
                }
            }
        }
    }

    fn start_application(&self) {
        self.app_state.borrow_mut().app_channel = Some(self.channel.clone());
        self.channel.unbounded_send(OutgoingMessage::Acknowledge);
    }

    fn start_window(&mut self, workspace_id: usize) {
        self.app_state.borrow_mut().window_channels.insert(workspace_id, self.channel.clone());
        self.channel.unbounded_send(OutgoingMessage::WindowState { });
    }

    fn open_workspace(&mut self, paths: Vec<PathBuf>) {
        let mut app_state = self.app_state.borrow_mut();

        let workspace_id = app_state.next_workspace_id;
        app_state.next_workspace_id += 1;
        app_state.workspace_views.insert(workspace_id, WorkspaceView::new());
        if let Some(ref mut app_channel) = app_state.app_channel {
            app_channel.unbounded_send(OutgoingMessage::OpenWindow { workspace_id });
        }
        self.channel.unbounded_send(OutgoingMessage::Acknowledge);
    }

    fn handle_action(&mut self, workspace_id: usize, view_type: String, view_id: usize, action: serde_json::Value) {
        let mut app_state = self.app_state.borrow_mut();

        if let Some(ref mut workspace_view) = app_state.workspace_views.get_mut(&workspace_id) {
            workspace_view.handle_action(view_type, view_id, action);
        }
    }
}
