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
    type Request: 'static + for<'a> Deserialize<'a>;
    type Response: 'static + Serialize;
    type Error: 'static + Serialize;

    fn state(&self, connection: &mut ConnectionToClient) -> Self::State;
    fn poll_update(&mut self, connection: &mut ConnectionToClient) -> Async<Option<Self::Update>>;
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
    fn poll_update(&mut self, connection: &mut ConnectionToClient) -> Async<Option<Vec<u8>>>;
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

#[derive(Serialize, Deserialize)]
enum RpcError {
    ServiceNotFound,
}

#[derive(Serialize, Deserialize)]
enum MessageToServer {
    Request {
        service_id: ServiceId,
        request_id: RequestId,
        payload: Vec<u8>,
    },
}

pub struct ConnectionToClient {
    next_id: ServiceId,
    services: HashMap<ServiceId, Rc<RefCell<RawBytesService>>>,
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
    client_states: HashMap<ServiceId, ServiceClientState>,
    incoming: Box<Stream<Item = Vec<u8>, Error = io::Error>>,
}

impl<T: Service> ServiceClient<T> {
    pub fn state(&self) -> T::State {
        let state = self.connection.upgrade().and_then(|connection| {
            let connection = connection.borrow();
            connection
                .client_states
                .get(&self.id)
                .map(|state| deserialize(&state.initial).unwrap())
        });

        match state {
            Some(state) => state,
            None => unimplemented!(),
        }
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
    ) -> Box<Future<Item = T::Response, Error = T::Error>> {
        unimplemented!()
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

    pub fn add_service<T: 'static + Service>(&mut self, service: T) -> ServiceId {
        let id = self.next_id;
        self.next_id += 1;
        self.services.insert(id, Rc::new(RefCell::new(service)));
        self.inserted.insert(id);
        id
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
                        if let Some(service) = self.services.get(&service_id).cloned() {
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
            if let Some(service) = self.services.get(id).cloned() {
                insertions.insert(*id, service.borrow().state(self));
            }
        }
        let mut updates: HashMap<ServiceId, Vec<Vec<u8>>> = HashMap::new();
        let service_ids = self.services.keys().cloned().collect::<Vec<ServiceId>>();
        for id in service_ids {
            let service = self.services.get(&id).unwrap().clone();
            let mut service_borrow = service.borrow_mut();
            loop {
                match service_borrow.poll_update(self) {
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
            Ok((Some(bytes), incoming)) => {
                let mut connection =
                    ConnectionToServer(Rc::new(RefCell::new(ConnectionToServerState {
                        client_states: HashMap::new(),
                        incoming: Box::new(incoming),
                    })));
                connection.update(deserialize(&bytes).unwrap()).map(|_| {
                    let bootstrap_client = connection.get_client(0).unwrap();
                    (connection, bootstrap_client)
                })
            }
            Ok((None, _)) => Err(format!("Connection was interrupted during handshake")),
            Err((error, _)) => Err(format!("{}", error)),
        }))
    }

    pub fn get_client<T: Service>(&self, id: ServiceId) -> Option<ServiceClient<T>> {
        self.0
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
                        connection: Rc::downgrade(&self.0),
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
                for (id, state) in insertions {
                    let (updates_tx, updates_rx) = unsync::mpsc::unbounded();
                    self.0.borrow_mut().client_states.insert(
                        id,
                        ServiceClientState {
                            has_client: false,
                            initial: state,
                            updates_tx,
                            updates_rx: Some(updates_rx),
                        },
                    );
                }

                if updates.len() > 0 {
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

                if removals.len() > 0 {
                    unimplemented!()
                }

                if responses.len() > 0 {
                    unimplemented!()
                }
                Ok(())
            }
            MessageToClient::Err(description) => Err(description),
        }
    }
}

impl Stream for ConnectionToServer {
    type Item = Vec<u8>;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        loop {
            let poll_result = self.0.borrow_mut().incoming.poll();
            match poll_result {
                Ok(Async::Ready(Some(bytes))) => match self.update(deserialize(&bytes).unwrap()) {
                    Ok(_) => continue,
                    Err(description) => eprintln!("Error occurred on server: {}", description),
                },
                Ok(Async::Ready(None)) => unimplemented!(),
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(error) => {
                    eprintln!("Error polling incoming connection: {}", error);
                    return Err(());
                }
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

    fn poll_update(&mut self, connection: &mut ConnectionToClient) -> Async<Option<Vec<u8>>> {
        T::poll_update(self, connection)
            .map(|option| option.map(|update| serialize(&update).unwrap()))
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
    extern crate tokio_core;

    use super::*;
    use futures::{Future, Sink};

    #[test]
    fn test_connection() {
        let mut reactor = tokio_core::reactor::Core::new().unwrap();
        let handle = reactor.handle();

        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let (client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();

        let service = TestService::new(42);
        let server = ConnectionToClient::new(
            client_to_server_rx.map_err(|_| unreachable!()),
            service.clone(),
        );
        handle.spawn(send_all(server_to_client_tx, server));

        let (client, service_client) = reactor
            .run(ConnectionToServer::new::<_, TestService>(
                server_to_client_rx.map_err(|_| unreachable!()),
            ))
            .unwrap();
        handle.spawn(send_all(client_to_server_tx, client));
        assert_eq!(service_client.state(), 42);

        service.increment_by(2);
        service.increment_by(4);
        service.increment_by(5);
        let client_updates = service_client.updates().unwrap();
        let (update, client_updates) = run(&mut reactor, client_updates.into_future());
        assert_eq!(update, Some(2));
        let (update, client_updates) = run(&mut reactor, client_updates.into_future());
        assert_eq!(update, Some(4));
        let (update, client_updates) = run(&mut reactor, client_updates.into_future());
        assert_eq!(update, Some(5));
    }

    fn run<F: Future>(reactor: &mut tokio_core::reactor::Core, future: F) -> F::Item {
        match reactor.run(future) {
            Ok(result) => result,
            Err(_) => panic!("Unexpected error"),
        }
    }

    fn send_all<I, S1, S2>(sink: S1, stream: S2) -> Box<Future<Item = (), Error = ()>>
    where
        S1: 'static + Sink<SinkItem = I>,
        S2: 'static + Stream<Item = I>,
    {
        Box::new(
            sink.send_all(stream.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        )
    }

    #[derive(Clone)]
    struct TestService(Rc<RefCell<TestServiceState>>);

    struct TestServiceState {
        count: usize,
        updates_tx: unsync::mpsc::UnboundedSender<usize>,
        updates_rx: unsync::mpsc::UnboundedReceiver<usize>,
    }

    impl TestService {
        fn new(count: usize) -> Self {
            let (updates_tx, updates_rx) = unsync::mpsc::unbounded();
            TestService(Rc::new(RefCell::new(TestServiceState {
                count,
                updates_tx,
                updates_rx,
            })))
        }

        fn increment_by(&self, count: usize) {
            let mut state = self.0.borrow_mut();
            state.count += count;
            state.updates_tx.unbounded_send(count).unwrap();
        }
    }

    impl Service for TestService {
        type State = usize;
        type Update = usize;
        type Request = ();
        type Response = ();
        type Error = String;

        fn state(&self, connection: &mut ConnectionToClient) -> Self::State {
            self.0.borrow().count
        }

        fn poll_update(&mut self, _: &mut ConnectionToClient) -> Async<Option<Self::Update>> {
            self.0.borrow_mut().updates_rx.poll().unwrap()
        }
    }
}
