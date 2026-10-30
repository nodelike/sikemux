#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyContext {
    pub session_id: String,
    pub session_name: String,
    pub session_kind: LaunchKind,
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

/// What a terminal was opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchKind {
    /// A shell in a project.
    Project,
    /// A command the person or an agent runs.
    Command,
    Agent,
    Task,
    Ssh,
    /// A project's setup script.
    Setup,
}

impl LaunchKind {
    /// How a shell's environment names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Command => "command",
            Self::Agent => "agent",
            Self::Task => "task",
            Self::Ssh => "ssh",
            Self::Setup => "setup",
        }
    }
}
