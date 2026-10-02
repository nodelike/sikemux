//! The CLI's `open`, which the core hands to the window. Each request waits
//! here until the page has opened every target, and with `--wait` until their
//! tabs close.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use sikemux_core::cli::protocol::{
    CliCloseReason, CliFrontendRequest, CliOpenFailure, CliOpenOutcome, CliOpenRequest,
    CliOpenResult, CliTargetKind,
};
use sikemux_core::protocol::CallId;
use tauri::{AppHandle, Emitter, Manager};

const CLI_EVENT: &str = "cli-open-available";
const FRONTEND_ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

struct RequestEntry {
    request: CliOpenRequest,
    dispatched: bool,
    pending_targets: HashSet<String>,
    opened_targets: Vec<String>,
    failed_targets: Vec<CliOpenFailure>,
    open_tabs: HashMap<String, (String, String)>,
    closed_before_ack: HashSet<(String, String)>,
    accepted: Option<mpsc::Sender<CliOpenOutcome>>,
    closed: Option<mpsc::Sender<CliCloseReason>>,
}

type Receivers = (
    mpsc::Receiver<CliOpenOutcome>,
    Option<mpsc::Receiver<CliCloseReason>>,
);

#[derive(Default)]
pub struct CliOpens {
    requests: Mutex<HashMap<String, RequestEntry>>,
}

impl CliOpens {
    fn enqueue(&self, request: CliOpenRequest) -> Result<Receivers, String> {
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (closed_tx, closed_rx) = mpsc::channel();
        let mut requests = self.requests.lock().map_err(|_| "CLI open lock poisoned")?;
        if requests.contains_key(&request.id) {
            return Err("duplicate CLI request id".into());
        }
        let pending_targets = request
            .targets
            .iter()
            .map(|target| target.id.clone())
            .collect();
        let wait = request.wait;
        requests.insert(
            request.id.clone(),
            RequestEntry {
                request,
                dispatched: false,
                pending_targets,
                opened_targets: Vec::new(),
                failed_targets: Vec::new(),
                open_tabs: HashMap::new(),
                closed_before_ack: HashSet::new(),
                accepted: Some(accepted_tx),
                closed: wait.then_some(closed_tx),
            },
        );
        Ok((accepted_rx, wait.then_some(closed_rx)))
    }

    fn claim(&self, reset: bool) -> Vec<CliFrontendRequest> {
        let Ok(mut requests) = self.requests.lock() else {
            return Vec::new();
        };
        if reset {
            for entry in requests.values_mut() {
                if !entry.pending_targets.is_empty() {
                    entry.dispatched = false;
                }
            }
        }
        requests
            .values_mut()
            .filter_map(|entry| {
                if entry.dispatched || entry.pending_targets.is_empty() {
                    return None;
                }
                entry.dispatched = true;
                let mut request = entry.request.clone();
                request
                    .targets
                    .retain(|target| entry.pending_targets.contains(&target.id));
                Some(CliFrontendRequest { request })
            })
            .collect()
    }

    fn mark_result(&self, result: CliOpenResult) {
        let Ok(mut requests) = self.requests.lock() else {
            return;
        };
        let mut remove = false;
        let Some(entry) = requests.get_mut(&result.request_id) else {
            return;
        };
        let Some(target) = entry
            .request
            .targets
            .iter()
            .find(|target| target.id == result.target_id)
        else {
            return;
        };
        let target_kind = target.kind;
        let target_path = target.path.clone();
        if !entry.pending_targets.remove(&result.target_id) {
            return;
        }
        if result.path != target_path {
            entry.failed_targets.push(CliOpenFailure {
                target_id: result.target_id,
                message: "Sikemux reported the wrong path for this CLI target".into(),
            });
        } else if let Some(message) = result.error {
            entry.failed_targets.push(CliOpenFailure {
                target_id: result.target_id,
                message,
            });
        } else if target_kind == CliTargetKind::File && result.pane_id.is_none() {
            entry.failed_targets.push(CliOpenFailure {
                target_id: result.target_id,
                message: "Sikemux did not attach this file to an editor pane".into(),
            });
        } else {
            entry.opened_targets.push(result.target_id.clone());
            if let Some(pane_id) = result.pane_id {
                let closed_key = (pane_id.clone(), result.path.clone());
                if !entry.closed_before_ack.remove(&closed_key) {
                    entry
                        .open_tabs
                        .insert(result.target_id, (pane_id, result.path));
                }
            }
        }
        if entry.pending_targets.is_empty() {
            if let Some(sender) = entry.accepted.take() {
                let _ = sender.send(CliOpenOutcome {
                    opened: entry.opened_targets.clone(),
                    failed: entry.failed_targets.clone(),
                });
            }
            if entry.request.wait && entry.open_tabs.is_empty() {
                if let Some(sender) = entry.closed.take() {
                    let _ = sender.send(CliCloseReason::TabsClosed);
                }
                remove = true;
            } else {
                remove = !entry.request.wait;
            }
        }
        if remove {
            requests.remove(&result.request_id);
        }
    }

    fn tabs_closed(&self, pane_id: &str, paths: &[String]) {
        let closed: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let Ok(mut requests) = self.requests.lock() else {
            return;
        };
        let mut completed = Vec::new();
        for (request_id, entry) in requests.iter_mut() {
            for target in &entry.request.targets {
                if entry.pending_targets.contains(&target.id)
                    && closed.contains(target.path.as_str())
                {
                    entry
                        .closed_before_ack
                        .insert((pane_id.to_string(), target.path.clone()));
                }
            }
            entry.open_tabs.retain(|_, (candidate_pane, path)| {
                candidate_pane != pane_id || !closed.contains(path.as_str())
            });
            if entry.request.wait && entry.pending_targets.is_empty() && entry.open_tabs.is_empty()
            {
                if let Some(sender) = entry.closed.take() {
                    let _ = sender.send(CliCloseReason::TabsClosed);
                }
                completed.push(request_id.clone());
            }
        }
        for request_id in completed {
            requests.remove(&request_id);
        }
    }

