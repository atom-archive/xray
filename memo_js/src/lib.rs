#![feature(macros_in_extern)]

extern crate bincode;
extern crate futures;
extern crate js_sys;
extern crate memo_core;
#[macro_use]
extern crate serde_derive;
extern crate base64;
extern crate serde;
extern crate wasm_bindgen;
extern crate wasm_bindgen_futures;

use futures::{Async, Future, Poll, Stream};
use memo_core::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::Cell;
use std::char;
use std::collections::HashMap;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, JsFuture};

#[wasm_bindgen]
struct WorkTree {}

#[derive(Serialize, Deserialize)]
struct AsyncResult<T> {
    value: Option<T>,
    done: bool,
}

#[wasm_bindgen]
extern "C" {
    type AsyncIterator;

    #[wasm_bindgen(method)]
    fn next(this: &AsyncIterator) -> js_sys::Promise;
}

struct AsyncIteratorToStream<T, E> {
    next_value: JsFuture,
    iterator: AsyncIterator,
    _phantom: PhantomData<(T, E)>,
}

#[wasm_bindgen]
struct StreamToAsyncIterator(Rc<Cell<Option<Box<Stream<Item = JsValue, Error = JsValue>>>>>);

impl<T, E> Stream for AsyncIteratorToStream<T, E>
where
    E: for<'de> Deserialize<'de>,
    T: for<'de> Deserialize<'de>,
{
    type Item = T;
    type Error = E;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.next_value.poll() {
            Ok(Async::Ready(result)) => {
                let result: AsyncResult<T> = result.into_serde().unwrap();
                if result.done {
                    Ok(Async::Ready(None))
                } else {
                    self.next_value = JsFuture::from(self.iterator.next());
                    Ok(Async::Ready(result.value))
                }
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(error) => Err(error.into_serde().unwrap()),
        }
    }
}

impl StreamToAsyncIterator {
    fn new<E, S, T>(stream: S) -> Self
    where
        E: Serialize,
        S: 'static + Stream<Item = T, Error = E>,
        T: Serialize,
    {
        let js_value_stream = stream
            .map(|value| {
                JsValue::from_serde(&AsyncResult {
                    value: Some(value),
                    done: false,
                }).unwrap()
            }).map_err(|error| JsValue::from_serde(&error).unwrap());

        StreamToAsyncIterator(Rc::new(Cell::new(Some(Box::new(js_value_stream)))))
    }
}

#[wasm_bindgen]
impl StreamToAsyncIterator {
    pub fn next(&mut self) -> Option<js_sys::Promise> {
        let stream_rc = self.0.clone();
        self.0.take().map(|stream| {
            future_to_promise(stream.into_future().then(move |result| match result {
                Ok((next, rest)) => {
                    stream_rc.set(Some(rest));
                    Ok(next.unwrap_or(
                        JsValue::from_serde(&AsyncResult::<()> {
                            value: None,
                            done: true,
                        }).unwrap(),
                    ))
                }
                Err((error, _)) => Err(error),
            }))
        })
    }
}
