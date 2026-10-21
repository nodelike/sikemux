//! Wire format between the core and its clients.
//!
//! Every frame is a 4-byte big-endian length, then one kind byte, then the
//! payload. The length counts the kind byte and the payload. Control frames
//! carry JSON; terminal bytes travel raw in their own frame kinds.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read};
use std::path::PathBuf;

use crate::pty::agent_detection::{DetectionExplain, ManifestReloadReport};
use crate::pty::launch::{PtyContext, PtyDirectCommand};
use crate::pty::output_log::{OutputPage, OutputQuery};
use crate::pty::shell_protocol::{PtyShellMetadataEvent, ShellMetadataSnapshot, SshLink};
use crate::pty::task::{TaskSource, TaskSpawnRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL: &str = "sikemux-core";
pub const PROTOCOL_VERSION: u32 = 11;
/// The oldest version a device may speak and still be served. A change a
/// device from an older release can still read bumps only `PROTOCOL_VERSION`,
/// so phones waiting on an app store review keep working.
pub const OLDEST_PROTOCOL_VERSION: u32 = 9;
/// How long a core waits for a sleeping chat it was asked to wake to come
/// back up, its agent's adapter and CLI with it. A device waits longer, so the
/// core's reason for giving up reaches it.
pub const WAKE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
/// Room for the largest attach snapshot plus its header.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// The largest file a device may send for a chat. Sent as base64, it still
/// fits in one frame.
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

pub type SessionId = u64;
pub type RequestId = u64;
pub type CallId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 0,
    /// Core to client: session id (u64 BE), then the terminal's bytes.
    Output = 1,
    /// Core to client, answering an attach: request id (u64 BE), session id
    /// (u64 BE), header length (u32 BE), [`AttachHeader`] JSON, replay bytes.
    Snapshot = 2,
    /// Client to core, a write: request id (u64 BE), session id (u64 BE), then
    /// the bytes for the terminal. Answered like any other request.
    Input = 3,
    /// A request every core answers, whatever protocol version it speaks:
    /// sent instead of a hello, and answered once.
    Frozen = 0x46,
}

impl FrameKind {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Control),
            1 => Some(Self::Output),
            2 => Some(Self::Snapshot),
            3 => Some(Self::Input),
            0x46 => Some(Self::Frozen),
            _ => None,
        }
    }
}

pub fn encode_frozen<T: Serialize>(message: &T) -> serde_json::Result<Vec<u8>> {
    Ok(encode_frame(
        FrameKind::Frozen,
        &[&serde_json::to_vec(message)?],
    ))
}

