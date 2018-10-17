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

use futures::{Future, Stream};
use memo_core::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::Cell;
use std::char;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

#[wasm_bindgen]
struct WorkTree {}

#[wasm_bindgen]
extern "C" {
    type AsyncIterator;

    #[wasm_bindgen(method)]
    fn next(this: &AsyncIterator) -> js_sys::Promise;
}

#[wasm_bindgen]
struct StreamToIterator(Rc<Cell<Option<Box<Stream<Item = JsValue, Error = JsValue>>>>>);

#[derive(Serialize, Deserialize)]
struct AsyncResult<T> {
    value: Option<T>,
    done: bool,
}

#[wasm_bindgen]
impl StreamToIterator {
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
                        })
                        .unwrap(),
                    ))
                }
                Err((error, _)) => Err(error),
            }))
        })
    }
}

impl StreamToIterator {
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
                })
                .unwrap()
            })
            .map_err(|error| JsValue::from_serde(&error).unwrap());

        StreamToIterator(Rc::new(Cell::new(Some(Box::new(js_value_stream)))))
    }
}

#[wasm_bindgen]
impl WorkTree {
    pub fn new() -> (Self, StreamToIterator) {
        panic!()
    }
}
