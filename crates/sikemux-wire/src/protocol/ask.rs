//! Each request a client sends the core, and what the core answers it with.

use std::path::PathBuf;

use crate::grid::{FindOptions, FindResult, GridPoint};
use crate::pty::agent_detection::{DetectionExplain, ManifestReloadReport};
use crate::pty::output_log::{OutputPage, OutputQuery};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    AllowedDevice, Attention, ChatAccount, ChatAttachment, ChatContext, ChatInfo, ChatLaunch,
    ChatMark, ChatStart, DeviceAccess, HostInfo, HostRegistration, LaunchIdentity, NotifyPrefs,
    PermissionMode, Request, Response, SessionId, SessionInfo, SpawnTarget, StreamId,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spawn {
    pub launch: LaunchIdentity,
    pub target: Box<SpawnTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resize {
    pub id: SessionId,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kill {
    pub id: SessionId,
}

/// Starts a stream of the session's cells, answered with a snapshot frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attach {
    pub id: SessionId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detach {
    pub stream: StreamId,
}

/// A key, text, paste, mouse or focus change for the session's program.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInput {
    pub id: SessionId,
    pub input: crate::grid::TerminalInput,
}

/// History lines `start..start + count`, answered with a lines frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLines {
    pub id: SessionId,
    pub start: u64,
    pub count: u32,
}

/// The text between two cells, or all the terminal holds without them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalText {
    pub id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<(GridPoint, GridPoint)>,
}

/// Searches the session's history and screen. The current match is the
/// first after `from`, or before it going `backwards`, or at it when
/// `inclusive`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalFind {
    pub id: SessionId,
    pub query: String,
    #[serde(default)]
    pub options: FindOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<GridPoint>,
    #[serde(default)]
    pub backwards: bool,
    #[serde(default)]
    pub inclusive: bool,
}

/// Answers with the first line on the session's screen that contains
/// `containing`, once one shows, or with none when the session ends first.
/// Needs no client to watch the terminal.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWaitFor {
    pub id: SessionId,
    pub containing: String,
}

/// Moves the cursor's line to the top of the screen and drops the history
/// above it, without telling the program.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalClear {
    pub id: SessionId,
}

/// Turns off the input modes a crashed program may have left on, for the
/// core's screen and every attached client alike.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetModes {
    pub id: SessionId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutput {
    pub id: SessionId,
    pub query: OutputQuery,
}

/// Whether the session's shell is at its prompt, running nothing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellAtPrompt {
    pub id: SessionId,
}

/// Where the person's own agent detection rules live. Loads them at once.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configure {
    pub manifest_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainAgentDetection {
    pub agent_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shutdown {
    pub stop_all: bool,
}

/// Starts a chat agent and answers once its session is ready. The client
/// that asks hears the agent's events from the start.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpStart {
    pub launch: Box<ChatLaunch>,
}

/// Takes up a chat agent the core already runs: the answer replays what
/// it said so far, and its events follow. A client that watched the chat
/// before passes where it got to, and hears only what it missed when the
/// core still has that.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAttach {
    pub agent_id: String,
    pub since: Option<ChatMark>,
}

/// The turns before event `before` of the chat's run `feed`, as many as
/// `turns`, for a phone paging back from its attachment's `older_before`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpHistory {
    pub agent_id: String,
    pub feed: String,
    pub before: u64,
    pub turns: u32,
}

/// Stops the chat's events reaching this client.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpDetach {
    pub agent_id: String,
}

/// Asks the app to start a chat it put to sleep. Answers once it runs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpWake {
    pub agent_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPrompt {
    pub agent_id: String,
    /// Names the message, so it can be edited later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
}

/// Takes the chat back to before the person's message `message_id`, and
/// sends `text` in its place.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpEdit {
    pub agent_id: String,
    pub message_id: String,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
}

/// Puts a message into the running turn. Answered with `promptRequired`
/// when the turn ended first and the message should be a prompt instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSteer {
    pub agent_id: String,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
}

/// A file from a device for a chat's next message, `data` in base64. Kept
/// on the host beside the pictures pasted into the app's chats, and
/// answered with its path there for the prompt's `paths`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachFile {
    pub agent_id: String,
    pub name: String,
    pub mime: String,
    pub data: String,
}

