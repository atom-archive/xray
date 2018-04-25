#![feature(proc_macro, wasm_custom_section, wasm_import_module)]

extern crate bytes;
extern crate futures;
extern crate wasm_bindgen;
#[macro_use]
extern crate xray_core;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use bytes::Bytes;
use futures::executor::{self, Notify, Spawn};
use futures::unsync::mpsc;
use futures::{future, stream};
use futures::{Async, AsyncSink, Future, Poll, Sink, Stream};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::mem;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use xray_core::app::Command;
use xray_core::{cross_platform, App, ViewId, WindowId, WindowUpdate};

#[derive(Serialize, Debug)]
#[serde(tag = "type")]
enum OutgoingMessage {
    OpenWindow { window_id: WindowId },
    UpdateWindow(WindowUpdate),
    Error { description: String },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum IncomingMessage {
    Action {
        view_id: ViewId,
        action: serde_json::Value,
    },
}

#[derive(Clone)]
pub struct Executor(Rc<RefCell<ExecutorState>>);

struct ExecutorState {
    next_spawn_id: usize,
    futures: HashMap<usize, Rc<RefCell<Spawn<Future<Item = (), Error = ()>>>>>,
    pending: HashMap<usize, Rc<RefCell<Spawn<Future<Item = (), Error = ()>>>>>,
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
pub struct Server {
    executor: Executor,
    app: Rc<RefCell<App>>,
}

struct FileProvider;

#[wasm_bindgen(module = "../lib/support")]
extern "C" {
    #[wasm_bindgen(js_name = notifyOnNextTick)]
    fn notify_on_next_tick(notify: NotifyHandle);

    pub type JsSink;

    #[wasm_bindgen(method)]
    fn send(this: &JsSink, message: Vec<u8>);

    #[wasm_bindgen(method)]
    fn close(this: &JsSink);
}

impl Executor {
    fn new() -> Self {
        let state = Rc::new(RefCell::new(ExecutorState {
            next_spawn_id: 0,
            futures: HashMap::new(),
            pending: HashMap::new(),
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
    pub fn notify_from_js_on_next_tick(&self) {
        if let Some(state) = self.0.upgrade() {
            let notify_handle;
            let mut pending = HashMap::new();
            {
                let mut state = state.borrow_mut();
                notify_handle = state.notify_handle.as_ref().unwrap().clone();
                mem::swap(&mut state.pending, &mut pending);
            }

            for (id, task) in pending {
                if let Ok(mut task) = task.try_borrow_mut() {
                    match task.poll_future_notify(&notify_handle, id) {
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
        if let Some(state) = self.0.upgrade() {
            let mut state = state.borrow_mut();
            if !state.pending.contains_key(&id) {
                if let Some(task) = state.futures.get(&id).cloned() {
                    state.pending.insert(id, task);
                    if state.pending.len() == 1 {
                        notify_on_next_tick(self.clone());
                    }
                }
            }
        }
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
impl Server {
    pub fn new() -> Self {
        let foreground_executor = Rc::new(Executor::new());
        // TODO: use a requestIdleCallback-based executor here instead.
        let background_executor = foreground_executor.clone();
        Server {
            app: App::new(
                false,
                foreground_executor.clone(),
                background_executor.clone(),
                FileProvider,
            ),
            executor: Executor::new(),
        }
    }

    pub fn start_app(&mut self, outgoing: JsSink) {
        use futures::future::Executor;

        let executor = self.executor.clone();

        if let Some(commands) = self.app.borrow_mut().commands() {
            let outgoing_commands = commands
                .map(|command| match command {
                    Command::OpenWindow(window_id) => OutgoingMessage::OpenWindow { window_id },
                })
                .map(|command| serde_json::to_vec(&command).unwrap())
                .map_err(|_| unreachable!());
            executor
                .execute(Box::new(outgoing.send_all(outgoing_commands)).then(|_| Ok(())))
                .unwrap();
        } else {
            eprintln!("connect_app can only be called once")
        }
    }

    pub fn start_window(&mut self, window_id: WindowId, incoming: Receiver, outgoing: JsSink) {
        use futures::future::Executor;

        let app = self.app.clone();

        let receive_incoming = incoming
            .map(|message| serde_json::from_slice(&message).unwrap())
            .for_each(move |message| {
                match message {
                    IncomingMessage::Action { view_id, action } => {
                        app.borrow_mut().dispatch_action(window_id, view_id, action);
                    }
                }
                Ok(())
            })
            .then(|_| Ok(()));

        self.executor.execute(Box::new(receive_incoming)).unwrap();

        match self.app.borrow_mut().start_window(&window_id, 0_f64) {
            Ok(updates) => {
                let serialized_updates = updates.map(|update| {
                    serde_json::to_vec(&OutgoingMessage::UpdateWindow(update)).unwrap()
                });
                self.executor
                    .execute(
                        outgoing
                            .send_all(serialized_updates.map_err(|_| unreachable!()))
                            .then(|_| Ok(())),
                    )
                    .unwrap();
            }
            Err(_) => {
                let error = stream::once(Ok(OutgoingMessage::Error {
                    description: format!("No window exists for id {}", window_id),
                })).map(|message| serde_json::to_vec(&message).unwrap());
                self.executor
                    .execute(Box::new(outgoing.send_all(error).then(|_| Ok(()))))
                    .unwrap();
            }
        };
    }

    pub fn connect_to_peer(&mut self, incoming: Receiver, outgoing: JsSink) {
        use futures::future::Executor;

        let executor = self.executor.clone();
        let connect_future = self.app
            .borrow_mut()
            .connect_to_server(incoming.map_err(|_| unreachable!()))
            .map_err(|error| eprintln!("RPC error: {}", error))
            .and_then(move |connection| {
                executor
                    .execute(Box::new(
                        outgoing.send_all(
                            connection
                                // TODO: go back to using Vec<u8> for outgoing messages in xray_core.
                                .map(|bytes| bytes.to_vec())
                                .map_err(|_| unreachable!()),
                        ).then(|_| Ok(())),
                    ))
                    .unwrap();
                Ok(())
            });
        self.executor.execute(Box::new(connect_future)).unwrap();
    }
}

impl xray_core::fs::FileProvider for FileProvider {
    fn open(
        &self,
        _: &cross_platform::Path,
    ) -> Box<Future<Item = Box<xray_core::fs::File>, Error = io::Error>> {
        unimplemented!()
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

    pub fn echo_stream(&self, incoming: Receiver, outgoing: JsSink) {
        use futures::future::Executor;
        self.executor
            .execute(Box::new(
                outgoing.send_all(incoming.map(|bytes| bytes.to_vec()))
                    .then(|_| Ok(())),
            ))
            .unwrap();
    }
}
