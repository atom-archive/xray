use fs;
use futures::sync::mpsc;
use futures::{stream, Future, Sink, Stream};
use futures_cpupool::CpuPool;
use messages::{IncomingMessage, OutgoingMessage};
use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use tokio_core::reactor;
use xray_core::{self, Peer, WindowId};

type OutboundSender = mpsc::UnboundedSender<OutgoingMessage>;

pub struct App {
    state: Rc<RefCell<AppState>>,
}

struct AppState {
    peer: Peer,
    app_channel: Option<OutboundSender>,
    reactor: reactor::Handle,
}

impl App {
    pub fn new(headless: bool, reactor: reactor::Handle) -> Self {
        let bg_executor = Rc::new(CpuPool::new_num_cpus());
        Self {
            state: Rc::new(RefCell::new(AppState {
                peer: Peer::new(headless, bg_executor),
                app_channel: None,
                reactor,
            })),
        }
    }

    pub fn accept_connection<'a, S>(&mut self, socket: S)
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
                        IncomingMessage::StartCli { headless } => {
                            Self::start_cli(app_state, outgoing, incoming, headless);
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
        let mut app_state = app_state.borrow_mut();
        if app_state.peer.headless() {
            app_state.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                description: "This is a headless application instance".into(),
            })));
        } else if app_state.app_channel.is_some() {
            app_state.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                description: "An application client is already registered".into(),
            })));
        } else {
            let (tx, rx) = mpsc::unbounded();
            app_state.app_channel = Some(tx.clone());
            let responses = rx.select(report_input_errors(
                incoming.map(move |message|
                    app_state_clone.borrow_mut().handle_app_message(message)
                )
            ));

            app_state.send_responses(outgoing, responses);
        }
    }

    fn start_cli<O, I>(app_state: Rc<RefCell<AppState>>, outgoing: O, incoming: I, headless: bool)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        match (app_state.borrow().peer.headless(), headless) {
            (true, false) => {
                return app_state.borrow().send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                    description: "Since Xray was initially started with --headless, all subsequent commands must be --headless".into()
                })));
            }
            (false, true) => {
                return app_state.borrow().send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                    description: "Since Xray was initially started without --headless, no subsequent commands may be --headless".into()
                })));
            },
            _ => {}
        }

        let app_state_clone = app_state.clone();
        let responses = stream::once(Ok(OutgoingMessage::Ok)).chain(report_input_errors(
            incoming.map(move |message| app_state_clone.borrow_mut().handle_app_message(message)),
        ));
        app_state.borrow_mut().send_responses(outgoing, responses);
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
            let window = app_state.peer.windows.get_mut(&window_id).unwrap();
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
                self.peer.dispatch_action(window_id, view_id, action);
            }
            _ => {
                eprintln!("Unexpected message {:?}", message);
            }
        };
    }

    fn open_workspace(&mut self, paths: Vec<PathBuf>) -> Result<(), &'static str> {
        if !paths.iter().all(|path| path.is_absolute()) {
            return Err("All paths must be absolute");
        }

        let roots = paths
            .iter()
            .map(|path| Box::new(fs::Tree::new(path).unwrap()) as Box<xray_core::fs::Tree>)
            .collect();

        self.peer.open_workspace(roots).map(|window_id| {
            self.app_channel.as_ref().map(|app_channel| {
                app_channel
                    .unbounded_send(OutgoingMessage::OpenWindow { window_id })
                    .expect("Error sending to app channel");
            })
        });

        Ok(())
    }

    fn send_responses<O, I>(&self, outgoing: O, responses: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = OutgoingMessage, Error = ()> {
        self.reactor.spawn(
            outgoing
                .send_all(responses.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );
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
