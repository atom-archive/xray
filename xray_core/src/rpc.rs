use bincode::{deserialize, serialize};
use futures::stream::FuturesUnordered;
use futures::task::{self, Task};
use futures::{future, unsync, Async, Future, Poll, Stream};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::marker::PhantomData;
use std::mem;
use std::rc::{Rc, Weak};

pub type RequestId = usize;
pub type ServiceId = usize;

pub trait Service {
    type State: 'static + Serialize + for<'a> Deserialize<'a>;
    type Update: 'static + Serialize + for<'a> Deserialize<'a>;
    type Request: 'static + Serialize + for<'a> Deserialize<'a>;
    type Response: 'static + Serialize + for<'a> Deserialize<'a>;
    type Error: 'static + Serialize + for<'a> Deserialize<'a>;

    fn state(&self, connection: &mut ConnectionToClient) -> Self::State;
    fn updates(
        &mut self,
        connection: &mut ConnectionToClient,
    ) -> Box<Stream<Item = Self::Update, Error = ()>>;
    fn request(
        &mut self,
        _request: Self::Request,
        _connection: &mut ConnectionToClient,
    ) -> Option<Box<Future<Item = Self::Response, Error = Self::Error>>> {
        None
    }
}

trait RawBytesService {
    fn state(&self, connection: &mut ConnectionToClient) -> Vec<u8>;
    fn request(
        &mut self,
        request: Vec<u8>,
        connection: &mut ConnectionToClient,
    ) -> Option<Box<Future<Item = Vec<u8>, Error = Vec<u8>>>>;
}

pub struct ServiceClient<T: Service> {
    id: ServiceId,
    connection: Weak<RefCell<ConnectionToServerState>>,
    _marker: PhantomData<T>,
}

struct ServiceClientState {
    has_client: bool,
    initial: Vec<u8>,
    updates_rx: Option<unsync::mpsc::UnboundedReceiver<Vec<u8>>>,
    updates_tx: unsync::mpsc::UnboundedSender<Vec<u8>>,
    pending_requests: HashMap<RequestId, unsync::oneshot::Sender<Result<Vec<u8>, Vec<u8>>>>,
}

#[derive(Serialize, Deserialize)]
enum MessageToClient {
    Update {
        insertions: HashMap<ServiceId, Vec<u8>>,
        updates: HashMap<ServiceId, Vec<Vec<u8>>>,
        removals: HashSet<ServiceId>,
        responses: HashMap<ServiceId, Vec<(RequestId, Response)>>,
    },
    Err(String),
}

#[derive(Serialize, Deserialize)]
enum Response {
    Ok(Vec<u8>),
    Err(Vec<u8>),
    RpcErr(RpcError),
}

#[derive(Debug, Serialize, Deserialize)]
enum RpcError {
    ServiceNotFound,
}

#[derive(Debug, Serialize, Deserialize)]
enum MessageToServer {
    Request {
        service_id: ServiceId,
        request_id: RequestId,
        payload: Vec<u8>,
    },
}

pub struct ConnectionToClient {
    next_id: ServiceId,
    services: HashMap<
        ServiceId,
        (
            Rc<RefCell<RawBytesService>>,
            Box<Stream<Item = Vec<u8>, Error = ()>>,
        ),
    >,
    inserted: HashSet<ServiceId>,
    removed: HashSet<ServiceId>,
    incoming: Box<Stream<Item = Vec<u8>, Error = io::Error>>,
    pending_responses: FuturesUnordered<Box<Future<Item = ResponseEnvelope, Error = ()>>>,
    pending_task: Option<Task>,
}

struct ResponseEnvelope {
    service_id: ServiceId,
    request_id: RequestId,
    response: Response,
}

pub struct ConnectionToServer(Rc<RefCell<ConnectionToServerState>>);

struct ConnectionToServerState {
    next_request_id: RequestId,
    client_states: HashMap<ServiceId, ServiceClientState>,
    incoming: Box<Stream<Item = Vec<u8>, Error = io::Error>>,
    outgoing_tx: unsync::mpsc::UnboundedSender<MessageToServer>,
    outgoing_rx: unsync::mpsc::UnboundedReceiver<MessageToServer>,
}

impl<T: Service> ServiceClient<T> {
    pub fn state(&self) -> Option<T::State> {
        let state = self.connection.upgrade().and_then(|connection| {
            let connection = connection.borrow();
            connection
                .client_states
                .get(&self.id)
                .map(|state| deserialize(&state.initial).unwrap())
        });

        state
    }

