//! Questions agents put to the person. The core keeps them and tells the
//! window when they change; the page lists and answers them through this.

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::agent_names::call;
use crate::error::{AppError, AppResult};
use crate::pty::PtyManager;

/// The core's question calls, the only ones the page may make through this.
const METHODS: [&str; 3] = ["questions", "answerQuestion", "answerQuestions"];

/// The window call the core sends the open questions in.
pub(crate) const CHANGED_METHOD: &str = "agent.questions";

const CHANGED_EVENT: &str = "agent_questions";

#[tauri::command]
pub async fn agent_question_call(
    manager: State<'_, PtyManager>,
    method: String,
    params: Value,
) -> AppResult<Value> {
    if !METHODS.contains(&method.as_str()) {
        return Err(AppError::Other(format!("{method} is not a question call")));
    }
    let client = manager.client().await?;
    call(&client, method, params).await
}

/// Hands the page the open questions the core sent.
pub(crate) fn changed(app: &AppHandle, params: &Value) -> Result<Value, String> {
    app.emit_to("main", CHANGED_EVENT, &params["questions"])
        .map_err(|error| error.to_string())?;
    Ok(Value::Null)
}
