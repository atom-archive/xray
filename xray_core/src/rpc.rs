use bincode::{deserialize, serialize};
use bytes::Bytes;
use futures::task::{self, Task};
use futures::{Async, Future, Poll, Stream};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::rc::Rc;

pub type RequestId = usize;
pub type ServiceId = usize;
pub type Response<T, E> = Option<Box<Future<Item = T, Error = E>>>;

pub trait Service {
    type State: 'static + Serialize;
    type Update: 'static + Serialize;
    type Request: 'static + for<'a> Deserialize<'a>;
    type Response: 'static + Serialize;
    type Error: 'static + Serialize;

    fn state(&self, connection: &mut ConnectionToClient) -> Self::State;
    fn poll_updates(
        &mut self,
        connection: &mut ConnectionToClient,
    ) -> Async<Option<Vec<Self::Update>>>;
    fn request(
        &mut self,
        _request: Self::Request,
        _connection: &mut ConnectionToClient,
    ) -> Response<Self::Response, Self::Error> {
        None
    }
}

trait ErasedService {
    fn state(&self, connection: &mut ConnectionToClient) -> Vec<u8>;
    fn poll_updates(
        &mut self,
        connection: &mut ConnectionToClient,
    ) -> Async<Option<Vec<Vec<u8>>>>;
    fn request(
        &mut self,
        request: Bytes,
        connection: &mut ConnectionToClient,
    ) -> Option<Box<Future<Item = Vec<u8>, Error = Vec<u8>>>>;
}

#[derive(Serialize, Deserialize)]
struct MessageToClient {
    insertions: HashMap<ServiceId, Vec<u8>>,
    updates: HashMap<ServiceId, Vec<Vec<u8>>>,
    removals: HashSet<ServiceId>,
    responses: Vec<(RequestId, Vec<u8>)>,
}

#[derive(Serialize, Deserialize)]
enum MessageToServer {
    Request {
        id: RequestId,
        service: ServiceId,
        payload: Vec<u8>,
    },
}

pub struct ConnectionToClient {
    next_id: ServiceId,
    services: HashMap<ServiceId, Rc<RefCell<ErasedService>>>,
    inserted: HashSet<ServiceId>,
    removed: HashSet<ServiceId>,
    pending_task: Option<Task>,
}

impl ConnectionToClient {
    pub fn new<T: 'static + Service>(bootstrap: T) -> Self {
        let mut connection = Self {
            next_id: 0,
            services: HashMap::new(),
            inserted: HashSet::new(),
            removed: HashSet::new(),
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
}

impl Stream for ConnectionToClient {
    type Item = Vec<u8>;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
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
            match service_borrow.poll_updates(self) {
                Async::Ready(Some(service_updates)) => {
                    if !inserted.contains(&id) {
                        updates.insert(id, service_updates);
                    }
                },
                Async::Ready(None) => {
                    // TODO: Terminate service
                },
                Async::NotReady => {}
            }
        }

        let mut removals = HashSet::new();
        mem::swap(&mut removals, &mut self.removed);

        if insertions.len() > 0 || updates.len() > 0 || removals.len() > 0 {
            let message = serialize(&MessageToClient {
                insertions,
                updates,
                removals,
                responses, // Need to queue up pending responses
            }).unwrap();
            Ok(Async::Ready(Some(message)))
        } else {
            self.pending_task = task::current();
            Ok(Async::NotReady)
        }
    }
}

impl<T> ErasedService for T
where
    T: Service,
{
    fn state(&self, connection: &mut ConnectionToClient) -> Vec<u8> {
        serialize(&T::state(self, connection)).unwrap()
    }

    fn poll_updates(
        &mut self,
        connection: &mut ConnectionToClient,
    ) -> Async<Option<Vec<Vec<u8>>>> {
        T::poll_updates(self, connection).map(|option| {
            option.map(|updates| {
                updates
                    .into_iter()
                    .map(|update| serialize(&update).unwrap())
                    .collect()
            })
        })
    }

    fn request(
        &mut self,
        request: Bytes,
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
