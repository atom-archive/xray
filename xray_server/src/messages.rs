use serde_json;
use std::net::SocketAddr;
use std::path::PathBuf;
use xray_core::{ViewId, WindowId, WindowUpdate};

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    StartApp,
    StartCli {
        headless: bool,
    },
    TcpListen {
        port: u16,
    },
    WebsocketListen {
        port: u16,
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
