//! The messages each side sends: requests and their answers, events, and the
//! calls the core makes of the app's window.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::grid::FindResult;
use crate::pty::agent_detection::{DetectionExplain, ManifestReloadReport};
use crate::pty::launch::{PtyContext, PtyDirectCommand};
use crate::pty::output_log::OutputPage;
use crate::pty::task::TaskSpawnRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    ask, Attention, BuildChannel, BuildIdentity, CallId, ChatAttachment, ChatInfo, ChatStart,
    Event, HostInfo, RemoteStatus, RequestId, SessionId, SessionInfo, StreamId, Workspace,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClientMessage {
    /// A client speaks every version from `version` to `newest`. A core from
    /// before versions were agreed reads only `version`, so it is the oldest.
    Hello {
        protocol: String,
        version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        newest: Option<u32>,
    },
    Request {
        request_id: RequestId,
        request: Request,
    },
    /// The client has drawn every frame of the stream up to `frame`, so the
    /// core may send the next. Never answered.
    Ack { stream: StreamId, frame: u32 },
    /// The window's answer to a [`ServerMessage::WindowCall`].
    WindowReply {
        call_id: CallId,
        answer: WindowAnswer,
    },
    /// Every editor tab a waiting `open` call opened has closed.
    WindowOpenClosed { call_id: CallId },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    Spawn(ask::Spawn),
    Resize(ask::Resize),
    Kill(ask::Kill),
    List,
    Attach(ask::Attach),
    Detach(ask::Detach),
    TerminalInput(ask::TerminalInput),
    TerminalLines(ask::TerminalLines),
    TerminalText(ask::TerminalText),
    TerminalFind(ask::TerminalFind),
    TerminalWaitFor(ask::TerminalWaitFor),
    TerminalClear(ask::TerminalClear),
    ResetModes(ask::ResetModes),
    TaskOutput(ask::TaskOutput),
    ShellAtPrompt(ask::ShellAtPrompt),
    Configure(ask::Configure),
    ListManifests,
    ReloadManifests,
    ExplainAgentDetection(ask::ExplainAgentDetection),
    /// Kills every session and keeps running.
    StopAll,
    /// This connection is the app's window from now on, replacing any other.
    /// Tool calls that need the window are sent to it.
    RegisterWindow,
    /// The person did something in a plugin, such as signing in, that may
    /// change the tools it offers agents.
    PluginsChanged,
    Shutdown(ask::Shutdown),
    AcpStart(ask::AcpStart),
    AcpAttach(ask::AcpAttach),
    AcpHistory(ask::AcpHistory),
    AcpDetach(ask::AcpDetach),
    AcpList,
    AcpWake(ask::AcpWake),
    AcpPrompt(ask::AcpPrompt),
    AcpEdit(ask::AcpEdit),
    AcpSteer(ask::AcpSteer),
    AttachFile(ask::AttachFile),
    Call(ask::Call),
    AcpCancel(ask::AcpCancel),
    AcpStopTask(ask::AcpStopTask),
    AcpPermissionReply(ask::AcpPermissionReply),
    AcpStop(ask::AcpStop),
    AcpSetPermissionMode(ask::AcpSetPermissionMode),
    AcpSetConfig(ask::AcpSetConfig),
    AcpSwitchAccount(ask::AcpSwitchAccount),
    RemoteStatus,
    SetRemoteAccess(ask::SetRemoteAccess),
    SetDeviceAccess(ask::SetDeviceAccess),
    RevokeDevice(ask::RevokeDevice),
    /// A paired device forgetting this host: removes it from the paired
    /// devices and closes its connection once answered.
    Unpair,
    SetNotifications(ask::SetNotifications),
    /// A paired phone wants no more notifications from this host.
    ClearNotifications,
    SetForeground(ask::SetForeground),
    SignRegistration(ask::SignRegistration),
    SetOwner(ask::SetOwner),
    AnswerPairing(ask::AnswerPairing),
    AllowDevices(ask::AllowDevices),
    PublishBackdrop(ask::PublishBackdrop),
    /// The picture published with the backdrop, as a data URL.
    BackdropImage,
    PublishOnScreen(ask::PublishOnScreen),
    /// Sends this client the [`DeviceView`] now and whenever it changes, as
    /// paired devices get it.
    WatchView,
    FocusAgent(ask::FocusAgent),
    Workspace,
    /// What agents wait on a person for now.
    Attentions,
    /// The computer this core runs on, so a device can name it.
    Host,
    StartChat(ask::StartChat),
    ResumeChat(ask::ResumeChat),
}

