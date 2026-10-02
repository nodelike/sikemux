//! The window's side of agents' tool calls. The core answers most of them;
//! the ones that need the window arrive here. Browser and plugin tools are
//! answered in Rust, and the rest wait in a queue the page claims and answers.

use std::collections::HashMap;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use serde_json::Value;
pub use sikemux_core::cli::protocol::HarnessRequest;
use sikemux_core::cli::protocol::{is_browser_method, is_plugin_method};
use sikemux_core::protocol::{CallId, RunSelector, WindowAnswer, WindowCall};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::AppResult;
use crate::pty::PtyManager;

pub const MAX_PENDING: usize = 64;
const REPLY_TIMEOUT: Duration = Duration::from_secs(65);
/// A launch may wait on the person trusting `sikemux.json`.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

struct Pending {
    request: HarnessRequest,
    claimed: bool,
    reply: mpsc::Sender<Result<Value, String>>,
}

/// Calls waiting for the page to claim and answer them.
#[derive(Default)]
pub struct HarnessBroker {
    pending: Mutex<HashMap<String, Pending>>,
}

impl HarnessBroker {
    pub fn enqueue(
        &self,
        request: HarnessRequest,
    ) -> Result<mpsc::Receiver<Result<Value, String>>, String> {
        let mut pending = self.pending.lock().map_err(|_| "harness lock poisoned")?;
        if pending.len() >= MAX_PENDING || pending.contains_key(&request.id) {
            return Err("harness request capacity reached or duplicate request ID".into());
        }
        let (reply, receiver) = mpsc::channel();
        pending.insert(
            request.id.clone(),
            Pending {
                request,
                claimed: false,
                reply,
            },
        );
        Ok(receiver)
    }

    fn claim(&self) -> Vec<HarnessRequest> {
        let Ok(mut pending) = self.pending.lock() else {
            return vec![];
        };
        pending
            .values_mut()
            .filter_map(|entry| {
                if entry.claimed {
                    return None;
                }
                entry.claimed = true;
                Some(entry.request.clone())
            })
            .collect()
    }

    pub fn remove(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }

    fn reply(&self, id: &str, result: Result<Value, String>) {
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(entry) = pending.remove(id) {
                let _ = entry.reply.send(result);
            }
        }
    }

    /// The page that would have answered is gone.
    pub fn fail_all(&self, message: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            for (_, entry) in pending.drain() {
                let _ = entry.reply.send(Err(message.into()));
            }
        }
    }
}

/// Hands the window's answer back to the core.
pub(crate) fn answer_core(app: &AppHandle, call_id: CallId, result: Result<Value, String>) {
    if let Some(client) = app.state::<PtyManager>().current_client() {
        client.answer_window(call_id, WindowAnswer::from(result));
    }
}

/// Answers a call from the core on its own thread, since most answers wait on
/// the page or a browser tab.
pub(crate) fn answer_window_call(app: &AppHandle, call_id: CallId, call: WindowCall) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("sikemux-window-call".into())
        .spawn(move || match call {
            WindowCall::Harness { request } => {
                let result = run(&app, request);
                answer_core(&app, call_id, result);
            }
            WindowCall::Open { request } => crate::cli_open::answer(&app, call_id, request),
        });
    if let Err(error) = spawned {
        eprintln!("Sikemux could not answer a tool call: {error}");
    }
}

fn run(app: &AppHandle, request: HarnessRequest) -> Result<Value, String> {
    if is_browser_method(&request.method) {
        return crate::browser::tools::execute(app, &request);
    }
    if is_plugin_method(&request.method) {
        return crate::plugins::agent::execute(
            app,
            &request.project,
            &request.method,
            &request.params,
        );
    }
    let id = request.id.clone();
    let method = request.method.clone();
    let focus =
        method == "ui.open" && request.params.get("focus").and_then(Value::as_bool) == Some(true);
    let broker = app.state::<HarnessBroker>();
    let receiver = broker.enqueue(request)?;
    let _ = app.emit_to("main", "harness-request", ());
    let limit = if method == "task.start" {
        LAUNCH_TIMEOUT
    } else {
        REPLY_TIMEOUT
    };
    let result = receiver
        .recv_timeout(limit)
        .unwrap_or_else(|_| Err(timeout_message(&method)));
    broker.remove(&id);
    match result {
        Ok(mut value) if method == "workspace.inspect" => {
            if let (Some(object), Some(cli)) =
                (value.as_object_mut(), crate::cli_paths::cli_command_path())
            {
                object.insert("cli".into(), cli.to_string_lossy().into());
            }
            Ok(value)
        }
        Ok(value) => {
            if focus {
                bring_forward(app);
            }
            Ok(value)
        }
        error => error,
    }
}

/// An agent must not pull the person's keyboard away from another app they
/// are typing in, so unless Sikemux is already in front it only asks for
/// attention.
fn bring_forward(app: &AppHandle) {
    if let Some(window) = app.get_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        if window.is_focused().unwrap_or(false) {
            let _ = window.set_focus();
        } else {
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }
}

