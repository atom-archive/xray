#![feature(macros_in_extern)]

extern crate bincode;
extern crate futures;
extern crate hex;
extern crate js_sys;
extern crate memo_core;
#[macro_use]
extern crate serde_derive;
extern crate base64;
extern crate serde;
extern crate wasm_bindgen;
extern crate wasm_bindgen_futures;

use futures::{Async, Future, Poll, Stream};
use memo_core as memo;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::Cell;
use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, JsFuture};

#[wasm_bindgen]
pub struct WorkTree(memo::WorkTree);

#[derive(Serialize, Deserialize)]
struct AsyncResult<T> {
    value: Option<T>,
    done: bool,
}

struct AsyncIteratorToStream<T, E> {
    next_value: JsFuture,
    iterator: AsyncIteratorWrapper,
    _phantom: PhantomData<(T, E)>,
}

#[wasm_bindgen]
pub struct StreamToAsyncIterator(Rc<Cell<Option<Box<Stream<Item = JsValue, Error = JsValue>>>>>);

struct HexOid(memo::Oid);

struct Base64<T>(T);

#[derive(Deserialize)]
pub struct WorkTreeNewArgs {
    replica_id: memo::ReplicaId,
    base: HexOid,
    start_ops: Vec<Base64<memo::Operation>>,
}

#[wasm_bindgen]
pub struct WorkTreeNewResult {
    tree: Option<WorkTree>,
    operations: Option<StreamToAsyncIterator>,
}

#[derive(Deserialize)]
pub struct WorkTreeCreateFileArgs {
    path: String,
    file_type: memo::FileType,
}

#[derive(Copy, Clone, Serialize, Deserialize)]
struct EditRange {
    start: memo::Point,
    end: memo::Point,
}

#[wasm_bindgen(module = "./support")]
extern "C" {
    pub type AsyncIteratorWrapper;

    #[wasm_bindgen(method)]
    fn next(this: &AsyncIteratorWrapper) -> js_sys::Promise;

    pub type GitProviderWrapper;

    #[wasm_bindgen(method, js_name = baseEntries)]
    fn base_entries(this: &GitProviderWrapper, head: &str) -> AsyncIteratorWrapper;

    #[wasm_bindgen(method, js_name = baseText)]
    fn base_text(this: &GitProviderWrapper, head: &str, path: &str) -> js_sys::Promise;
}

#[wasm_bindgen]
impl WorkTree {
    pub fn new(git: GitProviderWrapper, args: JsValue) -> Result<WorkTreeNewResult, JsValue> {
        let WorkTreeNewArgs {
            replica_id,
            base: HexOid(base),
            start_ops,
        } = args.into_serde().unwrap();
        let (tree, operations) = memo::WorkTree::new(
            replica_id,
            base,
            start_ops.into_iter().map(|op| op.0),
            Rc::new(git),
        )
        .map_err(|e| e.to_string())?;
        Ok(WorkTreeNewResult {
            tree: Some(WorkTree(tree)),
            operations: Some(StreamToAsyncIterator::new(
                operations
                    .map(|op| Base64(op))
                    .map_err(|err| err.to_string()),
            )),
        })
    }

    pub fn version(&self) -> JsValue {
        JsValue::from_serde(&Base64(self.0.version())).unwrap()
    }