#[derive(Debug)]
pub struct Frame {
    pub kind: FrameKind,
    pub payload: Vec<u8>,
}

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
    /// The client has finished with this many output bytes of a session it is
    /// attached to. Never answered.
    Ack { id: SessionId, bytes: usize },
    /// The window's answer to a [`ServerMessage::WindowCall`].
    WindowReply {
        call_id: CallId,
        answer: WindowAnswer,
    },
    /// Every editor tab a waiting `open` call opened has closed.
    WindowOpenClosed { call_id: CallId },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request {
    Spawn {
        launch: LaunchIdentity,
        target: Box<SpawnTarget>,
    },
    Resize {
        id: SessionId,
        cols: u16,
        rows: u16,
    },
    Kill {
        id: SessionId,
    },
    List,
    Attach {
        id: SessionId,
    },
    Detach {
        id: SessionId,
    },
    /// Like `Attach` without the replay: the client already holds the screen.
    Subscribe {
        id: SessionId,
    },
    /// Turns off the input modes a crashed program may have left on, for the
    /// core's screen and every attached client alike.
    ResetModes {
        id: SessionId,
    },
    TaskOutput {
        id: SessionId,
        query: OutputQuery,
    },
    /// Whether the session's shell is at its prompt, running nothing.
    ShellAtPrompt {
        id: SessionId,
    },
    /// Where the person's own agent detection rules live. Loads them at once.
    Configure {
        manifest_dir: Option<PathBuf>,
    },
    ListManifests,
    ReloadManifests,
    ExplainAgentDetection {
        agent_id: String,
    },
    /// Kills every session and keeps running.
    StopAll,
    /// This connection is the app's window from now on, replacing any other.
    /// Tool calls that need the window are sent to it.
    RegisterWindow,
    /// The person did something in a plugin, such as signing in, that may
    /// change the tools it offers agents.
    PluginsChanged,
    Shutdown {
        stop_all: bool,
    },
    /// Starts a chat agent and answers once its session is ready. The client
    /// that asks hears the agent's events from the start.
    AcpStart {
        launch: Box<ChatLaunch>,
    },
    /// Takes up a chat agent the core already runs: the answer replays what
    /// it said so far, and its events follow. A client that watched the chat
    /// before passes where it got to, and hears only what it missed when the
    /// core still has that.
    AcpAttach {
        agent_id: String,
        since: Option<ChatMark>,
    },
    /// The turns before event `before` of the chat's run `feed`, as many as
    /// `turns`, for a phone paging back from its attachment's `older_before`.
    AcpHistory {
        agent_id: String,
        feed: String,
        before: u64,
        turns: u32,
    },
    /// Stops the chat's events reaching this client.
    AcpDetach {
        agent_id: String,
    },
    AcpList,
    /// Asks the app to start a chat it put to sleep. Answers once it runs.
    AcpWake {
        agent_id: String,
    },
    AcpPrompt {
        agent_id: String,
        /// Names the message, so it can be edited later.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    },
    /// Takes the chat back to before the person's message `message_id`, and
    /// sends `text` in its place.
    AcpEdit {
        agent_id: String,
        message_id: String,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    },
    /// Puts a message into the running turn. Answered with `promptRequired`
    /// when the turn ended first and the message should be a prompt instead.
    AcpSteer {
        agent_id: String,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    },
    /// A file from a device for a chat's next message, `data` in base64. Kept
    /// on the host beside the pictures pasted into the app's chats, and
    /// answered with its path there for the prompt's `paths`.
    AttachFile {
        agent_id: String,
        name: String,
        mime: String,
        data: String,
    },
    /// A request by name, its `params` and answer as JSON. The core alone
    /// knows each method, so a device gains one without its native code
    /// changing. A method is never removed or renamed once released.
    Call {
        method: String,
        #[serde(default)]
        params: Value,
    },
    AcpCancel {
        agent_id: String,
    },
    AcpStopTask {
        agent_id: String,
        task_id: String,
    },
    AcpPermissionReply {
        agent_id: String,
        request_id: String,
        option_id: Option<String>,
    },
    AcpStop {
        agent_id: String,
    },
    AcpSetPermissionMode {
        agent_id: String,
        mode: String,
    },
    AcpSetConfig {
        agent_id: String,
        config_id: String,
        value: String,
    },
    /// Starts the chat's agent again on `account`, on the same session. A turn
    /// that just failed because of the old account is sent again.
    AcpSwitchAccount {
        agent_id: String,
        account: ChatAccount,
    },
    RemoteStatus,
    /// Lets paired devices reach the core from other machines, or stops it
    /// and disconnects them. Only a signed-in host can turn it on. Kept across
    /// restarts.
    SetRemoteAccess {
        enabled: bool,
    },
    SetDeviceAccess {
        id: String,
        access: DeviceAccess,
    },
    /// Forgets a paired device and ends its connections.
    RevokeDevice {
        id: String,
    },
    /// A paired device forgetting this host: removes it from the paired
    /// devices and closes its connection once answered.
    Unpair,
    /// A paired phone's key for sealing its notifications from this host,
    /// and what it wants to hear about. Sent again whenever either changes;
    /// a new key replaces the old one.
    SetNotifications {
        key_id: u32,
        key: String,
        prefs: NotifyPrefs,
    },
    /// A paired phone wants no more notifications from this host.
    ClearNotifications,
    /// Whether the phone's app is in front, which it is when it connects.
    /// A phone in front showing a chat gets no notifications about it.
    SetForeground {
        foreground: bool,
    },
    /// Signs the text that registers this core with an account, built by
    /// `accounts::registration_message` from the server's challenge.
    SignRegistration {
        nonce: String,
        user_id: String,
    },
    /// The account this host is signed in to, or none after signing out.
    /// Signing in turns remote access on. Signing out turns it off and takes
    /// this host off the account, waiting a few seconds for the server to
    /// confirm. Kept across restarts.
    SetOwner {
        owner: Option<String>,
    },
    /// The person's answer to a phone that came with a ticket from the
    /// account.
    AnswerPairing {
        id: String,
        allow: bool,
        access: DeviceAccess,
    },
    /// Phones on the account the person let in when signing this host in.
    /// One already waiting to join gets in at once; the others get in
    /// without asking when they first connect.
    AllowDevices {
        devices: Vec<AllowedDevice>,
    },
    /// What the app draws behind its panes, so devices draw the same.
    /// Replaces what it published before.
    PublishBackdrop {
        texture: bool,
        image: Option<BackdropImage>,
    },
    /// The picture published with the backdrop, as a data URL.
    BackdropImage,
    /// The colours of the app's theme, by name, so devices draw in them.
    /// Replaces what it published before.
    PublishPalette {
        palette: BTreeMap<String, String>,
    },
    /// The agents the app shows now, replacing the last list, and whether its
    /// window is in front. An agent on screen in a window in front is never
    /// left unread, and one on screen is not put to sleep.
    PublishOnScreen {
        agent_ids: Vec<String>,
        focused: bool,
    },
    /// Sends this client the [`DeviceView`] now and whenever it changes, as
    /// paired devices get it.
    WatchView,
    /// Asks the app's window to show this agent. Refused when no window is
    /// open; a window that opens within a minute shows it then.
    FocusAgent {
        agent_id: String,
    },
    Workspace,
    /// What agents wait on a person for now.
    Attentions,
    /// The computer this core runs on, so a device can name it.
    Host,
    /// Starts a chat agent the way the app would, in one of its projects.
    StartChat {
        launcher: String,
        project: String,
        permission_mode: Option<String>,
        model: Option<String>,
        effort: Option<String>,
    },
    /// Takes up again one of the recent saved chats in the device view,
    /// named by its [`RecentInfo::id`], and answers like `StartChat`.
    ResumeChat {
        recent: String,
        permission_mode: Option<String>,
        model: Option<String>,
        effort: Option<String>,
    },
}

/// Everything the core needs to start a chat agent, resolved by the app: the
/// program and its environment, and the tool servers the agent is told of.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLaunch {
    pub agent_id: String,
    pub provider: String,
    pub cwd: PathBuf,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub mcp_servers: Vec<Value>,
    /// The provider's session to load instead of opening a new one.
    pub resume_id: Option<String>,
    pub permission_mode: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The account the agent signs in as, when the app names one.
    #[serde(default)]
    pub account: Option<ChatAccount>,
    /// Accounts the chat moves to, in order, when its own runs out of usage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<ChatAccount>,
}

