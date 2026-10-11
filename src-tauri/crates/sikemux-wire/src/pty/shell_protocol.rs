#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellPhase {
    #[default]
    Unknown,
    Prompt,
    Input,
    Running,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellMetadataSnapshot {
    pub revision: u64,
    pub cwd: Option<String>,
    pub phase: ShellPhase,
    pub last_exit_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellBoundary {
    Cwd,
    PromptStart,
    CommandStart,
    CommandExecuted,
    CommandFinished,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyShellMetadataEvent<Id = u32> {
    pub pty_id: Id,
    pub revision: u64,
    pub boundary: ShellBoundary,
    pub cwd: Option<String>,
    pub phase: ShellPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}
