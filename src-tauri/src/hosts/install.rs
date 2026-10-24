//! "Install Sikemux here", "Update", "Sign in" and "Start" on an SSH host. The core runs the installer,
//! or `sikemux login`, over the person's own SSH in a terminal the app shows, approves the
//! sign-in code that terminal shows and notes where the host was installed (ADR 0018); the app
//! only shows that terminal and says when it is done.

use serde::Serialize;
use serde_json::json;
use sikemux_core::protocol::SessionId;
use tauri::{AppHandle, Emitter, State};

use crate::error::{AppError, AppResult};
use crate::pty::{core_error, PtyManager};

const DONE_EVENT: &str = "host_installed";

/// An alias from ~/.ssh/config, never something ssh would read as an option.
pub(super) fn checked_alias(alias: &str) -> AppResult<&str> {
    let plain = !alias.is_empty()
        && !alias.starts_with('-')
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'));
    if plain {
        Ok(alias)
    } else {
        Err(AppError::BadArg(
            "that is not an SSH host from ~/.ssh/config",
        ))
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Installed {
    alias: String,
    /// Why it did not finish, when it did not.
    problem: Option<String>,
}

/// Opens a terminal on this host that runs the installer on the SSH host `alias` and signs it
/// in to the account, and answers with its session for the page to show. `host_installed`
/// says when it is on the account.
#[tauri::command]
pub async fn host_install(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    alias: String,
) -> AppResult<SessionId> {
    set_up(app, manager, alias, "install").await
}

/// Opens a terminal on this host that updates Sikemux on the SSH host `alias`, which runs an
/// older release, then restarts its core, and answers with its session for the page to show.
/// `host_installed` says when it is done; a host signed out signs in there.
#[tauri::command]
pub async fn host_update(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    alias: String,
) -> AppResult<SessionId> {
    set_up(app, manager, alias, "update").await
}

/// Opens a terminal on this host that runs `sikemux login` on the SSH host `alias`, which
/// already runs Sikemux, and answers with its session for the page to show. `host_installed`
/// says when it is on the account.
#[tauri::command]
pub async fn host_sign_in(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    alias: String,
) -> AppResult<SessionId> {
    set_up(app, manager, alias, "signIn").await
}

/// Asks the core to do `work` on the SSH host `alias`, and says when it has ended.
async fn set_up(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    alias: String,
    work: &str,
) -> AppResult<SessionId> {
    let alias = checked_alias(&alias)?.to_owned();
    let core = manager.client().await?;
    let started = core
        .call("installHost", json!({ "alias": alias, "work": work }))
        .await
        .map_err(core_error)?;
    let id: SessionId = serde_json::from_value(started["terminal"].clone())?;
    tauri::async_runtime::spawn(async move {
        let problem = match core.call("hostSetup", json!({ "alias": alias })).await {
            Ok(ended) => ended["problem"].as_str().map(str::to_owned),
            Err(error) => Some(core_error(error).to_string()),
        };
        if problem.is_none() {
            super::follow_links(&app);
        }
        let _ = app.emit_to("main", DONE_EVENT, Installed { alias, problem });
    });
    Ok(id)
}

/// Opens a terminal on this host that starts the stopped core on the SSH host `alias`, and
/// answers with its session for the page to show. `host_installed` says when it is up.
#[tauri::command]
pub async fn host_start(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    alias: String,
) -> AppResult<SessionId> {
    set_up(app, manager, alias, "start").await
}