/// One of the person's accounts with a provider, as the variables that point
/// the agent at it. They replace the provider's account variables in the
/// launch's environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAccount {
    pub id: String,
    pub label: String,
    pub env: BTreeMap<String, String>,
}

/// What a device learns of a recent chat: never the launcher behind it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentInfo {
    /// Names it in `ResumeChat`.
    pub id: String,
    pub provider: String,
    pub title: String,
    /// The app's project it ran in.
    pub project: String,
    pub cwd: PathBuf,
    /// Unix milliseconds.
    pub active_at: u64,
}

/// Something read elsewhere and handed to the agent whole, such as an issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatContext {
    pub uri: String,
    pub title: String,
    pub text: String,
}

/// The app's `acp_start` answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStart {
    pub session_id: String,
    pub capabilities: Value,
    pub setup: Value,
}

/// The kinds of the app's `acp_event`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatEventKind {
    Status,
    Ready,
    SessionUpdate,
    /// What a person sent the agent, for everyone watching but the sender.
    Prompt,
    TurnStarted,
    TurnCompleted,
    PermissionRequest,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatEvent {
    pub kind: ChatEventKind,
    pub payload: Value,
}

/// The last of a chat's events a client heard. `feed` changes whenever the
/// chat's agent starts again, which starts `seq` over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMark {
    pub feed: String,
    pub seq: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ChatAttachment {
    /// `replay` holds what the agent said since its session started or
    /// loaded, in order, so a client rebuilds the chat as if it had watched.
    /// Live events follow, numbered from `mark.seq + 1`.
    Live {
        start: Box<ChatStart>,
        permission_mode: String,
        running: bool,
        /// A turn has run in this session, so the provider keeps it.
        turned: bool,
        replay: Vec<ChatEvent>,
        mark: ChatMark,
        /// A phone is sent only the chat's last turns. Asking for the history
        /// before this event, with [`Request::AcpHistory`], pages back.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        older_before: Option<u64>,
    },
    /// The events the client missed since the mark it attached with, except
    /// the prompts it sent itself. Live events follow `mark`.
    Resumed {
        events: Vec<ChatEvent>,
        mark: ChatMark,
    },
    Missing,
    /// The session said more than the core keeps, so the client starts it
    /// again from the provider's own history.
    Restart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatState {
    Starting,
    Ready,
    /// The app has the chat open but its agent is not running: asleep, or it
    /// failed to start.
    Stopped,
}

/// Where an agent stands for the person, in the order lists show the groups:
/// waiting on them first. Only a running turn is Working; subagents and
/// shells left running are counted beside the agent instead.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum AgentGroup {
    /// It waits on a permission or an answer.
    Blocked,
    Working,
    /// It finished a turn the person has not seen.
    Done,
    #[default]
    Idle,
}

