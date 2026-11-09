use sikemux_core::server::call;
use sikemux_core::server::chat::calls::{ChatPageParams, ToolDetailParams};
use sikemux_core::server::chat::find::FindAnswer;
use sikemux_core::server::chat::find::FindParams;
use sikemux_core::server::launching::StartParams;
use sikemux_core::server::streams::StreamParams;
use sikemux_core::server::workspace::{AgentPatch, UpdateAgentParams};
use sikemux_view::changed::Topic;
use sikemux_view::chat::{ChatPage, ToolDetail};
use sikemux_view::device::HostInfo;
use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use sikemux_view::layout::PaneKind;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use sikemux_client::accounts::network;
use sikemux_client::client::{Attached, ClientError, ClientEvent, CoreClient};
use sikemux_client::remote::SecretKey;
use sikemux_core::server::call::Call;
use sikemux_grid::{decode_frame, decode_lines, Frame, Lines};
use sikemux_host::cli_paths;
use sikemux_host::core_start::{self, StartError};
use sikemux_host::login_item::{self, CoreLaunch};
use sikemux_linked::{Happened, Linked};
use sikemux_view::device::DeviceView;
use sikemux_wire::protocol::{
    ask, CallId, ChatAccount, ChatAttachment, ChatContext, ChatMark, DeviceAccess, ErrorKind,
    Event, LaunchIdentity, NetworkChange, NoticeGoal, PermissionMode, RemoteStatus, SessionId,
    SpawnTarget, StreamId, TerminalSpawn, WindowAnswer, WindowCall,
};
use sikemux_wire::pty::shell_protocol::{PtyShellMetadataEvent, ShellMetadataSnapshot};
use sikemux_wire::terminal::{FindOptions, FindResult, GridPoint, TerminalInput};
use tokio::sync::{mpsc, oneshot};

use crate::hosts::Place;
use crate::reads::Read;
use crate::retry::Backoff;

pub enum Update {
    /// The core's release, which the app ships with.
    Connected(String),
    /// The view of the core at the place.
    View(Place, Box<DeviceView>),
    /// A read's latest answer.
    Read(Place, Read, Value),
    /// The core turned down a read.
    Refused(Place, Read, String),
    /// Show this agent and come to the front, as the notch or a request held while no window
    /// was open asks.
    FocusAgent(String),
    /// The core turned down something the person did, saying why.
    Failed(String),
    Terminal(Place, SessionId, Screenful),
    Lost(String),
    /// The core said it stopped on purpose, as `sikemux stop` stops it, so the window starts it
    /// again only when the person asks with `Ask::StartCore`, or picks up one started elsewhere.
    Stopped,
    /// The core with this process id holds the socket but stopped answering, so the window
    /// offers to end it and start another when the person asks with `Ask::StartCore`.
    Stuck(u32),
    /// The core stopped everything it ran, as the person asked, so the app can quit.
    ShutDown,
    /// Where remote access stands, whenever it changes.
    Remote(Box<RemoteStatus>),
    /// A terminal's process ended, with its exit code when that could be read.
    TerminalExited(SessionId, Option<u32>),
    /// Something the core says the person should hear about an agent, such as it needing them.
    Notice {
        title: String,
        body: String,
        agent: String,
        goal: Option<NoticeGoal>,
    },
    /// A device or host joined the person's account.
    AccountDevice {
        key: String,
        name: String,
    },
    /// What came of a chat the window attached to, by its agent's id.
    Chat(String, ChatNews),
    /// Something the core asks of a window, such as an agent's browser step.
    WindowCall(CallId, WindowCall),
    /// The core at `place` said something under a topic moved; no paths means anything
    /// may have.
    Changed {
        place: Place,
        topic: Topic,
        project: Option<String>,
        paths: Option<Vec<String>>,
    },
    /// Whether this app reaches another host now, by the host's key.
    HostReach(String, bool),
    /// Another host's home folder, by the host's key, as its core says it once reached.
    HostHome(String, String),
    /// Why this app cannot reach the account's other hosts at all.
    HostsUnavailable(String),
    /// This host went offline, came back, or woke.
    Network(NetworkChange),
}

pub enum ChatNews {
    Attached(ChatAttachment),
    /// The core took the message sent, and what it answered the attach that followed it.
    Taken(Option<ChatAttachment>),
    /// The chat's changes numbered `seq`, read on the thread that heard them, or why they
    /// did not read.
    Changes {
        seq: u64,
        changes: Result<Vec<crate::chat::changes::Changed>, String>,
    },
    /// Earlier rows, or none when the core's answer did not read.
    Page(Option<ChatPage>),
    /// Later rows, with `newer_after`, or none when the answer did not read.
    Later(ChatPage),
    /// The rows around a place found.
    Around(ChatPage),
    /// The chat's latest rows.
    Latest(ChatPage),
    /// What `chatFind` answered the ask numbered `asked`.
    Found {
        asked: u64,
        found: FindAnswer,
    },
    /// What a tool call changed and printed.
    Detail {
        tool: String,
        detail: ToolDetail,
    },
    /// The core turned down what the person sent, which goes back to them with why.
    Refused {
        error: String,
        text: String,
    },
    /// The core took the reply to the permission request `request`, or failed to.
    Replied {
        request: String,
        error: Option<String>,
    },
    /// The core took the ask to stop the background task `task`, or failed to.
    TaskStopped {
        task: String,
        error: Option<String>,
    },
}

/// What an attached terminal sends, decoded on the core's thread.
pub enum Screenful {
    Frame(Frame),
    Lines(Lines),
    Copied(String),
    Found(FindResult),
    /// Where the shell is and how its last command went, as it stood on attaching.
    Shell(ShellMetadataSnapshot),
    ShellChanged(PtyShellMetadataEvent<SessionId>),
}

pub enum TerminalAsk {
    /// Starts its frames, the first a whole screen.
    Attach,
    Detach,
    Input(TerminalInput),
    /// The window drew this frame, so the core may send the next.
    Ack(u32),
    Resize(u16, u16),
    Lines {
        start: u64,
        count: u32,
    },
    /// Its text between two cells, or all of it, sent back as `Screenful::Copied`.
    Copy(Option<(GridPoint, GridPoint)>),
    /// Matches of `query` in the screen and history, the current one the
    /// first after `from`, or before it going `backwards`. Sent back as `Screenful::Found`.
    Find {
        query: String,
        options: FindOptions,
        from: Option<GridPoint>,
        backwards: bool,
        inclusive: bool,
    },
    Clear,
    /// Its text between two cells, sent back to `reply`.
    Text {
        range: Option<(GridPoint, GridPoint)>,
        reply: oneshot::Sender<String>,
    },
    /// Turns off the modes a program left on, such as mouse reporting, after
    /// it exits without doing so.
    ResetModes,
    /// Ends the program the terminal runs.
    Kill,
}

/// What a read is asked, and which folder's changes move it.
#[derive(Clone)]
pub struct Watch {
    params: Value,
    /// The folder whose git and code host changes move what it reads; without one, those
    /// topics changing for any folder do.
    folder: Option<String>,
    /// Counts the watches made of one read, so an answer to one it replaced is dropped.
    generation: u64,
}

impl Watch {
    fn new(params: Value, folder: Option<String>) -> Self {
        Self {
            params,
            folder,
            generation: 0,
        }
    }

    /// Whether `topic` changing for the project folder `project`, or for every project when
    /// none, could move what `read` reads. Git and code host changes name the folder that
    /// moved, so a watch on another project's folder lets them pass.
    fn moved_by(&self, read: Read, topic: Topic, project: Option<&str>) -> bool {
        if !read.topics().contains(&topic) {
            return false;
        }
        let folder = self
            .folder
            .as_deref()
            .filter(|_| matches!(topic, Topic::Git | Topic::CodeHost));
        match (folder, project) {
            (Some(folder), Some(project)) => {
                Path::new(folder).starts_with(project) || Path::new(project).starts_with(folder)
            }
            _ => true,
        }
    }
}

/// What the core is asked about how it detects agents.
pub enum Detection {
    List,
    Reload,
    Explain(String),
}