    pub fn updates(&self) -> Option<Box<Stream<Item = T::Update, Error = ()>>> {
        self.connection.upgrade().and_then(|connection| {
            let mut connection = connection.borrow_mut();
            let client_state = connection.client_states.get_mut(&self.id);
            client_state.and_then(|state| {
                state.updates_rx.take().map(|updates| {
                    let deserialized_updates = updates.map(|update| deserialize(&update).unwrap());
                    Box::new(deserialized_updates) as Box<Stream<Item = T::Update, Error = ()>>
                })
            })
        })
    }

    pub fn request(
        &self,
        request: T::Request,
    ) -> Option<Box<Future<Item = T::Response, Error = T::Error>>> {
        self.connection.upgrade().and_then(|connection| {
            let mut connection = connection.borrow_mut();
            let connection = &mut *connection;

            let request_id = connection.next_request_id;
            connection.next_request_id += 1;

            let outgoing_tx = &mut connection.outgoing_tx;
            connection.client_states.get_mut(&self.id).map(|state| {
                let (response_tx, response_rx) = unsync::oneshot::channel();
                state.pending_requests.insert(request_id, response_tx);
                let response_future =
                    response_rx.then(|raw_response| match raw_response.unwrap() {
                        Ok(payload) => Ok(deserialize(&payload).unwrap()),
                        Err(payload) => Err(deserialize(&payload).unwrap()),
                    });

                let request = MessageToServer::Request {
                    request_id,
                    service_id: self.id,
                    payload: serialize(&request).unwrap(),
                };
                outgoing_tx.unbounded_send(request);

                Box::new(response_future) as Box<Future<Item = T::Response, Error = T::Error>>
            })
        })
    }

    pub fn get_client<S: Service>(&self, id: ServiceId) -> Option<ServiceClient<S>> {
        self.connection
            .upgrade()
            .and_then(|connection| ConnectionToServer::service_client(&connection, id))
    }
}

impl ConnectionToClient {
    pub fn new<S, T>(incoming: S, bootstrap: T) -> Self
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
        T: 'static + Service,
    {
        let mut connection = Self {
            next_id: 0,
            services: HashMap::new(),
            inserted: HashSet::new(),
            removed: HashSet::new(),
            incoming: Box::new(incoming),
            pending_responses: FuturesUnordered::new(),
            pending_task: None,
        };
        connection.add_service(bootstrap);
        connection
    }

    pub fn add_service<T: 'static + Service>(&mut self, mut service: T) -> ServiceId {
        let id = self.next_id;
        self.next_id += 1;

        let service_updates = Box::new(
            service
                .updates(self)
                .map(|update| serialize(&update).unwrap()),
        );
        let service = Rc::new(RefCell::new(service));
        self.services.insert(id, (service, service_updates));
        self.inserted.insert(id);
        id
    }

    pub fn remove_service(&mut self, id: ServiceId) {
        self.services.remove(&id);
        self.removed.insert(id);
    }

    fn poll_incoming(&mut self) -> Result<bool, io::Error> {
        loop {
            match self.incoming.poll() {
                Ok(Async::Ready(Some(request))) => match deserialize(&request).unwrap() {
                    MessageToServer::Request {
                        request_id,
                        service_id,
                        payload,
                    } => {
                        if let Some(service) = self.services
                            .get(&service_id)
                            .map(|(service, _)| service.clone())
                        {
                            if let Some(response) = service.borrow_mut().request(payload, self) {
                                self.pending_responses.push(Box::new(response.then(
                                    move |response| {
                                        Ok(ResponseEnvelope {
                                            request_id,
                                            service_id,
                                            response: match response {
                                                Ok(payload) => Response::Ok(payload),
                                                Err(payload) => Response::Err(payload),
                                            },
                                        })
                                    },
                                )));
                            }
                        } else {
                            self.pending_responses
                                .push(Box::new(future::ok(ResponseEnvelope {
                                    request_id,
                                    service_id,
                                    response: Response::RpcErr(RpcError::ServiceNotFound),
                                })));
                        }
                    }
                },
                Ok(Async::Ready(None)) => return Ok(false),
                Ok(Async::NotReady) => return Ok(true),
                Err(error) => {
                    eprintln!("Error polling incoming connection: {}", error);
                    return Err(error);
                }
            }
        }
    }

    fn poll_outgoing(&mut self) -> Poll<Option<Vec<u8>>, ()> {
        let mut insertions = HashMap::new();
        let mut inserted = HashSet::new();
        mem::swap(&mut inserted, &mut self.inserted);
        for id in &inserted {
            if let Some((service, _)) = self.services.get(id) {
                insertions.insert(*id, service.borrow().state(self));
            }
        }
        let mut updates: HashMap<ServiceId, Vec<Vec<u8>>> = HashMap::new();
        let service_ids = self.services.keys().cloned().collect::<Vec<ServiceId>>();
        for id in service_ids {
            let (_, service_updates) = self.services.get_mut(&id).unwrap();
            loop {
                match service_updates.poll().unwrap() {
                    Async::Ready(Some(update)) => {
                        if !inserted.contains(&id) {
                            updates.entry(id).or_insert(Vec::new()).push(update);
                        }
                    }
                    Async::Ready(None) => unimplemented!("Terminate the service"),
                    Async::NotReady => break,
                }
            }
        }

        let mut removals = HashSet::new();
        mem::swap(&mut removals, &mut self.removed);

        let mut responses = HashMap::new();
        loop {
            match self.pending_responses.poll() {
                Ok(Async::Ready(Some(envelope))) => {
                    responses
                        .entry(envelope.service_id)
                        .or_insert(Vec::new())
                        .push((envelope.request_id, envelope.response));
                }
                Ok(Async::Ready(None)) | Ok(Async::NotReady) => break,
                Err(_) => unreachable!(),
            }
        }

        if insertions.len() > 0 || updates.len() > 0 || removals.len() > 0 || responses.len() > 0 {
            let message = serialize(&MessageToClient::Update {
                insertions,
                updates,
                removals,
                responses,
            }).unwrap();
            Ok(Async::Ready(Some(message)))
        } else {
            self.pending_task = Some(task::current());
            Ok(Async::NotReady)
        }
    }
}

