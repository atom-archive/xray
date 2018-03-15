use std::path::PathBuf;
use serde_json;
use app::WindowId;
use window::{self, ViewId};

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    StartWindow { window_id: WindowId },
    StartApplication,
    OpenWorkspace { paths: Vec<PathBuf> },
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
    WindowUpdate(window::WindowUpdate),
}