/// What a list says an agent is doing: its group, unless it is not running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    Blocked,
    Working,
    Done,
    #[default]
    Idle,
    /// The app stopped it while idle; it starts again when opened.
    Asleep,
    /// It ended or failed to start.
    Stopped,
    Starting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatInfo {
    pub agent_id: String,
    /// What the core calls the agent, such as Otter.
    #[serde(default)]
    pub name: Option<String>,
    pub provider: String,
    /// What the agent named the session, once it has.
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub state: ChatState,
    pub running: bool,
    pub pending_permissions: Vec<String>,
    /// Subagents still running for it.
    #[serde(default)]
    pub subagents: u32,
    /// Background shells and monitors still running for it.
    #[serde(default)]
    pub tasks: u32,
    /// The agent is retrying a request that got no answer over the network:
    /// the attempt it is on, or 0 when it did not say.
    #[serde(default)]
    pub reconnecting: Option<u32>,
    /// The paired device that started it. The app started the rest.
    pub started_by: Option<String>,
    /// The app's launcher a device started it with.
    pub launcher: Option<String>,
    pub permission_mode: String,
    pub model: Option<String>,
    /// The provider its turns are paid with, in place of the agent's own account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    pub effort: Option<String>,
    /// The app stopped its agent while idle; `AcpWake` starts it again.
    pub asleep: bool,
    /// It finished a turn or asked for something while the person was not
    /// looking at it in the app.
    pub unread: bool,
    /// When it last opened, started work, finished or asked for something,
    /// in Unix milliseconds. Unknown for chats from before the core started.
    pub active_at: Option<u64>,
    #[serde(default)]
    pub group: AgentGroup,
    #[serde(default)]
    pub status: AgentStatus,
    /// Its place among the open agents, chats and agent terminals together,
    /// in the order they want the person: 0 first.
    #[serde(default)]
    pub attention_rank: u32,
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
    /// Older events of a chat, in order. Paging goes on before `older_before`;
    /// without one this reaches the start of what the host keeps.
    ChatHistory {
        events: Vec<ChatEvent>,
        older_before: Option<u64>,
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

/// A project on this host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub pinned: bool,
    /// The space it is shown in, by id; none shows it in every space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// What the person picked by hand for it on each code host, by the
    /// host's plugin id, over what its remote and the host's accounts say.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub code_hosts: BTreeMap<String, CodeHostChoice>,
    /// What the project is known by on every host (ADR 0017): its remote,
    /// normalised, else its repository's first commit. A folder outside git
    /// has none. The core keeps it; clients only read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

/// A repository or account picked for a project on one code host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeHostChoice {
    /// `owner/name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// A set of projects the person shows on their own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceInfo {
    pub id: String,
    pub name: String,
    /// Empty when the person removed it.
    #[serde(default)]
    pub icon: String,
}

/// A saved chat kept at the top of the list, open or closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedChat {
    pub provider: String,
    /// The provider's own id for the conversation, which resumes it.
    pub session_id: String,
    pub title: String,
    /// The folder it ran in.
    pub project: PathBuf,
}

/// An agent in the workspace, kept whether or not it runs now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAgent {
    pub id: String,
    /// The CLI it runs, such as `claude`.
    pub provider: String,
    /// Its project, by id.
    pub project: String,
    pub cwd: PathBuf,
    pub title: String,
    /// The person named it, so the agent's own titles no longer replace it.
    #[serde(default)]
    pub renamed: bool,
    /// The agent that started it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    /// The provider's session, which resumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_id: Option<String>,
    pub permission_mode: String,
    /// The person's account it runs on, by profile id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model provider it runs on in place of its maker's account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Never put to sleep while idle.
    #[serde(default)]
    pub keep_alive: bool,
    /// Stopped, and woken again on its saved conversation when opened.
    #[serde(default)]
    pub asleep: bool,
    /// The git worktree it works in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<Value>,
    /// The character drawn for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buddy: Option<Value>,
    /// The terminal session running it, for an agent in a terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_id: Option<SessionId>,
    /// Unix milliseconds.
    #[serde(default)]
    pub created_at: u64,
}

/// What a device learns about a launcher: never its program or environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherInfo {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub permission_mode: String,
    /// The `configOptions` the provider's last session offered, such as its
    /// models and effort levels, or null before one has started.
    pub config_options: Value,
    /// Whether the agent is ready, signed out, missing or broken; absent when the app has not said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The models in `config_options`, as a picker shows them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ConfigPicker>,
    /// How hard the model thinks, as a picker shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ConfigPicker>,
}

/// One of a session's select options, read for a picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPicker {
    /// The config id to set it with.
    pub id: String,
    pub current: String,
    pub choices: Vec<ConfigChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChoice {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub projects: Vec<ProjectInfo>,
    #[serde(default)]
    pub spaces: Vec<SpaceInfo>,
    #[serde(default)]
    pub agents: Vec<WorkspaceAgent>,
    #[serde(default)]
    pub pinned: Vec<PinnedChat>,
    /// The mode agents that can bypass permissions start in.
    #[serde(default)]
    pub permission_mode: String,
    /// The plugins the person switched off, by id.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
    pub launchers: Vec<LauncherInfo>,
    /// The app's theme colours by name; empty until the app publishes them.
    pub palette: BTreeMap<String, String>,
    pub backdrop: Backdrop,
}

