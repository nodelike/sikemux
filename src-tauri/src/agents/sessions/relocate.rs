use crate::agents::AgentKind;

/// Files a stopped Claude or Codex session under another project folder, so it
/// lists there and resumes with that folder as its working directory.
#[tauri::command]
pub async fn agent_session_relocate(
    agent: AgentKind,
    from_cwd: String,
    to_cwd: String,
    session_id: String,
    config_path: Option<String>,
) -> Result<(), String> {
    sikemux_core::sessions::relocate::relocate(agent, from_cwd, to_cwd, session_id, config_path)
        .await
}