/// What the client asks of the core beyond the view it is sent.
pub enum Ask {
    /// Keeps `read` answered as `watch` asks, in place of whatever it watched before.
    Watch(Read, Watch),
    /// Stops answering `read`, dropping an answer still on its way.
    Unwatch(Read),
    /// One of the core's methods, for something the person did. What it changes
    /// comes back with the next view.
    Call(Method),
    /// Asks `read` once, its answer arriving as any other read's.
    Answer {
        read: Read,
        params: Value,
    },
    /// A call whose answer goes back to the one asking, for answers that belong to
    /// what asked, such as one file's diff.
    Request(Method, oneshot::Sender<Result<Value, CallError>>),
    /// A watch, stream, call or answer for another host's core, by the host's key, as this
    /// host's would be.
    On(String, Box<Ask>),
    /// A call whose answer arrives in parts, as `Stream` events under `stream`.
    Stream {
        stream: u64,
        method: Method,
        parts: mpsc::UnboundedSender<Part>,
    },
    Terminal(SessionId, TerminalAsk),
    Chat(ChatAsk),
    /// The core's agent detection manifests, read again or not, or why it reads one agent's
    /// terminal as it does.
    Detection {
        detection: Detection,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Makes this window the one the core hands its calls to, as when it comes to the front.
    RegisterWindow,
    /// The agents the window shows, and whether the person is looking at it, so the core
    /// stops marking them unread.
    OnScreen {
        agent_ids: Vec<String>,
        focused: bool,
    },
    AnswerWindow(CallId, WindowAnswer),
    /// Every tab a waiting `sikemux open` opened has closed.
    WindowOpenClosed(CallId),
    StartChat(NewChat),
    /// Stops every agent and terminal the core runs.
    Shutdown,
    /// Starts the core again after it stopped on purpose.
    StartCore,
    Remote(RemoteAsk),
    /// Carries a chat on to another of the person's accounts; a turn at work goes on there.
    MoveChat {
        agent: String,
        account: ChatAccount,
    },
    /// Starts a terminal running `spawn` outside any tab, answering its id.
    Spawn {
        spawn: Box<TerminalSpawn>,
        started: oneshot::Sender<Result<SessionId, String>>,
    },
}

/// Starts a chat with `launcher`'s agent in `project` and brings its tab to the front,
/// answering the new agent's id to `started` when it is there.
pub struct NewChat {
    launcher: String,
    project: String,
    /// The permission mode it starts in, for this launch only; none starts in the launcher's.
    mode: Option<PermissionMode>,
    model: Option<String>,
    effort: Option<String>,
    started: Option<oneshot::Sender<String>>,
}

/// What the window asks of remote access, each answered with where it stands now.
pub enum RemoteAsk {
    Status,
    Enable(bool),
    Access {
        device: String,
        access: DeviceAccess,
    },
    Revoke(String),
    /// Answers a phone waiting to connect.
    Answer {
        id: String,
        allow: bool,
        access: DeviceAccess,
    },
    /// Lets the account's phones connect before they ask.
    Allow(Vec<sikemux_wire::protocol::AllowedDevice>),
}

pub enum ChatAsk {
    /// Starts hearing a chat's changes, from `since` when the window held it before.
    Attach {
        agent: String,
        since: Option<ChatMark>,
    },
    /// Starts the session of a chat that dropped, and attaches to it again.
    Restart {
        agent: String,
    },
    /// Moves a chat that has not started to another agent, account or model: its session
    /// stops, the core changes the agent by `patch`, and a session starts on what it is now.
    Move {
        agent: String,
        patch: AgentPatch,
    },
    Detach {
        agent: String,
    },
    History {
        agent: String,
        feed: String,
        before: u64,
        turns: u32,
    },
    /// The rows opened after event `after`, from the core's `chatPage`.
    Later {
        agent: String,
        feed: String,
        after: u64,
    },
    /// The chat's latest rows, from the core's `chatPage`, for a chat that can't be attached to,
    /// such as one asleep.
    Latest {
        agent: String,
        feed: String,
    },
    /// The rows around event `around`, from the core's `chatPage`, to show a place found.
    Around {
        agent: String,
        feed: String,
        around: u64,
    },
    /// Looks through the whole chat with the core's `chatFind`, whose `params` the find bar
    /// makes; `asked` is the bar's count of what it asked, so a late answer is known.
    Find {
        agent: String,
        params: FindParams,
        asked: u64,
    },
    /// Words for the agent. The core starts a stopped chat on its own session to take them,
    /// and `attach` then attaches the window that was not attached yet.
    Prompt {
        agent: String,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        attach: bool,
    },
    /// A file the person handed the chat that lives nowhere the agent can read, such as a
    /// pasted picture, for the core to keep; answers where it keeps it.
    Keep {
        agent: String,
        name: String,
        mime: String,
        bytes: Vec<u8>,
        kept: oneshot::Sender<Result<PathBuf, String>>,
    },
    Cancel {
        agent: String,
    },
    /// A sent message written again: the chat rewinds to it and sends `text` in its place.
    Edit {
        agent: String,
        message: String,
        text: String,
        paths: Vec<String>,
    },
    /// The person's answer to a tool's request for permission; none turns it down.
    Permission {
        agent: String,
        request: String,
        option: Option<String>,
    },
    Mode {
        agent: String,
        mode: PermissionMode,
    },
    /// One of the session's settings, such as its model, set to one of its choices.
    Config {
        agent: String,
        config: String,
        value: String,
    },
    StopTask {
        agent: String,
        task: String,
    },
    /// What a tool call changed and printed, which its row opens on.
    Detail {
        agent: String,
        tool: String,
    },
}

impl ChatAsk {
    /// The agent whose chat this is about.
    pub fn agent(&self) -> &str {
        match self {
            ChatAsk::Attach { agent, .. }
            | ChatAsk::Restart { agent }
            | ChatAsk::Move { agent, .. }
            | ChatAsk::Detach { agent }
            | ChatAsk::History { agent, .. }
            | ChatAsk::Later { agent, .. }
            | ChatAsk::Latest { agent, .. }
            | ChatAsk::Around { agent, .. }
            | ChatAsk::Find { agent, .. }
            | ChatAsk::Prompt { agent, .. }
            | ChatAsk::Keep { agent, .. }
            | ChatAsk::Cancel { agent }
            | ChatAsk::Edit { agent, .. }
            | ChatAsk::Permission { agent, .. }
            | ChatAsk::Mode { agent, .. }
            | ChatAsk::Config { agent, .. }
            | ChatAsk::StopTask { agent, .. }
            | ChatAsk::Detail { agent, .. } => agent,
        }
    }
}

pub struct Link {
    pub updates: mpsc::UnboundedReceiver<Update>,
    pub asks: mpsc::UnboundedSender<Ask>,
}

/// The app's identifier, with `.dev` for dev builds, the Mac app's.
pub use login_item::IDENTIFIER;

/// Where this build's core listens, and how it is started when none does.
pub struct Home {
    pub socket: PathBuf,
    launch: Option<CoreLaunch>,
    manifest_dir: Option<PathBuf>,
}

impl Default for Home {
    fn default() -> Self {
        let folders = Folders::of_this_system();
        let socket = sikemux_core::default_socket_path().unwrap_or_default();
        let launch = cli_paths::cli_executable_path().map(|binary| CoreLaunch {
            binary,
            socket: socket.clone(),
            log: folders
                .as_ref()
                .map_or_else(std::env::temp_dir, |folders| folders.logs.clone())
                .join("core.log"),
            args: login_item::core_args(folders.as_ref().map(|folders| folders.data.clone())),
        });
        Self {
            socket,
            launch,
            manifest_dir: folders.map(|folders| folders.config.join("agent-detection")),
        }
    }
}

impl Home {
    /// A core someone else started and looks after, at `socket`: the window never starts or
    /// replaces it, nor changes whether this build's core starts at login.
    pub fn at(socket: PathBuf) -> Self {
        Self {
            socket,
            launch: None,
            manifest_dir: None,
        }
    }
}

/// Where the app keeps its data, its settings and its logs: the folders the Tauri app used on
/// each system, so an install's data carries over.
struct Folders {
    data: PathBuf,
    config: PathBuf,
    logs: PathBuf,
}

impl Folders {
    #[cfg(target_os = "macos")]
    fn of_this_system() -> Option<Self> {
        let library = sikemux_process::paths::home()?.join("Library");
        let support = library.join("Application Support").join(IDENTIFIER);
        Some(Self {
            data: support.clone(),
            config: support,
            logs: library.join("Logs").join(IDENTIFIER),
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn of_this_system() -> Option<Self> {
        let home = sikemux_process::paths::home()?;
        let base = |variable: &str, fallback: &str| {
            sikemux_process::paths::env_path(variable)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        let data = base("XDG_DATA_HOME", ".local/share").join(IDENTIFIER);
        Some(Self {
            logs: data.join("logs"),
            config: base("XDG_CONFIG_HOME", ".config").join(IDENTIFIER),
            data,
        })
    }
}

/// A new chat asked of this window's own core, until the core says it has begun.
struct Beginning {
    launcher: String,
    begun: oneshot::Sender<String>,
}

/// Starts a chat on `client`'s core and brings its tab to the front there:
/// as soon as `begun` names the new chat, rather than once its agent's program is ready, which
/// takes a real agent seconds.
async fn open_chat_on(
    client: Arc<CoreClient>,
    chat: NewChat,
    begun: Option<oneshot::Receiver<String>>,
) {
    let NewChat {
        launcher,
        project,
        mode,
        model,
        effort,
        started,
    } = chat;
    let asked = client.ask(ask::StartChat {
        launcher,
        project: project.clone(),
        permission_mode: mode,
        model,
        effort,
    });
    let (begun, shown) = {
        let early = async {
            let agent = begun?.await.ok()?;
            open_tab(&client, &project, &agent).await;
            Some(agent)
        };
        tokio::pin!(asked, early);
        tokio::select! {
            answer = &mut asked => (answer, None),
            shown = &mut early => (asked.await, shown),
        }
    };
    let opened = begun;
    match opened {
        Ok((agent, _)) => {
            if shown.as_ref() != Some(&agent) {
                open_tab(&client, &project, &agent).await;
            }
            if let Some(started) = started {
                let _ = started.send(agent);
            }
        }
        Err(error) => eprintln!("sikemux: the core would not open the chat: {error}"),
    }
}

async fn open_tab(client: &CoreClient, project: &str, agent: &str) {
    let _ = called::<call::OpenTab>(
        client,
        OpenTab::new(At::project(project), PaneKind::Agent, agent),
    )
    .await;
}

/// A call's failure, with what a client branches on. Words for it are the caller's.
#[derive(Clone, Debug)]
pub struct CallError {
    pub kind: Option<ErrorKind>,
    pub message: String,
    pub detail: Option<Value>,
    /// The core could not be asked at all.
    pub gone: bool,
}

impl CallError {
    fn gone() -> Self {
        Self {
            kind: None,
            message: "the core is not connected".into(),
            detail: None,
            gone: true,
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<CallError> for String {
    fn from(error: CallError) -> Self {
        error.message
    }
}

impl From<String> for CallError {
    fn from(message: String) -> Self {
        Self {
            kind: None,
            message,
            detail: None,
            gone: false,
        }
    }
}

impl From<&str> for CallError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<ClientError> for CallError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Refused {
                kind,
                message,
                detail,
            } => Self {
                kind: Some(kind),
                message,
                detail,
                gone: false,
            },
            other => other.to_string().into(),
        }
    }
}

/// Where this app's key on the network is kept.
const DEVICE_KEY: &str = "sikemux-device";

/// This app's name: what the account calls it, before " on <host>", and the desktop on its
/// notifications.
pub const NAME: &str = if cfg!(debug_assertions) {
    "Sikemux Dev"
} else {
    "Sikemux"
};

fn read_device_view(view: &str) -> Option<Box<DeviceView>> {
    serde_json::from_str(view)
        .map_err(|error| eprintln!("sikemux: the core's device view is unreadable: {error}"))
        .ok()
}

/// This app's key on the network, made the first time it is needed and kept where
/// `sikemux_keychain` keeps secrets; a build that keeps none cannot have one.
fn device_key() -> Result<SecretKey, String> {
    let account = if cfg!(debug_assertions) {
        "dev"
    } else {
        "default"
    };
    let unavailable = |error: sikemux_keychain::KeychainError| error.to_string();
    let saved = sikemux_keychain::read(DEVICE_KEY, account).map_err(unavailable)?;
    if let Some(key) = saved.as_deref().and_then(sikemux_linked::key_from_text) {
        return Ok(key);
    }
    let key = sikemux_linked::new_key();
    sikemux_keychain::write(DEVICE_KEY, account, &sikemux_linked::key_text(&key))
        .map_err(unavailable)?;
    Ok(key)
}

/// Goes on the network as this app's own device, once the core's host is on an account.
async fn reach_hosts(
    client: &CoreClient,
    updates: &mpsc::UnboundedSender<Update>,
) -> Option<(Arc<Linked>, mpsc::UnboundedReceiver<Happened>)> {
    let owned = client
        .remote_status()
        .await
        .is_ok_and(|status| status.owner.is_some());
    if !owned {
        return None;
    }
    let unavailable = |reason: String| {
        let _ = updates.send(Update::HostsUnavailable(reason));
    };
    let key = match tokio::task::spawn_blocking(device_key).await {
        Ok(Ok(key)) => key,
        Ok(Err(reason)) => {
            unavailable(reason);
            return None;
        }
        Err(error) => {
            unavailable(error.to_string());
            return None;
        }
    };
    let relays = network::fetch(&sikemux_core::accounts::api_base())
        .await
        .and_then(|network| network::usable_relays(&network))
        .unwrap_or_default();
    match Linked::bind(key, relays, NAME, std::env::consts::OS).await {
        Ok(linked) => Some(linked),
        Err(error) => {
            unavailable(error.to_string());
            None
        }
    }
}

/// Joins every host the core is linked with, and lets go of the rest.
fn follow_links(linked: &Option<Arc<Linked>>, client: &Arc<CoreClient>) {
    let Some(linked) = linked.clone() else {
        return;
    };
    let client = client.clone();
    tokio::spawn(async move {
        if let Err(error) = linked.follow(client).await {
            eprintln!("sikemux: the account's other hosts could not be followed: {error}");
        }
    });
}

/// What the window keeps each core answering.
#[derive(Default)]
struct Watches {
    here: HashMap<Read, Watch>,
    hosts: HashMap<String, HashMap<Read, Watch>>,
    generations: WatchGenerations,
}

impl Watches {
    /// Watches `read` on `host`, or here with none, in place of what it was watching there.
    fn set(&mut self, host: Option<&str>, read: Read, mut watch: Watch) -> &Watch {
        watch.generation = self.next_generation(host, read);
        match host {
            Some(host) => self.hosts.entry(host.to_owned()).or_default(),
            None => &mut self.here,
        }
        .entry(read)
        .insert_entry(watch)
        .into_mut()
    }

    /// Stops watching `read` on `host`, or here with none, dropping an answer still on its way.
    fn remove(&mut self, host: Option<&str>, read: Read) {
        self.next_generation(host, read);
        match host {
            Some(host) => {
                if let Some(table) = self.hosts.get_mut(host) {
                    table.remove(&read);
                }
            }
            None => {
                self.here.remove(&read);
            }
        }
    }

    fn next_generation(&self, host: Option<&str>, read: Read) -> u64 {
        let mut current = self.generations.lock().expect("watch generations");
        let key = (host.map(str::to_owned), read);
        let next = current.get(&key).map_or(0, |generation| generation + 1);
        current.insert(key, next);
        next
    }
}

/// One of another host's core methods, waiting a while for a host that is reconnecting.
async fn call_linked(
    linked: &Option<Arc<Linked>>,
    host: &str,
    method: &str,
    params: Value,
) -> Result<Value, CallError> {
    let linked = linked
        .as_ref()
        .ok_or("this app does not reach other hosts")?;
    Ok(linked.client(host).await?.call(method, params).await?)
}

/// Answers `key` from another host's core, as `read` does from this host's.
fn read_on(
    linked: &Option<Arc<Linked>>,
    host: &str,
    key: Read,
    watch: &Watch,
    current: &WatchGenerations,
    updates: &mpsc::UnboundedSender<Update>,
) {
    let (linked, host, watch, current, updates) = (
        linked.clone(),
        host.to_owned(),
        watch.clone(),
        current.clone(),
        updates.clone(),
    );
    tokio::spawn(async move {
        let answer = call_linked(&linked, &host, key.method(), watch.params).await;
        send_read(
            Some(host),
            key,
            watch.generation,
            answer.map_err(String::from),
            &current,
            &updates,
        );
    });
}

async fn heard_from(heard: &mut Option<mpsc::UnboundedReceiver<Happened>>) -> Option<Happened> {
    match heard {
        Some(heard) => heard.recv().await,
        None => std::future::pending().await,
    }
}

/// One of the core's methods and its params, which only a typed call makes, so no name is
/// spelled outside the core's table.
pub struct Method {
    name: &'static str,
    params: Value,
}

impl Method {
    fn of<C: Call>(params: C::Params) -> Self
    where
        C::Params: Serialize,
    {
        let params = serde_json::to_value(params).unwrap_or_else(|error| {
            eprintln!(
                "sikemux: the params of {} did not write: {error}",
                C::METHOD
            );
            Value::Null
        });
        Self {
            name: C::METHOD,
            params,
        }
    }
}

/// The call `C`, which nothing waits on the answer of.
pub fn typed_call<C: Call>(params: C::Params) -> Ask
where
    C::Params: Serialize,
{
    Ask::Call(Method::of::<C>(params))
}

/// One batch of a streamed answer; the last has `done`, and `error` when it failed.
pub struct Part {
    pub items: Vec<Value>,
    pub done: bool,
    pub error: Option<Value>,
}

/// A streamed answer's parts as they arrive. Dropping it stops the stream in the core.
pub struct Streaming {
    pub parts: mpsc::UnboundedReceiver<Part>,
    stream: u64,
    target: Target,
}

impl Drop for Streaming {
    fn drop(&mut self) {
        self.target.call_typed::<call::StreamCancel>(StreamParams {
            stream: Some(self.stream),
        });
    }
}

/// Where a pane's calls go: this host's core, or the core of the account host its project
/// is on. Panes ask through it and never name a host themselves.
#[derive(Clone)]
pub struct Target {
    asks: mpsc::UnboundedSender<Ask>,
    place: Place,
}

impl Target {
    pub fn new(asks: &mpsc::UnboundedSender<Ask>, place: Place) -> Self {
        Self {
            asks: asks.clone(),
            place,
        }
    }

    /// This host's core.
    pub fn here(asks: &mpsc::UnboundedSender<Ask>) -> Self {
        Self::new(asks, Place::This)
    }

    pub fn place(&self) -> &Place {
        &self.place
    }

    /// The way to this host's own core, for what stays here whatever the place.
    pub fn asks(&self) -> &mpsc::UnboundedSender<Ask> {
        &self.asks
    }

    /// Sends `ask` to the place's core.
    pub fn ask(&self, ask: Ask) {
        let _ = self.asks.send(match &self.place {
            Place::This => ask,
            Place::Account(host) => Ask::On(host.clone(), Box::new(ask)),
        });
    }

    /// Asks the place's core for the call `C`, and reads its answer as `C`'s answer.
    pub fn request_typed<C: Call>(
        &self,
        params: C::Params,
    ) -> impl Future<Output = Result<C::Answer, String>> + use<C>
    where
        C::Params: Serialize,
        C::Answer: DeserializeOwned,
    {
        let called = self.try_request_typed::<C>(params);
        async move { Ok(called.await?) }
    }

    /// As `request_typed`, for a caller that branches on how it fails.
    pub fn try_request_typed<C: Call>(
        &self,
        params: C::Params,
    ) -> impl Future<Output = Result<C::Answer, CallError>> + use<C>
    where
        C::Params: Serialize,
        C::Answer: DeserializeOwned,
    {
        let (reply, answer) = oneshot::channel();
        self.ask(Ask::Request(Method::of::<C>(params), reply));
        async move {
            let answer = answer.await.map_err(|_| CallError::gone())??;
            serde_json::from_value(answer).map_err(|error| {
                let said = format!("the core's answer to {} did not read: {error}", C::METHOD);
                eprintln!("sikemux: {said}");
                said.into()
            })
        }
    }

    /// Keeps the place's core answering `read`, whose call is `C`, in place of what it was
    /// watching there.
    pub fn watch<C: Call>(&self, read: Read, params: C::Params)
    where
        C::Params: Serialize,
    {
        self.watch_with::<C>(read, params, None);
    }

    /// Keeps the core answering `read`, whose git and code host changes are `folder`'s.
    pub fn watch_folder<C: Call>(&self, read: Read, params: C::Params, folder: String)
    where
        C::Params: Serialize,
    {
        self.watch_with::<C>(read, params, Some(folder));
    }

    fn watch_with<C: Call>(&self, read: Read, params: C::Params, folder: Option<String>)
    where
        C::Params: Serialize,
    {
        debug_assert_eq!(read.method(), C::METHOD);
        let params = Method::of::<C>(params).params;
        self.ask(Ask::Watch(read, Watch::new(params, folder)));
    }

    /// Keeps the core answering `read`, which takes nothing.
    pub fn watch_plain(&self, read: Read) {
        self.ask(Ask::Watch(read, Watch::new(json!({}), None)));
    }

    /// Stops the place's core answering `read`.
    pub fn unwatch(&self, read: Read) {
        self.ask(Ask::Unwatch(read));
    }

    /// Sends the call `C` without waiting for its answer.
    pub fn call_typed<C: Call>(&self, params: C::Params)
    where
        C::Params: Serialize,
    {
        self.ask(typed_call::<C>(params));
    }

    /// Starts a chat with `launcher`'s agent in `project` there, in `mode` for this launch or
    /// else the launcher's, answering the new agent's id to `started` when it is there.
    pub fn start_chat(
        &self,
        launcher: String,
        project: String,
        mode: Option<PermissionMode>,
        started: Option<oneshot::Sender<String>>,
    ) {
        self.start_chat_as(launcher, project, mode, (None, None), started);
    }

    /// Starts a chat as `start_chat` does, on the model and effort of `(model, effort)` where
    /// they are named.
    pub fn start_chat_as(
        &self,
        launcher: String,
        project: String,
        mode: Option<PermissionMode>,
        (model, effort): (Option<String>, Option<String>),
        started: Option<oneshot::Sender<String>>,
    ) {
        self.ask(Ask::StartChat(NewChat {
            launcher,
            project,
            mode,
            model,
            effort,
            started,
        }));
    }

    /// Asks `read`, whose call is `C`, once, the answer arriving as any other read's.
    pub fn answer<C: Call>(&self, read: Read, params: C::Params)
    where
        C::Params: Serialize,
    {
        debug_assert_eq!(read.method(), C::METHOD);
        let params = Method::of::<C>(params).params;
        self.ask(Ask::Answer { read, params });
    }

    /// Asks the call `C`, whose answer streams, with `params` made from the stream's number.
    pub fn stream<C: Call>(&self, params: impl FnOnce(u64) -> C::Params) -> Streaming
    where
        C::Params: Serialize,
    {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let stream = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (sender, parts) = mpsc::unbounded_channel();
        self.ask(Ask::Stream {
            stream,
            method: Method::of::<C>(params(stream)),
            parts: sender,
        });
        Streaming {
            parts,
            stream,
            target: self.clone(),
        }
    }
}

/// For a harness run beside a core of its own, at `SIKEMUX_CORE_SOCKET` with a HOME of its own:
/// the core it starts looks for agent CLIs only on its PATH, never finding the person's own.
/// Call it first in `main`, before any thread starts.
pub fn search_path_only_for_own_core() {
    if std::env::var_os(sikemux_core::SOCKET_ENV).is_some() {
        std::env::set_var(sikemux_process::vars::AGENT_SEARCH, "path");
    }
}

/// Talks to the core on a thread of its own, so the window's thread only draws.
pub fn start(home: Home) -> Link {
    // Downloads made in this process, such as dictation's model, need TLS to have a
    // crypto provider chosen before the first one, as the host chooses its own.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (updates_tx, updates) = mpsc::unbounded_channel();
    let (asks, asks_rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("core".into())
        .spawn(move || {
            crate::thread_priority::interactive();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .on_thread_start(crate::thread_priority::interactive)
                .build()
                .expect("tokio runtime");
            runtime.block_on(run(home, updates_tx, asks_rx));
        })
        .expect("core thread");
    Link { updates, asks }
}

/// Where the person's choice to force an update waits for the app it restarts into, which then
/// moves the core to the new build even while chats there cannot be carried over.
fn forced_update_marker() -> Option<PathBuf> {
    Folders::of_this_system().map(|folders| folders.logs.join("core-update-forced"))
}

/// Has the app the update restarts into stop what holds the core on its old build.
pub fn force_next_core_update() {
    let Some(marker) = forced_update_marker() else {
        return;
    };
    let written = marker
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&marker, b""));
    if let Err(error) = written {
        eprintln!("sikemux: the core will wait for its chats before it updates: {error}");
    }
}

/// Keeps the core starting at login while remote access is on, so phones reach this host
/// after a restart, and stops it starting then once remote access is off.
fn start_at_login(home: &Home, remote_on: Option<bool>) {
    let (Some(enabled), Some(launch)) = (remote_on, home.launch.clone()) else {
        return;
    };
    tokio::task::spawn_blocking(move || login_item::sync(IDENTIFIER, Some(&launch), enabled));
}

/// How long Quit and Stop Everything waits for the core to stop what it runs and answer, before
/// it counts the core as stuck and ends it by signal.
const SHUT_DOWN_ANSWER: Duration = Duration::from_secs(20);

/// A connection that held this long was a working one, so the next reconnect is quick again.
const HELD: Duration = Duration::from_secs(10);

async fn run(
    home: Home,
    updates: mpsc::UnboundedSender<Update>,
    mut asks: mpsc::UnboundedReceiver<Ask>,
) {
    let mut watches = Watches::default();
    let mut linked: Option<Arc<Linked>> = None;
    let mut heard: Option<mpsc::UnboundedReceiver<Happened>> = None;
    let current = watches.generations.clone();
    let failed = FailedWatches::default();
    let mut reconnect = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
    loop {
        if let Some(launch) = &home.launch {
            let force =
                forced_update_marker().is_some_and(|marker| std::fs::remove_file(marker).is_ok());
            match core_start::start(IDENTIFIER, launch, &sikemux_host::build_identity(), force)
                .await
            {
                Ok(()) | Err(StartError::Unreachable(_)) => {}
                Err(StartError::Incompatible { reason, .. }) => {
                    eprintln!("sikemux: the core cannot be used: {reason}");
                    let _ = updates.send(Update::Lost(reason));
                    tokio::time::sleep(reconnect.next().max(Duration::from_secs(5))).await;
                    continue;
                }
            }
        }
        match CoreClient::connect(&home.socket).await {
            Ok((client, mut events)) => {
                let connected = Instant::now();
                let mut stopping = false;
                let client = Arc::new(client);
                let _ = updates.send(Update::Connected(client.hello().build.version.clone()));
                if let Err(error) = client
                    .call(
                        "configureAgentManifests",
                        json!({ "manifestDir": home.manifest_dir }),
                    )
                    .await
                {
                    eprintln!("sikemux: the core could not read the agent manifests: {error}");
                }
                if let Err(error) = client.ask(ask::WatchView).await {
                    let _ = updates.send(Update::Lost(error.to_string()));
                }
                if let Err(error) = client.ask(ask::RegisterWindow).await {
                    eprintln!("sikemux: the core would not hand this window its calls: {error}");
                }
                let mut remote_on = client
                    .remote_status()
                    .await
                    .ok()
                    .map(|status| status.enabled);
                start_at_login(&home, remote_on);
                for (&key, watch) in &watches.here {
                    read(&client, key, watch, (&current, &failed), &updates);
                }
                if linked.is_none() {
                    if let Some((reached, events)) = reach_hosts(&client, &updates).await {
                        linked = Some(reached);
                        heard = Some(events);
                    }
                }
                follow_links(&linked, &client);
                let (settle_tx, mut settled) = mpsc::unbounded_channel();
                let mut streams = Streams::new(Place::This, &client, &updates, &settle_tx);
                let mut host_streams: HashMap<String, Streams> = HashMap::new();
                let mut waiting_terminals: HashMap<String, Vec<(SessionId, TerminalAsk)>> =
                    HashMap::new();
                let (reached_tx, mut reached) =
                    mpsc::unbounded_channel::<(String, Arc<CoreClient>)>();
                let mut streamed: HashMap<u64, mpsc::UnboundedSender<Part>> = HashMap::new();
                let mut beginnings: Vec<Beginning> = Vec::new();
                loop {
                    tokio::select! {
                        settled = settled.recv() => match settled {
                            Some((Place::This, settled)) => streams.settle(settled),
                            Some((Place::Account(host), settled)) => {
                                if let Some(streams) = host_streams.get_mut(&host) {
                                    streams.settle(settled);
                                }
                            }
                            None => {}
                        },
                        reached = reached.recv() => {
                            if let Some((host, client)) = reached {
                                let mut streams = Streams::new(Place::Account(host.clone()), &client, &updates, &settle_tx);
                                for (session, ask) in waiting_terminals.remove(&host).unwrap_or_default() {
                                    streams.ask(session, ask);
                                }
                                host_streams.insert(host, streams);
                            }
                        }
                        event = events.recv() => match event {
                            Some(ClientEvent::Grid { stream, frame }) => streams.frame(stream, frame),
                            Some(ClientEvent::DeviceView(view)) => {
                                if let Some(view) = read_device_view(&view) {
                                    let _ = updates.send(Update::View(Place::This, view));
                                }
                            }
                            Some(ClientEvent::Event(Event::Notice { title, body, agent_id, goal })) => {
                                let _ = updates.send(Update::Notice { title, body, agent: agent_id, goal });
                            }
                            Some(ClientEvent::Event(Event::FocusAgent { agent_id })) => {
                                let _ = updates.send(Update::FocusAgent(agent_id));
                            }
                            Some(ClientEvent::Event(Event::Exited { id, code, .. })) => {
                                let _ = updates.send(Update::TerminalExited(id, code));
                            }
                            Some(ClientEvent::Event(Event::Network { change })) => {
                                if change != NetworkChange::Offline {
                                    let again: Vec<Read> = failed.lock().expect("failed watches").drain().collect();
                                    for (&key, watch) in watches.here.iter().filter(|(key, _)| again.contains(key)) {
                                        read(&client, key, watch, (&current, &failed), &updates);
                                    }
                                }
                                let _ = updates.send(Update::Network(change));
                            }
                            Some(ClientEvent::Event(Event::AccountDevice { key, name, .. })) => {
                                let _ = updates.send(Update::AccountDevice { key, name });
                            }
                            Some(ClientEvent::Event(Event::Remote { status })) => {
                                follow_links(&linked, &client);
                                if remote_on != Some(status.enabled) {
                                    remote_on = Some(status.enabled);
                                    start_at_login(&home, remote_on);
                                }
                                let _ = updates.send(Update::Remote(Box::new(status)));
                            }
                            Some(ClientEvent::Event(Event::Chat { agent_id, seq, changes })) => {
                                let changes = crate::chat::changes::read(changes);
                                let _ = updates.send(Update::Chat(agent_id, ChatNews::Changes { seq, changes }));
                            }
                            Some(ClientEvent::Event(Event::ChatBegun { chat })) if chat.started_by.is_none() => {
                                beginnings.retain(|beginning| !beginning.begun.is_closed());
                                let launcher = chat.launcher.as_deref();
                                if let Some(at) = beginnings.iter().position(|beginning| launcher == Some(&beginning.launcher)) {
                                    let _ = beginnings.remove(at).begun.send(chat.agent_id);
                                }
                            }
                            Some(ClientEvent::Event(Event::Stream { stream, items, done, error })) => {
                                if let Some(parts) = streamed.get(&stream) {
                                    if parts.send(Part { items, done, error }).is_err() || done {
                                        streamed.remove(&stream);
                                    }
                                }
                            }
                            Some(ClientEvent::Event(Event::Changed { topic, project, paths })) => {
                                let topic = Topic::of_json(&topic);
                                stopping |= topic == Topic::Stopping;
                                for (&key, watch) in watches.here.iter().filter(|(&key, watch)| watch.moved_by(key, topic, project.as_deref())) {
                                    read(&client, key, watch, (&current, &failed), &updates);
                                }
                                let _ = updates.send(Update::Changed { place: Place::This, topic, project, paths });
                            }
                            Some(ClientEvent::WindowCall { call_id, call }) => {
                                let _ = updates.send(Update::WindowCall(call_id, call));
                            }
                            Some(ClientEvent::Event(Event::ShellMetadata(event))) => {
                                let _ = updates.send(Update::Terminal(Place::This, event.pty_id, Screenful::ShellChanged(event)));
                            }
                            Some(_) => {}
                            None => break,
                        },
                        happened = heard_from(&mut heard) => match happened {
                            Some(Happened::Connected(host)) => {
                                for (&key, watch) in watches.hosts.get(&host).into_iter().flatten() {
                                    read_on(&linked, &host, key, watch, &current, &updates);
                                }
                                if let Some(linked) = linked.clone() {
                                    let (host, updates) = (host.clone(), updates.clone());
                                    tokio::spawn(async move {
                                        let Ok(client) = linked.client(&host).await else {
                                            return;
                                        };
                                        let info = client.ask(ask::Host).await.map(serde_json::from_value::<HostInfo>);
                                        if let Ok(Ok(info)) = info {
                                            let home = info.home.to_string_lossy().into_owned();
                                            let _ = updates.send(Update::HostHome(host, home));
                                        }
                                    });
                                }
                                let _ = updates.send(Update::HostReach(host, true));
                            }
                            Some(Happened::Lost(host)) => {
                                for &key in watches.hosts.get(&host).into_iter().flat_map(HashMap::keys) {
                                    let _ = updates.send(Update::Refused(Place::Account(host.clone()), key, format!("{host} is out of reach")));
                                }
                                host_streams.remove(&host);
                                waiting_terminals.remove(&host);
                                let _ = updates.send(Update::HostReach(host, false));
                            }
                            Some(Happened::Event(host, event)) => match *event {
                                ClientEvent::Grid { stream, frame } => {
                                    if let Some(streams) = host_streams.get_mut(&host) {
                                        streams.frame(stream, frame);
                                    }
                                }
                                ClientEvent::Event(Event::Chat { agent_id, seq, changes }) => {
                                    let changes = crate::chat::changes::read(changes);
                                    let _ = updates.send(Update::Chat(agent_id, ChatNews::Changes { seq, changes }));
                                }
                                ClientEvent::DeviceView(view) => {
                                    if let Some(view) = read_device_view(&view) {
                                        let _ = updates.send(Update::View(Place::Account(host), view));
                                    }
                                }
                                ClientEvent::Event(Event::ShellMetadata(event)) => {
                                    let _ = updates.send(Update::Terminal(Place::Account(host), event.pty_id, Screenful::ShellChanged(event)));
                                }
                                ClientEvent::Event(Event::Changed { topic, project, paths }) => {
                                    let topic = Topic::of_json(&topic);
                                    let changed = watches
                                        .hosts
                                        .get(&host)
                                        .into_iter()
                                        .flatten()
                                        .filter(|(&key, watch)| watch.moved_by(key, topic, project.as_deref()));
                                    for (&key, watch) in changed {
                                        read_on(&linked, &host, key, watch, &current, &updates);
                                    }
                                    let _ = updates.send(Update::Changed { place: Place::Account(host), topic, project, paths });
                                }
                                ClientEvent::Event(Event::Stream { stream, items, done, error }) => {
                                    if let Some(parts) = streamed.get(&stream) {
                                        if parts.send(Part { items, done, error }).is_err() || done {
                                            streamed.remove(&stream);
                                        }
                                    }
                                }
                                _ => {}
                            },
                            None => heard = None,
                        },
                        ask = asks.recv() => match ask {
                            Some(Ask::Watch(key, watch)) => {
                                let watch = watches.set(None, key, watch);
                                read(&client, key, watch, (&current, &failed), &updates);
                            }
                            Some(Ask::Unwatch(key)) => watches.remove(None, key),
                            Some(Ask::RegisterWindow) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    if let Err(error) = client.ask(ask::RegisterWindow).await {
                                        eprintln!("sikemux: the core would not hand this window its calls: {error}");
                                    }
                                });
                            }
                            Some(Ask::OnScreen { agent_ids, focused }) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    report("tell the core which agent is on screen", client.ask(ask::PublishOnScreen { agent_ids, focused }).await);
                                });
                            }
                            Some(Ask::AnswerWindow(call_id, answer)) => client.answer_window(call_id, answer),
                            Some(Ask::WindowOpenClosed(call_id)) => client.window_open_closed(call_id),
                            Some(Ask::Answer { read, params }) => {
                                let (client, updates) = (client.clone(), updates.clone());
                                tokio::spawn(async move {
                                    let _ = updates.send(match client.call(read.method(), params).await {
                                        Ok(value) => Update::Read(Place::This, read, value),
                                        Err(error) => Update::Refused(Place::This, read, error.to_string()),
                                    });
                                });
                            }
                            Some(Ask::On(host, ask)) => match *ask {
                                Ask::Watch(key, watch) => {
                                    let watch = watches.set(Some(&host), key, watch);
                                    read_on(&linked, &host, key, watch, &current, &updates);
                                }
                                Ask::Unwatch(key) => watches.remove(Some(&host), key),
                                Ask::Stream { stream, method, parts } => {
                                    streamed.insert(stream, parts.clone());
                                    let linked = linked.clone();
                                    tokio::spawn(async move {
                                        if let Err(error) = call_linked(&linked, &host, method.name, method.params).await {
                                            let error = json!({ "category": "call", "message": error.message });
                                            let _ = parts.send(Part { items: Vec::new(), done: true, error: Some(error) });
                                        }
                                    });
                                }
                                Ask::Call(method) => {
                                    let (linked, updates) = (linked.clone(), updates.clone());
                                    tokio::spawn(async move {
                                        if let Err(error) = call_linked(&linked, &host, method.name, method.params).await {
                                            let _ = updates.send(Update::Failed(error.into()));
                                        }
                                    });
                                }
                                Ask::Answer { read, params } => {
                                    let (linked, updates) = (linked.clone(), updates.clone());
                                    tokio::spawn(async move {
                                        let answer = call_linked(&linked, &host, read.method(), params).await;
                                        let place = Place::Account(host);
                                        let _ = updates.send(match answer {
                                            Ok(value) => Update::Read(place, read, value),
                                            Err(error) => Update::Refused(place, read, error.into()),
                                        });
                                    });
                                }
                                Ask::Request(method, reply) => {
                                    let linked = linked.clone();
                                    tokio::spawn(async move {
                                        let _ = reply.send(call_linked(&linked, &host, method.name, method.params).await);
                                    });
                                }
                                Ask::Terminal(session, ask) => match host_streams.get_mut(&host) {
                                    Some(streams) => streams.ask(session, ask),
                                    None => {
                                        let first = !waiting_terminals.contains_key(&host);
                                        waiting_terminals.entry(host.clone()).or_default().push((session, ask));
                                        if let (true, Some(linked)) = (first, linked.clone()) {
                                            let reached = reached_tx.clone();
                                            tokio::spawn(async move {
                                                match linked.client(&host).await {
                                                    Ok(client) => drop(reached.send((host, client))),
                                                    Err(error) => eprintln!("sikemux: {host}'s terminals are out of reach: {error}"),
                                                }
                                            });
                                        }
                                    }
                                },
                                Ask::StartChat(chat) => {
                                    let linked = linked.clone();
                                    tokio::spawn(async move {
                                        let client = match &linked {
                                            Some(linked) => linked.client(&host).await.ok(),
                                            None => None,
                                        };
                                        match client {
                                            Some(client) => open_chat_on(client, chat, None).await,
                                            None => eprintln!("sikemux: {host} is out of reach, so no chat started there"),
                                        }
                                    });
                                }
                                Ask::Chat(ask) => {
                                    let (linked, updates) = (linked.clone(), updates.clone());
                                    tokio::spawn(async move {
                                        let Some(client) = (match &linked {
                                            Some(linked) => linked.client(&host).await.ok(),
                                            None => None,
                                        }) else {
                                            return eprintln!("sikemux: {host}'s chats are out of reach");
                                        };
                                        let (told, mut heard) = mpsc::unbounded_channel();
                                        let forward = tokio::spawn(async move {
                                            while let Some(update) = heard.recv().await {
                                                let _ = updates.send(update);
                                            }
                                        });
                                        chat(&client, ask, &told).await;
                                        drop(told);
                                        let _ = forward.await;
                                    });
                                }
                                _ => eprintln!("sikemux: that is not asked of another host"),
                            },
                            Some(Ask::Stream { stream, method, parts }) => {
                                streamed.insert(stream, parts.clone());
                                let client = client.clone();
                                tokio::spawn(async move {
                                    if let Err(error) = client.call(method.name, method.params).await {
                                        let error = json!({ "category": "call", "message": error.to_string() });
                                        let _ = parts.send(Part { items: Vec::new(), done: true, error: Some(error) });
                                    }
                                });
                            }
                            Some(Ask::Request(method, reply)) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    let answer = client.call(method.name, method.params).await;
                                    let _ = reply.send(answer.map_err(CallError::from));
                                });
                            }
                            Some(Ask::Detection { detection, reply }) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    let (method, params) = match detection {
                                        Detection::List => ("agentManifests", json!({})),
                                        Detection::Reload => ("reloadAgentManifests", json!({})),
                                        Detection::Explain(agent) => ("explainAgentDetection", json!({ "agentId": agent })),
                                    };
                                    let answer = client.call(method, params).await;
                                    let _ = reply.send(answer.map_err(|error| error.to_string()));
                                });
                            }
                            Some(Ask::Chat(ask)) => {
                                let (client, updates) = (client.clone(), updates.clone());
                                tokio::spawn(async move { chat(&client, ask, &updates).await });
                            }
                            Some(Ask::Call(method)) => {
                                let (client, updates) = (client.clone(), updates.clone());
                                tokio::spawn(async move {
                                    if let Err(error) = client.call(method.name, method.params).await {
                                        let _ = updates.send(Update::Failed(error.to_string()));
                                    }
                                });
                            }
                            Some(Ask::Terminal(session, ask)) => streams.ask(session, ask),
                            Some(Ask::StartChat(chat)) => {
                                let (begun, heard) = oneshot::channel();
                                beginnings.push(Beginning { launcher: chat.launcher.clone(), begun });
                                tokio::spawn(open_chat_on(client.clone(), chat, Some(heard)));
                            }
                            Some(Ask::StartCore) => {}
                            Some(Ask::Shutdown) => {
                                let answered = tokio::time::timeout(SHUT_DOWN_ANSWER, client.ask(ask::Shutdown { stop_all: true })).await;
                                if !matches!(answered, Ok(Ok(()))) {
                                    let (socket, pid) = (home.socket.clone(), client.hello().pid);
                                    let _ = tokio::task::spawn_blocking(move || core_start::end(&socket, pid)).await;
                                }
                                let _ = updates.send(Update::ShutDown);
                            }
                            Some(Ask::MoveChat { agent, account }) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    if let Err(error) = client.ask(ask::AcpSwitchAccount { agent_id: agent, account }).await {
                                        eprintln!("sikemux: the chat stayed on its account: {error}");
                                    }
                                });
                            }
                            Some(Ask::Spawn { spawn, started }) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    let launch = LaunchIdentity {
                                        version: client.hello().build.version.clone(),
                                        cli_executable: cli_paths::cli_executable_path(),
                                        cli_endpoint: sikemux_core::cli::endpoint::default_endpoint_path(),
                                    };
                                    let spawned = client.spawn(launch, SpawnTarget::Terminal(*spawn)).await;
                                    let _ = started.send(spawned.map_err(|error| error.to_string()));
                                });
                            }
                            Some(Ask::Remote(ask)) => {
                                let (client, updates) = (client.clone(), updates.clone());
                                tokio::spawn(async move {
                                    let status = match ask {
                                        RemoteAsk::Status => client.remote_status().await,
                                        RemoteAsk::Enable(enabled) => client.set_remote_access(enabled).await,
                                        RemoteAsk::Access { device, access } => client.set_device_access(device, access).await,
                                        RemoteAsk::Revoke(device) => client.revoke_device(device).await,
                                        RemoteAsk::Answer { id, allow, access } => client.answer_pairing(id, allow, access).await,
                                        RemoteAsk::Allow(devices) => client.allow_devices(devices).await,
                                    };
                                    match status {
                                        Ok(status) => {
                                            let _ = updates.send(Update::Remote(Box::new(status)));
                                        }
                                        Err(error) => eprintln!("sikemux: the core refused a remote access change: {error}"),
                                    }
                                });
                            }
                            None => return,
                        },
                    }
                }
                if stopping {
                    let _ = updates.send(Update::Stopped);
                    if !wait_to_start(&home, &mut asks, &mut watches).await {
                        return;
                    }
                    reconnect.reset();
                    continue;
                }
                let _ = updates.send(Update::Lost("the core closed the connection".into()));
                if connected.elapsed() >= HELD {
                    reconnect.reset();
                }
            }
            Err(error) => {
                eprintln!("sikemux: no core answers: {error}");
                let socket = home.socket.clone();
                let stuck = tokio::task::spawn_blocking(move || core_start::stuck(&socket))
                    .await
                    .ok()
                    .flatten();
                let Some(pid) = stuck else {
                    let _ = updates.send(Update::Lost(error.to_string()));
                    tokio::time::sleep(reconnect.next()).await;
                    continue;
                };
                eprintln!("sikemux: the core, pid {pid}, holds the socket but does not answer");
                let _ = updates.send(Update::Stuck(pid));
                if !wait_to_start(&home, &mut asks, &mut watches).await {
                    return;
                }
                let socket = home.socket.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if core_start::stuck(&socket) == Some(pid) && !core_start::end(&socket, pid) {
                        eprintln!("sikemux: the core, pid {pid}, would not stop");
                    }
                })
                .await;
                reconnect.reset();
                continue;
            }
        }
        tokio::time::sleep(reconnect.next()).await;
    }
}

