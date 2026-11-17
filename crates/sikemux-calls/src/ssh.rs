use serde::{Deserialize, Serialize};

/// How a host's terminals reach it, the best of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reach {
    /// No terminal is open to it.
    Idle,
    Failed,
    Connecting,
    Ok,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHost {
    /// What `ssh` is given, and what the host is called.
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// The config's line, from 0, whose `Host` names it.
    pub line: usize,
    /// The file an `Include` brought it from, when it is not ~/.ssh/config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub status: Reach,
}
