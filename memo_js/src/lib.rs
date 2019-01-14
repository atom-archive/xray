#![feature(macros_in_extern)]

use bincode;
use futures::{Async, Future, Poll, Stream};
use memo_core as memo;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_derive::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::marker::PhantomData;
use std::mem;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{future_to_promise, JsFuture};

trait JsValueExt {
    fn into_operation(self) -> Result<Option<memo::Operation>, JsValue>;
    fn into_ranges_vec(self) -> Result<Vec<Range<memo::Point>>, JsValue>;
    fn into_error_message(self) -> Result<String, String>;
}

trait IntoJsError {
    fn into_js_err(self) -> JsValue;
}

#[wasm_bindgen]
pub struct WorkTree(memo::WorkTree);

#[derive(Deserialize)]
struct AsyncResult<T> {
    value: Option<T>,
    done: bool,
}

struct AsyncIteratorToStream<T> {
    next_value: JsFuture,
    iterator: AsyncIteratorWrapper,
    _phantom: PhantomData<T>,
}

#[wasm_bindgen]
pub struct StreamToAsyncIterator(Rc<Cell<Option<Box<Stream<Item = JsValue, Error = JsValue>>>>>);

#[wasm_bindgen]
pub struct WorkTreeNewResult {
    tree: Option<WorkTree>,
    operations: Option<StreamToAsyncIterator>,
}

#[wasm_bindgen]
pub struct AddSelectionSetResult {
    set_id: memo::LocalSelectionSetId,
    operation: Option<OperationEnvelope>,
}

#[wasm_bindgen]
pub struct OperationEnvelope(memo::OperationEnvelope);

#[derive(Serialize)]
struct Change {
    start: memo::Point,
    end: memo::Point,
    text: String,
}

#[derive(Serialize)]
struct Entry {
    #[serde(rename = "type")]
    file_type: memo::FileType,
    depth: usize,
    name: String,
    path: String,
    status: memo::FileStatus,
    visible: bool,
}

#[derive(Deserialize, Serialize)]
struct JsRange {
    start: memo::Point,
    end: memo::Point,
}

#[derive(Deserialize, Serialize)]
struct JsSelections {
    local: HashMap<memo::LocalSelectionSetId, Vec<JsRange>>,
    remote: HashMap<memo::ReplicaId, Vec<Vec<JsRange>>>,
}

pub struct HexOid(memo::Oid);

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

    pub type ChangeObserver;

    #[wasm_bindgen(method)]
    fn changed(
        this: &ChangeObserver,
        buffer_id: JsValue,
        changes: JsValue,
        selection_ranges: JsValue,
    );
}

#[wasm_bindgen]
impl WorkTree {
    pub fn new(
        git: GitProviderWrapper,
        observer: ChangeObserver,
        replica_id: JsValue,
        base: JsValue,
        js_start_ops: js_sys::Array,
    ) -> Result<WorkTreeNewResult, JsValue> {
        let replica_id = replica_id.into_serde().map_err(|e| {
            format!("ReplicaId {:?} must be a valid UUID: {}", replica_id, e).into_js_err()
        })?;

        let base = base
            .into_serde::<Option<HexOid>>()
            .map_err(|e| e.into_js_err())?
            .map(|b| b.0);

        let mut start_ops = Vec::new();
        for js_op in js_start_ops.values() {
            if let Some(op) = js_op?.into_operation()? {
                start_ops.push(op);
            }
        }

        let (tree, operations) = memo::WorkTree::new(
            replica_id,
            base,
            start_ops,
            Rc::new(git),
            Some(Rc::new(observer)),
        )
        .map_err(|e| e.into_js_err())?;
        Ok(WorkTreeNewResult {
            tree: Some(WorkTree(tree)),
            operations: Some(StreamToAsyncIterator::new(
                operations
                    .map(|op| JsValue::from(OperationEnvelope::new(op)))
                    .map_err(|e| e.into_js_err()),
            )),
        })
    }

