use crate::agents::AgentKind;

/// Names a session the way the provider's own rename does, so the new name
/// shows wherever the session is listed, including the provider's resume picker.
#[tauri::command]
pub async fn agent_session_rename(
    agent: AgentKind,
    cwd: String,
    session_id: String,
    title: String,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<(), String> {
    sikemux_core::sessions::rename::rename(
        agent,
        cwd,
        session_id,
        title,
        executable_path,
        config_path,
    )
    .await
}
