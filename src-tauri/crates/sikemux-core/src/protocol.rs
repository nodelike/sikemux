//! Wire format between the core and its clients.
//!
//! Every frame is a 4-byte big-endian length, then one kind byte, then the
//! payload. The length counts the kind byte and the payload. Control frames
//! carry JSON; terminal bytes travel raw in their own frame kinds.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_pty::agent_detection::{DetectionExplain, ManifestReloadReport};
use sikemux_pty::launch::{PtyContext, PtyDirectCommand};
use sikemux_pty::output_log::{OutputPage, OutputQuery};
use sikemux_pty::shell_protocol::{PtyShellMetadataEvent, ShellMetadataSnapshot};
use sikemux_pty::task::{TaskSource, TaskSpawnRequest};

use crate::cli::protocol::{CliOpenRequest, HarnessRequest};

pub const PROTOCOL: &str = "sikemux-core";
pub const PROTOCOL_VERSION: u32 = 8;
/// Room for the largest attach snapshot plus its header.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

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
    /// See [`frozen`]: sent instead of a hello, and answered once.
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

/// Requests every core answers, whatever protocol version it speaks, so an app
/// can ask a core it cannot otherwise talk to to replace itself or stop.
///
/// The shape of everything in this module is fixed for good: a client sends
/// one [`FrameKind::Frozen`] frame holding a [`FrozenRequest`] as its first
/// frame, the core answers with one [`FrameKind::Frozen`] frame holding a
/// [`FrozenReply`], and closes the connection. Add new requests as new `op`
/// values; never change or remove a field.
pub mod frozen {
    use std::path::PathBuf;

    use serde::{Deserialize, Serialize};

    use super::BuildIdentity;

    /// The format of the state a core hands to its replacement. A core only
    /// replaces itself with a binary that reads its format.
    pub const RESUME_FORMAT: u32 = 4;

    /// Every hand-over format a core of this build takes over from.
    pub const READS_FORMATS: [u32; 4] = [1, 2, 3, RESUME_FORMAT];

    /// The argument that makes a core binary print its [`UpgradeInfo`].
    pub const UPGRADE_INFO_ARG: &str = "--upgrade-info";

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
    pub enum FrozenRequest {
        /// Replace this core with `binary` in the same process, keeping every
        /// session.
        Upgrade { binary: PathBuf },
        /// Stop every session and exit.
        StopEverything,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(
        tag = "status",
        rename_all = "camelCase",
        rename_all_fields = "camelCase"
    )]
    pub enum FrozenReply {
        Accepted,
        Refused {
            message: String,
        },
        /// The core replaces itself later, once the chat turns it is running
        /// end, and goes on serving until then.
        Deferred {
            message: String,
        },
    }

    /// What `<binary> core --upgrade-info` prints, as one JSON object.
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UpgradeInfo {
        pub version: String,
        pub commit: String,
        pub built_at: u64,
        pub source: String,
        /// The oldest hand-over format the binary takes over from. Cores
        /// that predate `reads_formats` compare their own format with this.
        pub resume_format: u32,
        #[serde(default)]
        pub reads_formats: Vec<u32>,
    }

    impl UpgradeInfo {
        pub fn of(build: &BuildIdentity) -> Self {
            Self {
                version: build.version.clone(),
                commit: build.commit.clone(),
                built_at: build.built_at,
                source: build.source.clone(),
                resume_format: READS_FORMATS[0],
                reads_formats: READS_FORMATS.to_vec(),
            }
        }

        /// Whether the binary takes over from a core writing `format`.
        pub fn reads(&self, format: u32) -> bool {
            if self.reads_formats.is_empty() {
                self.resume_format == format
            } else {
                self.reads_formats.contains(&format)
            }
        }

