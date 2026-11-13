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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShownStatus {
    /// This host is on no account.
    #[default]
    Off,
    /// The account's vault waits for a host that holds its key to let this one in.
    Waiting,
    Open,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownVault {
    pub status: ShownStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// How many of the account's hosts hold the vault's key.
    pub holders: usize,
    /// The recovery key of the vault this host made, until the person says they saved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    pub sign_ins: Vec<ShownSignIn>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownSignIn {
    pub id: String,
    pub plugin: String,
    pub label: String,
    /// The host it was made on.
    pub origin: String,
    pub hosts: Vec<ShownHost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_use: Option<ShownUse>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownHost {
    pub key: String,
    pub name: String,
    pub this: bool,
    pub online: bool,
    pub allowed: bool,
    /// The projects its agents may use the sign-in in; none for every project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    pub refreshes: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowParams {
    pub entry: String,
    pub host: String,
    #[serde(default)]
    pub projects: Option<Vec<String>>,
}
