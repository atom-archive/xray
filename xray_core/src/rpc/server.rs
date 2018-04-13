use super::messages::{MessageToClient, MessageToServer, RequestId, Response, ServiceId};
use super::Error;
use bincode::{deserialize, serialize};
use futures::stream::FuturesUnordered;
use futures::task::{self, Task};
use futures::{future, Async, Future, Poll, Stream};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::mem;
use std::rc::{Rc, Weak};

pub enum Never {}

pub trait Service {
    type State: 'static + Serialize + for<'a> Deserialize<'a>;
    type Update: 'static + Serialize + for<'a> Deserialize<'a>;
    type Request: 'static + Serialize + for<'a> Deserialize<'a>;
    type Response: 'static + Serialize + for<'a> Deserialize<'a>;

    fn init(&mut self, connection: &Connection) -> Self::State;
    fn poll_update(&mut self, connection: &Connection) -> Async<Option<Self::Update>>;
    fn request(
        &mut self,
        _request: Self::Request,
        _connection: &Connection,
    ) -> Option<Box<Future<Item = Self::Response, Error = Never>>> {
        None
    }
}

trait RawBytesService {
    fn init(&mut self, connection: &mut Connection) -> Vec<u8>;
    fn poll_update(&mut self, connection: &mut Connection) -> Async<Option<Vec<u8>>>;
    fn request(
        &mut self,
        request: Vec<u8>,
        connection: &mut Connection,
    ) -> Option<Box<Future<Item = Vec<u8>, Error = Never>>>;
}

pub struct Connection {
    state: Rc<RefCell<ConnectionState>>,
    root_service: Option<ServiceHandle>,
}

struct ConnectionState {
    next_id: ServiceId,
    services: HashMap<ServiceId, Rc<RefCell<RawBytesService>>>,
    inserted: HashSet<ServiceId>,
    removed: HashSet<ServiceId>,
    incoming: Box<Stream<Item = Vec<u8>, Error = io::Error>>,
    pending_responses: FuturesUnordered<Box<Future<Item = ResponseEnvelope, Error = Never>>>,
    pending_task: Option<Task>,
}

pub struct ServiceHandle {
    pub service_id: ServiceId,
    connection: Weak<RefCell<ConnectionState>>,
}

struct ResponseEnvelope {
    service_id: ServiceId,
    request_id: RequestId,
    response: Response,
}

impl Connection {
    pub fn new<S, T>(incoming: S, root_service: T) -> Self
    where
        S: 'static + Stream<Item = Vec<u8>, Error = io::Error>,
        T: 'static + Service,
    {
        let mut connection = Self {
            state: Rc::new(RefCell::new(ConnectionState {
                next_id: 0,
                services: HashMap::new(),
                inserted: HashSet::new(),
                removed: HashSet::new(),
                incoming: Box::new(incoming),
                pending_responses: FuturesUnordered::new(),
                pending_task: None,
            })),
            root_service: None,
        };
        connection.root_service = Some(connection.add_service(root_service));
        connection
    }

    pub fn add_service<T: 'static + Service>(&self, service: T) -> ServiceHandle {
        let mut state = self.state.borrow_mut();
        let id = state.next_id;
        state.next_id += 1;
        let service = Rc::new(RefCell::new(service));
        state.services.insert(id, service);
        state.inserted.insert(id);

        ServiceHandle {
            connection: Rc::downgrade(&self.state),
            service_id: id,
        }
    }

    fn poll_incoming(&mut self) -> Result<bool, io::Error> {
        loop {
            let poll = self.state.borrow_mut().incoming.poll();
            match poll {
                Ok(Async::Ready(Some(request))) => match deserialize(&request).unwrap() {
                    MessageToServer::Request {
                        request_id,
                        service_id,
                        payload,
                    } => {
                        if let Some(service) = self.get_service(service_id) {
                            if let Some(response) = service.borrow_mut().request(payload, self) {
                                self.state.borrow_mut().pending_responses.push(Box::new(
                                    response.map(move |response| ResponseEnvelope {
                                        request_id,
                                        service_id,
                                        response: Ok(response),
                                    }),
                                ));
                            }
                        } else {
                            self.state
                                .borrow_mut()
                                .pending_responses
                                .push(Box::new(future::ok(ResponseEnvelope {
                                    request_id,
                                    service_id,
                                    response: Err(Error::ServiceNotFound),
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
        let existing_service_ids = {
            let state = self.state.borrow();
            state
                .services
                .keys()
                .cloned()
                .filter(|id| !state.inserted.contains(id))
                .collect::<Vec<ServiceId>>()
        };

        let mut updates: HashMap<ServiceId, Vec<Vec<u8>>> = HashMap::new();
        for id in existing_service_ids {
            if let Some(service) = self.get_service(id) {
                while let Async::Ready(Some(update)) = service.borrow_mut().poll_update(self) {
                    updates.entry(id).or_insert(Vec::new()).push(update);
                }
            }
        }

        let mut insertions = HashMap::new();
        while self.state.borrow().inserted.len() > 0 {
            let inserted = mem::replace(&mut self.state.borrow_mut().inserted, HashSet::new());
            for id in inserted {
                if let Some(service) = self.get_service(id) {
                    let mut service = service.borrow_mut();
                    insertions.insert(id, service.init(self));
                    while let Async::Ready(Some(update)) = service.poll_update(self) {
                        updates.entry(id).or_insert(Vec::new()).push(update);
                    }
                }
            }
        }

        let mut state = self.state.borrow_mut();
        let mut removals = HashSet::new();
        mem::swap(&mut removals, &mut state.removed);

        let mut responses = HashMap::new();
        loop {
            match state.pending_responses.poll() {
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
            state.pending_task = Some(task::current());
            Ok(Async::NotReady)
        }
    }

    fn get_service(&self, id: ServiceId) -> Option<Rc<RefCell<RawBytesService>>> {
        self.state.borrow_mut().services.get(&id).cloned()
    }
}

impl Stream for Connection {
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

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.upgrade() {
            let mut connection = connection.borrow_mut();
            connection.services.remove(&self.service_id);
            connection.removed.insert(self.service_id);
            connection.pending_task.as_ref().map(|task| task.notify());
        }
    }
}

impl<T> RawBytesService for T
where
    T: Service,
{
    fn init(&mut self, connection: &mut Connection) -> Vec<u8> {
        serialize(&T::init(self, connection)).unwrap()
    }

    fn poll_update(&mut self, connection: &mut Connection) -> Async<Option<Vec<u8>>> {
        T::poll_update(self, connection)
            .map(|option| option.map(|update| serialize(&update).unwrap()))
    }

    fn request(
        &mut self,
        request: Vec<u8>,
        connection: &mut Connection,
    ) -> Option<Box<Future<Item = Vec<u8>, Error = Never>>> {
        T::request(self, deserialize(&request).unwrap(), connection).map(|future| {
            Box::new(future.map(|item| serialize(&item).unwrap()))
                as Box<Future<Item = Vec<u8>, Error = Never>>
        })
    }
}
