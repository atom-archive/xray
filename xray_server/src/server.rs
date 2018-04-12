use fs;
use futures::{stream, Future, Sink, Stream};
use futures_cpupool::CpuPool;
use messages::{IncomingMessage, OutgoingMessage};
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor;
use tokio_io::AsyncRead;
use xray_core::{self, App, WindowId};
use xray_core::app::Command;

#[derive(Clone)]
pub struct Server {
    app: xray_core::App,
    reactor: reactor::Handle,
}

impl Server {
    pub fn new(headless: bool, reactor: reactor::Handle) -> Self {
        let foreground = Rc::new(reactor.clone());
        let background = Rc::new(CpuPool::new_num_cpus());
        let io = fs::IoProvider::new();
        Server {
            app: App::new(headless, foreground, background, io),
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
                .map(move |(first_message, incoming)| {
                    first_message.map(|first_message| match first_message {
                        IncomingMessage::StartApp => {
                            server.start_app(outgoing, incoming);
                        }
                        IncomingMessage::StartCli { headless } => {
                            server.start_cli(outgoing, incoming, headless);
                        }
                        IncomingMessage::StartWindow { window_id, height } => {
                            server.start_window(outgoing, incoming, window_id, height);
                        }
                        _ => eprintln!("Unexpected message {:?}", first_message),
                    });
                })
                .then(|_| Ok(())),
        );
    }

    fn start_app<O, I>(&self, outgoing: O, incoming: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        if self.app.headless() {
            self.send_responses(
                outgoing,
                stream::once(Ok(OutgoingMessage::Error {
                    description: "This is a headless application instance".into(),
                })),
            );
        } else {
            if let Some(commands) = self.app.commands() {
                let server = self.clone();
                let responses = commands
                    .map(|update|
                        match update {
                            Command::OpenWindow(window_id) => OutgoingMessage::OpenWindow { window_id }
                        }
                    )
                    .select(
                        report_input_errors(incoming.map(move |message| server.handle_app_message(message))
                    ));

                self.send_responses(outgoing, responses);
            } else {
                self.send_responses(
                    outgoing,
                    stream::once(Ok(OutgoingMessage::Error {
                        description: "An application client is already registered".into(),
                    })),
                );
            }
        }
    }

    fn start_cli<O, I>(&self, outgoing: O, incoming: I, headless: bool)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        match (self.app.headless(), headless) {
            (true, false) => {
                return self.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                    description: "Since Xray was initially started with --headless, all subsequent commands must be --headless".into()
                })));
            }
            (false, true) => {
                return self.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                    description: "Since Xray was initially started without --headless, no subsequent commands may be --headless".into()
                })));
            }
            _ => {}
        }

        let server = self.clone();
        let responses = stream::once(Ok(OutgoingMessage::Ok)).chain(report_input_errors(
            incoming.map(move |message| server.handle_app_message(message)),
        ));
        self.send_responses(outgoing, responses);
    }

    pub fn start_window<O, I>(&self, outgoing: O, incoming: I, window_id: WindowId, height: f64)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = IncomingMessage, Error = io::Error>,
    {
        let server = self.clone();
        let receive_incoming = incoming
            .for_each(move |message| {
                server.handle_window_message(window_id, message);
                Ok(())
            })
            .then(|_| Ok(()));
        self.reactor.spawn(receive_incoming);

        match self.app.start_window(&window_id, height) {
            Ok(updates) => {
                self.send_responses(outgoing, updates.map(|update| OutgoingMessage::UpdateWindow(update)));
            },
            Err(_) => {
                self.send_responses(outgoing, stream::once(Ok(OutgoingMessage::Error {
                    description: format!("No window exists for id {}", window_id)
                })));
            }
        }
    }

    fn handle_app_message(&self, message: IncomingMessage) -> OutgoingMessage {
        let result = match message {
            IncomingMessage::OpenWorkspace { paths } => self.open_workspace(paths),
            IncomingMessage::Listen { port } => self.listen(port),
            IncomingMessage::ConnectToWorkspace { address } => self.connect_to_workspace(address),
            _ => Err(format!("Unexpected message {:?}", message)),
        };

        match result {
            Ok(_) => OutgoingMessage::Ok,
            Err(description) => OutgoingMessage::Error { description },
        }
    }

    fn handle_window_message(&self, window_id: WindowId, message: IncomingMessage) {
        match message {
            IncomingMessage::Action { view_id, action } => {
                self.app.dispatch_action(window_id, view_id, action);
            }
            _ => {
                eprintln!("Unexpected message {:?}", message);
            }
        }
    }

    fn open_workspace(&self, paths: Vec<PathBuf>) -> Result<(), String> {
        if !paths.iter().all(|path| path.is_absolute()) {
            return Err("All paths must be absolute".to_owned());
        }

        let roots = paths
            .iter()
            .map(|path| fs::Tree::new(path).unwrap())
            .collect();
        self.app.open_workspace(roots);
        Ok(())
    }

    fn listen(&self, port: u16) -> Result<(), String> {
        let local_addr = SocketAddr::new("127.0.0.1".parse().unwrap(), port);
        let listener = TcpListener::bind(&local_addr, &self.reactor)
            .map_err(|_| "Error binding address".to_owned())?;
        let handle_incoming = listener
            .incoming()
            .map_err(|_| eprintln!("Error accepting incoming connection"))
            .for_each(move |(socket, _)| {
                socket.set_nodelay(true).unwrap();
                Ok(())
            });
        self.reactor.spawn(handle_incoming);
        Ok(())
    }

    fn connect_to_workspace(&self, address: SocketAddr) -> Result<(), String> {
        let stream = TcpStream::connect(&address, &self.reactor).wait().map_err(|_| "Could not connect to address")?;
        stream.set_nodelay(true).unwrap();
        let (rx, tx) = stream.split();
        Ok(())
    }

    fn send_responses<O, I>(&self, outgoing: O, responses: I)
    where
        O: 'static + Sink<SinkItem = OutgoingMessage>,
        I: 'static + Stream<Item = OutgoingMessage, Error = ()>,
    {
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