    pub fn apply_ops(&mut self, ops: JsValue) -> Result<StreamToAsyncIterator, JsValue> {
        let ops = ops
            .into_serde::<Vec<Base64<memo::Operation>>>()
            .map(|ops| ops.into_iter().map(|Base64(op)| op.clone()))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;

        self.0
            .apply_ops(ops)
            .map(|fixup_ops| {
                StreamToAsyncIterator::new(
                    fixup_ops
                        .map(|op| Base64(op))
                        .map_err(|err| err.to_string()),
                )
            })
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn create_file(&self, args: JsValue) -> Result<JsValue, JsValue> {
        let WorkTreeCreateFileArgs { path, file_type } = args.into_serde().unwrap();
        self.0
            .create_file(&PathBuf::from(path), file_type)
            .map(|operation| JsValue::from_serde(&Base64(operation)).unwrap())
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn open_text_file(&mut self, path: String) -> js_sys::Promise {
        future_to_promise(
            self.0
                .open_text_file(path)
                .map(|buffer_id| JsValue::from_serde(&buffer_id).unwrap())
                .map_err(|error| JsValue::from_str(&error.to_string())),
        )
    }

    pub fn text(&self, buffer_id: JsValue) -> Result<JsValue, JsValue> {
        self.0
            .text(buffer_id.into_serde().unwrap())
            .map(|text| JsValue::from_str(&text.into_string()))
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn edit(
        &self,
        buffer_id: JsValue,
        old_ranges: JsValue,
        new_text: &str,
    ) -> Result<JsValue, JsValue> {
        let buffer_id = buffer_id.into_serde().unwrap();
        let old_ranges = old_ranges
            .into_serde::<Vec<EditRange>>()
            .map_err(|error| JsValue::from_str(&error.to_string()))?
            .into_iter()
            .map(|EditRange { start, end }| start..end);

        self.0
            .edit_2d(buffer_id, old_ranges, new_text)
            .map(|op| JsValue::from_serde(&Base64(op)).unwrap())
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }
}

#[wasm_bindgen]
impl WorkTreeNewResult {
    pub fn tree(&mut self) -> WorkTree {
        self.tree.take().unwrap()
    }

    pub fn operations(&mut self) -> StreamToAsyncIterator {
        self.operations.take().unwrap()
    }
}

impl<T, E> AsyncIteratorToStream<T, E> {
    fn new(iterator: AsyncIteratorWrapper) -> Self {
        AsyncIteratorToStream {
            next_value: JsFuture::from(iterator.next()),
            iterator,
            _phantom: PhantomData,
        }
    }
}

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
                })
                .unwrap()
            })
            .map_err(|error| JsValue::from_serde(&error).unwrap());

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
                        })
                        .unwrap(),
                    ))
                }
                Err((error, _)) => Err(error),
            }))
        })
    }
}

impl memo::GitProvider for GitProviderWrapper {
    fn base_entries(
        &self,
        oid: memo::Oid,
    ) -> Box<Stream<Item = memo::DirEntry, Error = io::Error>> {
        let iterator = GitProviderWrapper::base_entries(self, &hex::encode(oid));
        Box::new(
            AsyncIteratorToStream::new(iterator)
                .map_err(|error: String| io::Error::new(io::ErrorKind::Other, error)),
        )
    }

    fn base_text(
        &self,
        oid: memo::Oid,
        path: &Path,
    ) -> Box<Future<Item = String, Error = io::Error>> {
        Box::new(
            JsFuture::from(GitProviderWrapper::base_text(
                self,
                &hex::encode(oid),
                path.to_string_lossy().as_ref(),
            ))
            .map(|value| value.as_string().unwrap())
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.as_string().unwrap())),
        )
    }
}

impl Serialize for HexOid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        hex::encode(self.0).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HexOid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        let bytes = hex::decode(&String::deserialize(deserializer)?).map_err(Error::custom)?;
        let mut oid = memo::Oid::default();
        oid.copy_from_slice(&bytes);
        Ok(HexOid(oid))
    }
}

impl<T: Serialize> Serialize for Base64<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::Error;
        base64::encode(&bincode::serialize(&self.0).map_err(Error::custom)?).serialize(serializer)
    }
}

impl<'de1, T: for<'de2> Deserialize<'de2>> Deserialize<'de1> for Base64<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de1>,
    {
        use serde::de::Error;
        let bytes = base64::decode(&String::deserialize(deserializer)?).map_err(Error::custom)?;
        let inner = bincode::deserialize::<T>(&bytes).map_err(D::Error::custom)?;
        Ok(Base64(inner))
    }
}
