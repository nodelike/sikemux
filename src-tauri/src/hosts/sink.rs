use std::sync::Arc;

use serde_json::json;
use sikemux_core::client::EventSink;
use sikemux_core::protocol::{Event, StreamId};
use sikemux_pty::shell_protocol::PTY_SHELL_METADATA_EVENT;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use crate::pty::sink::{deliver, report_exited, PtyExited, PTY_EXITED_EVENT};

use super::{Host, Hosts};

/// What another host sends, turned into what the page hears from this one:
/// its terminals under the page's ids for them, its chats as this host's
/// are, and its view and changes named by the host.
pub(super) struct RemoteSink {
    app: AppHandle,
    host: Arc<Host>,
    closed: mpsc::UnboundedSender<()>,
}

impl RemoteSink {
    pub(super) fn new(app: AppHandle, host: Arc<Host>, closed: mpsc::UnboundedSender<()>) -> Self {
        Self { app, host, closed }
    }

    fn emit<S: serde::Serialize + Clone>(&self, event: &str, payload: S) {
        let _ = self.app.emit_to("main", event, payload);
    }
}

impl EventSink for RemoteSink {
    fn grid(&self, stream: StreamId, frame: &[u8]) {
        deliver(&self.host.streams, self.host.current(), stream, frame);
    }

    fn event(&self, event: Event) {
        match event {
            Event::Exited {
                id,
                code,
                signal,
                killed,
            } => {
                let id = self.host.page_id(id);
                self.emit(
                    PTY_EXITED_EVENT,
                    PtyExited {
                        id,
                        code,
                        signal,
                        killed,
                    },
                );
                report_exited(&self.host.streams, id);
            }
            Event::ShellMetadata(mut metadata) => {
                metadata.pty_id = self.host.page_id(metadata.pty_id);
                self.emit(PTY_SHELL_METADATA_EVENT, metadata);
            }
            Event::TerminalTitle { id, title } => self.emit(
                "terminal_title",
                json!({ "id": self.host.page_id(id), "title": title }),
            ),
            Event::Chat {
                agent_id,
                seq,
                changes,
            } => crate::acp::deliver(&self.app, &agent_id, seq, changes),
            Event::DeviceView { view } => {
                if let Some(hosts) = self.app.try_state::<Hosts>() {
                    hosts.emit_view(&self.host, *view);
                }
            }
            Event::Changed {
                topic,
                project,
                paths,
            } => self.emit(
                "core_changed",
                json!({ "topic": topic, "project": project, "paths": paths, "host": self.host.id() }),
            ),
            Event::Notice {
                title,
                body,
                agent_id,
            } => self.emit(
                "agent_notice",
                json!({ "title": title, "body": body, "agentId": agent_id }),
            ),
            Event::Stream {
                stream,
                items,
                done,
                error,
            } => self.emit(
                "core_stream",
                json!({ "stream": stream, "items": items, "done": done, "error": error }),
            ),
            Event::SshLink { .. }
            | Event::AccountDevice { .. }
            | Event::TaskOutput { .. }
            | Event::AgentState(_)
            | Event::Remote { .. }
            | Event::ChatBegun { .. }
            | Event::Network { .. }
            | Event::FocusAgent { .. }
            | Event::Attention { .. }
            | Event::AttentionCleared { .. } => {}
        }
    }

    fn closed(&self) {
        let _ = self.closed.send(());
    }
}
