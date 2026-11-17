use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct RecoverParams {
    pub key: String,
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
    /// The recovery key waits to be saved, but only this computer's own window shows it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recovery_in_window: bool,
    /// The vault's sign-ins, which every host on the account uses.
    pub sign_ins: Vec<ShownSignIn>,
    /// The sign-ins that stay on this host, out of the vault.
    pub kept: Vec<ShownSignIn>,
    /// New sign-ins on this host stay here.
    pub keep_new: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownSignIn {
    pub plugin: String,
    pub identity: String,
    pub label: String,
}

/// Keeps a sign-in on this host and out of the vault, or puts it back in.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepParams {
    pub plugin: String,
    pub identity: String,
    pub kept: bool,
}

#[derive(Serialize, Deserialize)]
pub struct KeepNewParams {
    pub on: bool,
}
