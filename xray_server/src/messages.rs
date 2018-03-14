use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    StartWindow,
    StartApplication,
    OpenWorkspace { paths: Vec<PathBuf> },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    Acknowledge,
    OpenWindow { workspace_id: usize },
    WindowState,
}