/// Something only the app's window can do, asked of it by the core. Requests
/// are the core's CLI requests as JSON, so changing those leaves this crate alone.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WindowCall {
    Harness {
        request: Value,
    },
    /// The CLI's `open`. The answer lists what opened; a waiting call is
    /// later followed by [`ClientMessage::WindowOpenClosed`].
    Open {
        request: Value,
    },
    /// Opens a plugin's pane at the place `at` names in it, which the plugin
    /// alone reads, as the person asked.
    OpenPlugin {
        plugin: String,
        at: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WindowAnswer {
    Value { value: Value },
    Error { message: String },
}

impl From<Result<Value, String>> for WindowAnswer {
    fn from(result: Result<Value, String>) -> Self {
        match result {
            Ok(value) => Self::Value { value },
            Err(message) => Self::Error { message },
        }
    }
}

impl From<WindowAnswer> for Result<Value, String> {
    fn from(answer: WindowAnswer) -> Self {
        match answer {
            WindowAnswer::Value { value } => Ok(value),
            WindowAnswer::Error { message } => Err(message),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchIdentity {
    pub version: String,
    pub cli_executable: Option<PathBuf>,
    pub cli_endpoint: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SpawnTarget {
    Terminal(TerminalSpawn),
    Task { request: TaskSpawnRequest },
}

/// The arguments of the app's `pty_spawn`. `env` is applied last, after the
/// Sikemux identity and the agent profile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSpawn {
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<String>,
    pub startup: Option<String>,
    pub direct_command: Option<PtyDirectCommand>,
    pub context: Option<PtyContext>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub continues: Option<Continuation>,
}

/// Starts the new terminal on the screen of an ended one, below a line
/// saying why, and closes the ended one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Continuation {
    pub session: SessionId,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerMessage {
    /// `version` is the one both sides speak from here on.
    HelloAck {
        protocol: String,
        version: u32,
        pid: u32,
        build: BuildIdentity,
    },
    /// Sent instead of `HelloAck` when the client speaks no version the core
    /// does; `version` is the newest the core speaks. The core then closes
    /// the connection.
    HelloRejected {
        protocol: String,
        version: u32,
        pid: u32,
        message: String,
    },
    Response {
        request_id: RequestId,
        response: Response,
    },
    Error {
        request_id: Option<RequestId>,
        message: String,
    },
    Event {
        event: Event,
    },
    /// Sent only to the registered window, which answers with
    /// [`ClientMessage::WindowReply`].
    WindowCall {
        call_id: CallId,
        call: WindowCall,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Response {
    Spawned {
        id: SessionId,
    },
    Done,
    TerminalText {
        text: String,
    },
    TerminalLine {
        line: Option<String>,
    },
    Found {
        result: FindResult,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    TaskOutput {
        page: OutputPage,
    },
    ShellAtPrompt {
        at_prompt: bool,
    },
    Manifests {
        report: ManifestReloadReport,
    },
    DetectionExplain {
        explain: Box<DetectionExplain>,
    },
    ChatStarted {
        start: ChatStart,
    },
    ChatAttached {
        attachment: ChatAttachment,
    },
    /// Rows from before what a client holds, a [`crate::chat::ChatPage`] as
    /// JSON. Paging goes on before its `olderBefore`; without one this reaches
    /// the start of what the host keeps.
    ChatHistory {
        page: Value,
    },
    Chats {
        chats: Vec<ChatInfo>,
    },
    Steered {
        outcome: String,
    },
    ChatConfig {
        value: Value,
    },
    Remote {
        status: Box<RemoteStatus>,
    },
    Workspace {
        workspace: Workspace,
    },
    Attentions {
        attentions: Vec<Attention>,
    },
    ChatBegun {
        agent_id: String,
        start: ChatStart,
    },
    Host {
        host: HostInfo,
    },
    BackdropImage {
        data_url: Option<String>,
    },
    Registration {
        registration: HostRegistration,
    },
    Attached {
        path: PathBuf,
    },
    Called {
        result: Value,
    },
}

/// What the app sends the accounts server to register this core as a host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRegistration {
    /// The core's public key, the same one paired devices dial.
    pub key: String,
    pub name: String,
    pub channel: BuildChannel,
    /// The key's Ed25519 signature over the registration text, in hex.
    pub signature: String,
}
