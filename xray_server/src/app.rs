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
use tokio_core::reactor;

type OutboundSender = mpsc::UnboundedSender<OutgoingMessage>;
pub type WindowId = usize;

pub struct App {
    shared: Rc<RefCell<Shared>>,
}

struct Shared {
    app_channel: Option<OutboundSender>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
    reactor: reactor::Handle,
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
    pub fn new(reactor: reactor::Handle) -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared {
                next_window_id: 1,
                app_channel: None,
                windows: HashMap::new(),
                reactor,
            })),
        }
    }

    pub fn add_connection<'a, S>(&mut self, socket: S)
    where
        S: 'static + Stream<Item = IncomingMessage, Error = io::Error> + Sink<SinkItem = OutgoingMessage>,
    {
        let (outgoing, incoming) = socket.split();
        let (tx, rx) = mpsc::unbounded();
        let mut client = Client::new(tx, self.shared.clone());

        let mut shared = self.shared.borrow_mut();
        shared.reactor.spawn(incoming.for_each(move |message| {
            client.handle_message(message);
            Ok(())
        }).then(|_| Ok(())));

        // let send_outgoing = outgoing.send_all(rx.map_err(|_| unreachable!())).then(|_| Ok(()));

        // Box::new(handle_incoming.select(send_outgoing).then(|_| Ok(())))
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
        let mut app_state = self.app_state.borrow_mut();
        if let Some(window_updates) = app_state.windows.get_mut(&window_id).unwrap().updates() {
            let outgoing_messages = window_updates.map(|update| OutgoingMessage::WindowUpdate(update));
            app_state.reactor.spawn(
                self.channel.clone()
                    .send_all(outgoing_messages.map_err(|_| unreachable!()))
                    .then(|_| Ok(()))
            );
        } else {
            unimplemented!();
        }
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