/// A request by name, its `params` and answer as JSON. The core alone
/// knows each method, so a device gains one without its native code
/// changing. A method is never removed or renamed once released.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Call {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpCancel {
    pub agent_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpStopTask {
    pub agent_id: String,
    pub task_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionReply {
    pub agent_id: String,
    pub request_id: String,
    pub option_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpStop {
    pub agent_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSetPermissionMode {
    pub agent_id: String,
    pub mode: PermissionMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSetConfig {
    pub agent_id: String,
    pub config_id: String,
    pub value: String,
}

/// Starts the chat's agent again on `account`, on the same session. A turn
/// that just failed because of the old account is sent again.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSwitchAccount {
    pub agent_id: String,
    pub account: ChatAccount,
}

/// Lets paired devices reach the core from other machines, or stops it
/// and disconnects them. Only a signed-in host can turn it on. Kept across
/// restarts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRemoteAccess {
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDeviceAccess {
    pub id: String,
    pub access: DeviceAccess,
}

/// Forgets a paired device and ends its connections.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeDevice {
    pub id: String,
}

/// A paired phone's key for sealing its notifications from this host,
/// and what it wants to hear about. Sent again whenever either changes;
/// a new key replaces the old one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetNotifications {
    pub key_id: u32,
    pub key: String,
    pub prefs: NotifyPrefs,
}

/// Whether the phone's app is in front, which it is when it connects.
/// A phone in front showing a chat gets no notifications about it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetForeground {
    pub foreground: bool,
}

/// Signs the text that registers this core with an account, built by
/// `accounts::registration_message` from the server's challenge.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignRegistration {
    pub nonce: String,
    pub user_id: String,
}

/// The account this host is signed in to, or none after signing out.
/// Signing in turns remote access on. Signing out turns it off and takes
/// this host off the account, waiting a few seconds for the server to
/// confirm. Kept across restarts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetOwner {
    pub owner: Option<String>,
}

/// The person's answer to a phone that came with a ticket from the
/// account.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerPairing {
    pub id: String,
    pub allow: bool,
    pub access: DeviceAccess,
}

/// Phones on the account the person let in when signing this host in.
/// One already waiting to join gets in at once; the others get in
/// without asking when they first connect.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowDevices {
    pub devices: Vec<AllowedDevice>,
}

/// What the app draws behind its panes, so devices draw the same.
/// Replaces what it published before.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishBackdrop {
    pub texture: bool,
    pub image: Option<super::BackdropImage>,
}

/// The agents the app shows now, replacing the last list, and whether its
/// window is in front. An agent on screen in a window in front is never
/// left unread, and one on screen is not put to sleep.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishOnScreen {
    pub agent_ids: Vec<String>,
    pub focused: bool,
}

/// Asks the app's window to show this agent. Refused when no window is
/// open; a window that opens within a minute shows it then.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusAgent {
    pub agent_id: String,
}

/// Starts a chat agent the way the app would, in one of its projects.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartChat {
    pub launcher: String,
    pub project: String,
    pub permission_mode: Option<PermissionMode>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// Takes up again one of the recent saved chats in the device view,
/// named by its [`RecentInfo::id`], and answers like `StartChat`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeChat {
    pub recent: String,
    pub permission_mode: Option<PermissionMode>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

macro_rules! unit_requests {
    ($($request:ident),* $(,)?) => {$(
        #[derive(Clone, Copy, Debug)]
        pub struct $request;

        impl From<$request> for Request {
            fn from(_: $request) -> Self {
                Request::$request
            }
        }
    )*};
}

unit_requests! {
    List,
    ListManifests,
    ReloadManifests,
    StopAll,
    RegisterWindow,
    PluginsChanged,
    AcpList,
    RemoteStatus,
    Unpair,
    ClearNotifications,
    BackdropImage,
    WatchView,
    Workspace,
    Attentions,
    Host,
}

macro_rules! requests {
    ($($request:ident),* $(,)?) => {$(
        impl From<$request> for Request {
            fn from(request: $request) -> Self {
                Request::$request(request)
            }
        }
    )*};
}

requests! {
    Spawn,
    Resize,
    Kill,
    Attach,
    Detach,
    TerminalInput,
    TerminalLines,
    TerminalText,
    TerminalFind,
    TerminalWaitFor,
    TerminalClear,
    ResetModes,
    TaskOutput,
    ShellAtPrompt,
    Configure,
    ExplainAgentDetection,
    Shutdown,
    AcpStart,
    AcpAttach,
    AcpHistory,
    AcpDetach,
    AcpWake,
    AcpPrompt,
    AcpEdit,
    AcpSteer,
    AttachFile,
    Call,
    AcpCancel,
    AcpStopTask,
    AcpPermissionReply,
    AcpStop,
    AcpSetPermissionMode,
    AcpSetConfig,
    AcpSwitchAccount,
    SetRemoteAccess,
    SetDeviceAccess,
    RevokeDevice,
    SetNotifications,
    SetForeground,
    SignRegistration,
    SetOwner,
    AnswerPairing,
    AllowDevices,
    PublishBackdrop,
    PublishOnScreen,
    FocusAgent,
    StartChat,
    ResumeChat,
}

