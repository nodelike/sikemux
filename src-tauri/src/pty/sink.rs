use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sikemux_core::client::EventSink;
use sikemux_core::protocol::{CallId, ChatEventKind, Event, SessionId, WindowCall};
use sikemux_pty::shell_protocol::{SshLink, PTY_SHELL_METADATA_EVENT};
use tauri::{AppHandle, Emitter, Manager};

use crate::inbox::LastView;
use crate::observability::{global_observability, Metadata, ScalarValue};

use super::streams::StreamTable;
use super::{PtyManager, MAIN_WEBVIEW};

static OUTPUT_FRAMES: AtomicU64 = AtomicU64::new(0);
static OUTPUT_BYTES: AtomicU64 = AtomicU64::new(0);
const SLOW_BROADCAST: Duration = Duration::from_millis(8);
/// One in this many output frames carries full timing instrumentation.
const OBSERVED_BROADCASTS: u64 = 64;

pub(super) fn output_totals() -> (u64, u64) {
    (
        OUTPUT_FRAMES.load(Ordering::Relaxed),
        OUTPUT_BYTES.load(Ordering::Relaxed),
    )
}

pub(crate) const PTY_EXITED_EVENT: &str = "pty_exited";
const PTY_SSH_LINK_EVENT: &str = "pty_ssh_link";

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PtySshLink {
    id: SessionId,
    link: SshLink,
}

/// How a session's process ended, for every session, watched or not, so the
/// page can bring back an agent that died while its pane was hidden.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyExited {
    pub id: SessionId,
    pub code: Option<u32>,
    pub signal: Option<String>,
    pub killed: bool,
}

/// Turns what the core sends into webview channel messages and app events.
/// Runs on the connection's reader task, in the order the core sent it.
pub(super) struct AppSink {
    app: AppHandle,
    streams: Arc<StreamTable>,
    view: LastView,
}

impl AppSink {
    pub(super) fn new(app: AppHandle, streams: Arc<StreamTable>) -> Self {
        Self {
            app,
            streams,
            view: LastView::default(),
        }
    }

    fn emit<S: serde::Serialize + Clone>(&self, event: &str, payload: S) {
        if self.app.emit_to(MAIN_WEBVIEW, event, payload).is_err() {
            global_observability().increment_counter("pty.emit_errors", 1);
        }
    }
}

pub(crate) fn deliver(streams: &StreamTable, id: SessionId, bytes: &[u8]) {
    let delivery = streams.output(id, bytes);
    if delivery.is_empty() {
        return;
    }
    let dead = delivery.send(bytes);
    if !dead.is_empty() {
        global_observability().increment_counter("pty.channel_send_errors", dead.len() as u64);
        streams.drop_dead(id, &dead);
    }
}

/// An empty chunk is the frontend's sign that the process is gone.
pub(crate) fn report_exited(streams: &StreamTable, id: SessionId) {
    streams.exited(id).send(&[]);
}

pub(crate) fn report_all_exited(streams: &StreamTable) {
    for delivery in streams.take_all() {
        delivery.send(&[]);
    }
}

impl EventSink for AppSink {
    fn output(&self, id: SessionId, bytes: &[u8]) {
        crate::hosts::install::watch_output(id, bytes);
        let observer = global_observability();
        // Every observability call allocates a name and takes one global lock,
        // and this runs for every frame of every terminal. Instrument a
        // sample, plus anything that turns out to be slow.
        let sampled = OUTPUT_FRAMES
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(OBSERVED_BROADCASTS);
        OUTPUT_BYTES.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let operation = sampled.then(|| {
            let mut metadata = Metadata::new();
            metadata.insert("bytes".to_owned(), ScalarValue::from(bytes.len()));
            observer.slow_operation("pty.broadcast", SLOW_BROADCAST, None, metadata)
        });
        let started = Instant::now();
        deliver(&self.streams, id, bytes);
        match operation {
            Some(operation) => {
                operation.finish(crate::observability::SpanOutcome::Success);
            }
            None => {
                let elapsed = started.elapsed();
                if elapsed >= SLOW_BROADCAST {
                    observer.observe_latency("pty.broadcast", elapsed);
                }
            }
        }
    }

