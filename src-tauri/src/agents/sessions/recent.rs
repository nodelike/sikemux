use sikemux_core::sessions::recent::{recent_page, RecentPage, RecentRequest};
use tauri::async_runtime::spawn_blocking;

/// Saved chats across providers and projects, newest first, a page at a time.
#[tauri::command]
pub async fn agent_recent_sessions(request: RecentRequest) -> Result<RecentPage, String> {
    spawn_blocking(move || recent_page(request))
        .await
        .map_err(|error| format!("agent_recent_sessions join: {error}"))?
}
