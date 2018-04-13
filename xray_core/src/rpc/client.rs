use super::messages::{MessageToClient, MessageToServer, RequestId, Response, ServiceId};
use super::{server, Error};
use bincode::{deserialize, serialize};
use futures::{self, stream, unsync, Async, Future, Poll, Stream};
use serde::{Deserialize, Serialize};
use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};

pub struct Service<T: server::Service> {
    id: ServiceId,
    connection: Weak<RefCell<ConnectionState>>,
    _marker: PhantomData<T>,
}

pub struct FullUpdateService<T: server::Service> {
    latest_state: Rc<RefCell<Result<T::State, Error>>>,
    service: Service<T>,
}

struct ServiceState {
    has_client: bool,
    state: Vec<u8>,
    updates_rx: Option<unsync::mpsc::UnboundedReceiver<Vec<u8>>>,
    updates_tx: unsync::mpsc::UnboundedSender<Vec<u8>>,
    pending_requests: HashMap<RequestId, unsync::oneshot::Sender<Result<Vec<u8>, Error>>>,
}

pub struct Connection(Rc<RefCell<ConnectionState>>);

struct ConnectionState {
    next_request_id: RequestId,
    client_states: HashMap<ServiceId, ServiceState>,
    incoming: Box<Stream<Item = Vec<u8>, Error = io::Error>>,
    outgoing_tx: unsync::mpsc::UnboundedSender<MessageToServer>,
    outgoing_rx: unsync::mpsc::UnboundedReceiver<MessageToServer>,
}

impl<T: server::Service> Service<T> {
    pub fn state(&self) -> Result<T::State, Error> {
        let connection = self.connection.upgrade().ok_or(Error::ConnectionDropped)?;
        let connection = connection.borrow();
        let client_state = connection
            .client_states
            .get(&self.id)
            .ok_or(Error::ServiceDropped)?;
        Ok(deserialize(&client_state.state).unwrap())
    }

    pub fn updates(&self) -> Result<Box<Stream<Item = T::Update, Error = ()>>, Error> {
        let connection = self.connection.upgrade().ok_or(Error::ConnectionDropped)?;
        let mut connection = connection.borrow_mut();
        let client_state = connection
            .client_states
            .get_mut(&self.id)
            .ok_or(Error::ServiceDropped)?;
        let updates = client_state.updates_rx.take().ok_or(Error::UpdatesTaken)?;
        let deserialized_updates = updates.map(|update| deserialize(&update).unwrap());
        Ok(Box::new(deserialized_updates))
    }

    pub fn request(
        &self,
        request: T::Request,
    ) -> Result<Box<Future<Item = T::Response, Error = Error>>, Error> {
        let connection = self.connection.upgrade().ok_or(Error::ConnectionDropped)?;
        let mut connection = connection.borrow_mut();

        let request_id = connection.next_request_id;
        connection.next_request_id += 1;

        let (response_tx, response_rx) = unsync::oneshot::channel();
        connection
            .client_states
            .get_mut(&self.id)
            .ok_or(Error::ServiceDropped)?
            .pending_requests
            .insert(request_id, response_tx);
        let response_future = response_rx
            .map_err(|_: futures::Canceled| Error::ServiceDropped)
            .and_then(|response| response.map(|payload| deserialize(&payload).unwrap()));

        let request = MessageToServer::Request {
            request_id,
            service_id: self.id,
            payload: serialize(&request).unwrap(),
        };
        connection.outgoing_tx.unbounded_send(request).unwrap();

        Ok(Box::new(response_future))
    }

    pub fn get_service<S: server::Service>(&self, id: ServiceId) -> Result<Service<S>, Error> {
        let connection = self.connection.upgrade().ok_or(Error::ConnectionDropped)?;
        Ok(Connection::service(&connection, id)?)
    }
}

impl<T, S> FullUpdateService<S>
where
    T: 'static + Serialize + for<'a> Deserialize<'a>,
    S: server::Service<State = T, Update = T>,
{
    pub fn new(service: Service<S>) -> Self {
        FullUpdateService {
            latest_state: Rc::new(RefCell::new(service.state())),
            service,
        }
    }

    pub fn latest_state(&self) -> Result<Ref<T>, Error> {
        let state = self.latest_state.borrow();
        if state.is_ok() {
            Ok(Ref::map(state, |state| state.as_ref().unwrap()))
        } else {
            Err(state.as_ref().err().unwrap().clone())
        }
    }

    pub fn updates(&self) -> Result<Box<Stream<Item = (), Error = ()>>, Error> {
        let latest_state_1 = self.latest_state.clone();
        let latest_state_2 = self.latest_state.clone();
        self.service.updates().map(|updates| {
            let update_latest_state = updates.map(move |update| {
                *latest_state_1.borrow_mut() = Ok(update);
            });
            let clear_latest_state = stream::once(Ok(())).map(move |_| {
                *latest_state_2.borrow_mut() = Err(Error::ServiceDropped);
            });
            Box::new(update_latest_state.chain(clear_latest_state))
                as Box<Stream<Item = (), Error = ()>>
        })
    }

    pub fn request(
        &self,
        request: S::Request,
    ) -> Result<Box<Future<Item = S::Response, Error = Error>>, Error> {
        self.service.request(request)
    }

    pub fn get_service<S2: server::Service>(&self, id: ServiceId) -> Result<Service<S2>, Error> {
        self.service.get_service(id)
    }
}

impl Connection {
    pub fn new<S, B>(incoming: S) -> Box<Future<Item = (Self, Service<B>), Error = String>>
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
        B: 'static + server::Service,
    {
        Box::new(incoming.into_future().then(|result| match result {
            Ok((Some(payload), incoming)) => {
                let (outgoing_tx, outgoing_rx) = unsync::mpsc::unbounded();
                let mut connection = Connection(Rc::new(RefCell::new(ConnectionState {
                    next_request_id: 0,
                    client_states: HashMap::new(),
                    incoming: Box::new(incoming),
                    outgoing_tx,
                    outgoing_rx,
                })));
                connection.update(deserialize(&payload).unwrap()).map(|_| {
                    let root_service = Self::service(&connection.0, 0).unwrap();
                    (connection, root_service)
                })
            }
            Ok((None, _)) => Err("Connection was interrupted during handshake".to_string()),
            Err((error, _)) => Err(format!("{}", error)),
        }))
    }

    fn service<S: server::Service>(
        connection: &Rc<RefCell<ConnectionState>>,
        id: ServiceId,
    ) -> Result<Service<S>, Error> {
        let mut connection_state = connection.borrow_mut();
        let service_state = connection_state
            .client_states
            .get_mut(&id)
            .ok_or(Error::ServiceNotFound)?;

        if service_state.has_client {
            Err(Error::ServiceTaken)
        } else {
            service_state.has_client = true;
            Ok(Service {
                id,
                connection: Rc::downgrade(&connection),
                _marker: PhantomData,
            })
        }
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
                ServiceState {
                    has_client: false,
                    state,
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
                        let _ = service_state.updates_tx.unbounded_send(update);
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
                            Ok(payload) => {
                                request_tx.send(Ok(payload)).unwrap();
                            }
                            Err(error) => {
                                request_tx.send(Err(error)).unwrap();
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

impl Stream for Connection {
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
                Ok(Async::Ready(None)) => return Ok(Async::Ready(None)),
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