/// How often a window whose core stopped looks for one started elsewhere, such as by `sikemux start`.
const STOPPED_LOOK: Duration = Duration::from_secs(2);

/// Waits, after the core stopped on purpose, for the person to start it or for a core that
/// answers. What the window watches keeps up meanwhile; anything asked of the core is dropped,
/// which tells whoever waits for an answer that there is none. False once the window is gone.
async fn wait_to_start(
    home: &Home,
    asks: &mut mpsc::UnboundedReceiver<Ask>,
    watches: &mut Watches,
) -> bool {
    let mut look = tokio::time::interval(STOPPED_LOOK);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ask = asks.recv() => match ask {
                Some(Ask::StartCore) => return true,
                Some(Ask::Watch(key, watch)) => {
                    watches.set(None, key, watch);
                }
                Some(Ask::Unwatch(key)) => watches.remove(None, key),
                Some(Ask::On(host, ask)) => match *ask {
                    Ask::Watch(key, watch) => {
                        watches.set(Some(&host), key, watch);
                    }
                    Ask::Unwatch(key) => watches.remove(Some(&host), key),
                    _ => {}
                },
                Some(_) => {}
                None => return false,
            },
            _ = look.tick() => {
                if home.socket.exists() && CoreClient::connect(&home.socket).await.is_ok() {
                    return true;
                }
            }
        }
    }
}

