use sikemux_core::protocol::BuildChannel;
use sikemux_core::usage::{self, Active, Install};
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::pty::PtyManager;

/// Sends one anonymous `app_active` event per install per UTC day, carrying a
/// hash of the account the core is signed in to.
#[tauri::command]
pub async fn usage_report_active(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    channel: BuildChannel,
) -> AppResult<()> {
    if !Install::App.reports() {
        return Ok(());
    }
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("usage data directory unavailable: {error}")))?;
    let account = match manager.client().await {
        Ok(core) => core
            .remote_status()
            .await
            .ok()
            .and_then(|status| status.owner),
        Err(_) => None,
    };
    let active = Active {
        install: Install::App,
        version: &app.package_info().version.to_string(),
        channel,
        account: account.as_deref(),
    };
    usage::report_active(&data_dir, active)
        .await
        .map_err(AppError::Http)
}
