extern crate xray_core;

use std::io;
use std::rc::Rc;
use tokio_core::reactor::Handle;
use futures::stream::{self, Stream};
use futures::sync::mpsc;
use futures::sink::Sink;
use tokio_io::codec::Framed;
use messages::{IncomingMessage, OutgoingMessage};
use futures::{Async, Future};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

type Tx = mpsc::UnboundedSender<OutgoingMessage>;
type Rx = mpsc::UnboundedReceiver<OutgoingMessage>;

struct WorkspaceView {

}

pub struct App {
    shared: Rc<RefCell<Shared>>,
}

struct Shared {
    next_workspace_id: usize,
    application_sender: Option<Tx>,
    workspace_views: HashMap<usize, WorkspaceView>,
    window_senders: HashMap<usize, Tx>,
}

pub struct Client<Socket>
where
    Socket: Stream<Item = IncomingMessage> + Sink<SinkItem = OutgoingMessage>,
{
    socket: Socket,
    rx: Option<Rx>,
    shared: Rc<RefCell<Shared>>,
}

impl<Socket> Future for Client<Socket>
where
    Socket: Stream<Item = IncomingMessage> + Sink<SinkItem = OutgoingMessage>,
{
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        match self.socket.poll() {
            Ok(Async::NotReady) => {},
            Err(error) => {
                println!("Error");
                return Ok(Async::Ready(()))
            },
            Ok(Async::Ready(None)) => {
                return Ok(Async::Ready(()))
            },
            Ok(Async::Ready(Some(v))) => {
                return match v {
                    IncomingMessage::StartWindow => Ok(self.start_window()),
                    IncomingMessage::StartApplication => Ok(self.start_application()),
                    IncomingMessage::OpenWorkspace{paths} => Ok(self.open_workspace(paths)),
                }
            },
        }

        if let Some(ref mut rx) = self.rx {
            match rx.poll() {
                Ok(Async::Ready(Some(v))) => {
                    self.socket.start_send(v);
                },
                _ => {},
            }
        }

        Ok(Async::NotReady)
    }
}

impl<Socket> Client<Socket>
where
    Socket: Stream<Item = IncomingMessage> + Sink<SinkItem = OutgoingMessage>,
{
    fn start_application(&mut self) -> Async<()> {
        let mut shared = self.shared.borrow_mut();
        let (tx, rx) = mpsc::unbounded();
        shared.application_sender = Some(tx);
        self.rx = Some(rx);
        self.socket.start_send(OutgoingMessage::Acknowledge);
        Async::NotReady
    }

    fn start_window(&mut self) -> Async<()> {
        self.socket.start_send(OutgoingMessage::WindowState);
        Async::NotReady
    }

    fn open_workspace(&mut self, paths: Vec<PathBuf>) -> Async<()> {
        let mut shared = self.shared.borrow_mut();
        let workspace_id = shared.next_workspace_id;
        shared.next_workspace_id += 1;

        if let &mut Some(ref mut sender) = &mut shared.application_sender {
            sender.start_send(OutgoingMessage::OpenWindow{workspace_id});
        }

        self.socket.start_send(OutgoingMessage::Acknowledge);
        Async::Ready(())
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared {
                next_workspace_id: 1,
                application_sender: None,
                window_senders: HashMap::new(),
                workspace_views: HashMap::new(),
            })),
        }
    }

    pub fn add_connection<Socket>(&mut self, socket: Socket) -> Client<Socket>
    where
        Socket: Stream<Item = IncomingMessage> + Sink<SinkItem = OutgoingMessage>,
    {
        Client {
            socket: socket,
            rx: None,
            shared: self.shared.clone(),
        }
    }
}