async fn chat(client: &CoreClient, ask: ChatAsk, updates: &mpsc::UnboundedSender<Update>) {
    let (agent, news) = match ask {
        ChatAsk::Restart { agent } => match restarted(client, agent, false).await {
            Some(news) => news,
            None => return,
        },
        ChatAsk::Move { agent, patch } => {
            report(
                "stop",
                client
                    .ask(ask::AcpStop {
                        agent_id: agent.clone(),
                    })
                    .await,
            );
            let changed =
                called::<call::UpdateAgent>(client, UpdateAgentParams::new(agent.clone(), patch))
                    .await;
            if let Err(error) = changed {
                return eprintln!("sikemux: the core did not move chat {agent}: {error}");
            }
            match restarted(client, agent, true).await {
                Some(news) => news,
                None => return,
            }
        }
        ChatAsk::Attach { agent, since } => match attach(client, &agent, since).await {
            Ok(attachment) => (agent, ChatNews::Attached(attachment)),
            Err(error) => {
                return eprintln!("sikemux: the core refused to attach chat {agent}: {error}")
            }
        },
        ChatAsk::History {
            agent,
            feed,
            before,
            turns,
        } => match client
            .ask(ask::AcpHistory {
                agent_id: agent.clone(),
                feed,
                before,
                turns,
            })
            .await
        {
            Ok(page) => (agent, ChatNews::Page(page_of(page))),
            Err(error) => return eprintln!("sikemux: the core gave no earlier page: {error}"),
        },
        ChatAsk::Later { agent, feed, after } => {
            let asked = ChatPageParams {
                after: Some(after),
                ..ChatPageParams::new(&agent, feed)
            };
            match called::<call::ChatPage>(client, asked).await {
                Ok(page) => (agent, ChatNews::Later(page)),
                Err(error) => return eprintln!("sikemux: the core gave no later page: {error}"),
            }
        }
        ChatAsk::Latest { agent, feed } => {
            let asked = ChatPageParams {
                before: Some(u64::MAX),
                ..ChatPageParams::new(&agent, feed)
            };
            match called::<call::ChatPage>(client, asked).await {
                Ok(page) => (agent, ChatNews::Latest(page)),
                Err(error) => return eprintln!("sikemux: the core gave no latest page: {error}"),
            }
        }
        ChatAsk::Around {
            agent,
            feed,
            around,
        } => {
            let asked = ChatPageParams {
                around: Some(around),
                ..ChatPageParams::new(&agent, feed)
            };
            match called::<call::ChatPage>(client, asked).await {
                Ok(page) => (agent, ChatNews::Around(page)),
                Err(error) => return eprintln!("sikemux: the core gave no page there: {error}"),
            }
        }
        ChatAsk::Find {
            agent,
            params,
            asked,
        } => match called::<call::ChatFind>(client, params).await {
            Ok(found) => (agent, ChatNews::Found { asked, found }),
            Err(error) => return eprintln!("sikemux: the core's find failed: {error}"),
        },
        ChatAsk::Detach { agent } => {
            return report(
                "detach",
                client.ask(ask::AcpDetach { agent_id: agent }).await,
            )
        }
        ChatAsk::Prompt {
            agent,
            text,
            paths,
            context,
            attach: attaching,
        } => match client
            .ask(ask::AcpPrompt {
                agent_id: agent.clone(),
                message_id: None,
                text: text.clone(),
                paths,
                context,
            })
            .await
        {
            Ok(()) if !attaching => (agent, ChatNews::Taken(None)),
            Ok(()) => match attach(client, &agent, None).await {
                Ok(attachment) => (agent, ChatNews::Taken(Some(attachment))),
                Err(error) => {
                    eprintln!("sikemux: the core refused to attach chat {agent}: {error}");
                    (agent, ChatNews::Taken(None))
                }
            },
            Err(error) => (
                agent,
                ChatNews::Refused {
                    error: error.to_string(),
                    text,
                },
            ),
        },
        ChatAsk::Keep {
            agent,
            name,
            mime,
            bytes,
            kept,
        } => {
            let _ = kept.send(
                client
                    .attach_file(agent, name, mime, &bytes)
                    .await
                    .map_err(|error| error.to_string()),
            );
            return;
        }
        ChatAsk::Edit {
            agent,
            message,
            text,
            paths,
        } => match client
            .ask(ask::AcpEdit {
                agent_id: agent.clone(),
                message_id: message,
                text: text.clone(),
                paths,
                context: Vec::new(),
            })
            .await
        {
            Ok(()) => return,
            Err(error) => (
                agent,
                ChatNews::Refused {
                    error: error.to_string(),
                    text,
                },
            ),
        },
        ChatAsk::Cancel { agent } => {
            return report("stop", client.ask(ask::AcpCancel { agent_id: agent }).await)
        }
        ChatAsk::Permission {
            agent,
            request,
            option,
        } => {
            let replied = client
                .ask(ask::AcpPermissionReply {
                    agent_id: agent.clone(),
                    request_id: request.clone(),
                    option_id: option,
                })
                .await;
            let error = replied
                .err()
                .map(CallError::from)
                .filter(|error| error.kind != Some(ErrorKind::NotPending))
                .map(String::from);
            (agent, ChatNews::Replied { request, error })
        }
        ChatAsk::Config {
            agent,
            config,
            value,
        } => {
            return report(
                "change the settings of",
                client
                    .ask(ask::AcpSetConfig {
                        agent_id: agent,
                        config_id: config,
                        value,
                    })
                    .await
                    .map(drop),
            )
        }
        ChatAsk::StopTask { agent, task } => {
            let stopped = client
                .ask(ask::AcpStopTask {
                    agent_id: agent.clone(),
                    task_id: task.clone(),
                })
                .await;
            let error = stopped.err().map(|error| error.to_string());
            (agent, ChatNews::TaskStopped { task, error })
        }
        ChatAsk::Detail { agent, tool } => {
            let asked = ToolDetailParams {
                agent_id: agent.clone(),
                tool_call_id: tool.clone(),
            };
            match called::<call::ChatToolDetail>(client, asked).await {
                Ok(detail) => (agent, ChatNews::Detail { tool, detail }),
                Err(error) => {
                    return eprintln!("sikemux: the core refused a tool's detail: {error}")
                }
            }
        }
        ChatAsk::Mode { agent, mode } => {
            return report(
                "switch the mode of",
                client
                    .ask(ask::AcpSetPermissionMode {
                        agent_id: agent,
                        mode,
                    })
                    .await,
            )
        }
    };
    let _ = updates.send(Update::Chat(agent, news));
}

