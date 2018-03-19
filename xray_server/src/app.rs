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
    inner: Rc<RefCell<Inner>>,
}

struct Inner {
    app_channel: Option<OutboundSender>,
    next_window_id: WindowId,
    windows: HashMap<WindowId, Window>,
    reactor: reactor::Handle,
}

impl App {
    pub fn new(reactor: reactor::Handle) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
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
        let inner = self.inner.clone();
        let incoming = incoming.map_err(|error| {
            eprintln!("Error reading incoming message: {:?}", error);
            error
        });
        self.inner.borrow_mut().reactor.spawn(incoming.into_future().map(|(first_message, incoming)| {
            first_message.map(|first_message| {
                match first_message {
                    IncomingMessage::StartApp => {
                        Self::start_app(inner, outgoing, incoming);
                    },
                    IncomingMessage::StartCli => {
                        Self::start_cli(inner, incoming);
                    },
                    IncomingMessage::StartWindow { window_id } => {
                        Self::start_window(inner, outgoing, incoming, window_id);
                    }
                    _ => eprintln!("Unexpected message {:?}", first_message),
                }
            });
        }).then(|_| Ok(())));
    }

    fn start_app<O, I>(inner: Rc<RefCell<Inner>>, outgoing: O, incoming: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>
    {
        {
            let mut inner = inner.borrow_mut();
            let (tx, rx) = mpsc::unbounded();
            if inner.app_channel.is_some() {
                eprintln!("Redundant app client");
                return;
            }

            inner.app_channel = Some(tx);
            inner.reactor.spawn(
                outgoing.send_all(rx.map_err(|_| unreachable!())).then(|_| Ok(()))
            );
        }

        Self::handle_app_messages(inner, incoming);
    }

    fn start_cli<I>(inner: Rc<RefCell<Inner>>, incoming: I)
    where
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>
    {
        Self::handle_app_messages(inner, incoming);
    }

    fn start_window<O, I>(inner: Rc<RefCell<Inner>>, outgoing: O, incoming: I, window_id: WindowId)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>
    {
        let inner_clone = inner.clone();
        let mut inner = inner.borrow_mut();
        let window_updates = inner.windows.get_mut(&window_id).unwrap().updates();
        let receive_incoming = incoming.for_each(move |message| {
            inner_clone.borrow_mut().handle_window_message(window_id, message);
            Ok(())
        }).then(|_| Ok(()));

        let outgoing_messages = window_updates.map(|update| OutgoingMessage::UpdateWindow(update));
        let send_outgoing = outgoing
            .send_all(outgoing_messages.map_err(|_| unreachable!()))
            .then(|_| Ok(()));

        inner.reactor.spawn(
            receive_incoming
                .select(send_outgoing)
                .then(|_: Result<((), _), ((), _)>| Ok(()))
        );
    }

    fn handle_app_messages<I>(inner: Rc<RefCell<Inner>>, incoming: I)
    where
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>
    {
        let inner_clone = inner.clone();
        let inner = inner.borrow_mut();
        inner.reactor.spawn(
            incoming.for_each(move |message| {
                inner_clone.borrow_mut().handle_app_message(message);
                Ok(())
            }).then(|_| Ok(()))
        );
    }
}

impl Inner {
    fn handle_app_message(&mut self, message: IncomingMessage) {
        match message {
            IncomingMessage::OpenWorkspace { paths } => {
                self.open_workspace(paths);
            }
            _ => {
                eprintln!("Unexpected message {:?}", message);
            }
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
