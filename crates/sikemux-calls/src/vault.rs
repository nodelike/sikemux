use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownUse {
    /// The agent, such as `@Otter`, or `Sikemux` for the host itself.
    pub actor: String,
    pub host: String,
    pub count: i64,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeParams {
    pub entry: String,
    pub host: String,
}

#[derive(Serialize, Deserialize)]
pub struct RecoverParams {
    pub key: String,
}

#[derive(Serialize, Deserialize)]
pub struct UsesParams {
    pub entry: String,
}