/// A page of a chat's rows the core answered, or none when it does not read.
fn page_of(page: Value) -> Option<ChatPage> {
    serde_json::from_value(page)
        .inspect_err(|error| eprintln!("sikemux: the core's chat page does not read: {error}"))
        .ok()
}

/// Asks `client`'s core for the call `C`, and reads its answer here, off the window's thread.
async fn called<C: Call>(client: &CoreClient, params: C::Params) -> Result<C::Answer, String>
where
    C::Params: Serialize,
    C::Answer: DeserializeOwned,
{
    let params = serde_json::to_value(params).map_err(|error| error.to_string())?;
    let answer = client
        .call(C::METHOD, params)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_value(answer)
        .map_err(|error| format!("the core's answer to {} did not read: {error}", C::METHOD))
}

/// Starts `agent` again and attaches to it; one `held` waits for the person before its agent starts.
async fn restarted(client: &CoreClient, agent: String, held: bool) -> Option<(String, ChatNews)> {
    let started = called::<call::StartChat>(
        client,
        StartParams {
            agent_id: agent.clone(),
            waits_for_person: held,
            again: false,
        },
    )
    .await;
    if let Err(error) = started {
        eprintln!("sikemux: the core did not start chat {agent} again: {error}");
        return None;
    }
    match attach(client, &agent, None).await {
        Ok(attachment) => Some((agent, ChatNews::Attached(attachment))),
        Err(error) => {
            eprintln!("sikemux: the core refused to attach chat {agent}: {error}");
            None
        }
    }
}

