//! What waits on the person, as the core ranks it for every device. The app
//! watches the core's device view and hands the page its inbox whenever it
//! changes.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use sikemux_core::client::CoreClient;
use sikemux_core::protocol::Request;
use tauri::{AppHandle, Emitter, State};

use crate::agent_names::call;
use crate::error::{AppError, AppResult};
use crate::pty::{core_error, PtyManager};

/// The core's inbox calls, the only ones the page may make through this.
const METHODS: [&str; 3] = ["inbox", "digest", "agentMessages"];

const CHANGED_EVENT: &str = "inbox_changed";

#[tauri::command]
pub async fn inbox_call(
    manager: State<'_, PtyManager>,
    method: String,
    params: Value,
) -> AppResult<Value> {
    if !METHODS.contains(&method.as_str()) {
        return Err(AppError::Other(format!("{method} is not an inbox call")));
    }
    let client = manager.client().await?;
    call(&client, method, params).await
}

/// Asks a core the app just connected to for its device view as it changes.
pub(crate) async fn connected(client: &Arc<CoreClient>) {
    let watched = async {
        client
            .submit(Request::WatchView, |reply| reply)
            .map_err(core_error)?
            .await
            .map_err(core_error)?
            .map_err(core_error)
    };
    if let Err(error) = watched.await {
        eprintln!("Sikemux could not watch its core's inbox: {error}");
    }
}

/// The inbox last handed to the page, so an unchanged one is not sent again.
#[derive(Default)]
pub(crate) struct LastInbox(Mutex<Value>);

impl LastInbox {
    pub(crate) fn changed(&self, app: &AppHandle, inbox: Value) {
        let Ok(mut last) = self.0.lock() else {
            return;
        };
        if *last == inbox {
            return;
        }
        if app.emit_to("main", CHANGED_EVENT, &inbox).is_ok() {
            *last = inbox;
        }
    }
}
