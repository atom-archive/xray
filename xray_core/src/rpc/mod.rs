pub mod client;
mod messages;
pub mod server;

pub use self::messages::ServiceId;

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{future, unsync, Async, Future, Poll, Sink, Stream};
    use std::cell::RefCell;
    use std::fmt::Debug;
    use std::rc::Rc;
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
        let child_svc_client = svc_client.get_service::<TestService>(1).unwrap();
        assert_eq!(child_svc_client.state(), Some(12));
        assert!(svc_client.get_service::<TestService>(1).is_none());

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

    #[test]
    fn test_drop_service_client() {
        let mut reactor = reactor::Core::new().unwrap();
        let svc = TestService::new(42);
        let svc_client = connect(&mut reactor, svc.clone());
        let mut svc_client_updates = svc_client.updates().unwrap();

        svc.increment_by(1);
        assert_eq!(poll_wait(&mut reactor, &mut svc_client_updates), Some(1));

        drop(svc_client);
        assert_eq!(poll_wait(&mut reactor, &mut svc_client_updates), None);
    }

    #[test]
    fn test_finish_service_updates_stream() {
        let mut reactor = reactor::Core::new().unwrap();
        let svc = TestService::new(42);
        let svc_client = connect(&mut reactor, svc.clone());
        let mut svc_client_updates = svc_client.updates().unwrap();

        svc.increment_by(2);
        svc.increment_by(3);
        svc.finish_update_stream();
        assert_eq!(poll_wait(&mut reactor, &mut svc_client_updates), None);
        assert!(svc_client.state().is_none());
        assert!(svc_client.updates().is_none());
        assert!(svc_client.request(TestRequest::Increment(1)).is_none());
    }

    #[test]
    fn test_interrupting_connection_to_client() {
        let (client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();
        let client_to_server_rx = client_to_server_rx.map_err(|_| unreachable!());
        let mut server = server::Connection::new(client_to_server_rx, TestService::new(42));
        drop(client_to_server_tx);
        assert_eq!(server.poll(), Ok(Async::Ready(None)));
    }

    #[test]
    fn test_interrupting_connection_to_server_on_handshake() {
        let mut reactor = reactor::Core::new().unwrap();
        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let server_to_client_rx = server_to_client_rx.map_err(|_| unreachable!());
        drop(server_to_client_tx);
        let client_future = client::Connection::new::<_, TestService>(server_to_client_rx);
        assert!(reactor.run(client_future).is_err());
    }

    #[test]
    fn test_interrupting_connection_to_server() {
        let mut reactor = reactor::Core::new().unwrap();

        let (server_to_client_tx, server_to_client_rx) = unsync::mpsc::unbounded();
        let server_to_client_rx = server_to_client_rx.map_err(|_| unreachable!());
        let (client_to_server_tx, client_to_server_rx) = unsync::mpsc::unbounded();
        let client_to_server_rx = client_to_server_rx.map_err(|_| unreachable!());

        let server = server::Connection::new(client_to_server_rx, TestService::new(42));
        reactor.handle().spawn(
            server_to_client_tx
                .send_all(server.map_err(|_| unreachable!()))
                .then(|_| Ok(())),
        );

        let client_future = client::Connection::new::<_, TestService>(server_to_client_rx);
        let (mut client, svc_client) = reactor.run(client_future).unwrap();

        drop(reactor);
        assert_eq!(client.poll(), Ok(Async::Ready(None)));
    }

    fn connect<S: 'static + server::Service>(
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

            let mut indices_to_delete = Vec::new();
            for (index, updates_tx) in state.update_txs.iter_mut().enumerate() {
                match updates_tx.unbounded_send(count) {
                    Ok(()) => {}
                    Err(_) => indices_to_delete.push(index),
                }
            }

            for index in indices_to_delete.into_iter().rev() {
                state.update_txs.remove(index);
            }
        }

        fn finish_update_stream(&self) {
            let mut state = self.0.borrow_mut();
            state.update_txs.clear();
        }
    }

    impl server::Service for TestService {
        type State = usize;
        type Update = usize;
        type Request = TestRequest;
        type Response = TestServiceResponse;
        type Error = String;

        fn state(&self, connection: &mut server::Connection) -> Self::State {
            self.0.borrow().count
        }

        fn updates(
            &mut self,
            _: &mut server::Connection,
        ) -> Box<Stream<Item = Self::Update, Error = ()>> {
            let (updates_tx, updates_rx) = unsync::mpsc::unbounded();
            let mut state = self.0.borrow_mut();
            state.update_txs.push(updates_tx);
            Box::new(updates_rx)
        }

        fn request(
            &mut self,
            request: Self::Request,
            connection: &mut server::Connection,
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
