#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyContext {
    pub session_id: String,
    pub session_name: String,
    pub session_kind: String,
    pub project: Option<String>,
    pub window_id: Option<String>,
    pub pane_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    #[serde(default)]
    pub initial_prompt_submitted: bool,
    /// Explicit opt-in. Absent/false preserves the exact historical shell
    /// launch path and performs no startup-file or argv injection.
    #[serde(default)]
    pub shell_integration: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyAgentProfile {
    pub config_path: Option<String>,
    #[serde(default)]
    pub environment_keys: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyDirectCommand {
    pub program: String,
    pub args: Vec<String>,
    pub profile: Option<PtyAgentProfile>,
}