/// What the app draws behind its panes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backdrop {
    /// The dithered grain that moves behind each pane.
    pub texture: bool,
    /// Names the picture shown in place of the grain; `BackdropImage` fetches it.
    pub image: Option<String>,
}

/// A picture shown behind the panes, shrunk to suit a phone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackdropImage {
    /// Changes whenever the picture does, so a device fetches it only then.
    pub id: String,
    pub data_url: String,
}

/// What a paired device was approved to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceAccess {
    /// Everything a person at the host can do in a session.
    Full,
    /// Read sessions and answer agents' permission requests.
    Watch,
}

/// The computer a core runs on, as a person would recognise it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    /// The name set in System Settings, such as "Kishore's MacBook Pro".
    pub name: String,
    /// The kind of computer, such as "MacBook Pro" or "Mac mini".
    pub model: String,
    /// The Sikemux release running there, such as "0.4.3-nightly.5".
    pub version: String,
    pub channel: BuildChannel,
}

/// Which kind of Sikemux build a core belongs to. Each keeps its own key and
/// paired devices, so one host can show up once per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildChannel {
    Dev,
    Nightly,
    Stable,
}

impl BuildChannel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Nightly => "nightly",
            Self::Stable => "stable",
        }
    }

    /// A release's channel by its version: `0.5.0-nightly.3` is a nightly.
    pub fn of_release(version: &str) -> Self {
        if version.contains("-nightly") {
            Self::Nightly
        } else {
            Self::Stable
        }
    }
}

/// A device approved to reach this core from another machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// The device's public key, which is what the core recognises it by.
    pub id: String,
    pub name: String,
    pub platform: String,
    pub access: DeviceAccess,
    /// Milliseconds since the Unix epoch.
    pub paired_at: u64,
    pub last_seen: Option<u64>,
}

/// A phone the person lets in ahead of it asking, by its account key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowedDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub access: DeviceAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    /// The core's public key, which paired devices dial.
    pub core_id: String,
    /// Where the core can be reached directly, while remote access is on.
    pub addresses: Vec<String>,
    pub devices: Vec<DeviceInfo>,
    /// Ids of the devices connected now.
    pub connected: Vec<String>,
    /// Phones that came with a ticket from the account and wait for the
    /// person to answer.
    pub pending: Vec<PendingDevice>,
    /// The account this host is signed in to.
    pub owner: Option<String>,
    /// The live connection to that account, or why the account let this
    /// host go.
    #[serde(default)]
    pub account: Option<AccountLink>,
    /// Set while the accounts server no longer works with this build. Remote
    /// access and the account stay off until it updates.
    #[serde(default)]
    pub update_required: Option<UpdateRequired>,
    /// The phones that asked this host for notifications.
    #[serde(default)]
    pub notifications: Vec<PhoneNotifications>,
    /// The person asked to allow each new device and host here, rather than
    /// let in whatever their account signed in (ADR 0018).
    #[serde(default)]
    pub require_approval: bool,
}

