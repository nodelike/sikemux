//! Agents' names, and the saved sessions they ran. The core gives the names
//! and keeps them, and searches the sessions; these commands pass the app's
//! asks on to it.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use sikemux_core::client::{CoreClient, Reply};
use sikemux_core::protocol::{Request, Response};
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::pty::{core_error, PtyManager};

/// The core's calls the page may make through this.
const METHODS: [&str; 4] = ["nameAgent", "renameAgent", "agentsInApp", "agentSessions"];

/// Tells the core every agent the app holds.
const IN_APP: &str = "agentsInApp";

/// The agents the app last said it holds, said again to a core it reconnects to.
#[derive(Default)]
pub struct AgentsInApp(Mutex<Option<Value>>);

#[tauri::command]
pub async fn agent_name_call(
    manager: State<'_, PtyManager>,
    in_app: State<'_, AgentsInApp>,
    method: String,
    params: Value,
) -> AppResult<Value> {
    if !METHODS.contains(&method.as_str()) {
        return Err(AppError::Other(format!("{method} is not a name call")));
    }
    if method == IN_APP {
        if let Ok(mut last) = in_app.0.lock() {
            *last = Some(params.clone());
        }
    }
    let client = manager.client().await?;
    call(&client, method, params).await
}

/// Gives a core the app just connected to the agents the app holds.
pub(crate) async fn connected(app: &AppHandle, client: &Arc<CoreClient>) {
    let last = app
        .try_state::<AgentsInApp>()
        .and_then(|in_app| in_app.0.lock().ok().and_then(|last| last.clone()));
    if let Some(params) = last {
        if let Err(error) = call(client, IN_APP.to_owned(), params).await {
            eprintln!("Sikemux could not tell its core which agents it holds: {error}");
        }
    }
}

pub(crate) async fn call(client: &CoreClient, method: String, params: Value) -> AppResult<Value> {
    let reply = client
        .submit(Request::Call { method, params }, |reply| reply)
        .map_err(core_error)?
        .await
        .map_err(core_error)?
        .map_err(core_error)?;
    match reply {
        Reply::Response(Response::Called { result }) => Ok(result),
        _ => Err(AppError::Other(
            "the core answered a call with something else".into(),
        )),
    }
}
