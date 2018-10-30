
use std::io::Write;
use std::borrow::Cow;
use quick_protobuf::{MessageRead, MessageWrite, BytesReader, Writer, Result};
use quick_protobuf::sizeofs::*;
use super::*;

#[derive(Debug, Default, PartialEq, Clone)]
pub struct BufferOperation<'a> {
    pub start_id: Option<Timestamp<'a>>,
    pub start_offset: Option<u64>,
    pub end_id: Option<Timestamp<'a>>,
    pub end_offset: Option<u64>,
    pub version_in_range: Option<GlobalTimestamp<'a>>,
    pub new_text: Option<Cow<'a, str>>,
    pub local_timestamp: Option<Timestamp<'a>>,
    pub lamport_timestamp: Option<Timestamp<'a>>,
}

impl<'a> MessageRead<'a> for BufferOperation<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(10) => msg.start_id = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(16) => msg.start_offset = Some(r.read_uint64(bytes)?),
                Ok(26) => msg.end_id = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(32) => msg.end_offset = Some(r.read_uint64(bytes)?),
                Ok(42) => msg.version_in_range = Some(r.read_message::<GlobalTimestamp>(bytes)?),
                Ok(50) => msg.new_text = Some(r.read_string(bytes).map(Cow::Borrowed)?),
                Ok(58) => msg.local_timestamp = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(66) => msg.lamport_timestamp = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for BufferOperation<'a> {
    fn get_size(&self) -> usize {
        0
        + self.start_id.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.start_offset.as_ref().map_or(0, |m| 1 + sizeof_varint(*(m) as u64))
        + self.end_id.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.end_offset.as_ref().map_or(0, |m| 1 + sizeof_varint(*(m) as u64))
        + self.version_in_range.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.new_text.as_ref().map_or(0, |m| 1 + sizeof_len((m).len()))
        + self.local_timestamp.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.lamport_timestamp.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        if let Some(ref s) = self.start_id { w.write_with_tag(10, |w| w.write_message(s))?; }
        if let Some(ref s) = self.start_offset { w.write_with_tag(16, |w| w.write_uint64(*s))?; }
        if let Some(ref s) = self.end_id { w.write_with_tag(26, |w| w.write_message(s))?; }
        if let Some(ref s) = self.end_offset { w.write_with_tag(32, |w| w.write_uint64(*s))?; }
        if let Some(ref s) = self.version_in_range { w.write_with_tag(42, |w| w.write_message(s))?; }
        if let Some(ref s) = self.new_text { w.write_with_tag(50, |w| w.write_string(&**s))?; }
        if let Some(ref s) = self.local_timestamp { w.write_with_tag(58, |w| w.write_message(s))?; }
        if let Some(ref s) = self.lamport_timestamp { w.write_with_tag(66, |w| w.write_message(s))?; }
        Ok(())
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct Timestamp<'a> {
    pub replica_id: Option<ReplicaId<'a>>,
    pub value: Option<u64>,
}

impl<'a> MessageRead<'a> for Timestamp<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(10) => msg.replica_id = Some(r.read_message::<ReplicaId>(bytes)?),
                Ok(16) => msg.value = Some(r.read_uint64(bytes)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for Timestamp<'a> {
    fn get_size(&self) -> usize {
        0
        + self.replica_id.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.value.as_ref().map_or(0, |m| 1 + sizeof_varint(*(m) as u64))
    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        if let Some(ref s) = self.replica_id { w.write_with_tag(10, |w| w.write_message(s))?; }
        if let Some(ref s) = self.value { w.write_with_tag(16, |w| w.write_uint64(*s))?; }
        Ok(())
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct GlobalTimestamp<'a> {
    pub timestamps: Vec<Timestamp<'a>>,
}

impl<'a> MessageRead<'a> for GlobalTimestamp<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(10) => msg.timestamps.push(r.read_message::<Timestamp>(bytes)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for GlobalTimestamp<'a> {
    fn get_size(&self) -> usize {
        0
        + self.timestamps.iter().map(|s| 1 + sizeof_len((s).get_size())).sum::<usize>()
    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        for s in &self.timestamps { w.write_with_tag(10, |w| w.write_message(s))?; }
        Ok(())
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct ReplicaId<'a> {
    pub uuid: Option<Cow<'a, [u8]>>,
}

impl<'a> MessageRead<'a> for ReplicaId<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(10) => msg.uuid = Some(r.read_bytes(bytes).map(Cow::Borrowed)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for ReplicaId<'a> {
    fn get_size(&self) -> usize {
        0
        + self.uuid.as_ref().map_or(0, |m| 1 + sizeof_len((m).len()))
    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        if let Some(ref s) = self.uuid { w.write_with_tag(10, |w| w.write_bytes(&**s))?; }
        Ok(())
    }
}