    fn event(&self, event: Event) {
        match event {
            Event::Exited {
                id,
                code,
                signal,
                killed,
            } => {
                crate::hosts::install::ended(id);
                self.emit(
                    PTY_EXITED_EVENT,
                    PtyExited {
                        id,
                        code,
                        signal,
                        killed,
                    },
                );
                report_exited(&self.streams, id);
            }
            Event::ShellMetadata(metadata) => self.emit(PTY_SHELL_METADATA_EVENT, metadata),
            Event::SshLink { id, link } => self.emit(PTY_SSH_LINK_EVENT, PtySshLink { id, link }),
            Event::TaskOutput { .. } => {}
            Event::AgentState(state) => {
                if state.state == "stopped" {
                    crate::simulator::agent_stopped(&self.app, &state.agent_id);
                }
            }
            Event::Chat {
                agent_id, event, ..
            } => {
                if matches!(event.kind, ChatEventKind::Status)
                    && matches!(event.payload["state"].as_str(), Some("stopped" | "error"))
                {
                    crate::simulator::agent_stopped(&self.app, &agent_id);
                }
                crate::acp::deliver(&self.app, &agent_id, event)
            }
            Event::Remote { status } => {
                crate::account::notice_remote(&self.app, &status);
                crate::remote::follow_remote_access(&self.app, status.enabled);
                crate::hosts::follow_links(&self.app);
                self.emit("remote_status_changed", status);
            }
            Event::ChatBegun { .. } => {}
            Event::Network { change } => self.emit("network_changed", change),
            Event::Notice {
                title,
                body,
                agent_id,
            } => self.emit(
                "agent_notice",
                serde_json::json!({ "title": title, "body": body, "agentId": agent_id }),
            ),
            Event::AccountDevice {
                key,
                name,
                platform,
            } => {
                let own = self
                    .app
                    .try_state::<crate::hosts::Hosts>()
                    .and_then(|hosts| hosts.own_key());
                if own.as_deref() != Some(key.as_str()) {
                    self.emit(
                        "account_device",
                        serde_json::json!({ "key": key, "name": name, "platform": platform }),
                    );
                }
            }
            Event::FocusAgent { agent_id } => {
                crate::agent_notification::focus_agent(&self.app, agent_id)
            }
            Event::DeviceView { view } => self.view.changed(&self.app, view),
            Event::TerminalTitle { id, title } => self.emit(
                "terminal_title",
                serde_json::json!({ "id": id, "title": title }),
            ),
            Event::Changed {
                topic,
                project,
                paths,
            } => {
                let change =
                    serde_json::json!({ "topic": topic, "project": project, "paths": paths });
                match project.filter(|_| topic == "git") {
                    Some(project) => {
                        let app = self.app.clone();
                        crate::files::moved(project, paths, move || {
                            let _ = app.emit_to(MAIN_WEBVIEW, "core_changed", change);
                        });
                    }
                    None => self.emit("core_changed", change),
                }
            }
            Event::AttentionCleared { id, agent_id } => {
                crate::acp::permission_cleared(&self.app, &agent_id, id)
            }
            Event::Attention { .. } => {}
        }
    }

    fn window_call(&self, call_id: CallId, call: WindowCall) {
        crate::harness::answer_window_call(&self.app, call_id, call);
    }

    fn closed(&self) {
        if let Some(manager) = self.app.try_state::<PtyManager>() {
            manager.disconnected();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{report_all_exited, report_exited};
    use crate::pty::streams::StreamTable;

    fn recording() -> (
        tauri::ipc::Channel<tauri::ipc::Response>,
        Arc<Mutex<Vec<String>>>,
    ) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let channel = tauri::ipc::Channel::new(move |body| {
            let text = match body {
                tauri::ipc::InvokeResponseBody::Json(json) => json,
                tauri::ipc::InvokeResponseBody::Raw(bytes) => format!("raw {}", bytes.len()),
            };
            sink.lock().expect("seen").push(text);
            Ok(())
        });
        (channel, seen)
    }

    #[test]
    fn an_exit_reaches_every_pane_as_an_empty_chunk() {
        let streams = StreamTable::default();
        let (pane, chunks) = recording();
        {
            let mut guard = streams.lock();
            guard.begin_attach(4).expect("begin");
            guard.finish_attach(4, pane).expect("attach");
        }
        report_exited(&streams, 4);
        assert_eq!(chunks.lock().expect("chunks").as_slice(), ["raw 0"]);
    }

    #[test]
    fn losing_the_core_ends_every_pane_and_forgets_it() {
        let streams = StreamTable::default();
        let (pane, chunks) = recording();
        {
            let mut guard = streams.lock();
            guard.begin_attach(11).expect("begin");
            guard.finish_attach(11, pane).expect("attach");
        }
        report_all_exited(&streams);
        assert_eq!(chunks.lock().expect("chunks").as_slice(), ["raw 0"]);
        assert!(streams.take_all().is_empty());
    }
}