/// Attaches to a chat. One that isn't running answers `Missing` and stays stopped until
/// the person sends to it.
async fn attach(
    client: &CoreClient,
    agent: &str,
    since: Option<ChatMark>,
) -> Result<ChatAttachment, ClientError> {
    client
        .ask(ask::AcpAttach {
            agent_id: agent.to_owned(),
            since,
        })
        .await
}

fn report(what: &str, done: Result<(), ClientError>) {
    if let Err(error) = done {
        eprintln!("sikemux: the core refused to {what} a chat: {error}");
    }
}

enum Settled {
    Attached(SessionId, Attached),
    Detached(StreamId),
    Resized(SessionId),
}

/// The least time between two resizes of one terminal, each of which has the core wrap its
/// whole scrollback again, so dragging a divider sends the sizes it settles on, not every one.
const RESIZE_GAP: Duration = Duration::from_millis(50);

/// The terminals attached on one connection: their streams, and frames that
/// arrived before the attach that started them was answered.
struct Streams {
    /// The core the terminals run on.
    place: Place,
    client: Arc<CoreClient>,
    updates: mpsc::UnboundedSender<Update>,
    sessions: HashMap<StreamId, SessionId>,
    streams: HashMap<SessionId, StreamId>,
    early: HashMap<StreamId, Vec<Vec<u8>>>,
    settle_tx: mpsc::UnboundedSender<(Place, Settled)>,
    /// The terminals with a resize on its way, and the size asked for since, sent after it.
    resizing: HashMap<SessionId, Option<(u16, u16)>>,
}