impl Stream for ConnectionToClient {
    type Item = Vec<u8>;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.poll_incoming() {
            Ok(true) => {}
            Ok(false) => return Ok(Async::Ready(None)),
            Err(error) => {
                let description = format!("{}", error);
                let message = serialize(&MessageToClient::Err(description)).unwrap();
                return Ok(Async::Ready(Some(message)));
            }
        }

        self.poll_outgoing()
    }
}

impl ConnectionToServer {
    pub fn new<S, B>(incoming: S) -> Box<Future<Item = (Self, ServiceClient<B>), Error = String>>
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
        B: 'static + Service,
    {
        Box::new(incoming.into_future().then(|result| match result {
            Ok((Some(payload), incoming)) => {
                let (outgoing_tx, outgoing_rx) = unsync::mpsc::unbounded();
                let mut connection =
                    ConnectionToServer(Rc::new(RefCell::new(ConnectionToServerState {
                        next_request_id: 0,
                        client_states: HashMap::new(),
                        incoming: Box::new(incoming),
                        outgoing_tx,
                        outgoing_rx,
                    })));
                connection.update(deserialize(&payload).unwrap()).map(|_| {
                    let bootstrap_client = Self::service_client(&connection.0, 0).unwrap();
                    (connection, bootstrap_client)
                })
            }
            Ok((None, _)) => Err(format!("Connection was interrupted during handshake")),
            Err((error, _)) => Err(format!("{}", error)),
        }))
    }

    fn service_client<S: Service>(
        connection: &Rc<RefCell<ConnectionToServerState>>,
        id: ServiceId,
    ) -> Option<ServiceClient<S>> {
        connection
            .borrow_mut()
            .client_states
            .get_mut(&id)
            .and_then(|state| {
                if state.has_client {
                    None
                } else {
                    state.has_client = true;
                    Some(ServiceClient {
                        id,
                        connection: Rc::downgrade(&connection),
                        _marker: PhantomData,
                    })
                }
            })
    }

    fn update(&mut self, message: MessageToClient) -> Result<(), String> {
        match message {
            MessageToClient::Update {
                insertions,
                updates,
                removals,
                responses,
            } => {
                self.process_insertions(insertions);
                self.process_updates(updates);
                self.process_removals(removals);
                self.process_responses(responses);
                Ok(())
            }
            MessageToClient::Err(description) => Err(description),
        }
    }

    fn process_insertions(&self, insertions: HashMap<ServiceId, Vec<u8>>) {
        let mut connection = self.0.borrow_mut();
        for (id, state) in insertions {
            let (updates_tx, updates_rx) = unsync::mpsc::unbounded();
            connection.client_states.insert(
                id,
                ServiceClientState {
                    has_client: false,
                    initial: state,
                    updates_tx,
                    updates_rx: Some(updates_rx),
                    pending_requests: HashMap::new(),
                },
            );
        }
    }

