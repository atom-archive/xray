mod messages;
mod server;
mod fs;
mod json_lines_codec;

extern crate bytes;
extern crate futures;
extern crate futures_cpupool;
extern crate ignore;
extern crate parking_lot;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_process;
extern crate tokio_uds;
extern crate websocket;
extern crate xray_core;

use std::env;
use futures::Stream;
use tokio_core::reactor::Core;
use tokio_io::AsyncRead;
use tokio_uds::UnixListener;
use json_lines_codec::JsonLinesCodec;
use messages::{IncomingMessage, OutgoingMessage};
use server::Server;

fn main() {
    let headless =
        env::var("XRAY_HEADLESS").expect("Missing XRAY_HEADLESS environment variable") != "0";
    let socket_path =
        env::var("XRAY_SOCKET_PATH").expect("Missing XRAY_SOCKET_PATH environment variable");

    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let mut server = Server::new(headless, handle.clone());

    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(socket_path, &handle).unwrap();

    let handle_connections = listener.incoming().for_each(move |(socket, _)| {
        let framed_socket =
            socket.framed(JsonLinesCodec::<IncomingMessage, OutgoingMessage>::new());
        server.accept_connection(framed_socket);
        Ok(())
    });

    println!("Listening");
    core.run(handle_connections).unwrap();
}
