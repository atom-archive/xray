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
use xray_core::{self, App, WindowId};

type OutboundSender = mpsc::UnboundedSender<OutgoingMessage>;

#[derive(Clone)]
pub struct Server {
    app: xray_core::App,
    app_channel: Rc<RefCell<Option<OutboundSender>>>,
    reactor: reactor::Handle,
}

impl Server {
    pub fn new(headless: bool, reactor: reactor::Handle) -> Self {
        let bg_executor = Rc::new(CpuPool::new_num_cpus());
        Server {
            app: App::new(headless, bg_executor),
            app_channel: Rc::new(RefCell::new(None)),
            reactor,
        }
    }

    pub fn accept_connection<'a, S>(&mut self, socket: S)
    where
        S: 'static
            + Stream<Item = IncomingMessage, Error = io::Error>
            + Sink<SinkItem = OutgoingMessage>,
    {
        let (outgoing, incoming) = socket.split();
        let server = self.clone();
        self.reactor.spawn(
            incoming
                .into_future()
                .map(|(first_message, incoming)| {
                    first_message.map(|first_message| match first_message {
                        IncomingMessage::StartApp => {
                            Self::start_app(server, outgoing, incoming);
                        }
                        IncomingMessage::StartCli { headless } => {
                            Self::start_cli(server, outgoing, incoming, headless);
                        }
                        IncomingMessage::StartWindow { window_id, height } => {
                            Self::start_window(server, outgoing, incoming, window_id, height);
                        }
                        _ => eprintln!("Unexpected message {:?}", first_message),
                    });
                })
                .then(|_| Ok(())),
        );
    }

    fn start_app<O, I>(server: Server, outgoing: O, incoming: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        if server.app.headless() {
            server.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                description: "This is a headless application instance".into(),
            })));
        } else if server.app_channel.borrow().is_some() {
            server.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                description: "An application client is already registered".into(),
            })));
        } else {
            let (tx, rx) = mpsc::unbounded();
            server.app_channel.borrow_mut().get_or_insert(tx.clone());
            let server_clone = server.clone();
            let responses = rx.select(report_input_errors(
                incoming.map(move |message| server_clone.handle_app_message(message))
            ));

            server.send_responses(outgoing, responses);
        }
    }

    fn start_cli<O, I>(server: Server, outgoing: O, incoming: I, headless: bool)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        match (server.app.headless(), headless) {
            (true, false) => {
                return server.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                    description: "Since Xray was initially started with --headless, all subsequent commands must be --headless".into()
                })));
            }
            (false, true) => {
                return server.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                    description: "Since Xray was initially started without --headless, no subsequent commands may be --headless".into()
                })));
            },
            _ => {}
        }

        let server_clone = server.clone();
        let responses = stream::once(Ok(OutgoingMessage::Ok)).chain(report_input_errors(
            incoming.map(move |message| server_clone.handle_app_message(message)),
        ));
        server.send_responses(outgoing, responses);
    }

    fn start_window<O, I>(
        server: Server,
        outgoing: O,
        incoming: I,
        window_id: WindowId,
        height: f64,
    ) where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        let server_clone = server.clone();
        let receive_incoming = incoming
            .for_each(move |message| {
                server_clone.handle_window_message(window_id, message);
                Ok(())
            })
            .then(|_| Ok(()));

        let outgoing_messages = server
            .app
            .window_updates(&window_id, height)
            .map(|update| OutgoingMessage::UpdateWindow(update));

        let send_outgoing = outgoing
            .send_all(outgoing_messages.map_err(|_| unreachable!()))
            .then(|_| Ok(()));

        server.reactor.spawn(
            receive_incoming
                .select(send_outgoing)
                .then(|_: Result<((), _), ((), _)>| Ok(())),
        );
    }
}

impl Server {
    fn handle_app_message(&self, message: IncomingMessage) -> OutgoingMessage {
        let result = match message {
            IncomingMessage::OpenWorkspace { paths } => self.open_workspace(paths),
            _ => Err(format!("Unexpected message {:?}", message)),
        };

        match result {
            Ok(_) => OutgoingMessage::Ok,
            Err(description) => OutgoingMessage::Error { description }
        }
    }

    fn handle_window_message(&self, window_id: usize, message: IncomingMessage) {
        match message {
            IncomingMessage::Action { view_id, action } => {
                self.app.dispatch_action(window_id, view_id, action);
            }
            _ => {
                eprintln!("Unexpected message {:?}", message);
            }
        };
    }

    fn open_workspace(&self, paths: Vec<PathBuf>) -> Result<(), String> {
        if !paths.iter().all(|path| path.is_absolute()) {
            return Err("All paths must be absolute".to_owned());
        }

        let roots = paths
            .iter()
            .map(|path| Box::new(fs::Tree::new(path).unwrap()) as Box<xray_core::fs::Tree>)
            .collect();

        self.app.open_workspace(roots).map(|window_id| {
            self.app_channel.borrow().as_ref().map(|app_channel| {
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
