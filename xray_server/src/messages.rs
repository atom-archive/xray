use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    OpenWorkspace { paths: Vec<PathBuf> },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    OpenWindow { workspace_id: usize },
}
