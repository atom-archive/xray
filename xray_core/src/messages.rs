use app::WindowId;
use std::path::PathBuf;
use serde_json;
use window::{ViewId, WindowUpdate};

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    StartApp,
    StartCli { headless: bool },
    Listen {
        port: u16
    },
    StartWindow {
        window_id: WindowId,
        height: f64,
    },
    OpenWorkspace {
        paths: Vec<PathBuf>,
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