impl Streams {
    fn new(
        place: Place,
        client: &Arc<CoreClient>,
        updates: &mpsc::UnboundedSender<Update>,
        settle_tx: &mpsc::UnboundedSender<(Place, Settled)>,
    ) -> Self {
        Self {
            place,
            client: client.clone(),
            updates: updates.clone(),
            sessions: HashMap::new(),
            streams: HashMap::new(),
            early: HashMap::new(),
            settle_tx: settle_tx.clone(),
            resizing: HashMap::new(),
        }
    }

    fn send(&self, session: SessionId, screenful: Screenful) {
        let _ = self
            .updates
            .send(Update::Terminal(self.place.clone(), session, screenful));
    }

    fn frame(&mut self, stream: StreamId, bytes: Vec<u8>) {
        match self.sessions.get(&stream) {
            Some(&session) => {
                if let Some(frame) = decode_frame(&bytes) {
                    self.send(session, Screenful::Frame(frame));
                }
            }
            None => self.early.entry(stream).or_default().push(bytes),
        }
    }

    fn settle(&mut self, settled: Settled) {
        match settled {
            Settled::Attached(session, attached) => {
                self.sessions.insert(attached.stream, session);
                self.streams.insert(session, attached.stream);
                if let Some(shell) = attached.shell {
                    self.send(session, Screenful::Shell(shell));
                }
                self.frame(attached.stream, attached.frame);
                for bytes in self.early.remove(&attached.stream).unwrap_or_default() {
                    self.frame(attached.stream, bytes);
                }
            }
            Settled::Detached(stream) => {
                self.sessions.remove(&stream);
                self.early.remove(&stream);
            }
            Settled::Resized(session) => {
                if let Some((cols, rows)) = self.resizing.remove(&session).flatten() {
                    self.resize(session, cols, rows);
                }
            }
        }
    }

