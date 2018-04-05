use bincode::{deserialize, serialize};
use bytes::{Bytes, BytesMut};
use futures::unsync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::{future, Future, Poll, Stream, Sink};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub type RequestId = usize;
pub type ServiceId = usize;
pub type Response = Option<Box<Future<Item = BytesMut, Error = BytesMut>>>;

pub trait Service {
    fn serialize(&self) -> BytesMut;
    fn poll(&mut self) -> Poll<Option<BytesMut>, BytesMut>;
    fn request(&mut self, request: Bytes) -> Response;
}

/// Represents a connection from the server to a client.
///
/// This object is instantiated on the server side when a client connects. It tracks a
/// collection of *services* which represent entities in the domain. When a new services is added, a
/// `serialize` of the service is sent to the client, along with all future updates returned from
/// `poll`. Client are routed to the `request` method, which can return and optional future with a
/// response to send back to the client.
struct ConnectionToClient {
    services: HashMap<ServiceId, Rc<RefCell<Service>>>,
    tx: UnboundedSender<MessageToClient>,
    outgoing: Box<Stream<Item = BytesMut, Error = ()>>,
}

/// Represents a connection from the client to a server on the client.
struct ConnectionToServer {}

pub struct ServiceClient {
    initial_state: BytesMut,
    updates_tx: UnboundedSender<BytesMut>,
    updates_rx: Option<UnboundedReceiver<BytesMut>>,
}

#[derive(Serialize, Deserialize)]
enum MessageToServer {
    Request {
        id: RequestId,
        service: ServiceId,
        payload: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
enum MessageToClient {
    NewService {
        id: ServiceId,
        payload: Vec<u8>,
    },
    ServiceUpdate {
        id: ServiceId,
        payload: Vec<u8>,
    },
    Response {
        request: RequestId,
        payload: Vec<u8>,
    },
}

impl ConnectionToClient {
    pub fn new<O, I>(outgoing: O, incoming: I) -> Self
    where
        O: Sink<SinkItem = BytesMut, SinkError = ()>,
        I: Stream<Item = BytesMut, Error = ()>,
    {
        unimplemented!()
    }
}

impl Stream for ConnectionToClient {
    type Item = BytesMut;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.outgoing.poll()
    }
}

impl ServiceClient {
    fn new(initial_state: BytesMut) -> Self {
        let (updates_tx, updates_rx) = mpsc::unbounded();

        Self {
            initial_state,
            updates_tx,
            updates_rx: Some(updates_rx),
        }
    }

    fn initial_state(&self) -> &[u8] {
        self.initial_state.as_ref()
    }

    fn updates(&mut self) -> Option<UnboundedReceiver<BytesMut>> {
        self.updates_rx.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // This test is feeling convoluted and I wonder if this stuff might be better tested
    // from the application code at a higher level.

    // #[test]
    // fn test_requests() {
    //     use notify_cell::{NotifyCell, NotifyCellObserver};
    //
    //     let service = TestService::new();
    //     let count = service.count.clone();
    //
    //     let (server_tx, server_rx) = mpsc::unbounded();
    //     let (client_tx, client_rx) = mpsc::unbounded();
    //     let server_to_client = ConnectionToClient::new(client_tx, server_rx, service);
    //     let client_to_server = ConnectionToServer::new(server_tx, client_rx);
    //
    //     let future = client_to_server
    //         .bootstrap()
    //         .and_then(|client| {
    //             assert_eq!(client.)
    //             client.request()
    //         });
    //
    //     struct TestService {
    //         count: Rc<NotifyCell<usize>>,
    //         count_observer: NotifyCellObserver<usize>,
    //     }
    //
    //     #[derive(Serialize, Deserialize)]
    //     struct TestState {
    //         count: usize,
    //     }
    //
    //     #[derive(Serialize, Deserialize)]
    //     enum TestRequest {u
    //         Add(usize),
    //     }
    //
    //     struct TestResponse {
    //         count: usize
    //     }
    //
    //     impl TestService {
    //         fn new() {
    //             TestService {
    //                 count: Rc::new(NotifyCell::new(0))
    //             }
    //         }
    //     }
    //
    //     impl Service for TestService {
    //         fn serialize(&self) -> BytesMut {
    //             let count = self.count.get();
    //             serialize(TestState { count })
    //         }
    //
    //         fn poll(&mut self) -> Poll<Option<BytesMut>, BytesMut> {
    //             self.count_observer.poll().map(|count| {
    //                 count.map(|count| {
    //                     serialize(TestState { count })
    //                 })
    //             })
    //         }
    //
    //         fn request(&mut self, request: Bytes) -> Response {
    //             let request: TestRequest = deserialize(request);
    //             match request {
    //                 TestRequest::Add(value) => {
    //                     self.count.set(self.count.get() + value);
    //                     futures::ok(serialize(TestResponse))
    //                 }
    //             }
    //         }
    //     }
    // }
}