/// A request the core answers with a [`Response`], and what that answer holds.
/// `Attach` and `TerminalLines` are answered with frames of their own instead.
pub trait Ask: Into<Request> {
    type Answer;

    fn answer(response: Response) -> Option<Self::Answer>;

    fn reply(answer: Self::Answer) -> Response;
}

macro_rules! answers {
    ($($request:ident -> $answer:ty { $value:tt => $response:ident { $($field:ident),* } })*) => {$(
        impl Ask for $request {
            type Answer = $answer;

            fn answer(response: Response) -> Option<$answer> {
                match response {
                    Response::$response { $($field),* } => Some($value),
                    _ => None,
                }
            }

            fn reply($value: $answer) -> Response {
                Response::$response { $($field),* }
            }
        }
    )*};
}

macro_rules! done {
    ($($request:ident),* $(,)?) => {
        answers! { $($request -> () { () => Done {} })* }
    };
}
done! {
    Resize,
    Kill,
    Detach,
    TerminalInput,
    TerminalClear,
    ResetModes,
    StopAll,
    RegisterWindow,
    PluginsChanged,
    Shutdown,
    AcpDetach,
    AcpWake,
    AcpPrompt,
    AcpEdit,
    AcpCancel,
    AcpStopTask,
    AcpPermissionReply,
    AcpStop,
    AcpSetPermissionMode,
    AcpSwitchAccount,
    Unpair,
    SetNotifications,
    ClearNotifications,
    SetForeground,
    PublishBackdrop,
    PublishOnScreen,
    WatchView,
    FocusAgent,
}

answers! {
    Spawn -> SessionId { id => Spawned { id } }
    List -> Vec<SessionInfo> { sessions => Sessions { sessions } }
    TerminalText -> String { text => TerminalText { text } }
    TerminalFind -> FindResult { result => Found { result } }
    TerminalWaitFor -> Option<String> { line => TerminalLine { line } }
    ShellAtPrompt -> bool { at_prompt => ShellAtPrompt { at_prompt } }
    TaskOutput -> OutputPage { page => TaskOutput { page } }
    Configure -> ManifestReloadReport { report => Manifests { report } }
    ListManifests -> ManifestReloadReport { report => Manifests { report } }
    ReloadManifests -> ManifestReloadReport { report => Manifests { report } }
    ExplainAgentDetection -> Box<DetectionExplain> { explain => DetectionExplain { explain } }
    AcpStart -> ChatStart { start => ChatStarted { start } }
    AcpAttach -> ChatAttachment { attachment => ChatAttached { attachment } }
    AcpHistory -> Value { page => ChatHistory { page } }
    AcpList -> Vec<ChatInfo> { chats => Chats { chats } }
    AcpSteer -> String { outcome => Steered { outcome } }
    AcpSetConfig -> Value { value => ChatConfig { value } }
    AttachFile -> PathBuf { path => Attached { path } }
    Call -> Value { result => Called { result } }
    RemoteStatus -> Box<super::RemoteStatus> { status => Remote { status } }
    SetOwner -> Box<super::RemoteStatus> { status => Remote { status } }
    SetRemoteAccess -> Box<super::RemoteStatus> { status => Remote { status } }
    SetDeviceAccess -> Box<super::RemoteStatus> { status => Remote { status } }
    RevokeDevice -> Box<super::RemoteStatus> { status => Remote { status } }
    AnswerPairing -> Box<super::RemoteStatus> { status => Remote { status } }
    AllowDevices -> Box<super::RemoteStatus> { status => Remote { status } }
    SignRegistration -> HostRegistration { registration => Registration { registration } }
    BackdropImage -> Option<String> { data_url => BackdropImage { data_url } }
    Workspace -> super::Workspace { workspace => Workspace { workspace } }
    Attentions -> Vec<Attention> { attentions => Attentions { attentions } }
    Host -> HostInfo { host => Host { host } }
    StartChat -> (String, ChatStart) { (agent_id, start) => ChatBegun { agent_id, start } }
    ResumeChat -> (String, ChatStart) { (agent_id, start) => ChatBegun { agent_id, start } }
}
