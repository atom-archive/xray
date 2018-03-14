use std::path::PathBuf;
use serde_json;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    StartWindow { workspace_id: usize },
    StartApplication,
    OpenWorkspace { paths: Vec<PathBuf> },
    Action {
        view_type: String,
        view_id: usize,
        action: serde_json::Value,
    },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    Acknowledge,
    OpenWindow { workspace_id: usize },
    WindowState {  },
}
