#![feature(arbitrary_self_types, futures_api, macros_in_extern, pin)]

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

use futures::compat::{self, Future01CompatExt};
use futures::{future::LocalFutureObj, prelude::*, stream::LocalStreamObj, task::LocalWaker, Poll};
use memo_core as memo;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io;
use std::marker::PhantomData;
use std::path::Path;
use std::pin::Pin;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, JsFuture};

#[wasm_bindgen]
pub struct WorkTree(memo::WorkTree<GitProviderWrapper>);

#[derive(Serialize, Deserialize)]
struct AsyncResult<T> {
    value: Option<T>,
    done: bool,
}

#[wasm_bindgen(module = "./support")]
extern "C" {
    pub type AsyncIteratorWrapper;

    #[wasm_bindgen(method)]
    fn next(this: &AsyncIteratorWrapper) -> js_sys::Promise;

    pub type GitProviderWrapper;

    #[wasm_bindgen(method, js_name = baseEntries)]
    fn base_entries(this: &GitProviderWrapper, head: &str) -> AsyncIteratorWrapper;
}

struct AsyncIteratorToStream<T, E> {
    next_value: compat::Compat01As03<JsFuture>,
    iterator: AsyncIteratorWrapper,
    _phantom: PhantomData<(T, E)>,
}

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
    operations: Option<LocalFutureObj<'static, Result<Vec<memo::Operation>, memo::Error>>>,
}

#[derive(Serialize)]
pub struct WorkTreeNewTextFileResult {
    file_id: Base64<memo::FileId>,
    operation: Base64<memo::Operation>,
}

#[wasm_bindgen]
impl WorkTree {
    pub fn new(git: GitProviderWrapper, args: JsValue) -> WorkTreeNewResult {
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
        );
        WorkTreeNewResult {
            tree: Some(WorkTree(tree)),
            operations: Some(LocalFutureObj::new(Box::new(operations))),
        }
    }

    pub fn new_text_file(&mut self) -> JsValue {
        let (file_id, operation) = self.0.new_text_file();
        JsValue::from_serde(&WorkTreeNewTextFileResult {
            file_id: Base64(file_id),
            operation: Base64(operation),
        })
        .unwrap()
    }

    pub fn open_text_file(&mut self, file_id: JsValue) -> js_sys::Promise {
        let Base64(file_id) = file_id.into_serde().unwrap();
        let future = self
            .0
            .open_text_file(file_id)
            .map_ok(|id| JsValue::from_serde(&id).unwrap())
            .map_err(|e| JsValue::from_serde(&e.to_string()).unwrap());
        future_to_promise(LocalFutureObj::new(Box::new(future)).compat())
    }
}

#[wasm_bindgen]
impl WorkTreeNewResult {
    pub fn tree(&mut self) -> WorkTree {
        self.tree.take().unwrap()
    }

    pub fn operations(&mut self) -> js_sys::Promise {
        let operations = self.operations.take().unwrap();
        future_to_promise(
            LocalFutureObj::new(Box::new(
                operations
                    .map_ok(|op| JsValue::from_serde(&Base64(op)).unwrap())
                    .map_err(|e| JsValue::from_serde(&e.to_string()).unwrap()),
            ))
            .compat(),
        )
    }
}

impl<T, E> AsyncIteratorToStream<T, E> {
    fn new(iterator: AsyncIteratorWrapper) -> Self {
        AsyncIteratorToStream {
            next_value: compat::Compat01As03::new(JsFuture::from(iterator.next())),
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
    type Item = Result<T, E>;

    fn poll_next(self: Pin<&mut Self>, lw: &LocalWaker) -> Poll<Option<Self::Item>> {
        match self.next_value.poll(lw) {
            Ok(Poll::Ready(result)) => {
                let result: AsyncResult<T> = result.into_serde().unwrap();
                if result.done {
                    Poll::Ready(None)
                } else {
                    self.next_value =
                        compat::Compat01As03::new(JsFuture::from(self.iterator.next()));
                    Poll::Ready(Some(Ok(result.value.unwrap())))
                }
            }
            Ok(Poll::Pending) => Poll::Pending,
            Err(error) => Poll::Ready(Some(Err(error.into_serde().unwrap()))),
        }
    }
}

impl memo::GitProvider for GitProviderWrapper {
    type BaseEntriesStream = LocalStreamObj<'static, Result<memo::DirEntry, io::Error>>;
    type BaseTextFuture = LocalFutureObj<'static, Result<String, io::Error>>;

    fn base_entries(&self, oid: memo::Oid) -> Self::BaseEntriesStream {
        let iterator = GitProviderWrapper::base_entries(self, &hex::encode(oid));
        LocalStreamObj::new(Box::new(
            AsyncIteratorToStream::new(iterator)
                .map_err(|error: String| io::Error::new(io::ErrorKind::Other, error)),
        ))
    }

    fn base_text(&self, oid: memo::Oid, path: &Path) -> Self::BaseTextFuture {
        unimplemented!()
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