    fn process_updates(&self, updates: HashMap<ServiceId, Vec<Vec<u8>>>) {
        let mut connection = self.0.borrow_mut();
        for (service_id, updates) in updates {
            connection
                .client_states
                .get_mut(&service_id)
                .map(|service_state| {
                    for update in updates {
                        service_state.updates_tx.unbounded_send(update);
                    }
                });
        }
    }

    fn process_removals(&self, removals: HashSet<ServiceId>) {
        let mut connection = self.0.borrow_mut();
        for id in removals {
            connection.client_states.remove(&id);
        }
    }

    fn process_responses(&self, responses: HashMap<ServiceId, Vec<(RequestId, Response)>>) {
        let mut connection = self.0.borrow_mut();
        for (service_id, responses) in responses {
            if let Some(state) = connection.client_states.get_mut(&service_id) {
                for (request_id, response) in responses {
                    let request_tx = state.pending_requests.remove(&request_id);
                    if let Some(request_tx) = request_tx {
                        match response {
                            Response::Ok(payload) => {
                                request_tx.send(Ok(payload));
                            }
                            Response::Err(payload) => {
                                request_tx.send(Err(payload));
                            }
                            Response::RpcErr(error) => {
                                eprintln!("Server error during RPC: {:?}", error);
                            }
                        }
                    } else {
                        eprintln!("Received response for unknown request {}", request_id);
                    }
                }
            }
        }
    }
}

impl Stream for ConnectionToServer {
    type Item = Vec<u8>;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        loop {
            let incoming_message = self.0.borrow_mut().incoming.poll();
            match incoming_message {
                Ok(Async::Ready(Some(payload))) => {
                    match self.update(deserialize(&payload).unwrap()) {
                        Ok(_) => continue,
                        Err(description) => eprintln!("Error occurred on server: {}", description),
                    }
                }
                Ok(Async::Ready(None)) => unimplemented!(),
                Ok(Async::NotReady) => break,
                Err(error) => {
                    eprintln!("Error polling incoming connection: {}", error);
                    return Err(());
                }
            }
        }

        match self.0.borrow_mut().outgoing_rx.poll() {
            Ok(Async::Ready(Some(message))) => {
                return Ok(Async::Ready(Some(serialize(&message).unwrap())))
            }
            Ok(Async::Ready(None)) => unreachable!(),
            Ok(Async::NotReady) => {}
            Err(_) => {
                eprintln!("Error polling outgoing messages");
                return Err(());
            }
        }

        Ok(Async::NotReady)
    }
}

