use sikemux_core::sessions::context::{read_session_context, SessionContext};
use tauri::async_runtime::spawn_blocking;

use crate::agents::AgentKind;

/// A resumed chat hears nothing about its context window until it runs a turn,
/// so its transcript on disk fills the gap.
#[tauri::command]
pub async fn agent_session_context(
    agent: AgentKind,
    cwd: String,
    session_id: String,
    config_path: Option<String>,
) -> Option<SessionContext> {
    spawn_blocking(move || read_session_context(agent, &cwd, &session_id, config_path.as_deref()))
        .await
        .ok()
        .flatten()
}
