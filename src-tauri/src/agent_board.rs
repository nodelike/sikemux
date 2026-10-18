//! Each project's board of cards. The core keeps it and tells the window when
//! it changes; the page reads it and the person changes it through this.

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::agent_names::call;
use crate::error::{AppError, AppResult};
use crate::pty::PtyManager;

/// The core's board calls, the only ones the page may make through this.
const METHODS: [&str; 5] = ["boards", "board", "boardAdd", "boardUpdate", "boardRemove"];

/// The window call the core sends changed boards in.
pub(crate) const CHANGED_METHOD: &str = "agent.board";

const CHANGED_EVENT: &str = "agent_board";

#[tauri::command]
pub async fn agent_board_call(
    manager: State<'_, PtyManager>,
    method: String,
    params: Value,
) -> AppResult<Value> {
    if !METHODS.contains(&method.as_str()) {
        return Err(AppError::Other(format!("{method} is not a board call")));
    }
    let client = manager.client().await?;
    call(&client, method, params).await
}

/// Hands the page the boards the core sent.
pub(crate) fn changed(app: &AppHandle, params: &Value) -> Result<Value, String> {
    app.emit_to("main", CHANGED_EVENT, &params["boards"])
        .map_err(|error| error.to_string())?;
    Ok(Value::Null)
}