    pub fn version(&self) -> Vec<u8> {
        bincode::serialize(&self.0.version()).unwrap()
    }

    pub fn observed(&self, version_bytes: &[u8]) -> Result<bool, JsValue> {
        let version = bincode::deserialize(&version_bytes).map_err(|e| e.into_js_err())?;
        Ok(self.0.observed(version))
    }

    pub fn head(&self) -> JsValue {
        JsValue::from_serde(&self.0.head().map(|head| HexOid(head))).unwrap()
    }

    pub fn reset(&mut self, base: JsValue) -> Result<StreamToAsyncIterator, JsValue> {
        let base = base
            .into_serde::<Option<HexOid>>()
            .map_err(|e| e.into_js_err())?
            .map(|b| b.0);
        Ok(StreamToAsyncIterator::new(
            self.0
                .reset(base)
                .map(|op| JsValue::from(OperationEnvelope::new(op)))
                .map_err(|e| e.into_js_err()),
        ))
    }

    pub fn apply_ops(&mut self, js_ops: js_sys::Array) -> Result<StreamToAsyncIterator, JsValue> {
        let mut ops = Vec::new();
        for js_op in js_ops.values() {
            if let Some(op) = js_op?.into_operation()? {
                ops.push(op);
            }
        }

        self.0
            .apply_ops(ops)
            .map(|fixup_ops| {
                StreamToAsyncIterator::new(
                    fixup_ops
                        .map(|op| JsValue::from(OperationEnvelope::new(op)))
                        .map_err(|e| e.into_js_err()),
                )
            })
            .map_err(|e| e.into_js_err())
    }

    pub fn create_file(
        &self,
        path: String,
        file_type: JsValue,
    ) -> Result<OperationEnvelope, JsValue> {
        let file_type = file_type.into_serde().map_err(|e| e.into_js_err())?;
        self.0
            .create_file(&path, file_type)
            .map(|operation| OperationEnvelope::new(operation))
            .map_err(|e| e.into_js_err())
    }

    pub fn rename(&self, old_path: String, new_path: String) -> Result<OperationEnvelope, JsValue> {
        self.0
            .rename(&old_path, &new_path)
            .map(|operation| OperationEnvelope::new(operation))
            .map_err(|e| e.into_js_err())
    }

    pub fn remove(&self, path: String) -> Result<OperationEnvelope, JsValue> {
        self.0
            .remove(&path)
            .map(|operation| OperationEnvelope::new(operation))
            .map_err(|e| e.into_js_err())
    }

    pub fn exists(&self, path: String) -> bool {
        self.0.exists(&path)
    }

    pub fn open_text_file(&mut self, path: String) -> js_sys::Promise {
        future_to_promise(
            self.0
                .open_text_file(path)
                .map(|buffer_id| JsValue::from_serde(&buffer_id).unwrap())
                .map_err(|e| e.into_js_err()),
        )
    }

    pub fn path(&self, buffer_id: JsValue) -> Result<Option<String>, JsValue> {
        let buffer_id = buffer_id.into_serde().map_err(|e| e.into_js_err())?;
        Ok(self
            .0
            .path(buffer_id)
            .map(|path| path.to_string_lossy().into_owned()))
    }

    pub fn text(&self, buffer_id: JsValue) -> Result<JsValue, JsValue> {
        let buffer_id = buffer_id.into_serde().map_err(|e| e.into_js_err())?;
        self.0
            .text(buffer_id)
            .map(|text| JsValue::from_str(&text.into_string()))
            .map_err(|e| e.into_js_err())
    }

    pub fn buffer_deferred_ops_len(&self, buffer_id: JsValue) -> Result<u32, JsValue> {
        let buffer_id = buffer_id.into_serde().map_err(|e| e.into_js_err())?;
        self.0
            .buffer_deferred_ops_len(buffer_id)
            .map(|len| len as u32)
            .map_err(|e| e.into_js_err())
    }

