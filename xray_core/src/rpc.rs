use serde::{Deserialize, Serialize};
use bincode::{deserialize, serialize};
use bytes::{Bytes, BytesMut};
use futures::unsync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::{future, Future, Poll, Stream, Sink};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::marker::PhantomData;

pub type RequestId = usize;
pub type ServiceId = usize;
pub type Response<T, E> = Option<Box<Future<Item = T, Error = E>>>;

pub trait Service {
    type State: 'static + Serialize;
    type Update: 'static + Serialize;
    type Request: 'static + for<'a> Deserialize<'a>;
    type Response: 'static + Serialize;
    type Error: 'static + Serialize;

    // fn init(&mut self, handle: ServiceHandle<Self>) where Self: Sized;
    fn state(&self) -> Self::State;
    fn request(&mut self, request: Self::Request) -> Response<Self::Response, Self::Error>;
}

trait ErasedServiceWrapper {
    fn state(&self) -> Vec<u8>;
    fn request(&mut self, request: Bytes) -> Option<Box<Future<Item = Vec<u8>, Error = Vec<u8>>>>;
}

struct ConnectionToClient(Rc<RefCell<ConnectionToClientState>>);

struct ConnectionToClientState {
    services: HashMap<ServiceId, Weak<ErasedServiceWrapper>>
}

struct ServiceWrapper<T: Service> {
    id: ServiceId,
    service: T,
    connection: Weak<RefCell<ConnectionToClientState>>,
}

pub struct ServiceHandle<T: Service>(Rc<ServiceWrapper<T>>);

impl<T: Service> ServiceWrapper<T> {
    fn new(id: ServiceId, service: T, connection: Weak<RefCell<ConnectionToClientState>>) -> Self {
        ServiceWrapper { id, service, connection }
    }
}

impl<T> ErasedServiceWrapper for ServiceWrapper<T> where T: Service {
    fn state(&self) -> Vec<u8> {
        serialize(&self.service.state()).unwrap()
    }

    fn request(&mut self, request: Bytes) -> Option<Box<Future<Item = Vec<u8>, Error = Vec<u8>>>> {
        self.service
            .request(deserialize(&request).unwrap())
            .map(|future| {
                Box::new(
                    future
                        .map(|item| serialize(&item).unwrap())
                        .map_err(|err| serialize(&err).unwrap())
                ) as Box<Future<Item = Vec<u8>, Error = Vec<u8>>>
            })
    }
}

impl<T: Service> Drop for ServiceWrapper<T> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.upgrade() {
            connection.borrow_mut().services.remove(&self.id);
        }
    }
}

impl<T, T1, T2, T3, T4, T5> ServiceHandle<T>
where
    T: Service<State=T1, Update=T2, Request=T3, Response=T4, Error=T5>,
    T1: 'static + Serialize,
    T2: 'static + Serialize,
    T3: 'static + for<'a> Deserialize<'a>,
    T4: 'static + Serialize,
    T5: 'static + Serialize,
{
    fn send_update(&mut self, update: T::Update) {

    }
}

impl<T, T1, T2, T3, T4> ServiceHandle<T>
where
    T: Service<State=T1, Update=T1, Request=T2, Response=T3, Error=T4>,
    T1: 'static + Serialize,
    T2: 'static + for<'a> Deserialize<'a>,
    T3: 'static + Serialize,
    T4: 'static + Serialize,
{
    fn set_updated(&mut self) {

    }
}