/// What a phone wants to hear about from this host. The phone owns these
/// and sends them on every connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyPrefs {
    /// Permission requests, and terminal agents waiting for input.
    pub needs_you: bool,
    pub finished: bool,
    pub problems: bool,
    pub when: NotifyWhen,
    #[serde(default)]
    pub muted: Vec<NotifyMute>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotifyWhen {
    /// Only while nobody is using this host: locked, or untouched for a
    /// couple of minutes.
    Away,
    Always,
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyMute {
    pub agent_id: String,
    /// Milliseconds since the Unix epoch, or `None` while the agent runs.
    pub until: Option<u64>,
}

/// Whether a phone's notifications from this host reach it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneNotifications {
    pub device_id: String,
    pub state: NotificationState,
    /// Milliseconds since the Unix epoch when `state` began.
    pub since: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationState {
    /// The phone asked for none from this host.
    Off,
    /// The last one was delivered, or none was sent yet.
    On,
    /// Notifications are turned off on the phone itself.
    PhoneOff,
    /// The push services could not deliver the last one.
    NotReaching,
    /// The phone is not on this host's account, so the server will not
    /// pass notifications to it.
    OtherAccount,
    /// This host is not signed in to an account, which notifications go
    /// through.
    SignedOut,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRequired {
    pub current: String,
    pub minimum: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLink {
    pub state: AccountLinkState,
    /// Why the account let this host go, once `state` is `removed`. None
    /// when the server no longer knew this host at all.
    pub reason: Option<crate::accounts::protocol::RevokeReason>,
    /// Milliseconds since the Unix epoch when `state` began.
    pub since: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountLinkState {
    Connecting,
    Live,
    /// The last attempt failed; another follows.
    Offline,
    /// Taken off the account from elsewhere, or the account was deleted.
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDevice {
    /// Names this request in `AnswerPairing`.
    pub id: String,
    pub device_id: String,
    /// What the device calls itself. Nothing vouches for it.
    pub name: String,
    pub platform: String,
    /// The device came with a ticket from the host's account, as every
    /// device does.
    #[serde(default)]
    pub from_account: bool,
    /// When the phone stops waiting for an answer, in milliseconds since the
    /// Unix epoch.
    pub expires_at: u64,
}

/// Which build of the sidecar a core runs. `source` fingerprints the code
/// the core is compiled from, so two compilations of the same code are the
/// same build although they finished at different times.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildIdentity {
    pub version: String,
    pub commit: String,
    pub built_at: u64,
    pub source: String,
}

/// Makes a build pass for another one, so tests can upgrade a core to the
/// binary it already runs: the value replaces the commit and the source, and
/// the build time is zero.
pub const BUILD_ID_OVERRIDE_ENV: &str = "SIKEMUX_BUILD_ID_OVERRIDE";

impl BuildIdentity {
    pub fn new(version: &str, commit: &str, built_at: u64, source: &str) -> Self {
        match std::env::var(BUILD_ID_OVERRIDE_ENV) {
            Ok(id) if !id.is_empty() => Self {
                version: version.into(),
                commit: id.clone(),
                built_at: 0,
                source: id,
            },
            _ => Self {
                version: version.into(),
                commit: commit.into(),
                built_at,
                source: source.into(),
            },
        }
    }

    /// Whether both run the same code, whenever and from whichever commit
    /// each was compiled.
    pub fn same_build(&self, other: &Self) -> bool {
        self.version == other.version && self.source == other.source
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    Terminal,
    Task,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: SessionId,
    pub kind: SessionKind,
    pub pid: Option<u32>,
    pub running: bool,
    pub cols: u16,
    pub rows: u16,
    pub attached: usize,
    pub project: Option<String>,
    pub pane_id: Option<String>,
    pub agent_id: Option<String>,
    /// What the core calls an agent terminal's agent, such as Otter.
    #[serde(default)]
    pub agent_name: Option<String>,
    pub agent_type: Option<String>,
    pub task_execution_id: Option<String>,
    /// The last state published for an agent terminal.
    pub agent_state: Option<String>,
    /// What a task session was started as, so a client that did not start it
    /// can take it over.
    pub task: Option<TaskSessionInfo>,
    /// Set once the process was reaped.
    pub exit: Option<SessionExit>,
    /// A client asked for the process to end.
    pub killed: bool,
    /// The paired device that started it. The app started the rest.
    pub started_by: Option<String>,
    /// The name the app gives an agent terminal.
    pub title: Option<String>,
    /// What the program in the terminal last set its title to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    /// The agent finished or asked for something while the person was not
    /// looking at it in the app.
    pub unread: bool,
    /// When the agent last started work, finished or asked for something, in
    /// Unix milliseconds.
    pub active_at: Option<u64>,
    /// For an agent terminal the device view lists, as for a chat.
    #[serde(default)]
    pub group: Option<AgentGroup>,
    #[serde(default)]
    pub status: Option<AgentStatus>,
    #[serde(default)]
    pub attention_rank: Option<u32>,
    /// For an SSH terminal, where its connection last stood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshLink>,
}

/// A task's launch request without its environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSessionInfo {
    pub execution_id: String,
    pub terminal_key: String,
    pub task_id: String,
    pub label: String,
    pub project: String,
    pub source: TaskSource,
    pub command: String,
    pub cwd: String,
    pub agent_id: Option<String>,
}

/// `code` is absent when the status could not be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExit {
    pub code: Option<u32>,
    pub signal: Option<String>,
}

/// What happened to this host's network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkChange {
    Online,
    Offline,
    /// Still online, over a different interface.
    Changed,
    /// The Mac woke from sleep.
    Woke,
}

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
    /// Sent only to the clients that started or attached to the chat.
    /// `seq` counts the chat's events, so a client can tell which ones the
    /// attach answer already held.
    Chat {
        agent_id: String,
        seq: u64,
        event: ChatEvent,
    },
    /// Sent only to paired devices: what they show of this host, whole, when
    /// they connect and whenever any of it changes.
    DeviceView {
        view: DeviceView,
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
        topic: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paths: Option<Vec<String>>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub workspace: Workspace,
    pub sessions: Vec<SessionInfo>,
    pub chats: Vec<ChatInfo>,
    pub attentions: Vec<Attention>,
    /// The newest saved chats in this host's projects, none of them open.
    #[serde(default)]
    pub recent: Vec<RecentInfo>,
    /// What waits on the person, ranked, as the core's inbox JSON. Kept as
    /// JSON so a change to its shape reaches phones over the air.
    #[serde(default)]
    pub inbox: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttentionKind {
    /// Answer with `AcpPermissionReply`, naming `id` and one of the options
    /// in `request`.
    Permission,
}

/// Something an agent is waiting on a person for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    pub id: String,
    pub kind: AttentionKind,
    pub agent_id: String,
    pub provider: String,
    pub cwd: PathBuf,
    /// The agent's own request, with what it wants to do and the options.
    pub request: Value,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    #[serde(default)]
    pub choices: PermissionChoices,
}

/// What a permission request comes down to for a client that offers only
/// allow, always allow and deny: the option each answers with, and what the
/// agent wants to do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionChoices {
    pub allow_once: Option<String>,
    /// The allow that lasts longest, beyond the session when one does.
    pub allow_always: Option<String>,
    pub reject: Option<String>,
    /// The shell command it wants to run, when it is one.
    pub command: Option<String>,
    /// What its tool call is called.
    pub title: Option<String>,
}

/// The app's `agent_state_changed` payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStateEvent {
    pub agent_id: String,
    pub state: String,
    pub sequence: u64,
    pub source: String,
    pub confidence: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachHeader {
    pub alternate_screen: bool,
    pub shell: Option<ShellMetadataSnapshot>,
    /// The session's `Exited` event was sent before this snapshot, so the
    /// client will not hear of the exit again.
    pub exited: bool,
}

pub fn encode_frame(kind: FrameKind, parts: &[&[u8]]) -> Vec<u8> {
    let payload_len: usize = parts.iter().map(|part| part.len()).sum();
    let mut frame = Vec::with_capacity(5 + payload_len);
    frame.extend_from_slice(&((payload_len + 1) as u32).to_be_bytes());
    frame.push(kind as u8);
    for part in parts {
        frame.extend_from_slice(part);
    }
    frame
}

pub fn encode_control<T: Serialize>(message: &T) -> serde_json::Result<Vec<u8>> {
    Ok(encode_frame(
        FrameKind::Control,
        &[&serde_json::to_vec(message)?],
    ))
}

pub fn encode_output(id: SessionId, bytes: &[u8]) -> Vec<u8> {
    encode_frame(FrameKind::Output, &[&id.to_be_bytes(), bytes])
}

pub fn encode_input(request_id: RequestId, id: SessionId, bytes: &[u8]) -> Vec<u8> {
    encode_frame(
        FrameKind::Input,
        &[&request_id.to_be_bytes(), &id.to_be_bytes(), bytes],
    )
}

pub fn encode_snapshot(
    request_id: RequestId,
    id: SessionId,
    header: &AttachHeader,
    replay: &[u8],
) -> serde_json::Result<Vec<u8>> {
    let header = serde_json::to_vec(header)?;
    Ok(encode_frame(
        FrameKind::Snapshot,
        &[
            &request_id.to_be_bytes(),
            &id.to_be_bytes(),
            &(header.len() as u32).to_be_bytes(),
            &header,
            replay,
        ],
    ))
}

fn split_u64(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let (head, rest) = bytes.split_first_chunk::<8>()?;
    Some((u64::from_be_bytes(*head), rest))
}

pub fn decode_output(payload: &[u8]) -> Option<(SessionId, &[u8])> {
    split_u64(payload)
}

pub fn decode_input(payload: &[u8]) -> Option<(RequestId, SessionId, &[u8])> {
    let (request_id, rest) = split_u64(payload)?;
    let (id, bytes) = split_u64(rest)?;
    Some((request_id, id, bytes))
}

pub fn decode_snapshot(payload: &[u8]) -> Option<(RequestId, SessionId, AttachHeader, &[u8])> {
    let (request_id, rest) = split_u64(payload)?;
    let (id, rest) = split_u64(rest)?;
    let (header_len, rest) = rest.split_first_chunk::<4>()?;
    let header_len = u32::from_be_bytes(*header_len) as usize;
    if header_len > rest.len() {
        return None;
    }
    let (header, replay) = rest.split_at(header_len);
    let header = serde_json::from_slice(header).ok()?;
    Some((request_id, id, header, replay))
}

/// Whether a frame from [`encode_frame`] is small enough for the other side
/// to read.
pub fn fits(frame: &[u8]) -> bool {
    frame.len() <= MAX_FRAME_BYTES + 5
}

fn frame_length(header: [u8; 4], max_payload: usize) -> io::Result<usize> {
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > max_payload + 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {length} is out of range"),
        ));
    }
    Ok(length)
}

