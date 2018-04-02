extern crate docopt;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;

use std::env;
use std::process::{Command, Stdio};
use std::path::{Path, PathBuf};
use std::error::Error;
use docopt::Docopt;
use std::os::unix::net::UnixStream;
use serde_json::value::Value;
use std::io::{Write, BufReader, BufRead};
use std::process;
use std::fs;

const USAGE: &'static str = "
Xray

Usage:
  xray [--socket-path=<path>] <path>...
  xray (-h | --help)

Options:
  -h --help     Show this screen.
";

const DEFAULT_SOCKET_PATH: &'static str = "/tmp/xray.sock";

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ServerResponse {
    Ok,
    Error { description: String },
}

#[derive(Debug, Deserialize)]
struct Args {
    flag_socket_path: Option<String>,
    arg_path: Vec<PathBuf>,
}

fn main() {
    process::exit(match main_inner() {
        Ok(()) => 0,
        Err(description) => {
            eprintln!("{}", description);
            1
        },
    })
}

fn main_inner() -> Result<(), String> {
    let args: Args = Docopt::new(USAGE)
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());

    let socket_path = args.flag_socket_path
        .as_ref()
        .map_or(DEFAULT_SOCKET_PATH, |path| path.as_str());

    let mut socket = UnixStream::connect(socket_path);
    if socket.is_err() {
        let electron_node_env = if cfg!(debug_assertions) {
            "development"
        } else {
            "production"
        };

        let src_path = env::var("XRAY_SRC_PATH").map_err(|_| "Must specify the XRAY_SRC_PATH environment variable")?;
        let electron_app_path = Path::new(&src_path).join("xray_electron");
        let electron_bin_path = electron_app_path.join("node_modules/.bin/electron");
        let open_command = Command::new(electron_bin_path)
            .arg(electron_app_path)
            .env("XRAY_SOCKET_PATH", socket_path)
            .env("NODE_ENV", electron_node_env)
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Failed to open Xray app {}", error))?;

        let mut stdout = open_command.stdout.unwrap();
        let mut reader = BufReader::new(&mut stdout);
        let mut line = String::new();
        while line != "Listening\n" {
            reader.read_line(&mut line).expect("Error reading app output");
        }
        socket = UnixStream::connect(socket_path);
    }

    let mut socket = socket.expect("Failed to connect to server");
    write_to_socket(&mut socket, json!({ "type": "StartCli" }))
        .expect("Failed to write to socket");

    let mut paths = Vec::new();
    for path in args.arg_path {
        paths.push(fs::canonicalize(&path)
            .map_err(|error| format!("Invalid path {:?} - {}", path, error))?);
    }

    write_to_socket(&mut socket, json!({ "type": "OpenWorkspace", "paths": paths }))
        .expect("Failed to write to socket");

    let mut reader = BufReader::new(&mut socket);
    let mut line = String::new();
    reader.read_line(&mut line)
        .expect("Error reading server response");
    let response: ServerResponse = serde_json::from_str(&line)
        .expect("Error parsing server response");

    match response {
        ServerResponse::Ok => Ok(()),
        ServerResponse::Error { description } => Err(description),
    }
}

fn write_to_socket(socket: &mut UnixStream, value: Value) -> Result<(), Box<Error>> {
    let vec = serde_json::to_vec(&value)?;
    socket.write(&vec)?;
    socket.write(b"\n")?;
    Ok(())
}