impl<T> RawBytesService for T
where
    T: Service,
{
    fn state(&self, connection: &mut ConnectionToClient) -> Vec<u8> {
        serialize(&T::state(self, connection)).unwrap()
    }

    fn request(
        &mut self,
        request: Vec<u8>,
        connection: &mut ConnectionToClient,
    ) -> Option<Box<Future<Item = Vec<u8>, Error = Vec<u8>>>> {
        T::request(self, deserialize(&request).unwrap(), connection).map(|future| {
            Box::new(
                future
                    .map(|item| serialize(&item).unwrap())
                    .map_err(|err| serialize(&err).unwrap()),
            ) as Box<Future<Item = Vec<u8>, Error = Vec<u8>>>
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{future, Future, Sink};
    use std::fmt::Debug;
    use tokio_core::reactor;

    #[test]
    fn test_connection() {
        let mut reactor = reactor::Core::new().unwrap();
        let svc = TestService::new(42);
        let svc_client_1 = connect(&mut reactor, svc.clone());
        assert_eq!(svc_client_1.state(), Some(42));

        svc.increment_by(2);
        let svc_client_2 = connect(&mut reactor, svc.clone());
        assert_eq!(svc_client_2.state(), Some(42 + 2));

        svc.increment_by(4);
        let mut svc_client_1_updates = svc_client_1.updates().unwrap();
        assert_eq!(poll_wait(&mut reactor, &mut svc_client_1_updates), Some(2));
        assert_eq!(poll_wait(&mut reactor, &mut svc_client_1_updates), Some(4));
        let mut svc_client_2_updates = svc_client_2.updates().unwrap();
        assert_eq!(poll_wait(&mut reactor, &mut svc_client_2_updates), Some(4));

        let request_future = svc_client_2.request(TestRequest::Increment(3));
        let response = reactor.run(request_future.unwrap()).unwrap();
        assert_eq!(response, TestServiceResponse::Ack);
        assert_eq!(poll_wait(&mut reactor, &mut svc_client_1_updates), Some(3));
        assert_eq!(poll_wait(&mut reactor, &mut svc_client_2_updates), Some(3));
    }

    #[test]
    fn test_add_remove_service() {
        let mut reactor = reactor::Core::new().unwrap();
        let svc = TestService::new(42);
        let svc_client = connect(&mut reactor, svc);

        let request_future = svc_client.request(TestRequest::CreateService(12));
        let response = reactor.run(request_future.unwrap()).unwrap();
        assert_eq!(response, TestServiceResponse::ServiceCreated(1));
        let child_svc_client = svc_client.get_client::<TestService>(1).unwrap();
        assert_eq!(child_svc_client.state(), Some(12));
        assert!(svc_client.get_client::<TestService>(1).is_none());

        let request_future = svc_client.request(TestRequest::DropService(1));
        let response = reactor.run(request_future.unwrap()).unwrap();
        assert_eq!(response, TestServiceResponse::Ack);
        assert!(child_svc_client.state().is_none());
        assert!(child_svc_client.updates().is_none());
        assert!(
            child_svc_client
                .request(TestRequest::Increment(5))
                .is_none()
        );
    }

    fn connect<S: 'static + Service>(reactor: &mut reactor::Core, service: S) -> ServiceClient<S> {
        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let server_to_client_rx = server_to_client_rx.map_err(|_| unreachable!());
        let (client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();
        let client_to_server_rx = client_to_server_rx.map_err(|_| unreachable!());

        let server = ConnectionToClient::new(client_to_server_rx, service);
        reactor.handle().spawn(
            server_to_client_tx
                .send_all(server.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );

        let client_future = ConnectionToServer::new(server_to_client_rx);
        let (client, service_client) = reactor.run(client_future).unwrap();
        reactor.handle().spawn(
            client_to_server_tx
                .send_all(client.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );

        service_client
    }

    fn poll_wait<S: 'static + Stream>(
        reactor: &mut reactor::Core,
        stream: &mut S,
    ) -> Option<S::Item>
    where
        S::Item: Debug,
        S::Error: Debug,
    {
        struct TakeOne<'a, S: 'a>(&'a mut S);

        impl<'a, S: 'a + Stream> Future for TakeOne<'a, S> {
            type Item = Option<S::Item>;
            type Error = S::Error;

            fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
                self.0.poll()
            }
        }

        reactor.run(TakeOne(stream)).unwrap()
    }

    #[derive(Clone)]
    struct TestService(Rc<RefCell<TestServiceState>>);

    struct TestServiceState {
        count: usize,
        update_txs: Vec<unsync::mpsc::UnboundedSender<usize>>,
    }

    #[derive(Serialize, Deserialize)]
    enum TestRequest {
        Increment(usize),
        CreateService(usize),
        DropService(ServiceId),
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum TestServiceResponse {
        Ack,
        ServiceCreated(ServiceId),
    }

    impl TestService {
        fn new(count: usize) -> Self {
            TestService(Rc::new(RefCell::new(TestServiceState {
                count,
                update_txs: Vec::new(),
            })))
        }

        fn increment_by(&self, count: usize) {
            let mut state = self.0.borrow_mut();
            state.count += count;
            for updates_tx in &mut state.update_txs {
                updates_tx.unbounded_send(count).unwrap();
            }
        }
    }

    impl Service for TestService {
        type State = usize;
        type Update = usize;
        type Request = TestRequest;
        type Response = TestServiceResponse;
        type Error = String;

        fn state(&self, connection: &mut ConnectionToClient) -> Self::State {
            self.0.borrow().count
        }

        fn updates(
            &mut self,
            _: &mut ConnectionToClient,
        ) -> Box<Stream<Item = Self::Update, Error = ()>> {
            let (updates_tx, updates_rx) = unsync::mpsc::unbounded();
            let mut state = self.0.borrow_mut();
            state.update_txs.push(updates_tx);
            Box::new(updates_rx)
        }

        fn request(
            &mut self,
            request: Self::Request,
            connection: &mut ConnectionToClient,
        ) -> Option<Box<Future<Item = Self::Response, Error = Self::Error>>> {
            match request {
                TestRequest::Increment(count) => {
                    self.increment_by(count);
                    Some(Box::new(future::ok(TestServiceResponse::Ack)))
                }
                TestRequest::CreateService(initial_count) => {
                    let service_id = connection.add_service(TestService::new(initial_count));
                    Some(Box::new(future::ok(TestServiceResponse::ServiceCreated(
                        service_id,
                    ))))
                }
                TestRequest::DropService(id) => {
                    connection.remove_service(id);
                    Some(Box::new(future::ok(TestServiceResponse::Ack)))
                }
            }
        }
    }
}
