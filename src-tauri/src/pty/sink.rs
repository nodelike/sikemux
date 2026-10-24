use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sikemux_core::client::EventSink;
use sikemux_core::protocol::{CallId, Event, Request, SessionId, StreamId, WindowCall};
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

/// Hands a frame to its pane. A pane whose webview is gone has its stream
/// ended, which the core is told on `client`'s connection.
pub(crate) fn deliver(
    streams: &StreamTable,
    client: Option<Arc<sikemux_core::client::CoreClient>>,
    stream: StreamId,
    frame: &[u8],
) {
    let Some(dead) = streams.deliver(stream, frame) else {
        return;
    };
    global_observability().increment_counter("pty.channel_send_errors", 1);
    if let Some(client) = client {
        let _ = client.submit(Request::Detach { stream: dead }, |_| ());
    }
}

/// An empty chunk is the frontend's sign that the process is gone.
pub(crate) fn report_exited(streams: &StreamTable, id: SessionId) {
    for channel in streams.channels(id) {
        let _ = channel.send(tauri::ipc::Response::new(Vec::new()));
    }
}

pub(crate) fn report_all_exited(streams: &StreamTable) {
    for channel in streams.take_all() {
        let _ = channel.send(tauri::ipc::Response::new(Vec::new()));
    }
}

impl EventSink for AppSink {
    fn grid(&self, stream: StreamId, frame: &[u8]) {
        let observer = global_observability();
        // Every observability call allocates a name and takes one global lock,
        // and this runs for every frame of every terminal. Instrument a
        // sample, plus anything that turns out to be slow.
        let sampled = OUTPUT_FRAMES
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(OBSERVED_BROADCASTS);
        OUTPUT_BYTES.fetch_add(frame.len() as u64, Ordering::Relaxed);
        let operation = sampled.then(|| {
            let mut metadata = Metadata::new();
            metadata.insert("bytes".to_owned(), ScalarValue::from(frame.len()));
            observer.slow_operation("pty.broadcast", SLOW_BROADCAST, None, metadata)
        });
        let started = Instant::now();
        let client = self
            .app
            .try_state::<PtyManager>()
            .and_then(|manager| manager.current_client());
        deliver(&self.streams, client, stream, frame);
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
                agent_id,
                seq,
                changes,
            } => {
                if crate::acp::ended(&changes) {
                    crate::simulator::agent_stopped(&self.app, &agent_id);
                }
                crate::acp::deliver(&self.app, &agent_id, seq, changes)
            }
            Event::Remote { status } => {
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
            Event::DeviceView { view } => self.view.changed(&self.app, *view),
            Event::TerminalTitle { id, title } => self.emit(
                "terminal_title",
                serde_json::json!({ "id": id, "title": title }),
            ),
            Event::Changed {
                topic,
                project,
                paths,
            } => {
                self.emit(
                    "core_changed",
                    serde_json::json!({ "topic": topic, "project": project, "paths": paths }),
                );
            }
            Event::Stream {
                stream,
                items,
                done,
                error,
            } => self.emit(
                "core_stream",
                serde_json::json!({ "stream": stream, "items": items, "done": done, "error": error }),
            ),
            Event::Attention { .. } | Event::AttentionCleared { .. } => {}
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
        streams.add(4, pane, 1);
        report_exited(&streams, 4);
        assert_eq!(chunks.lock().expect("chunks").as_slice(), ["raw 0"]);
    }

    #[test]
    fn losing_the_core_ends_every_pane_and_forgets_it() {
        let streams = StreamTable::default();
        let (pane, chunks) = recording();
        streams.add(11, pane, 2);
        report_all_exited(&streams);
        assert_eq!(chunks.lock().expect("chunks").as_slice(), ["raw 0"]);
        assert!(streams.take_all().is_empty());
    }
}
