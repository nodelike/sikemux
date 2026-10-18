use crate::agents::AgentKind;

/// Removes a Claude or Codex session from the provider's own storage for good,
/// so it no longer lists anywhere and can no longer be resumed.
#[tauri::command]
pub async fn agent_session_delete(
    agent: AgentKind,
    cwd: String,
    session_id: String,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<(), String> {
    sikemux_core::sessions::delete::delete(agent, cwd, session_id, executable_path, config_path)
        .await
}