        pub fn build(&self) -> BuildIdentity {
            BuildIdentity {
                version: self.version.clone(),
                commit: self.commit.clone(),
                built_at: self.built_at,
                source: self.source.clone(),
            }
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
    Hello {
        protocol: String,
        version: u32,
    },
    Request {
        request_id: RequestId,
        request: Request,
    },
    /// The client has finished with this many output bytes of a session it is
    /// attached to. Never answered.
    Ack {
        id: SessionId,
        bytes: usize,
    },
    /// The window's answer to a [`ServerMessage::WindowCall`].
    WindowReply {
        call_id: CallId,
        answer: WindowAnswer,
    },
    /// Every editor tab a waiting `open` call opened has closed.
    WindowOpenClosed {
        call_id: CallId,
    },
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
    /// The window is asking the person to trust the project's tasks before it
    /// launches this run.
    HarnessAwaitingTrust {
        execution_id: String,
    },
    /// Stops the active harness runs that match every field given.
    HarnessStopRuns {
        selector: RunSelector,
    },
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
    RemoteStatus,
    /// Lets paired devices reach the core from other machines, or stops it
    /// and disconnects them. Kept across restarts.
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
    /// A paired device forgetting this Mac: removes it from the paired
    /// devices and closes its connection once answered.
    Unpair,
    /// Shows a new pairing code, replacing any open one. Remote access must
    /// be on.
    OpenPairing,
    ClosePairing,
    /// The person's answer to a device that entered the right code.
    AnswerPairing {
        id: String,
        allow: bool,
        access: DeviceAccess,
    },
    /// The projects the app has open and how it starts each chat agent,
    /// replacing what it published before. Kept in memory only, since a
    /// launcher's environment may hold secrets; the app publishes again
    /// whenever it connects.
    PublishWorkspace {
        projects: Vec<ProjectInfo>,
        launchers: Vec<ChatLauncher>,
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
    /// The chats the app lists, so devices show them under the app's names,
    /// sleeping ones included. Replaces what it published before.
    PublishChats {
        chats: Vec<PublishedChat>,
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
}

/// A chat as the app lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedChat {
    pub agent_id: String,
    pub provider: String,
    /// Absent while the chat has no name beyond its agent's.
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub asleep: bool,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatInfo {
    pub agent_id: String,
    pub provider: String,
    /// What the agent named the session, once it has.
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub state: ChatState,
    pub running: bool,
    pub pending_permissions: Vec<String>,
    /// The paired device that started it. The app started the rest.
    pub started_by: Option<String>,
    /// The app's launcher a device started it with.
    pub launcher: Option<String>,
    pub permission_mode: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The app stopped its agent while idle; `AcpWake` starts it again.
    pub asleep: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSelector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// Something only the app's window can do, asked of it by the core.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WindowCall {
    Harness {
        request: HarnessRequest,
    },
    /// The CLI's `open`. The answer lists what opened; a waiting call is
    /// later followed by [`ClientMessage::WindowOpenClosed`].
    Open {
        request: CliOpenRequest,
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
    HelloAck {
        protocol: String,
        version: u32,
        pid: u32,
        build: BuildIdentity,
    },
    /// Sent instead of `HelloAck` when the client speaks another version; the
    /// core then closes the connection.
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
    Spawned { id: SessionId },
    Done,
    Sessions { sessions: Vec<SessionInfo> },
    TaskOutput { page: OutputPage },
    Manifests { report: ManifestReloadReport },
    DetectionExplain { explain: Box<DetectionExplain> },
    ChatStarted { start: ChatStart },
    ChatAttached { attachment: ChatAttachment },
    Chats { chats: Vec<ChatInfo> },
    Steered { outcome: String },
    ChatConfig { value: Value },
    Remote { status: RemoteStatus },
    Workspace { workspace: Workspace },
    Attentions { attentions: Vec<Attention> },
    ChatBegun { agent_id: String, start: ChatStart },
    Host { host: HostInfo },
    BackdropImage { data_url: Option<String> },
}

/// A project the app has open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

/// How the app starts one kind of chat agent, short of the agent's own id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLauncher {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub permission_mode: String,
}

/// What a device learns about a launcher: never its program or environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherInfo {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub permission_mode: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub projects: Vec<ProjectInfo>,
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
    /// Everything a person at the Mac can do in a session.
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
/// paired devices, so one Mac can show up once per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildChannel {
    Dev,
    Nightly,
    Stable,
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
    pub pairing: Option<PairingOffer>,
    /// Devices that entered the code and wait for the person to answer.
    pub pending: Vec<PendingDevice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingOffer {
    pub code: String,
    /// Milliseconds since the Unix epoch.
    pub expires_at: u64,
    /// The core's key and the code as one `sikemux://pair` link, for a QR code.
    pub link: String,
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
    /// Sent only to paired devices: what they show of this Mac, whole, when
    /// they connect and whenever any of it changes.
    DeviceView {
        view: DeviceView,
    },
    /// Sent only to clients on this Mac.
    Remote {
        status: RemoteStatus,
    },
    /// An agent started waiting on a person. Every client on this Mac hears
    /// it, whether or not it shows that agent; devices see it in their view.
    Attention {
        attention: Attention,
    },
    /// A paired device started a chat. Sent only to clients on this Mac,
    /// which show it beside their own.
    ChatBegun {
        chat: ChatInfo,
    },
    /// A device opened a sleeping chat. Sent only to clients on this Mac,
    /// which start it again.
    WakeChat {
        agent_id: String,
    },
    /// What an agent waited on was answered or withdrawn.
    AttentionCleared {
        id: String,
        agent_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub workspace: Workspace,
    pub sessions: Vec<SessionInfo>,
    pub chats: Vec<ChatInfo>,
    pub attentions: Vec<Attention>,
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
    let mut body = vec![0u8; frame_length(header, MAX_FRAME_BYTES)?];
    reader.read_exact(&mut body)?;
    split_frame(body).map(Some)
}

#[cfg(unix)]
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> io::Result<Option<Frame>> {
    read_frame_within(reader, MAX_FRAME_BYTES).await
}

/// Refuses a frame longer than `max_payload` before allocating room for it.
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
    let mut body = vec![0u8; frame_length(header, max_payload)?];
    reader.read_exact(&mut body).await?;
    split_frame(body).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

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
