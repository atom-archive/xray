extern crate xray_core;

use std::io;
use std::rc::Rc;
use tokio_core::reactor::Handle;
use futures::{Async, Future, Poll};
use futures::stream::{self, Stream};
use futures::sync::mpsc;
use futures::sink::Sink;
use tokio_io::codec::Framed;
use messages::{IncomingMessage, OutgoingMessage};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

type Tx = mpsc::UnboundedSender<OutgoingMessage>;
type Rx = mpsc::UnboundedReceiver<OutgoingMessage>;

struct WorkspaceView {
    id: usize,
}

pub struct App {
    shared: Rc<RefCell<Shared>>,
}

struct Shared {
    next_workspace_id: usize,
    application_sender: Option<Tx>,
    window_senders: HashMap<usize, Tx>,
    workspace_views: HashMap<usize, WorkspaceView>,
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

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            match self.socket.poll() {
                Err(_) => {
                    eprintln!("Client error");
                    return Ok(Async::Ready(()))
                },
                Ok(Async::Ready(None)) => {
                    return Ok(Async::Ready(()))
                },
                Ok(Async::Ready(Some(message))) => {
                    match message {
                        IncomingMessage::StartApplication => self.start_application(),
                        IncomingMessage::OpenWorkspace { paths } => self.open_workspace(paths),
                        IncomingMessage::StartWindow { workspace_id } => self.start_window(workspace_id),
                    }?;
                },
                Ok(Async::NotReady) => {
                    break;
                },
            }            
        }
        

        if let Some(ref mut rx) = self.rx {
            loop {
                match rx.poll() {
                    Ok(Async::Ready(Some(v))) => {
                        self.socket.start_send(v);
                    },
                    _ => {
                        break;
                    },
                }        
            }
            self.socket.poll_complete();
        }

        Ok(Async::NotReady)
    }
}

impl<Socket> Client<Socket>
where
    Socket: Stream<Item = IncomingMessage> + Sink<SinkItem = OutgoingMessage>,
{
    fn start_application(&mut self) -> Poll<(), ()> {
        let mut shared = self.shared.borrow_mut();
        let (tx, rx) = mpsc::unbounded();
        shared.application_sender = Some(tx);
        self.rx = Some(rx);
        self.socket.start_send(OutgoingMessage::Acknowledge);
        self.socket.poll_complete();
        Ok(Async::NotReady)
    }

    fn start_window(&mut self, workspace_id: usize) -> Poll<(), ()> {
        let mut shared = self.shared.borrow_mut();
        let workspace_view = shared.workspace_views.get(&workspace_id).ok_or(())?;
        
        self.socket.start_send(OutgoingMessage::WindowState);
        self.socket.poll_complete();
        Ok(Async::NotReady)
    }

    fn open_workspace(&mut self, paths: Vec<PathBuf>) -> Poll<(), ()> {
        let mut shared = self.shared.borrow_mut();
        let workspace_id = shared.next_workspace_id;
        shared.next_workspace_id += 1;
        
        shared.workspace_views.insert(workspace_id, WorkspaceView {
            id: workspace_id
        });
        
        if let &mut Some(ref mut sender) = &mut shared.application_sender {
            sender.start_send(OutgoingMessage::OpenWindow{workspace_id});
            sender.poll_complete();
        }

        self.socket.start_send(OutgoingMessage::Acknowledge);
        self.socket.poll_complete();
        Ok(Async::Ready(()))
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
