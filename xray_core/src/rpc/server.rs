use super::messages::{MessageToClient, MessageToServer, RequestId, Response, ServiceId};
use super::Error;
use bincode::{deserialize, serialize};
use bytes::Bytes;
use futures::stream::FuturesUnordered;
use futures::task::{self, Task};
use futures::{future, Async, Future, Poll, Stream};
use never::Never;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::mem;
use std::rc::{Rc, Weak};

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
    fn init(&mut self, connection: &mut Connection) -> Bytes;
    fn poll_update(&mut self, connection: &mut Connection) -> Async<Option<Bytes>>;
    fn request(
        &mut self,
        request: Bytes,
        connection: &mut Connection,
    ) -> Option<Box<Future<Item = Bytes, Error = Never>>>;
}

#[derive(Clone)]
pub struct Connection(Rc<RefCell<ConnectionState>>);

struct ConnectionState {
    next_service_id: ServiceId,
    services: HashMap<ServiceId, Rc<RefCell<RawBytesService>>>,
    client_service_handles: HashMap<ServiceId, ServiceHandle>,
    inserted: HashSet<ServiceId>,
    removed: HashSet<ServiceId>,
    incoming: Box<Stream<Item = Bytes, Error = io::Error>>,
    pending_responses:
        Rc<RefCell<FuturesUnordered<Box<Future<Item = ResponseEnvelope, Error = Never>>>>>,
    pending_task: Option<Task>,
}

struct ServiceRegistration {
    pub service_id: ServiceId,
    connection: Weak<RefCell<ConnectionState>>,
}

#[derive(Clone)]
pub struct ServiceHandle(Rc<ServiceRegistration>);

struct ResponseEnvelope {
    service_id: ServiceId,
    request_id: RequestId,
    response: Response,
}

impl Connection {
    pub fn new<S, T>(incoming: S, root_service: T) -> Self
    where
        S: 'static + Stream<Item = Bytes, Error = io::Error>,
        T: 'static + Service,
    {
        let connection = Connection(Rc::new(RefCell::new(ConnectionState {
            next_service_id: 0,
            services: HashMap::new(),
            client_service_handles: HashMap::new(),
            inserted: HashSet::new(),
            removed: HashSet::new(),
            incoming: Box::new(incoming),
            pending_responses: Rc::new(RefCell::new(FuturesUnordered::new())),
            pending_task: None,
        })));
        connection.add_service(root_service);
        connection
    }

    pub fn add_service<T: 'static + Service>(&self, service: T) -> ServiceHandle {
        let mut state = self.0.borrow_mut();

        let service_id = state.next_service_id;
        state.next_service_id += 1;
        let service = Rc::new(RefCell::new(service));
        state.services.insert(service_id, service);
        state.inserted.insert(service_id);

        let handle = ServiceHandle(Rc::new(ServiceRegistration {
            connection: Rc::downgrade(&self.0),
            service_id,
        }));

        state
            .client_service_handles
            .insert(service_id, handle.clone());

        handle
    }

    fn poll_incoming(&mut self) -> Result<bool, io::Error> {
        loop {
            let poll = self.0.borrow_mut().incoming.poll();
            match poll {
                Ok(Async::Ready(Some(request))) => match deserialize(&request).unwrap() {
                    MessageToServer::Request {
                        request_id,
                        service_id,
                        payload,
                    } => {
                        let pending_responses = self.0.borrow().pending_responses.clone();

                        if let Some(service) = self.take_service(service_id) {
                            if let Some(response) = service.borrow_mut().request(payload, self) {
                                pending_responses.borrow_mut().push(Box::new(response.map(
                                    move |response| ResponseEnvelope {
                                        request_id,
                                        service_id,
                                        response: Ok(response),
                                    },
                                )));
                            }
                        } else {
                            pending_responses.borrow_mut().push(Box::new(future::ok(
                                ResponseEnvelope {
                                    request_id,
                                    service_id,
                                    response: Err(Error::ServiceNotFound),
                                },
                            )));
                        }
                    }
                    MessageToServer::DroppedService(service_id) => {
                        let service_handle = {
                            let mut state = self.0.borrow_mut();
                            state.client_service_handles.remove(&service_id)
                        };

                        if service_handle.is_none() {
                            eprintln!("Dropping unknown service with id {}", service_id);
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

    fn poll_outgoing(&mut self) -> Poll<Option<Bytes>, ()> {
        let existing_service_ids = {
            let state = self.0.borrow();
            state
                .services
                .keys()
                .cloned()
                .filter(|id| !state.inserted.contains(id))
                .collect::<Vec<ServiceId>>()
        };

        let mut updates: HashMap<ServiceId, Vec<Bytes>> = HashMap::new();
        for id in existing_service_ids {
            if let Some(service) = self.take_service(id) {
                while let Async::Ready(Some(update)) = service.borrow_mut().poll_update(self) {
                    updates.entry(id).or_insert(Vec::new()).push(update);
                }
            }
        }

        let mut insertions = HashMap::new();
        while self.0.borrow().inserted.len() > 0 {
            let inserted = mem::replace(&mut self.0.borrow_mut().inserted, HashSet::new());
            for id in inserted {
                if let Some(service) = self.take_service(id) {
                    let mut service = service.borrow_mut();
                    insertions.insert(id, service.init(self));
                    while let Async::Ready(Some(update)) = service.poll_update(self) {
                        updates.entry(id).or_insert(Vec::new()).push(update);
                    }
                }
            }
        }

        let mut removals = HashSet::new();
        mem::swap(&mut removals, &mut self.0.borrow_mut().removed);

        let pending_responses = self.0.borrow_mut().pending_responses.clone();
        let mut responses = HashMap::new();
        loop {
            match pending_responses.borrow_mut().poll() {
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
            }).unwrap().into();
            Ok(Async::Ready(Some(message)))
        } else {
            self.0.borrow_mut().pending_task = Some(task::current());
            Ok(Async::NotReady)
        }
    }

    fn take_service(&self, id: ServiceId) -> Option<Rc<RefCell<RawBytesService>>> {
        self.0.borrow_mut().services.get(&id).cloned()
    }
}

impl Stream for Connection {
    type Item = Bytes;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.poll_incoming() {
            Ok(true) => {}
            Ok(false) => return Ok(Async::Ready(None)),
            Err(error) => {
                let description = format!("{}", error);
                let message = serialize(&MessageToClient::Err(description)).unwrap();
                return Ok(Async::Ready(Some(message.into())));
            }
        }

        self.poll_outgoing()
    }
}

impl ServiceHandle {
    pub fn service_id(&self) -> ServiceId {
        self.0.service_id
    }
}

impl Drop for ServiceRegistration {
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
    fn init(&mut self, connection: &mut Connection) -> Bytes {
        serialize(&T::init(self, connection)).unwrap().into()
    }

    fn poll_update(&mut self, connection: &mut Connection) -> Async<Option<Bytes>> {
        T::poll_update(self, connection)
            .map(|option| option.map(|update| serialize(&update).unwrap().into()))
    }

    fn request(
        &mut self,
        request: Bytes,
        connection: &mut Connection,
    ) -> Option<Box<Future<Item = Bytes, Error = Never>>> {
        T::request(self, deserialize(&request).unwrap(), connection).map(|future| {
            Box::new(future.map(|item| serialize(&item).unwrap().into()))
                as Box<Future<Item = Bytes, Error = Never>>
        })
    }
}
