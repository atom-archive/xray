use std::path::PathBuf;
use serde_json;
use app::WindowId;
use xray_core::window::{self, ViewId};

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    StartApp,
    StartCli,
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
    Acknowledge,
    OpenWindow { window_id: WindowId },
    UpdateWindow(window::WindowUpdate),
}
