use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveChatToHostParams {
    pub agent_id: String,
    /// The host it goes to, by its core's key.
    pub host: String,
    /// The project there, by its id there.
    pub project: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMoved {
    /// The chat on the host it went to.
    pub agent_id: String,
}

#[derive(Deserialize, Serialize)]
pub struct AliasParams {
    pub alias: String,
}

/// How a set-up ended: why it stopped short, if it did, and the host it added to the account.
#[derive(Deserialize, Serialize)]
pub struct SetupEnded {
    pub problem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOnParams {
    pub path: PathBuf,
    pub host: String,
    /// What is not committed goes too, as a commit on top.
    #[serde(default)]
    pub uncommitted: bool,
}
