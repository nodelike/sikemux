use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CliInstallState {
    Unavailable,
    NotInstalled,
    Installed,
    /// Only on Windows, where the command is a copy that a newer build replaces.
    Outdated,
    Conflict,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallStatus {
    pub state: CliInstallState,
    pub install_dir: String,
    pub cli_path: String,
    pub executable: Option<String>,
    pub path_configured: bool,
    pub message: String,
}
