
use std::io::Write;
use std::borrow::Cow;
use quick_protobuf::{MessageRead, MessageWrite, BytesReader, Writer, Result};
use quick_protobuf::sizeofs::*;
use super::*;

#[derive(Debug, Default, PartialEq, Clone)]
pub struct EpochOperation<'a> {
    pub variant: mod_EpochOperation::OneOfvariant<'a>,
}

impl<'a> MessageRead<'a> for EpochOperation<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(10) => msg.variant = mod_EpochOperation::OneOfvariant::InsertMetadata(r.read_message::<mod_EpochOperation::InsertMetadata>(bytes)?),
                Ok(18) => msg.variant = mod_EpochOperation::OneOfvariant::UpdateParent(r.read_message::<mod_EpochOperation::UpdateParent>(bytes)?),
                Ok(26) => msg.variant = mod_EpochOperation::OneOfvariant::EditText(r.read_message::<mod_EpochOperation::EditText>(bytes)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for EpochOperation<'a> {
    fn get_size(&self) -> usize {
        0
        + match self.variant {
            mod_EpochOperation::OneOfvariant::InsertMetadata(ref m) => 1 + sizeof_len((m).get_size()),
            mod_EpochOperation::OneOfvariant::UpdateParent(ref m) => 1 + sizeof_len((m).get_size()),
            mod_EpochOperation::OneOfvariant::EditText(ref m) => 1 + sizeof_len((m).get_size()),
            mod_EpochOperation::OneOfvariant::None => 0,
    }    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        match self.variant {            mod_EpochOperation::OneOfvariant::InsertMetadata(ref m) => { w.write_with_tag(10, |w| w.write_message(m))? },
            mod_EpochOperation::OneOfvariant::UpdateParent(ref m) => { w.write_with_tag(18, |w| w.write_message(m))? },
            mod_EpochOperation::OneOfvariant::EditText(ref m) => { w.write_with_tag(26, |w| w.write_message(m))? },
            mod_EpochOperation::OneOfvariant::None => {},
    }        Ok(())
    }
}

pub mod mod_EpochOperation {

use std::borrow::Cow;
use super::*;

#[derive(Debug, Default, PartialEq, Clone)]
pub struct InsertMetadata<'a> {
    pub file_id: Option<FileId<'a>>,
    pub file_type: Option<mod_EpochOperation::FileType>,
    pub parent_id: Option<FileId<'a>>,
    pub name_in_parent: Option<Cow<'a, str>>,
    pub local_timestamp: Option<Timestamp<'a>>,
    pub lamport_timestamp: Option<Timestamp<'a>>,
}

impl<'a> MessageRead<'a> for InsertMetadata<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(10) => msg.file_id = Some(r.read_message::<FileId>(bytes)?),
                Ok(16) => msg.file_type = Some(r.read_enum(bytes)?),
                Ok(26) => msg.parent_id = Some(r.read_message::<FileId>(bytes)?),
                Ok(34) => msg.name_in_parent = Some(r.read_string(bytes).map(Cow::Borrowed)?),
                Ok(42) => msg.local_timestamp = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(50) => msg.lamport_timestamp = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for InsertMetadata<'a> {
    fn get_size(&self) -> usize {
        0
        + self.file_id.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.file_type.as_ref().map_or(0, |m| 1 + sizeof_varint(*(m) as u64))
        + self.parent_id.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.name_in_parent.as_ref().map_or(0, |m| 1 + sizeof_len((m).len()))
        + self.local_timestamp.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.lamport_timestamp.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        if let Some(ref s) = self.file_id { w.write_with_tag(10, |w| w.write_message(s))?; }
        if let Some(ref s) = self.file_type { w.write_with_tag(16, |w| w.write_enum(*s as i32))?; }
        if let Some(ref s) = self.parent_id { w.write_with_tag(26, |w| w.write_message(s))?; }
        if let Some(ref s) = self.name_in_parent { w.write_with_tag(34, |w| w.write_string(&**s))?; }
        if let Some(ref s) = self.local_timestamp { w.write_with_tag(42, |w| w.write_message(s))?; }
        if let Some(ref s) = self.lamport_timestamp { w.write_with_tag(50, |w| w.write_message(s))?; }
        Ok(())
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct UpdateParent<'a> {
    pub child_id: Option<FileId<'a>>,
    pub new_parent_id: Option<FileId<'a>>,
    pub new_name_in_parent: Option<Cow<'a, str>>,
    pub local_timestamp: Option<Timestamp<'a>>,
    pub lamport_timestamp: Option<Timestamp<'a>>,
}