    fn resize(&mut self, session: SessionId, cols: u16, rows: u16) {
        if let Some(pending) = self.resizing.get_mut(&session) {
            *pending = Some((cols, rows));
            return;
        }
        self.resizing.insert(session, None);
        let (client, settle, host) = (
            self.client.clone(),
            self.settle_tx.clone(),
            self.place.clone(),
        );
        tokio::spawn(async move {
            let _ = client
                .ask(ask::Resize {
                    id: session,
                    cols,
                    rows,
                })
                .await;
            tokio::time::sleep(RESIZE_GAP).await;
            let _ = settle.send((host, Settled::Resized(session)));
        });
    }

    fn ask(&mut self, session: SessionId, ask: TerminalAsk) {
        let client = self.client.clone();
        let stream = self.streams.get(&session).copied();
        match ask {
            TerminalAsk::Attach => {
                let (settle, host) = (self.settle_tx.clone(), self.place.clone());
                tokio::spawn(async move {
                    match client.attach(session).await {
                        Ok(attached) => {
                            drop(settle.send((host, Settled::Attached(session, attached))))
                        }
                        Err(error) => {
                            eprintln!("sikemux: terminal {session} would not attach: {error}")
                        }
                    }
                });
            }
            TerminalAsk::Detach => {
                let Some(stream) = self.streams.remove(&session) else {
                    return;
                };
                let (settle, host) = (self.settle_tx.clone(), self.place.clone());
                tokio::spawn(async move {
                    let _ = client.ask(ask::Detach { stream }).await;
                    let _ = settle.send((host, Settled::Detached(stream)));
                });
            }
            TerminalAsk::Ack(frame) => {
                if let Some(stream) = stream {
                    client.ack(stream, frame);
                }
            }
            TerminalAsk::Input(input) => drop(tokio::spawn(async move {
                client.ask(ask::TerminalInput { id: session, input }).await
            })),
            TerminalAsk::Resize(cols, rows) => self.resize(session, cols, rows),
            TerminalAsk::Lines { start, count } => {
                let (updates, place) = (self.updates.clone(), self.place.clone());
                tokio::spawn(async move {
                    if let Some(lines) = client
                        .lines(session, start, count)
                        .await
                        .ok()
                        .and_then(|bytes| decode_lines(&bytes))
                    {
                        let _ =
                            updates.send(Update::Terminal(place, session, Screenful::Lines(lines)));
                    }
                });
            }
            TerminalAsk::Copy(range) => {
                let (updates, place) = (self.updates.clone(), self.place.clone());
                tokio::spawn(async move {
                    if let Ok(text) = client.text(session, range).await {
                        let _ =
                            updates.send(Update::Terminal(place, session, Screenful::Copied(text)));
                    }
                });
            }
            TerminalAsk::Find {
                query,
                options,
                from,
                backwards,
                inclusive,
            } => {
                let (updates, place) = (self.updates.clone(), self.place.clone());
                let request = ask::TerminalFind {
                    id: session,
                    query,
                    options,
                    from,
                    backwards,
                    inclusive,
                };
                tokio::spawn(async move {
                    let found = client.find(request).await.unwrap_or_default();
                    let _ = updates.send(Update::Terminal(place, session, Screenful::Found(found)));
                });
            }
            TerminalAsk::Clear => drop(tokio::spawn(async move {
                client.ask(ask::TerminalClear { id: session }).await
            })),
            TerminalAsk::Text { range, reply } => drop(tokio::spawn(async move {
                if let Ok(text) = client.text(session, range).await {
                    let _ = reply.send(text);
                }
            })),
            TerminalAsk::ResetModes => drop(tokio::spawn(async move {
                client.ask(ask::ResetModes { id: session }).await
            })),
            TerminalAsk::Kill => drop(tokio::spawn(async move {
                client.ask(ask::Kill { id: session }).await
            })),
        }
    }
}

type WatchGenerations = Arc<Mutex<HashMap<(Option<String>, Read), u64>>>;
type FailedWatches = Arc<Mutex<HashSet<Read>>>;

/// Reads `read`'s watch, noting in `failed` when it was refused, so it is read again once the
/// network is back.
fn read(
    client: &Arc<CoreClient>,
    key: Read,
    watch: &Watch,
    (current, failed): (&WatchGenerations, &FailedWatches),
    updates: &mpsc::UnboundedSender<Update>,
) {
    let (client, updates, watch, current, failed) = (
        client.clone(),
        updates.clone(),
        watch.clone(),
        current.clone(),
        failed.clone(),
    );
    tokio::spawn(async move {
        let answer = client.call(key.method(), watch.params).await;
        let answer = answer.map_err(|error| error.to_string());
        let mut failed = failed.lock().expect("failed watches");
        match answer.is_ok() {
            true => failed.remove(&key),
            false => failed.insert(key),
        };
        drop(failed);
        send_read(None, key, watch.generation, answer, &current, &updates);
    });
}

/// Sends what `key`'s watch was answered, unless another watch has replaced it.
fn send_read(
    host: Option<String>,
    key: Read,
    generation: u64,
    answer: Result<Value, String>,
    current: &WatchGenerations,
    updates: &mpsc::UnboundedSender<Update>,
) {
    let watched = (host, key);
    if current.lock().expect("watch generations").get(&watched) != Some(&generation) {
        return;
    }
    let (host, key) = watched;
    let place = host.map_or(Place::This, Place::Account);
    #[cfg(feature = "drive")]
    crate::drive::trace(format_args!("link read {key:?}"));
    let _ = updates.send(match answer {
        Ok(value) => Update::Read(place, key, value),
        Err(error) => Update::Refused(place, key, error),
    });
}

pub fn utc_offset_minutes() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}
