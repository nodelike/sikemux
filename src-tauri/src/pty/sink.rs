use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sikemux_core::client::EventSink;
use sikemux_core::protocol::{CallId, Event, SessionId, WindowCall};
use sikemux_pty::shell_protocol::PTY_SHELL_METADATA_EVENT;
use sikemux_pty::task::TaskProcessExit;
use tauri::{AppHandle, Emitter, Manager};

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

const PTY_EXITED_EVENT: &str = "pty_exited";

/// How a session's process ended, for every session, watched or not, so the
/// page can bring back an agent that died while its pane was hidden.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyExited {
    id: SessionId,
    code: Option<u32>,
    signal: Option<String>,
    killed: bool,
}

/// Turns what the core sends into webview channel messages and app events.
/// Runs on the connection's reader task, in the order the core sent it.
pub(super) struct AppSink {
    app: AppHandle,
    streams: Arc<StreamTable>,
}

impl AppSink {
    pub(super) fn new(app: AppHandle, streams: Arc<StreamTable>) -> Self {
        Self { app, streams }
    }

    fn emit<S: serde::Serialize + Clone>(&self, event: &str, payload: S) {
        if self.app.emit_to(MAIN_WEBVIEW, event, payload).is_err() {
            let _ = global_observability().increment_counter("pty.emit_errors", 1);
        }
    }
}

/// An absent exit code means the status could not be read, which a task
/// reports as a failure.
pub(super) fn task_exit(code: Option<u32>, signal: Option<String>) -> TaskProcessExit {
    TaskProcessExit {
        code: code.unwrap_or(1),
        signal,
    }
}

pub(super) fn deliver(streams: &StreamTable, id: SessionId, bytes: &[u8]) {
    let delivery = streams.output(id, bytes);
    if delivery.is_empty() {
        return;
    }
    let dead = delivery.send(bytes);
    if !dead.is_empty() {
        let _ =
            global_observability().increment_counter("pty.channel_send_errors", dead.len() as u64);
        streams.drop_dead(id, &dead);
    }
}

/// An empty chunk is the frontend's sign that the process is gone.
pub(super) fn report_exited(streams: &StreamTable, id: SessionId, exit: TaskProcessExit) {
    let (exit_channel, delivery) = streams.exited(id);
    if let Some(channel) = exit_channel {
        let _ = channel.send(exit);
    }
    delivery.send(&[]);
}

pub(super) fn report_all_exited(streams: &StreamTable) {
    for (exit_channel, delivery) in streams.take_all() {
        if let Some(channel) = exit_channel {
            let _ = channel.send(task_exit(None, None));
        }
        delivery.send(&[]);
    }
}

impl EventSink for AppSink {
    fn output(&self, id: SessionId, bytes: &[u8]) {
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
                self.emit(
                    PTY_EXITED_EVENT,
                    PtyExited {
                        id,
                        code,
                        signal: signal.clone(),
                        killed,
                    },
                );
                report_exited(&self.streams, id, task_exit(code, signal));
            }
            Event::ShellMetadata(metadata) => self.emit(PTY_SHELL_METADATA_EVENT, metadata),
            Event::TaskOutput { .. } => {}
            Event::AgentState(state) => self.emit("agent_state_changed", state),
            Event::Chat {
                agent_id, event, ..
            } => crate::acp::deliver(&self.app, &agent_id, event),
            Event::Remote { status } => self.emit("remote_status_changed", status),
            Event::ChatBegun { chat } => self.emit("remote_chat_begun", chat),
            Event::WakeChat { agent_id } => self.emit("remote_chat_wake", agent_id),
            Event::Attention { .. } | Event::AttentionCleared { .. } | Event::DeviceView { .. } => {
            }
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

    use sikemux_pty::task::TaskProcessExit;

    use super::{report_all_exited, report_exited, task_exit};
    use crate::pty::streams::StreamTable;

    fn recording<T: tauri::ipc::IpcResponse + 'static>(
    ) -> (tauri::ipc::Channel<T>, Arc<Mutex<Vec<String>>>) {
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
    fn an_exit_without_a_code_reports_failure() {
        assert_eq!(
            task_exit(None, None),
            TaskProcessExit {
                code: 1,
                signal: None
            }
        );
        assert_eq!(task_exit(Some(0), None).code, 0);
        assert_eq!(
            task_exit(Some(143), Some("Terminated".into()))
                .signal
                .as_deref(),
            Some("Terminated")
        );
    }

    #[test]
    fn an_exit_reaches_the_task_channel_once_and_every_pane_as_an_empty_chunk() {
        let streams = StreamTable::default();
        let (exit, exits) = recording::<TaskProcessExit>();
        let (pane, chunks) = recording::<tauri::ipc::Response>();
        {
            let mut guard = streams.lock().expect("lock");
            guard.register_task(4, exit);
            guard.begin_attach(4).expect("begin");
            guard.finish_attach(4, pane).expect("attach");
        }
        report_exited(&streams, 4, task_exit(Some(7), None));
        report_exited(&streams, 4, task_exit(Some(9), None));
        assert_eq!(exits.lock().expect("exits").as_slice(), [r#"{"code":7}"#]);
        assert_eq!(
            chunks.lock().expect("chunks").as_slice(),
            ["raw 0", "raw 0"]
        );
    }

    #[test]
    fn losing_the_core_ends_every_task_as_failed() {
        let streams = StreamTable::default();
        let (exit, exits) = recording::<TaskProcessExit>();
        streams.lock().expect("lock").register_task(11, exit);
        report_all_exited(&streams);
        assert_eq!(exits.lock().expect("exits").as_slice(), [r#"{"code":1}"#]);
        assert!(streams.take_all().is_empty());
    }
}