    pub fn edit(
        &self,
        buffer_id: JsValue,
        old_ranges: JsValue,
        new_text: &str,
    ) -> Result<OperationEnvelope, JsValue> {
        let buffer_id = buffer_id.into_serde().map_err(|e| e.into_js_err())?;
        let old_ranges = old_ranges.into_ranges_vec()?;
        self.0
            .edit_2d(buffer_id, old_ranges, new_text)
            .map(|op| OperationEnvelope::new(op))
            .map_err(|e| e.into_js_err())
    }

    pub fn add_selection_set(
        &self,
        buffer_id: JsValue,
        ranges: JsValue,
    ) -> Result<JsValue, JsValue> {
        let buffer_id = buffer_id.into_serde().map_err(|e| e.into_js_err())?;
        let ranges = ranges.into_ranges_vec()?;
        let (set_id, op) = self
            .0
            .add_selection_set(buffer_id, ranges)
            .map_err(|e| e.into_js_err())?;
        Ok(JsValue::from(AddSelectionSetResult {
            set_id,
            operation: Some(OperationEnvelope::new(op)),
        }))
    }

    pub fn replace_selection_set(
        &self,
        buffer_id: JsValue,
        set_id: JsValue,
        ranges: JsValue,
    ) -> Result<OperationEnvelope, JsValue> {
        let buffer_id = buffer_id.into_serde().map_err(|e| e.into_js_err())?;
        let set_id = set_id.into_serde().map_err(|e| e.into_js_err())?;
        let ranges = ranges.into_ranges_vec()?;
        let op = self
            .0
            .replace_selection_set(buffer_id, set_id, ranges)
            .map_err(|e| e.into_js_err())?;
        Ok(OperationEnvelope::new(op))
    }

    pub fn remove_selection_set(
        &self,
        buffer_id: JsValue,
        set_id: JsValue,
    ) -> Result<OperationEnvelope, JsValue> {
        let buffer_id = buffer_id.into_serde().map_err(|e| e.into_js_err())?;
        let set_id = set_id.into_serde().map_err(|e| e.into_js_err())?;
        let op = self
            .0
            .remove_selection_set(buffer_id, set_id)
            .map_err(|e| e.into_js_err())?;
        Ok(OperationEnvelope::new(op))
    }

    pub fn selection_ranges(&self, buffer_id: JsValue) -> Result<JsValue, JsValue> {
        let buffer_id = buffer_id.into_serde().map_err(|e| e.into_js_err())?;
        let selections = self
            .0
            .selection_ranges(buffer_id)
            .map_err(|e| e.into_js_err())?;
        let js_selections = JsSelections::from(selections);
        Ok(JsValue::from_serde(&js_selections).unwrap())
    }

    pub fn entries(&self, descend_into: JsValue, show_deleted: bool) -> Result<JsValue, JsValue> {
        let descend_into: Option<HashSet<PathBuf>> =
            descend_into.into_serde().map_err(|e| e.into_js_err())?;
        let mut entries = Vec::new();
        self.0.with_cursor(|cursor| loop {
            let entry = cursor.entry().unwrap();
            let mut descend = false;
            if show_deleted || entry.status != memo::FileStatus::Removed {
                let path = cursor.path().unwrap();
                entries.push(Entry {
                    file_type: entry.file_type,
                    depth: entry.depth,
                    name: entry.name.to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                    status: entry.status,
                    visible: entry.visible,
                });
                descend = descend_into.as_ref().map_or(true, |d| d.contains(path));
            }

            if !cursor.next(descend) {
                break;
            }
        });
        JsValue::from_serde(&entries).map_err(|e| e.into_js_err())
    }
}

