//! What waits on the person, as the core ranks it for every device, the
//! workspace every client shares, and how agents' faces look. The app watches the core's device view and
//! hands the page each of them whenever it changes.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use sikemux_core::client::CoreClient;
use sikemux_core::protocol::{DeviceView, Request};
use sikemux_process::sync::Locked;
use tauri::{AppHandle, Emitter};

use crate::pty::core_error;

const INBOX_EVENT: &str = "inbox_changed";
const WORKSPACE_EVENT: &str = "workspace_changed";
const FACE_STYLE_EVENT: &str = "face_style_changed";

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

/// The inbox and workspace last handed to the page, so an unchanged one is
/// not sent again.
#[derive(Default)]
pub(crate) struct LastView {
    inbox: Mutex<Value>,
    workspace: Mutex<Value>,
    face_style: Mutex<Value>,
}

impl LastView {
    pub(crate) fn changed(&self, app: &AppHandle, view: DeviceView) {
        emit_changed(app, &self.inbox, INBOX_EVENT, view.inbox);
        if let Ok(workspace) = serde_json::to_value(view.workspace) {
            emit_changed(app, &self.workspace, WORKSPACE_EVENT, workspace);
        }
        if let Ok(style) = serde_json::to_value(view.face_style) {
            emit_changed(app, &self.face_style, FACE_STYLE_EVENT, style);
        }
    }
}

fn emit_changed(app: &AppHandle, last: &Mutex<Value>, event: &str, value: Value) {
    let mut last = last.locked();
    if *last != value && app.emit_to("main", event, &value).is_ok() {
        *last = value;
    }
}
