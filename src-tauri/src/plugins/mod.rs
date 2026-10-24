//! The plugins run in the core, so the window has none of its own: this
//! command takes the panes' calls to the core. Their streams arrive as the
//! core's `Stream` events.

use serde_json::{json, Value};
use sikemux_core::server::plugins::{Failure, Outcome};
use sikemux_plugin_api::PluginError;

use crate::error::{AppError, AppResult};
use crate::pty::{core_error, PtyManager};

fn plugin_error(failure: Failure) -> AppError {
    let error = PluginError::new(failure.category, failure.message);
    AppError::Plugin {
        plugin: failure.plugin,
        error: match failure.status {
            Some(status) => error.with_status(status),
            None => error,
        },
    }
}

async fn forward(manager: &PtyManager, method: &str, params: Value) -> AppResult<Value> {
    let client = manager.client().await?;
    client.call(method, params).await.map_err(core_error)
}

#[tauri::command]
pub async fn plugin_call(
    manager: tauri::State<'_, PtyManager>,
    plugin: String,
    method: String,
    params: Value,
) -> AppResult<Value> {
    let answer = forward(
        &manager,
        "plugin",
        json!({ "plugin": plugin, "method": method, "params": params }),
    )
    .await?;
    match serde_json::from_value(answer)? {
        Outcome::Value(value) => Ok(value),
        Outcome::Failed(failure) => Err(plugin_error(failure)),
    }
}
