pub mod client;
mod messages;
pub mod server;

pub use self::messages::{ServiceId, Response};

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use futures::{future, unsync, Async, Future, Sink, Stream};
    use notify_cell::{NotifyCell, NotifyCellObserver};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use stream_ext::StreamExt;
    use tokio_core::reactor;

    #[test]
    fn test_connection() {
        let mut reactor = reactor::Core::new().unwrap();
        let model = TestModel::new(42);
        let client_1 = connect(&mut reactor, TestService::new(model.clone()));
        assert_eq!(client_1.state(), Some(42));

        model.increment_by(2);
        let client_2 = connect(&mut reactor, TestService::new(model.clone()));
        assert_eq!(client_2.state(), Some(44));

        model.increment_by(4);
        let mut client_1_updates = client_1.updates().unwrap();
        assert_eq!(client_1_updates.wait_next(&mut reactor), Some(44));
        assert_eq!(client_1_updates.wait_next(&mut reactor), Some(48));
        let mut client_2_updates = client_2.updates().unwrap();
        assert_eq!(client_2_updates.wait_next(&mut reactor), Some(48));

        let request_future = client_2.request(TestRequest::Increment(3));
        let response = reactor.run(request_future.unwrap()).unwrap();
        assert_eq!(response, TestServiceResponse::Ack);
        assert_eq!(client_1_updates.wait_next(&mut reactor), Some(51));
        assert_eq!(client_2_updates.wait_next(&mut reactor), Some(51));
    }

    #[test]
    fn test_add_remove_service() {
        let mut reactor = reactor::Core::new().unwrap();
        let model = TestModel::new(42);
        let client = connect(&mut reactor, TestService::new(model));

        let request_future = client.request(TestRequest::CreateService(12));
        let response = reactor.run(request_future.unwrap()).unwrap();
        assert_eq!(response, TestServiceResponse::ServiceCreated(1));
        let child_client = client.get_service::<TestService>(1).unwrap();
        assert_eq!(child_client.state(), Some(12));
        assert!(client.get_service::<TestService>(1).is_none());

        let request_future = client.request(TestRequest::DropService(1));
        let response = reactor.run(request_future.unwrap()).unwrap();
        assert_eq!(response, TestServiceResponse::Ack);
        assert!(child_client.state().is_none());
        assert!(child_client.updates().is_none());
        assert!(child_client.request(TestRequest::Increment(5)).is_none());

        drop(child_client);
        assert!(client.get_service::<TestService>(1).is_none());
    }

    #[test]
    fn test_drop_client() {
        let mut reactor = reactor::Core::new().unwrap();
        let model = TestModel::new(42);
        let root_client = connect(&mut reactor, TestService::new(model.clone()));
        reactor
            .run(root_client.request(TestRequest::CreateService(12)))
            .unwrap();

        assert!(root_client.get_service::<TestService>(1).is_some());
        assert!(root_client.get_service::<TestService>(1).is_none());
    }

    #[test]
    fn test_drop_client_updates() {
        let mut reactor = reactor::Core::new().unwrap();
        let model = TestModel::new(42);
        let root_client = connect(&mut reactor, TestService::new(model.clone()));

        let updates = root_client.updates();
        drop(updates);

        model.increment_by(3);
        reactor.turn(None);
    }

    #[test]
    fn test_interrupting_connection_to_client() {
        let (client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();
        let client_to_server_rx = client_to_server_rx.map_err(|_| unreachable!());
        let model = TestModel::new(42);
        let mut server = server::Connection::new(client_to_server_rx, TestService::new(model));
        drop(client_to_server_tx);
        assert_eq!(server.poll(), Ok(Async::Ready(None)));
    }

    #[test]
    fn test_interrupting_connection_to_server_during_handshake() {
        let mut reactor = reactor::Core::new().unwrap();
        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let server_to_client_rx = server_to_client_rx.map_err(|_| unreachable!());
        drop(server_to_client_tx);
        let client_future = client::Connection::new::<_, TestService>(server_to_client_rx);
        assert!(reactor.run(client_future).is_err());
    }

    #[test]
    fn test_interrupting_connection_to_server_after_handshake() {
        let mut reactor = reactor::Core::new().unwrap();

        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let server_to_client_rx = server_to_client_rx.map_err(|_| unreachable!());
        let (_client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();
        let client_to_server_rx = client_to_server_rx.map_err(|_| unreachable!());

        let model = TestModel::new(42);
        let server = server::Connection::new(client_to_server_rx, TestService::new(model));
        reactor.handle().spawn(
            server_to_client_tx
                .send_all(server.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );

        let client_future = client::Connection::new::<_, TestService>(server_to_client_rx);
        let (mut client, _) = reactor.run(client_future).unwrap();

        drop(reactor);
        assert_eq!(client.poll(), Ok(Async::Ready(None)));
    }

    pub fn connect<S: 'static + server::Service>(
        reactor: &mut reactor::Core,
        service: S,
    ) -> client::Service<S> {
        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let server_to_client_rx = server_to_client_rx.map_err(|_| unreachable!());
        let (client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();
        let client_to_server_rx = client_to_server_rx.map_err(|_| unreachable!());

        let server = server::Connection::new(client_to_server_rx, service);
        reactor.handle().spawn(
            server_to_client_tx
                .send_all(server.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );

        let client_future = client::Connection::new(server_to_client_rx);
        let (client, service_client) = reactor.run(client_future).unwrap();
        reactor.handle().spawn(
            client_to_server_tx
                .send_all(client.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );

        service_client
    }

    #[derive(Clone)]
    struct TestModel(Rc<RefCell<NotifyCell<usize>>>);

    struct TestService {
        model: TestModel,
        observer: NotifyCellObserver<usize>,
        child_services: HashMap<ServiceId, server::ServiceHandle>,
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
        fn new(model: TestModel) -> Self {
            let observer = model.0.borrow().observe();
            TestService {
                model,
                observer,
                child_services: HashMap::new(),
            }
        }
    }

    impl server::Service for TestService {
        type State = usize;
        type Update = usize;
        type Request = TestRequest;
        type Response = TestServiceResponse;

        fn state(&self, _: &server::Connection) -> Self::State {
            self.model.0.borrow().get()
        }

        fn poll_update(&mut self, _: &server::Connection) -> Async<Option<Self::Update>> {
            self.observer.poll().unwrap()
        }

        fn request(
            &mut self,
            request: Self::Request,
            connection: &server::Connection,
        ) -> Option<Box<Future<Item = Self::Response, Error = server::Never>>> {
            match request {
                TestRequest::Increment(count) => {
                    self.model.increment_by(count);
                    Some(Box::new(future::ok(TestServiceResponse::Ack)))
                }
                TestRequest::CreateService(initial_count) => {
                    let handle =
                        connection.add_service(TestService::new(TestModel::new(initial_count)));
                    let service_id = handle.service_id;
                    self.child_services.insert(service_id, handle);
                    Some(Box::new(future::ok(TestServiceResponse::ServiceCreated(
                        service_id,
                    ))))
                }
                TestRequest::DropService(id) => {
                    self.child_services.remove(&id);
                    Some(Box::new(future::ok(TestServiceResponse::Ack)))
                }
            }
        }
    }

    impl TestModel {
        fn new(count: usize) -> Self {
            TestModel(Rc::new(RefCell::new(NotifyCell::new(count))))
        }

        fn increment_by(&self, delta: usize) {
            let cell = self.0.borrow();
            cell.set(cell.get() + delta);
        }
    }
}
