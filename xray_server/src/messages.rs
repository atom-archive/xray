use std::net::SocketAddr;
use std::path::PathBuf;
use serde_json;
use xray_core::{WindowId, ViewId, WindowUpdate};

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    StartApp,
    StartCli { headless: bool },
    TcpListen {
        port: u16
    },
    StartWindow {
        window_id: WindowId,
        height: f64,
    },
    OpenWorkspace {
        paths: Vec<PathBuf>,
    },
    ConnectToPeer {
        address: SocketAddr,
    },
    Action {
        view_id: ViewId,
        action: serde_json::Value,
    },
}

#[derive(Serialize, Debug)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    OpenWindow { window_id: WindowId },
    UpdateWindow(WindowUpdate),
    Error { description: String },
    Ok,
}
