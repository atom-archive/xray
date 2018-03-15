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
use workspace::WorkspaceHandle;
use window::{Window, ViewId};

type OutboundSender = mpsc::UnboundedSender<OutgoingMessage>;
pub type WindowId = usize;

pub struct App {
    shared: Rc<RefCell<Shared>>,
}

struct Shared {
    app_channel: Option<OutboundSender>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
    window_channels: HashMap<WindowId, OutboundSender>,
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
        window_id: WindowId
    },
}

impl App {
    pub fn new() -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared {
                next_window_id: 1,
                app_channel: None,
                windows: HashMap::new(),
                window_channels: HashMap::new(),
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
                    IncomingMessage::StartWindow { window_id } => {
                        self.start_window(window_id);
                        self.state = ClientState::Window { window_id };
                    }
                    IncomingMessage::OpenWorkspace { paths } => {
                        self.open_workspace(paths);
                    },
                    _ => panic!("Unexpected message"),
                }
            },
            &ClientState::Application => {},
            &ClientState::Window { window_id } => {
                match message {
                    IncomingMessage::Action { view_id, action } => {
                        self.dispatch_action(window_id, view_id, action)
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

    fn start_window(&self, window_id: WindowId) {
        self.app_state.borrow_mut().window_channels.insert(window_id, self.channel.clone());
        self.channel.unbounded_send(OutgoingMessage::WindowState { });
    }

    fn open_workspace(&self, paths: Vec<PathBuf>) {
        self.app_state.borrow_mut().open_workspace(paths);
        self.channel.unbounded_send(OutgoingMessage::Acknowledge);
    }

    fn dispatch_action(&self, window_id: WindowId, view_id: ViewId, action: serde_json::Value) {
        self.app_state.borrow_mut().dispatch_action(window_id, view_id, action)
    }
}

impl Shared {
    fn open_workspace(&mut self, paths: Vec<PathBuf>) {
        let window_id = self.next_window_id;
        self.next_window_id += 1;

        let workspace = WorkspaceHandle::new(paths);
        let window = Window::new(workspace);
        self.windows.insert(window_id, window);

        if let Some(ref mut app_channel) = self.app_channel {
            app_channel.unbounded_send(OutgoingMessage::OpenWindow { window_id });
        }
    }

    fn dispatch_action(&mut self, window_id: WindowId, view_id: ViewId, action: serde_json::Value) {
        match self.windows.get_mut(&window_id) {
            Some(ref mut window) => window.dispatch_action(view_id, action),
            None => unimplemented!()
        };
    }
}
