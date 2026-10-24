//! The page's requests to a core: this host's own, or `host`, another the app
//! joined. The core decides who may make each, so the app passes every one on
//! unchanged.

use serde_json::Value;
use sikemux_core::protocol::{Request, Response};
use tauri::State;

use crate::error::AppResult;
use crate::hosts::{self, Hosts};
use crate::pty::{core_error, PtyManager};

#[tauri::command]
pub async fn core_call(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    host: Option<String>,
    method: String,
    params: Value,
) -> AppResult<Value> {
    let client = hosts::client(&manager, &hosts, host.as_deref()).await?;
    client.call(&method, params).await.map_err(core_error)
}

/// One of the core's requests as is, for the page to reach a request no app
/// command has its own work for.
#[tauri::command]
pub async fn core_request(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    host: Option<String>,
    request: Request,
) -> AppResult<Response> {
    let client = hosts::client(&manager, &hosts, host.as_deref()).await?;
    client.request(request).await.map_err(core_error)
}