fn timeout_message(method: &str) -> String {
    let tool = method.replace('.', "_");
    if method == "task.start" {
        format!(
            "{tool} got no answer from Sikemux within {} minutes",
            LAUNCH_TIMEOUT.as_secs() / 60
        )
    } else {
        format!(
            "{tool} got no answer from Sikemux within {} s. Check that the Sikemux window is open and responsive, then retry.",
            REPLY_TIMEOUT.as_secs()
        )
    }
}

#[tauri::command]
pub fn harness_claim(state: State<'_, HarnessBroker>) -> Vec<HarnessRequest> {
    state.claim()
}

#[tauri::command]
pub fn harness_reply(
    state: State<'_, HarnessBroker>,
    id: String,
    result: Option<Value>,
    error: Option<String>,
) {
    state.reply(
        &id,
        match error {
            Some(message) => Err(message),
            None => Ok(result.unwrap_or(Value::Null)),
        },
    );
}

/// The page is asking the person to trust `sikemux.json` before a launch.
#[tauri::command]
pub async fn harness_awaiting_trust(
    manager: State<'_, PtyManager>,
    execution_id: String,
) -> AppResult<()> {
    let client = manager.client().await?;
    client
        .harness_awaiting_trust(execution_id)
        .await
        .map_err(crate::pty::core_error)
}

/// Stops the harness runs that match every field given, and waits for them.
#[tauri::command]
pub async fn harness_stop_runs(
    manager: State<'_, PtyManager>,
    execution_id: Option<String>,
    project: Option<String>,
    agent_id: Option<String>,
) -> AppResult<()> {
    let client = manager.client().await?;
    client
        .harness_stop_runs(RunSelector {
            execution_id,
            project,
            agent_id,
        })
        .await
        .map_err(crate::pty::core_error)
}

#[tauri::command]
pub async fn harness_resolve_path(project: String, path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || resolve_project_path(project, path))
        .await
        .map_err(|error| format!("harness_resolve_path join: {error}"))?
}

fn resolve_project_path(project: String, path: String) -> Result<String, String> {
    let root = std::fs::canonicalize(project).map_err(|error| error.to_string())?;
    let target = std::fs::canonicalize(root.join(path)).map_err(|error| error.to_string())?;
    if !target.starts_with(&root) || !target.is_file() {
        return Err("File must be inside the project".into());
    }
    Ok(target.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str) -> HarnessRequest {
        HarnessRequest {
            id: id.into(),
            project: "/tmp".into(),
            agent_id: None,
            method: "workspace.inspect".into(),
            params: serde_json::json!({}),
        }
    }
    #[test]
    fn queue_claims_once_replies_and_releases_capacity() {
        let broker = HarnessBroker::default();
        let receiver = broker.enqueue(request("one")).unwrap();
        assert!(broker.enqueue(request("one")).is_err());
        assert_eq!(broker.claim().len(), 1);
        assert!(broker.claim().is_empty());
        broker.reply("one", Ok(Value::Bool(true)));
        assert_eq!(receiver.recv().unwrap().unwrap(), Value::Bool(true));
        assert!(broker.enqueue(request("one")).is_ok());
        broker.fail_all("gone");
    }
    #[test]
    fn a_page_that_goes_away_fails_what_it_had_claimed() {
        let broker = HarnessBroker::default();
        let receiver = broker.enqueue(request("one")).unwrap();
        assert_eq!(broker.claim().len(), 1);
        broker.fail_all("Sikemux reloaded");
        assert_eq!(receiver.recv().unwrap().unwrap_err(), "Sikemux reloaded");
    }
    #[test]
    fn timeouts_name_the_method() {
        assert!(timeout_message("task.start").starts_with("task_start "));
        let inspect = timeout_message("workspace.inspect");
        assert!(inspect.starts_with("workspace_inspect ") && inspect.contains("65 s"));
    }
    #[test]
    fn file_open_rejects_paths_outside_project() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(super::resolve_project_path(
            project.path().to_string_lossy().into_owned(),
            outside.path().to_string_lossy().into_owned()
        )
        .is_err());
        let file = project.path().join("test.txt");
        std::fs::write(&file, "ok").unwrap();
        assert!(super::resolve_project_path(
            project.path().to_string_lossy().into_owned(),
            "test.txt".into()
        )
        .is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), project.path().join("escape")).unwrap();
            assert!(super::resolve_project_path(
                project.path().to_string_lossy().into_owned(),
                "escape".into()
            )
            .is_err());
        }
    }
    #[test]
    fn a_full_queue_is_rejected_until_a_slot_frees() {
        let broker = HarnessBroker::default();
        for i in 0..MAX_PENDING {
            broker.enqueue(request(&i.to_string())).unwrap();
        }
        assert!(broker.enqueue(request("overflow")).is_err());
        broker.remove("0");
        assert!(broker.enqueue(request("new")).is_ok());
    }
}