/// A frame is read in chunks that double from this size, so a peer that
/// declares a long frame without sending it never gets room made for it.
const FIRST_CHUNK_BYTES: usize = 64 * 1024;

/// Grows `body` toward `length` and answers with the new room to read into.
fn next_chunk(body: &mut Vec<u8>, length: usize) -> Option<&mut [u8]> {
    let start = body.len();
    if start >= length {
        return None;
    }
    let end = (start * 2).max(FIRST_CHUNK_BYTES).min(length);
    body.resize(end, 0);
    body.get_mut(start..)
}

fn split_frame(mut body: Vec<u8>) -> io::Result<Frame> {
    let kind = body
        .first()
        .copied()
        .and_then(FrameKind::from_byte)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown frame kind"))?;
    body.remove(0);
    Ok(Frame {
        kind,
        payload: body,
    })
}

pub fn read_frame_sync<R: Read>(reader: &mut R) -> io::Result<Option<Frame>> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = frame_length(header, MAX_FRAME_BYTES)?;
    let mut body = Vec::new();
    while let Some(chunk) = next_chunk(&mut body, length) {
        reader.read_exact(chunk)?;
    }
    split_frame(body).map(Some)
}

#[cfg(unix)]
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> io::Result<Option<Frame>> {
    read_frame_within(reader, MAX_FRAME_BYTES).await
}

