use futures::{Future, Sink, Stream};
use futures::sync::mpsc;
use futures_cpupool::CpuPool;
use messages::{IncomingMessage, OutgoingMessage};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use serde_json;
use xray_core;
use xray_core::workspace::WorkspaceView;
use xray_core::window::{ViewId, Window};
use tokio_core::reactor;
use fs;

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
        S: 'static
            + Stream<Item = IncomingMessage, Error = io::Error>
            + Sink<SinkItem = OutgoingMessage>,
    {
        let (outgoing, incoming) = socket.split();
        let inner = self.inner.clone();
        let incoming = incoming.map_err(|error| {
            eprintln!("Error reading incoming message: {:?}", error);
            error
        });
        self.inner.borrow_mut().reactor.spawn(
            incoming
                .into_future()
                .map(|(first_message, incoming)| {
                    first_message.map(|first_message| match first_message {
                        IncomingMessage::StartApp => {
                            Self::start_app(inner, outgoing, incoming);
                        }
                        IncomingMessage::StartCli => {
                            Self::start_cli(inner, outgoing, incoming);
                        }
                        IncomingMessage::StartWindow { window_id, height } => {
                            Self::start_window(inner, outgoing, incoming, window_id, height);
                        }
                        _ => eprintln!("Unexpected message {:?}", first_message),
                    });
                })
                .then(|_| Ok(())),
        );
    }

    fn start_app<O, I>(inner: Rc<RefCell<Inner>>, outgoing: O, incoming: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        let mut inner_borrow = inner.borrow_mut();
        let (tx, rx) = mpsc::unbounded();
        if inner_borrow.app_channel.is_some() {
            eprintln!("Redundant app client");
            return;
        }

        inner_borrow.app_channel = Some(tx.clone());

        let receive_incoming = Self::handle_app_messages(inner.clone(), tx, incoming);
        let send_outgoing = outgoing
            .send_all(rx.map_err(|_| unreachable!()))
            .then(|_| Ok(()));
        inner_borrow.reactor.spawn(
            receive_incoming
                .select(send_outgoing)
                .then(|_: Result<((), _), ((), _)>| Ok(())),
        );
    }

    fn start_cli<O, I>(inner: Rc<RefCell<Inner>>, outgoing: O, incoming: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        inner
            .borrow_mut()
            .reactor
            .spawn(Self::handle_app_messages(inner.clone(), outgoing, incoming));
    }

    fn start_window<O, I>(
        inner: Rc<RefCell<Inner>>,
        outgoing: O,
        incoming: I,
        window_id: WindowId,
        height: f64,
    ) where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        let inner_clone = inner.clone();
        let mut inner = inner.borrow_mut();
        let window_updates = {
            let window = inner.windows.get_mut(&window_id).unwrap();
            window.set_height(height);
            window.updates()
        };
        let receive_incoming = incoming
            .for_each(move |message| {
                inner_clone
                    .borrow_mut()
                    .handle_window_message(window_id, message);
                Ok(())
            })
            .then(|_| Ok(()));

        let outgoing_messages = window_updates.map(|update| OutgoingMessage::UpdateWindow(update));
        let send_outgoing = outgoing
            .send_all(outgoing_messages.map_err(|_| unreachable!()))
            .then(|_| Ok(()));

        inner.reactor.spawn(
            receive_incoming
                .select(send_outgoing)
                .then(|_: Result<((), _), ((), _)>| Ok(())),
        );
    }

    fn handle_app_messages<O, I>(
        inner: Rc<RefCell<Inner>>,
        outgoing: O,
        incoming: I,
    ) -> Box<Future<Item = (), Error = ()>>
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        let responses = incoming
            .map(move |message| {
                inner.borrow_mut().handle_app_message(message)
            })
            .map_err(|_| unreachable!());
        Box::new(outgoing.send_all(responses).then(|_| Ok(())))
    }
}

impl Inner {
    fn handle_app_message(&mut self, message: IncomingMessage) -> OutgoingMessage {
        match message {
            IncomingMessage::OpenWorkspace { paths } => {
                match self.open_workspace(paths) {
                    Ok(()) => OutgoingMessage::Ok,
                    Err(description) => OutgoingMessage::Error {
                        description: description.to_string()
                    }
                }
            }
            _ => {
                OutgoingMessage::Error {
                    description: format!("Unexpected message {:?}", message)
                }
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

    fn open_workspace(&mut self, paths: Vec<PathBuf>) -> Result<(), &'static str> {
        let window_id = self.next_window_id;
        self.next_window_id += 1;

        let background_executor = Box::new(CpuPool::new_num_cpus());
        let mut window = Window::new(Some(background_executor), 0.0);

        if !paths.iter().all(|path| path.is_absolute()) {
            return Err("All paths must be absolute");
        }

        let roots = paths.iter()
            .map(|path| Box::new(fs::Tree::new(path).unwrap()) as Box<xray_core::fs::Tree>)
            .collect();

        let workspace_view_handle = window.add_view(WorkspaceView::new(roots));
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
