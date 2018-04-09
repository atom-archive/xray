use std::collections::{HashMap, HashSet};

pub type RequestId = usize;
pub type ServiceId = usize;

#[derive(Serialize, Deserialize)]
pub enum MessageToClient {
    Update {
        insertions: HashMap<ServiceId, Vec<u8>>,
        updates: HashMap<ServiceId, Vec<Vec<u8>>>,
        removals: HashSet<ServiceId>,
        responses: HashMap<ServiceId, Vec<(RequestId, Response)>>,
    },
    Err(String),
}

#[derive(Serialize, Deserialize)]
pub enum Response {
    Ok(Vec<u8>),
    Err(Vec<u8>),
    RpcErr(RpcError),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum RpcError {
    ServiceNotFound,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum MessageToServer {
    Request {
        service_id: ServiceId,
        request_id: RequestId,
        payload: Vec<u8>,
    },
    ServiceClientDropped(ServiceId),
}
