use bytes::{Bytes, BytesMut};
use futures::unsync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::{Future, Poll, Stream};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub type RequestId = usize;
pub type ServiceId = usize;
pub type Response = Option<Box<Future<Item = BytesMut, Error = BytesMut>>>;

pub trait Service {
    fn snapshot(&self) -> BytesMut;
    fn poll(&mut self) -> Poll<Option<BytesMut>, BytesMut>;
    fn request(&mut self, request: Bytes) -> Response;
}

enum MessageToServer {
    Request {
        id: RequestId,
        service: ServiceId,
        payload: BytesMut
    }
}

enum MessageToClient {
    NewService {
        id: ServiceId,
        payload: BytesMut
    },
    ServiceUpdate {
        id: ServiceId,
        payload: BytesMut
    },
    Response {
        request: RequestId,
        payload: BytesMut
    }
}

/// Represents a connection from the server to a client.
///
/// This object is instantiated on the server side when a client connects. It tracks a
/// collection of *services* which represent entities in the domain. When a new services is added, a
/// `snapshot` of the service is sent to the client, along with all future updates returned from
/// `poll`. Client are routed to the `request` method, which can return and optional future with a
/// response to send back to the client.
struct ConnectionToClient {
    services: HashMap<ServiceId, Rc<RefCell<Service>>>,
    tx: UnboundedSender<MessageToClient>,
    outgoing: Box<Stream<Item = BytesMut, Error = ()>>
}

/// Represents a connection from the client to a server on the client.
struct ConnectionToServer {}

pub struct ServiceClient {
    snapshot: BytesMut,
    updates_tx: UnboundedSender<BytesMut>,
    updates_rx: Option<UnboundedReceiver<BytesMut>>,
}

impl ConnectionToClient {
    pub fn new<I>(incoming: I) -> Self
    where
        I: Stream<Item = BytesMut, Error = ()>,
    {
        incoming.map(|request| {

        });

        let (tx, rx) = mpsc::unbounded();
        let outgoing = rx.select(other)
    }
}

impl Stream for ConnectionToClient {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {}
}

impl ServiceClient {
    fn new(snapshot: BytesMut) -> Self {
        let (updates_tx, updates_rx) = mpsc::unbounded();

        Self {
            snapshot,
            updates_tx,
            updates_rx: Some(updates_rx),
        }
    }

    fn updates(&mut self) -> Option<UnboundedReceiver<BytesMut>> {
        self.updates_rx.take()
    }
}
