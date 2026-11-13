use serde::{Deserialize, Serialize};

/// A chat to move onto one of the person's accounts once it signs in.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterSignIn {
    pub agent_id: String,
    pub account_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct LauncherParams {
    /// The launcher whose account to sign in to, such as `claude:work`.
    pub launcher: String,
}

/// The page a running sign-in opened, for a client to open where no browser did.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub agent: String,
    pub config_path: Option<String>,
    pub url: String,
}
