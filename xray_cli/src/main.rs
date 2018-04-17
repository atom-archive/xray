extern crate docopt;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use docopt::Docopt;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process;
use std::process::{Command, Stdio};

const USAGE: &'static str = "
Xray

Usage:
  xray [--socket-path=<path>] [--headless] [--listen=<port>] [--connect=<address>] [<path>...]
  xray (-h | --help)

Options:
  -h --help               Show this screen.
  -H --headless           Start Xray in headless mode.
  -l --listen=<port>      Listen on the specified port.
  -c --connect=<address>  Connect to the specified address.
";

type PortNumber = u16;

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ServerRequest {
    StartCli { headless: bool },
    OpenWorkspace { paths: Vec<PathBuf> },
    ConnectToPeer { address: SocketAddr },
    Listen { port: PortNumber },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ServerResponse {
    Ok,
    Error { description: String },
}

#[derive(Debug, Deserialize)]
struct Args {
    flag_socket_path: Option<String>,
    flag_headless: Option<bool>,
    flag_listen: Option<PortNumber>,
    flag_connect: Option<SocketAddr>,
    arg_path: Vec<PathBuf>,
}

fn main() {
    process::exit(match launch() {
        Ok(()) => 0,
        Err(description) => {
            eprintln!("{}", description);
            1
        }
    })
}

fn launch() -> Result<(), String> {
    let args: Args = Docopt::new(USAGE)
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());

    let headless = args.flag_headless.unwrap_or(false);

    const DEFAULT_SOCKET_PATH: &'static str = "/tmp/xray.sock";
    let socket_path = PathBuf::from(
        args.flag_socket_path
            .as_ref()
            .map_or(DEFAULT_SOCKET_PATH, |path| path.as_str()),
    );

    let mut socket = match UnixStream::connect(&socket_path) {
        Ok(socket) => socket,
        Err(_) => {
            let src_path = PathBuf::from(env::var("XRAY_SRC_PATH")
                .map_err(|_| "Must specify the XRAY_SRC_PATH environment variable")?);

            let server_bin_path;
            let node_env;
            if cfg!(build = "release") {
                server_bin_path = src_path.join("target/release/xray_server");
                node_env = "production";
            } else {
                server_bin_path = src_path.join("target/debug/xray_server");
                node_env = "development";
            }

            if headless {
                start_headless(&server_bin_path, &socket_path)?
            } else {
                start_electron(&src_path, &server_bin_path, &socket_path, &node_env)?
            }
        }
    };

    send_message(&mut socket, ServerRequest::StartCli { headless })?;

    if let Some(address) = args.flag_connect {
        send_message(&mut socket, ServerRequest::ConnectToPeer { address })?;
    } else if args.arg_path.len() > 0 {
        let mut paths = Vec::new();
        for path in args.arg_path {
            paths.push(fs::canonicalize(&path)
                .map_err(|error| format!("Invalid path {:?} - {}", path, error))?);
        }
        send_message(&mut socket, ServerRequest::OpenWorkspace { paths })?;
    }

    if let Some(port) = args.flag_listen {
        send_message(&mut socket, ServerRequest::Listen { port })?;
    }

    Ok(())
}

fn start_headless(server_bin_path: &Path, socket_path: &Path) -> Result<UnixStream, String> {
    let command = Command::new(server_bin_path)
        .env("XRAY_SOCKET_PATH", socket_path)
        .env("XRAY_HEADLESS", "1")
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Failed to open Xray app {}", error))?;

    let mut stdout = command.stdout.unwrap();
    let mut reader = BufReader::new(&mut stdout);
    let mut line = String::new();
    while line != "Listening\n" {
        reader
            .read_line(&mut line)
            .map_err(|_| String::from("Error reading app output"))?;
    }
    UnixStream::connect(socket_path).map_err(|_| String::from("Error connecting to socket"))
}

fn start_electron(
    src_path: &Path,
    server_bin_path: &Path,
    socket_path: &Path,
    node_env: &str,
) -> Result<UnixStream, String> {
    let electron_app_path = Path::new(src_path).join("xray_electron");
    let electron_bin_path = electron_app_path.join("node_modules/.bin/electron");
    let command = Command::new(electron_bin_path)
        .arg(electron_app_path)
        .env("XRAY_SERVER_PATH", server_bin_path)
        .env("XRAY_SOCKET_PATH", socket_path)
        .env("XRAY_HEADLESS", "0")
        .env("NODE_ENV", node_env)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Failed to open Xray app {}", error))?;

    let mut stdout = command.stdout.unwrap();
    let mut reader = BufReader::new(&mut stdout);
    let mut line = String::new();
    while line != "Listening\n" {
        reader
            .read_line(&mut line)
            .map_err(|_| String::from("Error reading app output"))?;
    }
    UnixStream::connect(socket_path).map_err(|_| String::from("Error connecting to socket"))
}

fn send_message(socket: &mut UnixStream, message: ServerRequest) -> Result<(), String> {
    let bytes = serde_json::to_vec(&message).expect("Error serializing message");
    socket
        .write(&bytes)
        .expect("Error writing to server socket");
    socket.write(b"\n").expect("Error writing to server socket");

    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("Error reading server response");
    match serde_json::from_str::<ServerResponse>(&line).expect("Error reading server response") {
        ServerResponse::Ok => Ok(()),
        ServerResponse::Error { description } => Err(description),
    }
}
