#![feature(proc_macro, wasm_custom_section, wasm_import_module)]

extern crate bytes;
extern crate futures;
extern crate wasm_bindgen;

use bytes::Bytes;
use futures::executor::{self, Notify, Spawn};
use futures::future;
use futures::unsync::mpsc;
use futures::{Async, AsyncSink, Future, Poll, Sink, Stream};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

#[derive(Clone)]
pub struct Executor(Rc<RefCell<ExecutorState>>);

struct ExecutorState {
    next_spawn_id: usize,
    futures: HashMap<usize, Rc<RefCell<Spawn<Future<Item = (), Error = ()>>>>>,
    notify_handle: Option<Arc<NotifyHandle>>,
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct NotifyHandle(Weak<RefCell<ExecutorState>>);

#[wasm_bindgen]
pub struct Channel {
    sender: Option<Sender>,
    receiver: Option<Receiver>,
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct Sender(Option<mpsc::UnboundedSender<Bytes>>);

#[wasm_bindgen]
pub struct Receiver(mpsc::UnboundedReceiver<Bytes>);

#[wasm_bindgen]
pub struct Server;

#[wasm_bindgen(module = "../lib/support")]
extern "C" {
    #[wasm_bindgen(js_name = notifyOnNextTick)]
    fn notify_on_next_tick(notify: NotifyHandle, id: usize);

    pub type JsSink;

    #[wasm_bindgen(method)]
    fn send(this: &JsSink, message: Vec<u8>);

    #[wasm_bindgen(method)]
    fn close(this: &JsSink);
}

// #[wasm_bindgen]
// extern "C" {
//     #[wasm_bindgen(js_namespace = console)]
//     fn log(s: &str);
// }

impl Executor {
    fn new() -> Self {
        let state = Rc::new(RefCell::new(ExecutorState {
            next_spawn_id: 0,
            futures: HashMap::new(),
            notify_handle: None,
        }));
        state.borrow_mut().notify_handle = Some(Arc::new(NotifyHandle(Rc::downgrade(&state))));
        Executor(state)
    }
}

impl<F: 'static + Future<Item = (), Error = ()>> future::Executor<F> for Executor {
    fn execute(&self, future: F) -> Result<(), future::ExecuteError<F>> {
        let id;
        let notify_handle;

        // Drop the dynamic borrow of state before polling the future for the first time,
        // because polling might cause a reentrant call to this method.
        {
            let mut state = self.0.borrow_mut();
            id = state.next_spawn_id;
            state.next_spawn_id += 1;
            notify_handle = state.notify_handle.as_ref().unwrap().clone();
        }

        let mut spawn = executor::spawn(future);
        match spawn.poll_future_notify(&notify_handle, id) {
            Ok(Async::NotReady) => {
                self.0
                    .borrow_mut()
                    .futures
                    .insert(id, Rc::new(RefCell::new(spawn)));
            }
            _ => {}
        }

        Ok(())
    }
}

#[wasm_bindgen]
impl NotifyHandle {
    pub fn notify_from_js_on_next_tick(&self, id: usize) {
        if let Some(state) = self.0.upgrade() {
            let spawn;
            let notify_handle;
            {
                let state = state.borrow();
                spawn = state.futures.get(&id).cloned();
                notify_handle = state.notify_handle.as_ref().unwrap().clone();
            }

            if let Some(spawn) = spawn {
                if let Ok(mut spawn) = spawn.try_borrow_mut() {
                    match spawn.poll_future_notify(&notify_handle, id) {
                        Ok(Async::NotReady) => {}
                        _ => {
                            state.borrow_mut().futures.remove(&id);
                        }
                    }
                }
            }
        }
    }
}

impl Notify for NotifyHandle {
    fn notify(&self, id: usize) {
        notify_on_next_tick(self.clone(), id);
    }
}

// The only convenient way of calling poll_future_notify is to wrap our notify handle in an Arc,
// which requires Notify to be Send and Sync. However, because we are integrating with JavaScript,
// we know that all of this code will be run in a single thread.
unsafe impl Send for NotifyHandle {}
unsafe impl Sync for NotifyHandle {}

#[wasm_bindgen]
impl Channel {
    pub fn new() -> Channel {
        let (tx, rx) = mpsc::unbounded();
        Self {
            sender: Some(Sender(Some(tx))),
            receiver: Some(Receiver(rx)),
        }
    }

    pub fn take_sender(&mut self) -> Sender {
        self.sender.take().unwrap()
    }

    pub fn take_receiver(&mut self) -> Receiver {
        self.receiver.take().unwrap()
    }
}

#[wasm_bindgen]
impl Sender {
    pub fn send(&mut self, message: Vec<u8>) -> bool {
        if let Some(ref mut tx) = self.0 {
            tx.unbounded_send(Bytes::from(message)).is_ok()
        } else {
            false
        }
    }

    pub fn dispose(&mut self) {
        self.0.take();
    }
}

impl Stream for Receiver {
    type Item = Bytes;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.0.poll()
    }
}

#[wasm_bindgen]
impl Server {
    pub fn new() -> Self {
        Server
    }

    pub fn connect_to_peer(_receiver: Receiver) {}
}

impl Sink for JsSink {
    type SinkItem = Vec<u8>;
    type SinkError = ();

    fn start_send(
        &mut self,
        item: Self::SinkItem,
    ) -> Result<AsyncSink<Self::SinkItem>, Self::SinkError> {
        JsSink::send(self, item);
        Ok(AsyncSink::Ready)
    }

    fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
        Ok(Async::Ready(()))
    }

    fn close(&mut self) -> Result<Async<()>, Self::SinkError> {
        JsSink::close(self);
        Ok(Async::Ready(()))
    }
}

#[wasm_bindgen]
#[cfg(feature = "js-tests")]
pub struct Test {
    executor: Executor,
}

#[wasm_bindgen]
#[cfg(feature = "js-tests")]
impl Test {
    pub fn new() -> Self {
        Self {
            executor: Executor::new(),
        }
    }

    pub fn echo_stream(&self, stream: Receiver, sink: JsSink) {
        use futures::future::Executor;
        self.executor
            .execute(Box::new(
                sink.send_all(stream.map(|bytes| bytes.to_vec()))
                    .then(|_| Ok(())),
            ))
            .unwrap();
    }
}
