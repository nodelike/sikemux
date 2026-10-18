//! Sharing a chat read-only. The core runs the shares; these commands pass
//! the app's asks on to it, and give a page watching someone's shared chat
//! the server's share socket and a sign-in to watch with.

use serde::Serialize;
use serde_json::Value;
use sikemux_core::client::Reply;
use sikemux_core::protocol::{Request, Response};
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::pty::{core_error, PtyManager};

/// The core's share calls, the only ones the page may make through this.
const METHODS: [&str; 3] = ["shareChat", "stopShare", "shares"];

#[tauri::command]
pub async fn share_call(
    manager: State<'_, PtyManager>,
    method: String,
    params: Value,
) -> AppResult<Value> {
    if !METHODS.contains(&method.as_str()) {
        return Err(AppError::Other(format!("{method} is not a share call")));
    }
    let client = manager.client().await?;
    let reply = client
        .submit(Request::Call { method, params }, |reply| reply)
        .map_err(core_error)?
        .await
        .map_err(core_error)?
        .map_err(core_error)?;
    match reply {
        Reply::Response(Response::Called { result }) => Ok(result),
        _ => Err(AppError::Other(
            "the core answered a share call with something else".into(),
        )),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAccess {
    socket: String,
    /// None when signed out.
    token: Option<String>,
}

#[tauri::command]
pub async fn share_access() -> AppResult<ShareAccess> {
    Ok(ShareAccess {
        socket: sikemux_core::accounts::live::socket_url(
            &sikemux_core::accounts::api_base(),
            "/v1/share",
        ),
        token: crate::account::access_token().await?,
    })
}
