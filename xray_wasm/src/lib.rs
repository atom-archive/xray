#![feature(proc_macro, wasm_custom_section, wasm_import_module)]

extern crate futures;
extern crate wasm_bindgen;

use futures::executor::Spawn;
use futures::future::{self, Executor};
use futures::unsync::mpsc;
use futures::{executor, Async, Future, Poll, Stream};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
#[derive(Clone)]
pub struct Sender(Option<mpsc::UnboundedSender<String>>);

#[wasm_bindgen]
pub struct Receiver(mpsc::UnboundedReceiver<String>);

#[wasm_bindgen]
pub struct ChannelPair {
    tx: Option<Sender>,
    rx: Option<Receiver>,
}

pub struct ForegroundExecutor(Rc<RefCell<ExecutorState>>);

struct ExecutorState {
    next_future_id: usize,
    futures: HashMap<usize, Rc<RefCell<Spawn<Future<Item = (), Error = ()>>>>>,
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct Notify(Weak<RefCell<ExecutorState>>);

#[wasm_bindgen]
pub struct Server;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[wasm_bindgen(module = "../lib/support")]
extern "C" {
    #[wasm_bindgen(js_name = notifyOnNextTick)]
    fn notify_on_next_tick(notify: Notify, id: usize);

    pub type JsSender;

    #[wasm_bindgen(constructor)]
    fn new() -> JsSender;

    #[wasm_bindgen(method)]
    fn send(this: &JsSender, message: &str);

    #[wasm_bindgen(method)]
    fn finish(this: &JsSender);
}

#[wasm_bindgen]
impl Sender {
    pub fn send(&mut self, message: String) {
        if let Some(ref mut tx) = self.0 {
            tx.unbounded_send(message);
        }
    }

    pub fn dispose(&mut self) {
        self.0.take();
    }
}

impl Stream for Receiver {
    type Item = String;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.0.poll()
    }
}

#[wasm_bindgen]
impl ChannelPair {
    pub fn new() -> ChannelPair {
        let (tx, rx) = mpsc::unbounded();
        Self {
            tx: Some(Sender(Some(tx))),
            rx: Some(Receiver(rx)),
        }
    }

    pub fn tx(&mut self) -> Sender {
        self.tx.take().unwrap()
    }

    pub fn rx(&mut self) -> Receiver {
        self.rx.take().unwrap()
    }
}

impl ForegroundExecutor {
    fn new() -> Self {
        ForegroundExecutor(Rc::new(RefCell::new(ExecutorState {
            next_future_id: 0,
            futures: HashMap::new(),
        })))
    }
}

impl<F: 'static + Future<Item = (), Error = ()>> Executor<F> for ForegroundExecutor {
    fn execute(&self, future: F) -> Result<(), future::ExecuteError<F>> {
        let mut state = self.0.borrow_mut();
        let id = state.next_future_id;
        state.next_future_id += 1;
        state
            .futures
            .insert(id, Rc::new(RefCell::new(executor::spawn(future))));

        state
            .futures
            .get_mut(&id)
            .unwrap()
            .borrow_mut()
            .poll_future_notify(&Arc::new(Notify(Rc::downgrade(&self.0))), id);

        Ok(())
    }
}

#[wasm_bindgen]
impl Notify {
    pub fn notify_on_next_tick(&self, id: usize) {
        if let Some(state) = self.0.upgrade() {
            let spawn = state.borrow_mut().futures.get(&id).cloned();
            if let Some(spawn) = spawn {
                if let Ok(mut spawn) = spawn.try_borrow_mut() {
                    let result = spawn.poll_future_notify(&Arc::new(self.clone()), id);
                    if let Ok(Async::Ready(_)) = result {
                        state.borrow_mut().futures.remove(&id);
                    }
                }
            }
        }
    }
}

impl executor::Notify for Notify {
    fn notify(&self, id: usize) {
        notify_on_next_tick(Notify(self.0.clone()), id);
    }
}

// The only convenient way of calling poll_future_notify is to wrap our notify handle in an Arc,
// which requires Notify to be Send and Sync. However, because we are integrating with JavaScript,
// we know that all of this code will be run in a single thread.
unsafe impl Send for Notify {}
unsafe impl Sync for Notify {}

#[wasm_bindgen]
impl Server {
    pub fn new() -> Self {
        Server
    }

    pub fn connect_to_peer(receiver: Receiver) {}
}

#[wasm_bindgen]
#[cfg(feature = "js-tests")]
pub struct Test {
    executor: ForegroundExecutor,
}

#[wasm_bindgen]
#[cfg(feature = "js-tests")]
impl Test {
    pub fn new() -> Self {
        Self {
            executor: ForegroundExecutor::new(),
        }
    }

    pub fn echo_stream(&mut self, outgoing: JsSender, incoming: Receiver) {
        let outgoing = Rc::new(outgoing);
        let outgoing_clone = outgoing.clone();
        self.executor
            .execute(Box::new(
                incoming
                    .for_each(move |message| {
                        outgoing.send(&message);
                        Ok(())
                    })
                    .and_then(move |_| {
                        outgoing_clone.finish();
                        Ok(())
                    }),
            ))
            .unwrap();
    }
}