#[wasm_bindgen]
impl WorkTreeNewResult {
    pub fn tree(&mut self) -> Result<WorkTree, JsValue> {
        self.tree
            .take()
            .ok_or(js_sys::Error::new("Cannot take tree twice").into())
    }

    pub fn operations(&mut self) -> Result<StreamToAsyncIterator, JsValue> {
        self.operations
            .take()
            .ok_or(js_sys::Error::new("Cannot take operations twice").into())
    }
}

#[wasm_bindgen]
impl AddSelectionSetResult {
    pub fn set_id(&mut self) -> JsValue {
        JsValue::from_serde(&self.set_id).unwrap()
    }

    pub fn operation(&mut self) -> Result<OperationEnvelope, JsValue> {
        self.operation
            .take()
            .ok_or(js_sys::Error::new("Cannot take operation twice").into())
    }
}

#[wasm_bindgen]
impl OperationEnvelope {
    fn new(operation: memo::OperationEnvelope) -> Self {
        OperationEnvelope(operation)
    }

    #[wasm_bindgen(js_name = epochId)]
    pub fn epoch_id(&self) -> Vec<u8> {
        let epoch_id = self.0.operation.epoch_id();
        let timestamp_bytes: [u8; 8] = unsafe { mem::transmute(epoch_id.value.to_be()) };
        let mut epoch_id_bytes = Vec::with_capacity(24);
        epoch_id_bytes.extend_from_slice(&timestamp_bytes);
        epoch_id_bytes.extend_from_slice(epoch_id.replica_id.as_bytes());
        epoch_id_bytes
    }

    #[wasm_bindgen(js_name = epochReplicaId)]
    pub fn epoch_replica_id(&self) -> JsValue {
        JsValue::from_serde(&self.0.operation.epoch_id().replica_id).unwrap()
    }

    #[wasm_bindgen(js_name = epochTimestamp)]
    pub fn epoch_timestamp(&self) -> JsValue {
        JsValue::from_serde(&self.0.operation.epoch_id().value).unwrap()
    }

    #[wasm_bindgen(js_name = epochHead)]
    pub fn epoch_head(&self) -> JsValue {
        JsValue::from_serde(&self.0.epoch_head.map(|head| HexOid(head))).unwrap()
    }

    pub fn operation(&self) -> Vec<u8> {
        self.0.operation.serialize()
    }

    #[wasm_bindgen(js_name = isSelectionUpdate)]
    pub fn is_selection_update(&self) -> bool {
        self.0.operation.is_selection_update()
    }
}

impl<T> AsyncIteratorToStream<T> {
    fn new(iterator: AsyncIteratorWrapper) -> Self {
        AsyncIteratorToStream {
            next_value: JsFuture::from(iterator.next()),
            iterator,
            _phantom: PhantomData,
        }
    }
}

impl<T> Stream for AsyncIteratorToStream<T>
where
    T: for<'de> Deserialize<'de>,
{
    type Item = T;
    type Error = String;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.next_value.poll() {
            Ok(Async::Ready(result)) => {
                let result: AsyncResult<T> = result.into_serde().map_err(|e| e.to_string())?;
                if result.done {
                    Ok(Async::Ready(None))
                } else {
                    self.next_value = JsFuture::from(self.iterator.next());
                    Ok(Async::Ready(result.value))
                }
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(error) => Err(error.into_error_message()?),
        }
    }
}

impl StreamToAsyncIterator {
    fn new<S>(stream: S) -> Self
    where
        S: 'static + Stream<Item = JsValue, Error = JsValue>,
    {
        let js_value_stream = stream.map(|value| {
            let result = JsValue::from(js_sys::Object::new());
            js_sys::Reflect::set(&result, &JsValue::from_str("value"), &value).unwrap();
            js_sys::Reflect::set(
                &result,
                &JsValue::from_str("done"),
                &JsValue::from_bool(false),
            )
            .unwrap();
            result
        });

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
                    Ok(next.unwrap_or_else(|| {
                        let result = JsValue::from(js_sys::Object::new());
                        js_sys::Reflect::set(
                            &result,
                            &JsValue::from_str("done"),
                            &JsValue::from_bool(true),
                        )
                        .unwrap();
                        result
                    }))
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
            .then(|value| match value {
                Ok(value) => value
                    .as_string()
                    .ok_or_else(|| String::from("Text is not a string")),
                Err(error) => Err(error.into_error_message()?),
            })
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error)),
        )
    }
}