    fn cancel(&self, request_id: &str) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.remove(request_id);
        }
    }

    /// The app is leaving: whoever waits on a request hears so.
    pub fn shutdown(&self) {
        if let Ok(mut requests) = self.requests.lock() {
            for entry in requests.values_mut() {
                if let Some(sender) = entry.accepted.take() {
                    let failed = entry
                        .pending_targets
                        .iter()
                        .map(|target_id| CliOpenFailure {
                            target_id: target_id.clone(),
                            message: "Sikemux exited before opening this target".into(),
                        })
                        .collect();
                    let _ = sender.send(CliOpenOutcome {
                        opened: entry.opened_targets.clone(),
                        failed,
                    });
                }
                if let Some(sender) = entry.closed.take() {
                    let _ = sender.send(CliCloseReason::AppExit);
                }
            }
            requests.clear();
        }
    }
}

/// Answers the core's `open` call. Blocks, so it runs on its own thread.
pub fn answer(app: &AppHandle, call_id: CallId, request: CliOpenRequest) {
    let opens = app.state::<CliOpens>();
    let request_id = request.id.clone();
    let (accepted, closed) = match opens.enqueue(request) {
        Ok(receivers) => receivers,
        Err(message) => {
            crate::harness::answer_core(app, call_id, Err(message));
            return;
        }
    };
    if let Some(window) = app.get_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let _ = app.emit_to("main", CLI_EVENT, &request_id);
    let outcome = match accepted.recv_timeout(FRONTEND_ACCEPT_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            opens.cancel(&request_id);
            crate::harness::answer_core(
                app,
                call_id,
                Err("Sikemux did not finish opening the request within 60 seconds".into()),
            );
            return;
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            crate::harness::answer_core(
                app,
                call_id,
                Err("Sikemux closed before the editor accepted the request".into()),
            );
            return;
        }
    };
    crate::harness::answer_core(
        app,
        call_id,
        serde_json::to_value(outcome).map_err(|error| error.to_string()),
    );
    if closed.is_some_and(|closed| closed.recv() == Ok(CliCloseReason::TabsClosed)) {
        if let Some(client) = app.state::<crate::pty::PtyManager>().current_client() {
            client.window_open_closed(call_id);
        }
    }
}

#[tauri::command]
pub fn cli_frontend_ready(state: tauri::State<'_, CliOpens>) -> Vec<CliFrontendRequest> {
    state.claim(true)
}

#[tauri::command]
pub fn cli_claim_open_requests(state: tauri::State<'_, CliOpens>) -> Vec<CliFrontendRequest> {
    state.claim(false)
}

#[tauri::command]
pub fn cli_open_result(state: tauri::State<'_, CliOpens>, result: CliOpenResult) {
    state.mark_result(result);
}

#[tauri::command]
pub fn cli_editor_tabs_closed(
    state: tauri::State<'_, CliOpens>,
    pane_id: String,
    paths: Vec<String>,
) {
    state.tabs_closed(&pane_id, &paths);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_core::cli::protocol::CliOpenTarget;

    fn request(wait: bool) -> CliOpenRequest {
        CliOpenRequest {
            id: "request".into(),
            cwd: "/repo".into(),
            wait,
            targets: vec![CliOpenTarget {
                id: "target".into(),
                kind: CliTargetKind::File,
                path: "/repo/file.rs".into(),
                project_root: "/repo".into(),
                line: None,
                column: None,
            }],
        }
    }

    fn opened(pane: Option<&str>) -> CliOpenResult {
        CliOpenResult {
            request_id: "request".into(),
            target_id: "target".into(),
            pane_id: pane.map(str::to_owned),
            path: "/repo/file.rs".into(),
            error: None,
        }
    }

    #[test]
    fn a_waiting_open_is_accepted_then_closed_with_its_tab() {
        let opens = CliOpens::default();
        let (accepted, closed) = opens.enqueue(request(true)).unwrap();
        assert!(opens.enqueue(request(true)).is_err());
        assert_eq!(opens.claim(false).len(), 1);
        assert!(opens.claim(false).is_empty());
        assert_eq!(opens.claim(true).len(), 1);
        opens.mark_result(opened(Some("pane")));
        assert_eq!(accepted.recv().unwrap().opened, ["target"]);
        let closed = closed.unwrap();
        assert!(closed.try_recv().is_err());
        opens.tabs_closed("pane", &["/repo/file.rs".into()]);
        assert_eq!(closed.recv().unwrap(), CliCloseReason::TabsClosed);
    }

    #[test]
    fn a_file_that_reached_no_pane_fails_and_leaving_releases_waiters() {
        let opens = CliOpens::default();
        let (accepted, _) = opens.enqueue(request(false)).unwrap();
        opens.mark_result(opened(None));
        assert_eq!(accepted.recv().unwrap().failed.len(), 1);

        let (accepted, closed) = opens.enqueue(request(true)).unwrap();
        opens.shutdown();
        assert_eq!(accepted.recv().unwrap().failed.len(), 1);
        assert_eq!(closed.unwrap().recv().unwrap(), CliCloseReason::AppExit);
    }
}
