use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    StartWindow { workspace_id: usize },
    StartApplication,
    OpenWorkspace { paths: Vec<PathBuf> },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    Acknowledge,
    OpenWindow { workspace_id: usize },
    WindowState,
}