impl memo::ChangeObserver for ChangeObserver {
    fn changed(
        &self,
        buffer_id: memo::BufferId,
        changes: Vec<memo::Change>,
        selection_ranges: memo::BufferSelectionRanges,
    ) {
        let changes = changes
            .into_iter()
            .map(|change| Change {
                start: change.range.start,
                end: change.range.end,
                text: String::from_utf16_lossy(&change.code_units),
            })
            .collect::<Vec<_>>();
        ChangeObserver::changed(
            self,
            JsValue::from_serde(&buffer_id).unwrap(),
            JsValue::from_serde(&changes).unwrap(),
            JsValue::from_serde(&JsSelections::from(selection_ranges)).unwrap(),
        );
    }
}

impl From<memo::BufferSelectionRanges> for JsSelections {
    fn from(selections: memo::BufferSelectionRanges) -> Self {
        let mut js_selections = JsSelections {
            local: HashMap::new(),
            remote: HashMap::new(),
        };

        for (set_id, ranges) in selections.local {
            js_selections.local.insert(
                set_id,
                ranges.into_iter().map(|range| range.into()).collect(),
            );
        }

        for (replica_id, sets) in selections.remote {
            let js_sets = sets
                .into_iter()
                .map(|ranges| ranges.into_iter().map(|range| range.into()).collect())
                .collect();
            js_selections.remote.insert(replica_id, js_sets);
        }

        js_selections
    }
}

impl From<Range<memo::Point>> for JsRange {
    fn from(range: Range<memo::Point>) -> Self {
        JsRange {
            start: range.start,
            end: range.end,
        }
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
        let hex_string = String::deserialize(deserializer)?;
        let bytes = hex::decode(&hex_string).map_err(Error::custom)?;
        let mut oid = memo::Oid::default();
        if oid.len() == bytes.len() {
            oid.copy_from_slice(&bytes);
            Ok(HexOid(oid))
        } else {
            Err(D::Error::custom(format!(
                "{} cannot be parsed as a valid object id. pass a full 40-character hex string.",
                hex_string
            )))
        }
    }
}

impl<T: ToString> IntoJsError for T {
    fn into_js_err(self) -> JsValue {
        js_sys::Error::new(&self.to_string()).into()
    }
}

impl JsValueExt for JsValue {
    fn into_operation(self) -> Result<Option<memo::Operation>, JsValue> {
        let js_bytes = self
            .dyn_into::<js_sys::Uint8Array>()
            .map_err(|_| "Operation must be Uint8Array".into_js_err())?;
        let mut bytes = Vec::with_capacity(js_bytes.byte_length() as usize);
        js_bytes.for_each(&mut |byte, _, _| bytes.push(byte));
        memo::Operation::deserialize(&bytes).map_err(|e| e.into_js_err())
    }

    fn into_ranges_vec(self) -> Result<Vec<Range<memo::Point>>, JsValue> {
        Ok(self
            .into_serde::<Vec<JsRange>>()
            .map_err(|e| e.into_js_err())?
            .into_iter()
            .map(|JsRange { start, end }| start..end)
            .collect())
    }

    fn into_error_message(self) -> Result<String, String> {
        match self.dyn_into::<js_sys::Error>() {
            Ok(js_err) => Ok(js_err.message().into()),
            Err(_) => Err(String::from(
                "An error occurred but can't be displayed because it's not an instance of an error",
            )),
        }
    }
}
