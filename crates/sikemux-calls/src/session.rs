use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct SessionParams {
    pub id: sikemux_wire::protocol::SessionId,
}
