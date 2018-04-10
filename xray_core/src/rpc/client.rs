use super::messages::{MessageToClient, MessageToServer, RequestId, Response, ServiceId};
use super::server;
use bincode::{deserialize, serialize};
use futures::{unsync, Async, Future, Poll, Stream};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};

pub struct Service<T: server::Service> {
    id: ServiceId,
    connection: Weak<RefCell<ConnectionState>>,
    _marker: PhantomData<T>,
}

struct ServiceState {
    has_client: bool,
    initial: Vec<u8>,
    updates_rx: Option<unsync::mpsc::UnboundedReceiver<Vec<u8>>>,
    updates_tx: unsync::mpsc::UnboundedSender<Vec<u8>>,
    pending_requests: HashMap<RequestId, unsync::oneshot::Sender<Result<Vec<u8>, Vec<u8>>>>,
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
                outgoing_tx.unbounded_send(request).unwrap();

                Box::new(response_future) as Box<Future<Item = T::Response, Error = T::Error>>
            })
        })
    }

    pub fn get_service<S: server::Service>(&self, id: ServiceId) -> Option<Service<S>> {
        self.connection
            .upgrade()
            .and_then(|connection| Connection::service(&connection, id))
    }
}

impl<T: server::Service> Drop for Service<T> {
    fn drop(&mut self) {
        self.connection.upgrade().map(|connection| {
            let mut connection = connection.borrow_mut();
            connection.client_states.get_mut(&self.id).map(|state| {
                if state.updates_rx.is_none() {
                    let (updates_tx, updates_rx) = unsync::mpsc::unbounded();
                    state.updates_tx = updates_tx;
                    state.updates_rx = Some(updates_rx);
                }

                state.has_client = false;
            });
        });
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
            Ok((None, _)) => Err(format!("Connection was interrupted during handshake")),
            Err((error, _)) => Err(format!("{}", error)),
        }))
    }

    fn service<S: server::Service>(
        connection: &Rc<RefCell<ConnectionState>>,
        id: ServiceId,
    ) -> Option<Service<S>> {
        connection
            .borrow_mut()
            .client_states
            .get_mut(&id)
            .and_then(|state| {
                if state.has_client {
                    None
                } else {
                    state.has_client = true;
                    Some(Service {
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
                ServiceState {
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
                        service_state.updates_tx.unbounded_send(update).unwrap();
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
                                request_tx.send(Ok(payload)).unwrap();
                            }
                            Response::Err(payload) => {
                                request_tx.send(Err(payload)).unwrap();
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