impl<'a> MessageRead<'a> for UpdateParent<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(10) => msg.child_id = Some(r.read_message::<FileId>(bytes)?),
                Ok(18) => msg.new_parent_id = Some(r.read_message::<FileId>(bytes)?),
                Ok(26) => msg.new_name_in_parent = Some(r.read_string(bytes).map(Cow::Borrowed)?),
                Ok(34) => msg.local_timestamp = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(42) => msg.lamport_timestamp = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for UpdateParent<'a> {
    fn get_size(&self) -> usize {
        0
        + self.child_id.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.new_parent_id.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.new_name_in_parent.as_ref().map_or(0, |m| 1 + sizeof_len((m).len()))
        + self.local_timestamp.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.lamport_timestamp.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        if let Some(ref s) = self.child_id { w.write_with_tag(10, |w| w.write_message(s))?; }
        if let Some(ref s) = self.new_parent_id { w.write_with_tag(18, |w| w.write_message(s))?; }
        if let Some(ref s) = self.new_name_in_parent { w.write_with_tag(26, |w| w.write_string(&**s))?; }
        if let Some(ref s) = self.local_timestamp { w.write_with_tag(34, |w| w.write_message(s))?; }
        if let Some(ref s) = self.lamport_timestamp { w.write_with_tag(42, |w| w.write_message(s))?; }
        Ok(())
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct EditText<'a> {
    pub file_id: Option<FileId<'a>>,
    pub edits: Vec<BufferOperation<'a>>,
    pub local_timestamp: Option<Timestamp<'a>>,
    pub lamport_timestamp: Option<Timestamp<'a>>,
}

impl<'a> MessageRead<'a> for EditText<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(10) => msg.file_id = Some(r.read_message::<FileId>(bytes)?),
                Ok(18) => msg.edits.push(r.read_message::<BufferOperation>(bytes)?),
                Ok(26) => msg.local_timestamp = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(34) => msg.lamport_timestamp = Some(r.read_message::<Timestamp>(bytes)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for EditText<'a> {
    fn get_size(&self) -> usize {
        0
        + self.file_id.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.edits.iter().map(|s| 1 + sizeof_len((s).get_size())).sum::<usize>()
        + self.local_timestamp.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
        + self.lamport_timestamp.as_ref().map_or(0, |m| 1 + sizeof_len((m).get_size()))
    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        if let Some(ref s) = self.file_id { w.write_with_tag(10, |w| w.write_message(s))?; }
        for s in &self.edits { w.write_with_tag(18, |w| w.write_message(s))?; }
        if let Some(ref s) = self.local_timestamp { w.write_with_tag(26, |w| w.write_message(s))?; }
        if let Some(ref s) = self.lamport_timestamp { w.write_with_tag(34, |w| w.write_message(s))?; }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum FileType {
    Directory = 1,
    Text = 2,
}

impl Default for FileType {
    fn default() -> Self {
        FileType::Directory
    }
}

impl From<i32> for FileType {
    fn from(i: i32) -> Self {
        match i {
            1 => FileType::Directory,
            2 => FileType::Text,
            _ => Self::default(),
        }
    }
}

impl<'a> From<&'a str> for FileType {
    fn from(s: &'a str) -> Self {
        match s {
            "Directory" => FileType::Directory,
            "Text" => FileType::Text,
            _ => Self::default(),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum OneOfvariant<'a> {
    InsertMetadata(mod_EpochOperation::InsertMetadata<'a>),
    UpdateParent(mod_EpochOperation::UpdateParent<'a>),
    EditText(mod_EpochOperation::EditText<'a>),
    None,
}

impl<'a> Default for OneOfvariant<'a> {
    fn default() -> Self {
        OneOfvariant::None
    }
}

}

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
pub struct FileId<'a> {
    pub variant: mod_FileId::OneOfvariant<'a>,
}

impl<'a> MessageRead<'a> for FileId<'a> {
    fn from_reader(r: &mut BytesReader, bytes: &'a [u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !r.is_eof() {
            match r.next_tag(bytes) {
                Ok(8) => msg.variant = mod_FileId::OneOfvariant::Base(r.read_uint64(bytes)?),
                Ok(18) => msg.variant = mod_FileId::OneOfvariant::New(r.read_message::<Timestamp>(bytes)?),
                Ok(t) => { r.read_unknown(bytes, t)?; }
                Err(e) => return Err(e),
            }
        }
        Ok(msg)
    }
}

impl<'a> MessageWrite for FileId<'a> {
    fn get_size(&self) -> usize {
        0
        + match self.variant {
            mod_FileId::OneOfvariant::Base(ref m) => 1 + sizeof_varint(*(m) as u64),
            mod_FileId::OneOfvariant::New(ref m) => 1 + sizeof_len((m).get_size()),
            mod_FileId::OneOfvariant::None => 0,
    }    }

    fn write_message<W: Write>(&self, w: &mut Writer<W>) -> Result<()> {
        match self.variant {            mod_FileId::OneOfvariant::Base(ref m) => { w.write_with_tag(8, |w| w.write_uint64(*m))? },
            mod_FileId::OneOfvariant::New(ref m) => { w.write_with_tag(18, |w| w.write_message(m))? },
            mod_FileId::OneOfvariant::None => {},
    }        Ok(())
    }
}

pub mod mod_FileId {

use super::*;

#[derive(Debug, PartialEq, Clone)]
pub enum OneOfvariant<'a> {
    Base(u64),
    New(Timestamp<'a>),
    None,
}

impl<'a> Default for OneOfvariant<'a> {
    fn default() -> Self {
        OneOfvariant::None
    }
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

