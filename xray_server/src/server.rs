use capnp_rpc::{self, rpc_twoparty_capnp, twoparty, RpcSystem};
use fs;
use futures::sync::mpsc;
use futures::{stream, Future, Sink, Stream};
use futures_cpupool::CpuPool;
use std::cell::RefCell;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use tokio_core::net::TcpListener;
use tokio_core::reactor;
use tokio_io::AsyncRead;
use xray_core::{self, schema_capnp, App, WindowId};
use xray_core::messages::{IncomingMessage, OutgoingMessage};

#[derive(Clone)]
pub struct Server {
    app: xray_core::App,
    reactor: reactor::Handle,
}

impl Server {
    pub fn new(headless: bool, reactor: reactor::Handle) -> Self {
        let bg_executor = Rc::new(CpuPool::new_num_cpus());
        Server {
            app: App::new(headless, bg_executor),
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
            server.send_responses(
                outgoing,
                stream::once(Ok(OutgoingMessage::Error {
                    description: "This is a headless application instance".into(),
                })),
            );
        } else if server.app.has_client() {
            server.send_responses(
                outgoing,
                stream::once(Ok(OutgoingMessage::Error {
                    description: "An application client is already registered".into(),
                })),
            );
        } else {
            let (tx, rx) = mpsc::unbounded();
            server.app.set_client_tx(tx);
            let server_clone = server.clone();
            let responses = rx.select(report_input_errors(
                incoming.map(move |message| server_clone.handle_app_message(message)),
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
            }
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
            IncomingMessage::Listen { port } => self.listen(port),
            _ => Err(format!("Unexpected message {:?}", message)),
        };

        match result {
            Ok(_) => OutgoingMessage::Ok,
            Err(description) => OutgoingMessage::Error { description },
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
        self.app.open_workspace(roots);
        Ok(())
    }

    fn listen(&self, port: u16) -> Result<(), String> {
        let local_addr = SocketAddr::new("127.0.0.1".parse().unwrap(), port);
        let listener = TcpListener::bind(&local_addr, &self.reactor)
            .map_err(|_| "Error binding address".to_owned())?;
        let reactor = self.reactor.clone();
        let app = self.app.clone();
        let handle_incoming = listener
            .incoming()
            .map_err(|_| eprintln!("Error accepting incoming connection"))
            .for_each(move |(socket, _)| {
                socket.set_nodelay(true).unwrap();

                let (rx, tx) = socket.split();
                let peer = schema_capnp::peer::ToClient::new(app.clone())
                    .from_server::<capnp_rpc::Server>();
                let network = twoparty::VatNetwork::new(
                    rx,
                    tx,
                    rpc_twoparty_capnp::Side::Server,
                    Default::default(),
                );
                let rpc = RpcSystem::new(Box::new(network), Some(peer.clone().client));
                reactor.spawn(rpc.map_err(|err| eprintln!("Cap'N Proto RPC Error: {}", err)));
                Ok(())
            });
        self.reactor.spawn(handle_incoming);
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
