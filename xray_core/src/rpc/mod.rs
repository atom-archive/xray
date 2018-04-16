pub mod client;
mod messages;
pub mod server;

pub use self::messages::{Response, ServiceId};
use never::Never;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Error {
    ConnectionDropped,
    ServiceDropped,
    ServiceNotFound,
    ServiceTaken,
    UpdatesTaken,
}

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
        let model = Rc::new(TestModel::new(42));
        let client_1 = connect(&mut reactor, TestService::new(model.clone()));
        assert_eq!(client_1.state(), Ok(42));

        model.increment_by(2);
        let client_2 = connect(&mut reactor, TestService::new(model.clone()));
        assert_eq!(client_2.state(), Ok(44));

        model.increment_by(4);
        let mut client_1_updates = client_1.updates().unwrap();
        assert_eq!(client_1_updates.wait_next(&mut reactor), Some(44));
        assert_eq!(client_1_updates.wait_next(&mut reactor), Some(48));
        let mut client_2_updates = client_2.updates().unwrap();
        assert_eq!(client_2_updates.wait_next(&mut reactor), Some(48));

        let request_future = client_2.request(TestRequest::Increment(3));
        let response = reactor.run(request_future).unwrap();
        assert_eq!(response, TestServiceResponse::Ack);
        assert_eq!(client_1_updates.wait_next(&mut reactor), Some(51));
        assert_eq!(client_2_updates.wait_next(&mut reactor), Some(51));
    }

    #[test]
    fn test_create_and_drop_service() {
        let mut reactor = reactor::Core::new().unwrap();
        let mut root_model = TestModel::new(42);
        let child_model = Rc::new(TestModel::new(12));
        root_model.child_model = Some(child_model.clone());
        let root_model = Rc::new(root_model);
        let client = connect(&mut reactor, TestService::new(root_model.clone()));
        assert_eq!(Rc::strong_count(&child_model), 2);

        let request_future = client.request(TestRequest::CreateService);
        let response = reactor.run(request_future).unwrap();
        assert_eq!(response, TestServiceResponse::ServiceCreated(1));
        let child_client = client.take_service::<TestService>(1).unwrap();
        let mut child_updates = child_client.updates().unwrap();
        assert_eq!(child_client.state(), Ok(12));
        assert!(client.take_service::<TestService>(1).is_err());
        assert_eq!(Rc::strong_count(&child_model), 3);

        let request_future = client.request(TestRequest::DropService);
        let response = reactor.run(request_future).unwrap();
        assert_eq!(response, TestServiceResponse::Ack);
        assert!(child_client.state().is_ok());
        assert!(
            reactor
                .run(child_client.request(TestRequest::Increment(5)))
                .is_ok()
        );
        assert_eq!(Rc::strong_count(&child_model), 3);

        drop(child_client);
        assert_eq!(child_updates.poll(), Ok(Async::Ready(Some(17))));
        assert_eq!(Rc::strong_count(&child_model), 3);

        drop(child_updates);
        reactor.turn(None); // Send drop message
        reactor.turn(None); // Receive drop message

        assert!(client.take_service::<TestService>(1).is_err());
        assert_eq!(Rc::strong_count(&child_model), 2);
    }

    #[test]
    fn test_add_service_on_init_or_update() {
        struct NoopService {
            init_called: bool,
            services_to_add_on_init: usize,
            services_to_add_on_update: usize,
            child_services: Vec<server::ServiceHandle>,
        }

        impl NoopService {
            fn new(services_to_add_on_init: usize, services_to_add_on_update: usize) -> Self {
                Self {
                    init_called: false,
                    services_to_add_on_init,
                    services_to_add_on_update,
                    child_services: Vec::new(),
                }
            }
        }

        impl server::Service for NoopService {
            type State = ();
            type Update = ();
            type Request = ();
            type Response = ();

            fn init(&mut self, connection: &server::Connection) -> Self::State {
                self.init_called = true;
                while self.services_to_add_on_init != 0 {
                    self.child_services
                        .push(connection.add_service(NoopService::new(0, 0)));
                    self.services_to_add_on_init -= 1;
                }
                ()
            }

            fn poll_update(
                &mut self,
                connection: &server::Connection,
            ) -> Async<Option<Self::Update>> {
                assert!(self.init_called);
                while self.services_to_add_on_update != 0 {
                    self.child_services
                        .push(connection.add_service(NoopService::new(0, 0)));
                    self.services_to_add_on_update -= 1;
                }
                Async::NotReady
            }
        }

        let mut reactor = reactor::Core::new().unwrap();
        let client = connect(&mut reactor, NoopService::new(1, 2));
        assert!(client.take_service::<NoopService>(1).is_ok());
        assert!(client.take_service::<NoopService>(2).is_ok());
        assert!(client.take_service::<NoopService>(3).is_ok());
        assert!(client.take_service::<NoopService>(4).is_err());
    }

    #[test]
    fn test_drop_client_updates() {
        let mut reactor = reactor::Core::new().unwrap();
        let model = Rc::new(TestModel::new(42));
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
        let model = Rc::new(TestModel::new(42));
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

        let model = Rc::new(TestModel::new(42));
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
    struct TestModel {
        cell: NotifyCell<usize>,
        child_model: Option<Rc<TestModel>>,
    }

    struct TestService {
        model: Rc<TestModel>,
        observer: NotifyCellObserver<usize>,
        child_service: Option<server::ServiceHandle>,
    }

    #[derive(Serialize, Deserialize)]
    enum TestRequest {
        Increment(usize),
        CreateService,
        DropService,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum TestServiceResponse {
        Ack,
        ServiceCreated(ServiceId),
    }

    impl TestService {
        fn new(model: Rc<TestModel>) -> Self {
            let observer = model.cell.observe();
            TestService {
                model,
                observer,
                child_service: None,
            }
        }
    }

    impl server::Service for TestService {
        type State = usize;
        type Update = usize;
        type Request = TestRequest;
        type Response = TestServiceResponse;

        fn init(&mut self, _: &server::Connection) -> Self::State {
            self.model.cell.get()
        }

        fn poll_update(&mut self, _: &server::Connection) -> Async<Option<Self::Update>> {
            self.observer.poll().unwrap()
        }

        fn request(
            &mut self,
            request: Self::Request,
            connection: &server::Connection,
        ) -> Option<Box<Future<Item = Self::Response, Error = Never>>> {
            match request {
                TestRequest::Increment(count) => {
                    self.model.increment_by(count);
                    Some(Box::new(future::ok(TestServiceResponse::Ack)))
                }
                TestRequest::CreateService => {
                    let handle = connection.add_service(TestService::new(
                        self.model.child_model.as_ref().unwrap().clone(),
                    ));
                    let service_id = handle.service_id();
                    self.child_service = Some(handle);
                    Some(Box::new(future::ok(TestServiceResponse::ServiceCreated(
                        service_id,
                    ))))
                }
                TestRequest::DropService => {
                    self.child_service.take();
                    Some(Box::new(future::ok(TestServiceResponse::Ack)))
                }
            }
        }
    }

    impl TestModel {
        fn new(count: usize) -> Self {
            TestModel {
                cell: NotifyCell::new(count),
                child_model: None,
            }
        }

        fn increment_by(&self, delta: usize) {
            self.cell.set(self.cell.get() + delta);
        }
    }
}
