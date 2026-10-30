//! What the core tells its clients without being asked.

use crate::pty::shell_protocol::{PtyShellMetadataEvent, SshLink};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    AgentStateEvent, Attention, ChatInfo, DeviceView, NetworkChange, RemoteStatus, SessionId,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    /// The session's process was reaped. `code` is absent when its status
    /// could not be read; `killed` means a client asked for it.
    Exited {
        id: SessionId,
        code: Option<u32>,
        signal: Option<String>,
        killed: bool,
    },
    /// The app's `pty_shell_metadata` payload, with `ptyId` holding the
    /// session id.
    ShellMetadata(PtyShellMetadataEvent<SessionId>),
    /// An SSH terminal connected, started connecting or gave up. Sent only
    /// to clients on this host.
    SshLink {
        id: SessionId,
        link: SshLink,
    },
    /// A task session has new output to page through with `TaskOutput`.
    TaskOutput {
        id: SessionId,
    },
    AgentState(AgentStateEvent),
    /// Sent only to the clients that started or attached to the chat: what
    /// changed in it, a list of [`crate::chat::ChatChange`] as JSON. `seq`
    /// counts the chat's changes, so a client can tell which the attach answer
    /// already held.
    Chat {
        agent_id: String,
        seq: u64,
        changes: Value,
    },
    /// Sent only to paired devices: what they show of this host, whole, when
    /// they connect and whenever any of it changes.
    DeviceView {
        view: Box<DeviceView>,
    },
    /// Sent only to clients on this host.
    Remote {
        status: RemoteStatus,
    },
    /// The network came back, went away or changed, or the computer woke.
    /// Sent only to clients on this host.
    Network {
        change: NetworkChange,
    },
    /// An agent started waiting on a person. Every client on this host hears
    /// it, whether or not it shows that agent; devices see it in their view.
    Attention {
        attention: Attention,
    },
    /// A paired device started a chat. Sent only to clients on this host,
    /// which show it beside their own.
    ChatBegun {
        chat: ChatInfo,
    },
    /// What an agent waited on was answered or withdrawn.
    AttentionCleared {
        id: String,
        agent_id: String,
    },
    /// Sent only to the app's window: show this agent and come to the front.
    FocusAgent {
        agent_id: String,
    },
    /// Sent only to the app's window: tell the person this, unless they are
    /// looking at the window.
    Notice {
        title: String,
        body: String,
        agent_id: String,
    },
    /// A device or host joined the person's account, which the app's window
    /// offers to remove (ADR 0018).
    AccountDevice {
        key: String,
        name: String,
        platform: String,
    },
    /// The program in a terminal set its title. Sent only to clients on this
    /// host; devices read it in their view.
    TerminalTitle {
        id: SessionId,
        title: String,
    },
    /// What a client reads through `Call` under `topic` changed, for
    /// `project` or for every project when it is absent, so the client reads
    /// it again. `paths` names what moved inside it; absent means anything
    /// may have. A client skips topics it does not know.
    Changed {
        topic: crate::changed::Topic,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paths: Option<Vec<String>>,
    },
    /// The next part of an answer that arrives in parts, sent only to the
    /// connection that started `stream`. The last part has `done`, and
    /// `error` when the stream failed: `{category, message}` and whatever
    /// else its source says about the failure.
    Stream {
        stream: u64,
        items: Vec<Value>,
        #[serde(default)]
        done: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Value>,
    },
}
