use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct UpdateInstallParams {
    /// Stop the chats an update would wait on rather than wait.
    #[serde(default)]
    pub force: bool,
}

#[derive(Deserialize, Serialize)]
pub struct HostUpdatesParams {
    pub on: bool,
}
