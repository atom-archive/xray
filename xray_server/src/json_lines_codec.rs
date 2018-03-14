extern crate serde;
extern crate serde_json;

use std::io;
use bytes::BytesMut;
use self::serde::{Deserialize, Serialize};
use tokio_io::codec::{Decoder, Encoder};
use std::marker::PhantomData;

pub struct JsonLinesCodec<In, Out> {
    phantom1: PhantomData<In>,
    phantom2: PhantomData<Out>,
}

impl<In, Out> JsonLinesCodec<In, Out>
where
    In: for<'a> serde::Deserialize<'a>,
    Out: serde::Serialize,
{
    pub fn new() -> Self {
        JsonLinesCodec {
            phantom1: PhantomData,
            phantom2: PhantomData,
        }
    }
}

impl<In, Out> Decoder for JsonLinesCodec<In, Out>
where
    In: for<'a> serde::Deserialize<'a>,
    Out: serde::Serialize,
{
    type Item = In;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(index) = buf.iter().position(|byte| *byte == b'\n') {
            let line = buf.split_to(index + 1);
            let item = serde_json::from_slice(&line[0..line.len() - 1])?;
            Ok(Some(item))
        } else {
            Ok(None)
        }
    }
}

impl<In, Out> Encoder for JsonLinesCodec<In, Out>
where
    In: for<'a> serde::Deserialize<'a>,
    Out: serde::Serialize,
{
    type Item = Out;
    type Error = io::Error;

    fn encode(&mut self, msg: Self::Item, buf: &mut BytesMut) -> io::Result<()> {
        let mut vec = serde_json::to_vec(&msg)?;
        vec.push(b'\n');
        buf.extend_from_slice(&vec);
        Ok(())
    }
}