/// Refuses a frame longer than `max_payload` before reading it.
#[cfg(unix)]
pub async fn read_frame_within<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    max_payload: usize,
) -> io::Result<Option<Frame>> {
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = frame_length(header, max_payload)?;
    let mut body = Vec::new();
    while let Some(chunk) = next_chunk(&mut body, length) {
        reader.read_exact(chunk).await?;
    }
    split_frame(body).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_reads_the_view_of_a_core_from_before_recent_chats() {
        let mut view = serde_json::to_value(DeviceView {
            workspace: Workspace::default(),
            sessions: Vec::new(),
            chats: Vec::new(),
            attentions: Vec::new(),
            recent: Vec::new(),
            inbox: Value::Null,
        })
        .unwrap();
        view.as_object_mut().unwrap().remove("recent");
        let read: DeviceView = serde_json::from_value(view).unwrap();
        assert!(read.recent.is_empty());
    }

    #[test]
    fn frames_carry_a_big_endian_length_that_counts_the_kind_byte() {
        let frame = encode_output(7, b"hi");
        assert_eq!(&frame[..4], &11u32.to_be_bytes());
        assert_eq!(frame[4], FrameKind::Output as u8);
        let parsed = read_frame_sync(&mut &frame[..]).unwrap().unwrap();
        assert_eq!(parsed.kind, FrameKind::Output);
        assert_eq!(decode_output(&parsed.payload), Some((7, &b"hi"[..])));
    }

    #[test]
    fn snapshot_frames_round_trip_header_and_raw_replay() {
        let header = AttachHeader {
            alternate_screen: true,
            shell: None,
            exited: true,
        };
        let frame = encode_snapshot(3, 9, &header, b"\xff\x00replay").unwrap();
        let parsed = read_frame_sync(&mut &frame[..]).unwrap().unwrap();
        let (request_id, id, decoded, replay) = decode_snapshot(&parsed.payload).unwrap();
        assert_eq!((request_id, id), (3, 9));
        assert_eq!(decoded, header);
        assert_eq!(replay, b"\xff\x00replay");
    }

    #[test]
    fn oversized_and_empty_frames_are_rejected() {
        let mut empty = &0u32.to_be_bytes()[..];
        assert!(read_frame_sync(&mut empty).is_err());
        let huge = ((MAX_FRAME_BYTES + 2) as u32).to_be_bytes();
        assert!(read_frame_sync(&mut &huge[..]).is_err());
        assert!(read_frame_sync(&mut &[][..]).unwrap().is_none());
    }

    #[tokio::test]
    async fn a_frame_past_the_reader_s_limit_is_refused_before_it_is_read() {
        let frame = encode_control(&"x".repeat(100)).unwrap();
        assert!(read_frame_within(&mut &frame[..], 64).await.is_err());
        assert!(read_frame_within(&mut &frame[..], 200)
            .await
            .unwrap()
            .is_some());
    }

    #[test]
    fn control_messages_are_tagged_camel_case_json() {
        let message = ClientMessage::Request {
            request_id: 4,
            request: Request::Shutdown { stop_all: true },
        };
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "request",
                "requestId": 4,
                "request": { "op": "shutdown", "stopAll": true }
            })
        );
        let event = ServerMessage::Event {
            event: Event::Exited {
                id: 2,
                code: Some(0),
                signal: None,
                killed: false,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(matches!(
            serde_json::from_str::<ServerMessage>(&json).unwrap(),
            ServerMessage::Event {
                event: Event::Exited { id: 2, .. }
            }
        ));
    }
}