// /// Represents a connection from the server to a client.
// ///
// /// This object is instantiated on the server side when a client connects. It tracks a
// /// collection of *services* which represent entities in the domain. When a new services is added, a
// /// `serialize` of the service is sent to the client, along with all future updates returned from
// /// `poll`. Client are routed to the `request` method, which can return and optional future with a
// /// response to send back to the client.
// struct ConnectionToClient {
//     services: HashMap<ServiceId, Rc<RefCell<Service>>>,
//     tx: UnboundedSender<MessageToClient>,
//     outgoing: Box<Stream<Item = BytesMut, Error = ()>>,
// }
//
// /// Represents a connection from the client to a server on the client.
// struct ConnectionToServer {}
//
// pub struct ServiceClient {
//     initial_state: BytesMut,
//     updates_tx: UnboundedSender<BytesMut>,
//     updates_rx: Option<UnboundedReceiver<BytesMut>>,
// }
//
// #[derive(Serialize, Deserialize)]
// enum MessageToServer {
//     Request {
//         id: RequestId,
//         service: ServiceId,
//         payload: Vec<u8>,
//     },
// }
//
// #[derive(Serialize, Deserialize)]
// enum MessageToClient {
//     NewService {
//         id: ServiceId,
//         payload: Vec<u8>,
//     },
//     ServiceUpdate {
//         id: ServiceId,
//         payload: Vec<u8>,
//     },
//     Response {
//         request: RequestId,
//         payload: Vec<u8>,
//     },
// }
//
// impl ConnectionToClient {
//     pub fn new<O, I>(outgoing: O, incoming: I) -> Self
//     where
//         O: Sink<SinkItem = BytesMut, SinkError = ()>,
//         I: Stream<Item = BytesMut, Error = ()>,
//     {
//         unimplemented!()
//     }
// }
//
// impl Stream for ConnectionToClient {
//     type Item = BytesMut;
//     type Error = ();
//
//     fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
//         self.outgoing.poll()
//     }
// }
//
// impl ServiceClient {
//     fn new(initial_state: BytesMut) -> Self {
//         let (updates_tx, updates_rx) = mpsc::unbounded();
//
//         Self {
//             initial_state,
//             updates_tx,
//             updates_rx: Some(updates_rx),
//         }
//     }
//
//     fn initial_state(&self) -> &[u8] {
//         self.initial_state.as_ref()
//     }
//
//     fn updates(&mut self) -> Option<UnboundedReceiver<BytesMut>> {
//         self.updates_rx.take()
//     }
// }
//
// mod foo {
//     use bytes::Bytes;
//     use futures::{Async, Poll, Stream};
//     use std::cell::{Ref, RefCell, RefMut};
//     use std::rc::Rc;
//
//     struct RemoteProject;
//
//     struct RemoteWorkspace {
//         project: Option<ServiceHandle<RemoteProject>>,
//         gateway: ServiceGateway
//     }
//
//     trait ServiceClient where Self: Sized {
//         fn deserialize(initial_state: Bytes, _: ServiceGateway, _: &mut ConnectionToServer) -> Self {
//             unimplemented!("`deserialize` must be implemented. initial_state was {:?}", initial_state)
//         }
//
//         fn update(&mut self, message: Bytes, _: &mut ConnectionToServer) {
//             unimplemented!("`update` must be implemented. message was {:?}", message)
//         }
//     }
//
//     struct ServiceHandle<T: ServiceClient> {
//         service: Rc<RefCell<T>>
//     }
//
//     type ServiceId = usize;
//     struct ServiceGateway;
//     struct ConnectionToServer;
//     struct Response;
//
//     impl<T: ServiceClient> ServiceHandle<T> {
//         fn as_ref<'a>(&'a self) -> Ref<'a, T> {
//             self.service.borrow()
//         }
//
//         fn as_mut<'a>(&'a mut self) -> RefMut<'a, T> {
//             self.service.borrow_mut()
//         }
//     }
//
//     impl ConnectionToServer {
//         fn get_service<T: ServiceClient>(&self, id: ServiceId) -> ServiceHandle<T> {
//             unimplemented!()
//         }
//     }
//
//     impl Stream for Response {
//         type Item = Bytes;
//         type Error = ();
//
//         fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
//             unimplemented!()
//         }
//     }
//
//     impl Drop for Response {
//         fn drop(&mut self) {
//             // Dropping the Response stream should cause the server to stop pushing new content.
//         }
//     }
//
//     impl ServiceGateway {
//         fn request(&self, body: Bytes) -> Response {
//             unimplemented!()
//         }
//     }
//
//     impl ServiceClient for RemoteWorkspace {
//         fn deserialize(initial_state: Bytes, gateway: ServiceGateway, _: &mut ConnectionToServer) -> Self {
//             Self {
//                 project: None,
//                 gateway
//             }
//         }
//
//         fn update(&mut self, message: Bytes, conn: &mut ConnectionToServer) {
//             // Assume that we're able to deserialize message and get a project_id back.
//             // let project_id = deserialize(message);
//             let project_id = 42;
//             let project_handle: ServiceHandle<RemoteProject> = conn.get_service(project_id);
//             self.project = Some(project_handle);
//         }
//     }
//
//     // This will eventually be impl Project for RemoteProject
//     impl RemoteProject {
//         fn search_files(&self, query: &str) {
//             let body = Bytes::from([1, 2, 3].as_ref());
//             // self.search = self.gateway.request(body).for_each(|response| {
//             //     search_results.push(deserialize(response));
//             //     updates.set(());
//             // });
//         }
//     }
//
//     impl ServiceClient for RemoteProject {}
// }
//
// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     // This test is feeling convoluted and I wonder if this stuff might be better tested
//     // from the application code at a higher level.
//
//     // #[test]
//     // fn test_requests() {
//     //     use notify_cell::{NotifyCell, NotifyCellObserver};
//     //
//     //     let service = TestService::new();
//     //     let count = service.count.clone();
//     //
//     //     let (server_tx, server_rx) = mpsc::unbounded();
//     //     let (client_tx, client_rx) = mpsc::unbounded();
//     //     let server_to_client = ConnectionToClient::new(client_tx, server_rx, service);
//     //     let client_to_server = ConnectionToServer::new(server_tx, client_rx);
//     //
//     //     let future = client_to_server
//     //         .bootstrap()
//     //         .and_then(|client| {
//     //             assert_eq!(client.)
//     //             client.request()
//     //         });
//     //
//     //     struct TestService {
//     //         count: Rc<NotifyCell<usize>>,
//     //         count_observer: NotifyCellObserver<usize>,
//     //     }
//     //
//     //     #[derive(Serialize, Deserialize)]
//     //     struct TestState {
//     //         count: usize,
//     //     }
//     //
//     //     #[derive(Serialize, Deserialize)]
//     //     enum TestRequest {u
//     //         Add(usize),
//     //     }
//     //
//     //     struct TestResponse {
//     //         count: usize
//     //     }
//     //
//     //     impl TestService {
//     //         fn new() {
//     //             TestService {
//     //                 count: Rc::new(NotifyCell::new(0))
//     //             }
//     //         }
//     //     }
//     //
//     //     impl Service for TestService {
//     //         fn serialize(&self) -> BytesMut {
//     //             let count = self.count.get();
//     //             serialize(TestState { count })
//     //         }
//     //
//     //         fn poll(&mut self) -> Poll<Option<BytesMut>, BytesMut> {
//     //             self.count_observer.poll().map(|count| {
//     //                 count.map(|count| {
//     //                     serialize(TestState { count })
//     //                 })
//     //             })
//     //         }
//     //
//     //         fn request(&mut self, request: Bytes) -> Response {
//     //             let request: TestRequest = deserialize(request);
//     //             match request {
//     //                 TestRequest::Add(value) => {
//     //                     self.count.set(self.count.get() + value);
//     //                     futures::ok(serialize(TestResponse))
//     //                 }
//     //             }
//     //         }
//     //     }
//     // }
// }
