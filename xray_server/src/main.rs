mod messages;
mod json_lines_codec;

extern crate bytes;
extern crate futures;
#[macro_use]
extern crate serde_derive;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_process;
extern crate tokio_uds;

use futures::{Sink, Stream};
use tokio_core::reactor::Core;
use tokio_io::AsyncRead;
use tokio_uds::UnixListener;
use json_lines_codec::JsonLinesCodec;
use messages::{IncomingMessage, OutgoingMessage};

fn main() {
    let app = App::new();

    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let listener = UnixListener::bind("/tmp/xray.sock", &handle).unwrap();

    let handle_connections = listener.incoming().for_each(move |(unix_stream, _)| {
        let (responses_sink, requests_stream) = unix_stream
            .framed(JsonLinesCodec::<IncomingMessage, OutgoingMessage>::new())
            .split();

        //     let responses = handle_spawn_requests(requests_stream, handle.clone());
        //     handle.spawn(responses_sink.send_all(responses).then(|_| Ok(())));
        Ok(())
    });

    core.run(handle_connections).unwrap();
}

struct App {
    next_workspace_id: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            next_workspace_id: 1,
        }
    }
}
