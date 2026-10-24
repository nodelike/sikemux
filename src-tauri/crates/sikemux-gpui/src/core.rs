use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sikemux_client::accounts::network;
use sikemux_client::client::{Attached, ClientError, ClientEvent, CoreClient};
use sikemux_client::remote::SecretKey;
use sikemux_host::cli_paths;
use sikemux_host::core_start::{self, StartError};
use sikemux_host::login_item::{self, CoreLaunch};
use sikemux_linked::{Happened, Linked};
use sikemux_wire::grid::{
    decode_frame, decode_lines, FindOptions, FindResult, Frame, GridPoint, Lines, TerminalInput,
};
use sikemux_wire::protocol::{
    CallId, ChatAccount, ChatAttachment, ChatContext, ChatMark, DeviceAccess, DeviceView, Event,
    LaunchIdentity, NetworkChange, RemoteStatus, Request, Response, SessionId, SpawnTarget,
    StreamId, TerminalSpawn, WindowAnswer, WindowCall,
};
use tokio::sync::{mpsc, oneshot};

use crate::hosts::Place;
use crate::retry::Backoff;

pub enum Update {
    /// The core's release, which the app ships with.
    Connected(String),
    /// What another host's core sent about one of its terminals or chats, by the host's key.
    From(String, Box<Update>),
    View(Box<DeviceView>),
    /// A watched call's latest answer, under the key it was watched as.
    Read(&'static str, Value),
    /// The core turned down what was asked under the key.
    Refused(&'static str, String),
    /// Show this agent and come to the front, as the notch or a request held while no window
    /// was open asks.
    FocusAgent(String),
    /// The core turned down something the person did, saying why.
    Failed(String),
    Terminal(SessionId, Screenful),
    Lost(String),
    /// The core said it stopped on purpose, as `sikemux stop` stops it, so the window starts it
    /// again only when the person asks with `Ask::StartCore`, or picks up one started elsewhere.
    Stopped,
    /// The core stopped everything it ran, as the person asked, so the app can quit.
    ShutDown,
    /// Where remote access stands, whenever it changes.
    Remote(Box<RemoteStatus>),
    /// Something the core says the person should hear about an agent, such as it needing them.
    Notice {
        title: String,
        body: String,
        agent: String,
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
    /// The core said something under a followed topic moved; no paths means anything may have.
    Changed {
        topic: String,
        project: Option<String>,
        paths: Option<Vec<String>>,
    },
    /// Another host's view, by the host's key, as its core sends it.
    HostView(String, Box<DeviceView>),
    /// Whether this app reaches another host now, by the host's key.
    HostReach(String, bool),
    /// Why this app cannot reach the account's other hosts at all.
    HostsUnavailable(String),
    /// This host went offline, came back, or woke.
    Network(NetworkChange),
}

pub enum ChatNews {
    Attached(ChatAttachment),
    /// The chat's changes numbered `seq`, a list of `ChatChange` as JSON.
    Changes {
        seq: u64,
        changes: Value,
    },
    /// Earlier rows, a `ChatPage` as JSON, or null when the core had none to give.
    Page(Value),
    /// Later rows, a `ChatPage` as JSON with `newerAfter`.
    Later(Value),
    /// The rows around a place found, a `ChatPage` as JSON.
    Around(Value),
    /// The chat's latest rows, a `ChatPage` as JSON.
    Latest(Value),
    /// What `chatFind` answered the ask numbered `asked`.
    Found {
        asked: u64,
        found: Value,
    },
    /// A `ToolDetail` as JSON.
    Detail {
        tool: String,
        detail: Value,
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
}

/// What the window asks of one terminal.
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

/// One of the core's `Watch` calls, read again whenever the core says one of
/// its topics changed.
#[derive(Clone)]
pub struct Watch {
    pub method: &'static str,
    pub params: Value,
    pub topics: &'static [&'static str],
    /// Counts the watches made under one key, so an answer to one it replaced is dropped.
    generation: u64,
}

impl Watch {
    pub fn new(method: &'static str, params: Value, topics: &'static [&'static str]) -> Self {
        Self {
            method,
            params,
            topics,
            generation: 0,
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
    /// Keeps `key` answered by `watch`, in place of whatever it watched before.
    Watch(&'static str, Watch),
    /// Stops answering `key`, wherever it was watched, dropping an answer still on its way.
    Unwatch(&'static str),
    /// One of the core's methods, for something the person did. What it changes
    /// comes back with the next view.
    Call {
        method: &'static str,
        params: Value,
    },
    /// A call whose answer the window waits for, sent back under `key`.
    Answer {
        key: &'static str,
        method: &'static str,
        params: Value,
    },
    /// A call whose answer goes back to the one asking, for answers that belong to
    /// what asked, such as one file's diff.
    Request {
        method: &'static str,
        params: Value,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// A watch, stream, call or answer for another host's core, by the host's key, as this
    /// host's would be. A key is watched in one place at a time.
    On(String, Box<Ask>),
    /// A call to another host's core, by the host's key, whose answer goes back to the one asking.
    HostRequest {
        host: String,
        method: &'static str,
        params: Value,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// A call whose answer arrives in parts, as `Stream` events under `stream`.
    Stream {
        stream: u64,
        method: &'static str,
        params: Value,
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
    /// Hands the window the core's changes under `topic`, with what moved.
    Follow(&'static str),
    /// Makes this window the one the core hands its calls to, as when it comes to the front.
    RegisterWindow,
    AnswerWindow(CallId, WindowAnswer),
    /// Every tab a waiting `sikemux open` opened has closed.
    WindowOpenClosed(CallId),
    /// Starts a chat with `launcher`'s agent in `project` and brings its tab to the front,
    /// answering the new agent's id to `started` when it is there.
    StartChat {
        launcher: String,
        project: String,
        /// The permission mode it starts in, for this launch only; none starts in the launcher's.
        mode: Option<String>,
        started: Option<oneshot::Sender<String>>,
    },
    /// Takes up one of the device view's recent chats again and brings its tab to the front.
    ResumeChat {
        recent: String,
        project: String,
    },
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
        patch: Value,
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
        params: Value,
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
        mode: String,
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

/// The app's own identifier. A dev build is Sikemux Dev, sharing its folders and core with the
/// Mac app's dev build; a release keeps its own, apart from the Mac app's.
pub const IDENTIFIER: &str = if cfg!(debug_assertions) {
    login_item::IDENTIFIER
} else {
    "com.nodelike.sikemux.gpui"
};

/// Where this build's core listens, and how it is started when none does.
pub struct Home {
    pub socket: PathBuf,
    launch: Option<CoreLaunch>,
    manifest_dir: Option<PathBuf>,
}

impl Default for Home {
    fn default() -> Self {
        let library = sikemux_process::paths::home().map(|home| home.join("Library"));
        let support = library
            .as_ref()
            .map(|library| library.join("Application Support").join(IDENTIFIER));
        let socket = sikemux_core::default_socket_path().unwrap_or_default();
        let launch = cli_paths::cli_executable_path().map(|binary| CoreLaunch {
            binary,
            socket: socket.clone(),
            log: library
                .map_or_else(std::env::temp_dir, |library| {
                    library.join("Logs").join(IDENTIFIER)
                })
                .join("core.log"),
            args: login_item::core_args(support.clone()),
        });
        Self {
            socket,
            launch,
            manifest_dir: support.map(|support| support.join("agent-detection")),
        }
    }
}

/// A new chat asked of this window's own core, until the core says it has begun.
struct Beginning {
    launcher: String,
    begun: oneshot::Sender<String>,
}

/// Starts or takes up again a chat on `client`'s core and brings its tab to the front there:
/// as soon as `begun` names the new chat, rather than once its agent's program is ready, which
/// takes a real agent seconds.
async fn open_chat_on(client: Arc<CoreClient>, ask: Ask, begun: Option<oneshot::Receiver<String>>) {
    let (opened, project, started, shown) = match ask {
        Ask::StartChat {
            launcher,
            project,
            mode,
            started,
        } => {
            let asked = client.request(Request::StartChat {
                launcher,
                project: project.clone(),
                permission_mode: mode,
                model: None,
                effort: None,
            });
            let (answer, shown) = {
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
            let begun = match answer {
                Ok(Response::ChatBegun { agent_id, start }) => Ok((agent_id, start)),
                Ok(_) => Err(ClientError::UnexpectedReply),
                Err(error) => Err(error),
            };
            (begun, project, started, shown)
        }
        Ask::ResumeChat { recent, project } => {
            (client.resume_chat(recent).await, project, None, None)
        }
        _ => return,
    };
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
    let tab = json!({ "project": project, "kind": "agent", "target": agent });
    let _ = client.call("openTab", tab).await;
}

/// Asks another host's core one of its methods, by the host's key, and waits for the answer.
pub fn request_on(
    asks: &mpsc::UnboundedSender<Ask>,
    host: &str,
    method: &'static str,
    params: Value,
) -> impl std::future::Future<Output = Result<Value, String>> {
    let (reply, answer) = oneshot::channel();
    let sent = asks.send(Ask::HostRequest {
        host: host.to_owned(),
        method,
        params,
        reply,
    });
    async move {
        let gone = "the core is not connected";
        sent.map_err(|_| gone.to_owned())?;
        answer.await.map_err(|_| gone.to_owned())?
    }
}

/// Where this app's key on the network is kept. A dev build takes the Mac app's dev key, so it is
/// the same device on the account; a release keeps one apart from the Mac app's.
const DEVICE_KEY: &str = if cfg!(debug_assertions) {
    "sikemux-device"
} else {
    "sikemux-gpui-device"
};

/// What the account calls this app, before " on <host>".
const DEVICE_NAME: &str = if cfg!(debug_assertions) {
    "Sikemux Dev"
} else {
    "Sikemux GPUI try-out"
};

/// This app's key on the network, made the first time it is needed and kept where
/// `sikemux_keychain` keeps secrets; a build that keeps none cannot have one.
fn device_key() -> Result<SecretKey, String> {
    let account = if cfg!(debug_assertions) {
        "dev"
    } else if sikemux_keychain::TRYOUT {
        "tryout"
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
    match Linked::bind(key, relays, DEVICE_NAME, "macos").await {
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

/// What another host said next, or never while this app reaches none.
/// `update`, as from the host it came from: none for this host.
fn from_host(host: &Option<String>, update: Update) -> Update {
    match host {
        Some(host) => Update::From(host.clone(), Box::new(update)),
        None => update,
    }
}

/// The generation the next watch of `key` is made under, wherever it is watched.
fn next_generation(current: &Arc<Mutex<HashMap<&'static str, u64>>>, key: &'static str) -> u64 {
    let mut current = current.lock().expect("watch generations");
    let next = current.get(key).map_or(0, |generation| generation + 1);
    current.insert(key, next);
    next
}

/// One of another host's core methods, waiting a while for a host that is reconnecting.
async fn call_on(
    linked: &Option<Arc<Linked>>,
    host: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let linked = linked
        .as_ref()
        .ok_or("this app does not reach other hosts")?;
    let client = linked
        .client(host)
        .await
        .map_err(|error| error.to_string())?;
    client
        .call(method, params)
        .await
        .map_err(|error| error.to_string())
}

/// Answers `key` from another host's core, as `read` does from this host's.
fn read_on(
    linked: &Option<Arc<Linked>>,
    host: &str,
    key: &'static str,
    watch: &Watch,
    current: &Arc<Mutex<HashMap<&'static str, u64>>>,
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
        let answer = call_on(&linked, &host, watch.method, watch.params).await;
        if current.lock().expect("watch generations").get(key) != Some(&watch.generation) {
            return;
        }
        let _ = updates.send(match answer {
            Ok(value) => Update::Read(key, value),
            Err(error) => Update::Refused(key, error),
        });
    });
}

async fn heard_from(heard: &mut Option<mpsc::UnboundedReceiver<Happened>>) -> Option<Happened> {
    match heard {
        Some(heard) => heard.recv().await,
        None => std::future::pending().await,
    }
}

/// Asks the core one of its methods and waits for the answer.
pub fn request(
    asks: &mpsc::UnboundedSender<Ask>,
    method: &'static str,
    params: Value,
) -> impl std::future::Future<Output = Result<Value, String>> {
    let (reply, answer) = oneshot::channel();
    let sent = asks.send(Ask::Request {
        method,
        params,
        reply,
    });
    async move {
        let gone = "the core is not connected";
        sent.map_err(|_| gone.to_owned())?;
        answer.await.map_err(|_| gone.to_owned())?
    }
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
        self.target
            .call("streamCancel", json!({ "stream": self.stream }));
    }
}

/// Asks the core one of its methods whose answer streams, adding the stream's number to
/// `params`.
pub fn stream(asks: &mpsc::UnboundedSender<Ask>, method: &'static str, params: Value) -> Streaming {
    Target::here(asks).stream(method, params)
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
        self.send(ask);
    }

    fn send(&self, ask: Ask) {
        let _ = self.asks.send(match &self.place {
            Place::This => ask,
            Place::Account(host) => Ask::On(host.clone(), Box::new(ask)),
        });
    }

    /// Asks one of the core's methods and waits for the answer.
    pub fn request(
        &self,
        method: &'static str,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>> {
        match &self.place {
            Place::This => Box::pin(request(&self.asks, method, params)),
            Place::Account(host) => Box::pin(request_on(&self.asks, host, method, params)),
        }
    }

    /// Keeps the core answering `watch` under `key`, here or on the place's host, wherever
    /// it was watched before.
    pub fn watch(&self, key: &'static str, watch: Watch) {
        self.send(Ask::Watch(key, watch));
    }

    /// Stops the core answering `key`, here or on any host.
    pub fn unwatch(&self, key: &'static str) {
        let _ = self.asks.send(Ask::Unwatch(key));
    }

    /// Tells the core something, with no answer waited for.
    pub fn call(&self, method: &'static str, params: Value) {
        self.send(Ask::Call { method, params });
    }

    /// Starts a chat with `launcher`'s agent in `project` there, in `mode` for this launch or
    /// else the launcher's, answering the new agent's id to `started` when it is there.
    pub fn start_chat(
        &self,
        launcher: String,
        project: String,
        mode: Option<String>,
        started: Option<oneshot::Sender<String>>,
    ) {
        self.send(Ask::StartChat {
            launcher,
            project,
            mode,
            started,
        });
    }

    /// Takes up one of the place's recent chats again.
    pub fn resume_chat(&self, recent: String, project: String) {
        self.send(Ask::ResumeChat { recent, project });
    }

    /// Asks once, the answer arriving as a read under `key`.
    pub fn answer(&self, key: &'static str, method: &'static str, params: Value) {
        self.send(Ask::Answer {
            key,
            method,
            params,
        });
    }

    /// Asks a method whose answer streams, adding the stream's number to `params`.
    pub fn stream(&self, method: &'static str, mut params: Value) -> Streaming {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let stream = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(params) = params.as_object_mut() {
            params.insert("stream".into(), stream.into());
        }
        let (sender, parts) = mpsc::unbounded_channel();
        self.send(Ask::Stream {
            stream,
            method,
            params,
            parts: sender,
        });
        Streaming {
            parts,
            stream,
            target: self.clone(),
        }
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

/// A connection that held this long was a working one, so the next reconnect is quick again.
const HELD: Duration = Duration::from_secs(10);

async fn run(
    home: Home,
    updates: mpsc::UnboundedSender<Update>,
    mut asks: mpsc::UnboundedReceiver<Ask>,
) {
    let mut watches: HashMap<&'static str, Watch> = HashMap::new();
    let mut host_watches: HashMap<String, HashMap<&'static str, Watch>> = HashMap::new();
    let mut followed: Vec<&'static str> = Vec::new();
    let mut linked: Option<Arc<Linked>> = None;
    let mut heard: Option<mpsc::UnboundedReceiver<Happened>> = None;
    let current = Arc::new(Mutex::new(HashMap::<&'static str, u64>::new()));
    let mut reconnect = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
    loop {
        if let Some(launch) = &home.launch {
            match core_start::start(IDENTIFIER, launch, &sikemux_host::build_identity(), false)
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
                if let Err(error) = client.configure(home.manifest_dir.clone()).await {
                    eprintln!("sikemux: the core could not read the agent manifests: {error}");
                }
                if let Err(error) = client.watch_view().await {
                    let _ = updates.send(Update::Lost(error.to_string()));
                }
                if let Err(error) = client.register_window().await {
                    eprintln!("sikemux: the core would not hand this window its calls: {error}");
                }
                for (key, watch) in &watches {
                    read(&client, key, watch, &current, &updates);
                }
                if linked.is_none() {
                    if let Some((reached, events)) = reach_hosts(&client, &updates).await {
                        linked = Some(reached);
                        heard = Some(events);
                    }
                }
                follow_links(&linked, &client);
                let (settle_tx, mut settled) = mpsc::unbounded_channel();
                let mut streams = Streams::new(None, &client, &updates, &settle_tx);
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
                            Some((None, settled)) => streams.settle(settled),
                            Some((Some(host), settled)) => {
                                if let Some(streams) = host_streams.get_mut(&host) {
                                    streams.settle(settled);
                                }
                            }
                            None => {}
                        },
                        reached = reached.recv() => {
                            if let Some((host, client)) = reached {
                                let mut streams = Streams::new(Some(host.clone()), &client, &updates, &settle_tx);
                                for (session, ask) in waiting_terminals.remove(&host).unwrap_or_default() {
                                    streams.ask(session, ask);
                                }
                                host_streams.insert(host, streams);
                            }
                        }
                        event = events.recv() => match event {
                            Some(ClientEvent::Grid { stream, frame }) => streams.frame(stream, frame),
                            Some(ClientEvent::Event(Event::DeviceView { view })) => {
                                let _ = updates.send(Update::View(view));
                            }
                            Some(ClientEvent::Event(Event::Notice { title, body, agent_id })) => {
                                let _ = updates.send(Update::Notice { title, body, agent: agent_id });
                            }
                            Some(ClientEvent::Event(Event::FocusAgent { agent_id })) => {
                                let _ = updates.send(Update::FocusAgent(agent_id));
                            }
                            Some(ClientEvent::Event(Event::Network { change })) => {
                                let _ = updates.send(Update::Network(change));
                            }
                            Some(ClientEvent::Event(Event::AccountDevice { key, name, .. })) => {
                                let _ = updates.send(Update::AccountDevice { key, name });
                            }
                            Some(ClientEvent::Event(Event::Remote { status })) => {
                                follow_links(&linked, &client);
                                let _ = updates.send(Update::Remote(Box::new(status)));
                            }
                            Some(ClientEvent::Event(Event::Chat { agent_id, seq, changes })) => {
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
                                stopping |= topic == "stopping";
                                for (key, watch) in watches.iter().filter(|(_, watch)| watch.topics.contains(&topic.as_str())) {
                                    read(&client, key, watch, &current, &updates);
                                }
                                if followed.contains(&topic.as_str()) {
                                    let _ = updates.send(Update::Changed { topic, project, paths });
                                }
                            }
                            Some(ClientEvent::WindowCall { call_id, call }) => {
                                let _ = updates.send(Update::WindowCall(call_id, call));
                            }
                            Some(_) => {}
                            None => break,
                        },
                        happened = heard_from(&mut heard) => match happened {
                            Some(Happened::Connected(host)) => {
                                for (key, watch) in host_watches.get(&host).into_iter().flatten() {
                                    read_on(&linked, &host, key, watch, &current, &updates);
                                }
                                let _ = updates.send(Update::HostReach(host, true));
                            }
                            Some(Happened::Lost(host)) => {
                                for key in host_watches.get(&host).into_iter().flat_map(HashMap::keys) {
                                    let _ = updates.send(Update::Refused(key, format!("{host} is out of reach")));
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
                                    let news = Update::Chat(agent_id, ChatNews::Changes { seq, changes });
                                    let _ = updates.send(Update::From(host, Box::new(news)));
                                }
                                ClientEvent::Event(Event::DeviceView { view }) => {
                                    let _ = updates.send(Update::HostView(host, view));
                                }
                                ClientEvent::Event(Event::Changed { topic, project, paths }) => {
                                    let changed = host_watches
                                        .get(&host)
                                        .into_iter()
                                        .flatten()
                                        .filter(|(_, watch)| watch.topics.contains(&topic.as_str()));
                                    for (key, watch) in changed {
                                        read_on(&linked, &host, key, watch, &current, &updates);
                                    }
                                    if followed.contains(&topic.as_str()) {
                                        let news = Update::Changed { topic, project, paths };
                                        let _ = updates.send(Update::From(host, Box::new(news)));
                                    }
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
                            Some(Ask::Watch(key, mut watch)) => {
                                watch.generation = next_generation(&current, key);
                                for table in host_watches.values_mut() {
                                    table.remove(key);
                                }
                                read(&client, key, &watch, &current, &updates);
                                watches.insert(key, watch);
                            }
                            Some(Ask::Unwatch(key)) => {
                                next_generation(&current, key);
                                watches.remove(key);
                                for table in host_watches.values_mut() {
                                    table.remove(key);
                                }
                            }
                            Some(Ask::RegisterWindow) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    if let Err(error) = client.register_window().await {
                                        eprintln!("sikemux: the core would not hand this window its calls: {error}");
                                    }
                                });
                            }
                            Some(Ask::AnswerWindow(call_id, answer)) => client.answer_window(call_id, answer),
                            Some(Ask::WindowOpenClosed(call_id)) => client.window_open_closed(call_id),
                            Some(Ask::Follow(topic)) => {
                                if !followed.contains(&topic) {
                                    followed.push(topic);
                                }
                            }
                            Some(Ask::Answer { key, method, params }) => {
                                let (client, updates) = (client.clone(), updates.clone());
                                tokio::spawn(async move {
                                    let _ = updates.send(match client.call(method, params).await {
                                        Ok(value) => Update::Read(key, value),
                                        Err(error) => Update::Refused(key, error.to_string()),
                                    });
                                });
                            }
                            Some(Ask::On(host, ask)) => match *ask {
                                Ask::Watch(key, mut watch) => {
                                    watch.generation = next_generation(&current, key);
                                    watches.remove(key);
                                    for table in host_watches.values_mut() {
                                        table.remove(key);
                                    }
                                    read_on(&linked, &host, key, &watch, &current, &updates);
                                    host_watches.entry(host).or_default().insert(key, watch);
                                }
                                Ask::Stream { stream, method, params, parts } => {
                                    streamed.insert(stream, parts.clone());
                                    let linked = linked.clone();
                                    tokio::spawn(async move {
                                        if let Err(error) = call_on(&linked, &host, method, params).await {
                                            let error = json!({ "category": "call", "message": error });
                                            let _ = parts.send(Part { items: Vec::new(), done: true, error: Some(error) });
                                        }
                                    });
                                }
                                Ask::Call { method, params } => {
                                    let (linked, updates) = (linked.clone(), updates.clone());
                                    tokio::spawn(async move {
                                        if let Err(error) = call_on(&linked, &host, method, params).await {
                                            let _ = updates.send(Update::Failed(error));
                                        }
                                    });
                                }
                                Ask::Answer { key, method, params } => {
                                    let (linked, updates) = (linked.clone(), updates.clone());
                                    tokio::spawn(async move {
                                        let _ = updates.send(match call_on(&linked, &host, method, params).await {
                                            Ok(value) => Update::Read(key, value),
                                            Err(error) => Update::Refused(key, error),
                                        });
                                    });
                                }
                                Ask::Request { method, params, reply } => {
                                    let linked = linked.clone();
                                    tokio::spawn(async move {
                                        let _ = reply.send(call_on(&linked, &host, method, params).await);
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
                                ask @ (Ask::StartChat { .. } | Ask::ResumeChat { .. }) => {
                                    let linked = linked.clone();
                                    tokio::spawn(async move {
                                        let client = match &linked {
                                            Some(linked) => linked.client(&host).await.ok(),
                                            None => None,
                                        };
                                        match client {
                                            Some(client) => open_chat_on(client, ask, None).await,
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
                                                let _ = updates.send(Update::From(host.clone(), Box::new(update)));
                                            }
                                        });
                                        chat(&client, ask, &told).await;
                                        drop(told);
                                        let _ = forward.await;
                                    });
                                }
                                _ => eprintln!("sikemux: that is not asked of another host"),
                            },
                            Some(Ask::HostRequest { host, method, params, reply }) => {
                                let linked = linked.clone();
                                tokio::spawn(async move {
                                    let answer = match linked {
                                        Some(linked) => match linked.client(&host).await {
                                            Ok(client) => client.call(method, params).await.map_err(|error| error.to_string()),
                                            Err(error) => Err(error.to_string()),
                                        },
                                        None => Err("this app does not reach other hosts".to_owned()),
                                    };
                                    let _ = reply.send(answer);
                                });
                            }
                            Some(Ask::Stream { stream, method, params, parts }) => {
                                streamed.insert(stream, parts.clone());
                                let client = client.clone();
                                tokio::spawn(async move {
                                    if let Err(error) = client.call(method, params).await {
                                        let error = json!({ "category": "call", "message": error.to_string() });
                                        let _ = parts.send(Part { items: Vec::new(), done: true, error: Some(error) });
                                    }
                                });
                            }
                            Some(Ask::Request { method, params, reply }) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    let answer = client.call(method, params).await;
                                    let _ = reply.send(answer.map_err(|error| error.to_string()));
                                });
                            }
                            Some(Ask::Detection { detection, reply }) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    let answer = match detection {
                                        Detection::List => client.list_manifests().await.map(serde_json::to_value),
                                        Detection::Reload => client.reload_manifests().await.map(serde_json::to_value),
                                        Detection::Explain(agent) => client
                                            .explain_agent_detection(agent)
                                            .await
                                            .map(serde_json::to_value),
                                    };
                                    let answer = match answer {
                                        Ok(Ok(value)) => Ok(value),
                                        Ok(Err(error)) => Err(error.to_string()),
                                        Err(error) => Err(error.to_string()),
                                    };
                                    let _ = reply.send(answer);
                                });
                            }
                            Some(Ask::Chat(ask)) => {
                                let (client, updates) = (client.clone(), updates.clone());
                                tokio::spawn(async move { chat(&client, ask, &updates).await });
                            }
                            Some(Ask::Call { method, params }) => {
                                let (client, updates) = (client.clone(), updates.clone());
                                tokio::spawn(async move {
                                    if let Err(error) = client.call(method, params).await {
                                        let _ = updates.send(Update::Failed(error.to_string()));
                                    }
                                });
                            }
                            Some(Ask::Terminal(session, ask)) => streams.ask(session, ask),
                            Some(ask @ (Ask::StartChat { .. } | Ask::ResumeChat { .. })) => {
                                let heard = match &ask {
                                    Ask::StartChat { launcher, .. } => {
                                        let (begun, heard) = oneshot::channel();
                                        beginnings.push(Beginning { launcher: launcher.clone(), begun });
                                        Some(heard)
                                    }
                                    _ => None,
                                };
                                tokio::spawn(open_chat_on(client.clone(), ask, heard));
                            }
                            Some(Ask::StartCore) => {}
                            Some(Ask::Shutdown) => {
                                let _ = client.shutdown(true).await;
                                let _ = updates.send(Update::ShutDown);
                            }
                            Some(Ask::MoveChat { agent, account }) => {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    if let Err(error) = client.acp_switch_account(agent, account).await {
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
                                        cli_endpoint: cli_paths::cli_endpoint_path(),
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
                                    let _ = updates.send(match status {
                                        Ok(status) => Update::Remote(Box::new(status)),
                                        Err(error) => Update::Refused("remote", error.to_string()),
                                    });
                                });
                            }
                            None => return,
                        },
                    }
                }
                if stopping {
                    let _ = updates.send(Update::Stopped);
                    if !wait_to_start(&home, &mut asks, &mut watches, &mut followed, &current).await
                    {
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
                let _ = updates.send(Update::Lost(error.to_string()));
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
    watches: &mut HashMap<&'static str, Watch>,
    followed: &mut Vec<&'static str>,
    current: &Arc<Mutex<HashMap<&'static str, u64>>>,
) -> bool {
    let mut look = tokio::time::interval(STOPPED_LOOK);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ask = asks.recv() => match ask {
                Some(Ask::StartCore) => return true,
                Some(Ask::Watch(key, mut watch)) => {
                    watch.generation = next_generation(current, key);
                    watches.insert(key, watch);
                }
                Some(Ask::Unwatch(key)) => {
                    next_generation(current, key);
                    watches.remove(key);
                }
                Some(Ask::Follow(topic)) => {
                    if !followed.contains(&topic) {
                        followed.push(topic);
                    }
                }
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
            report("stop", client.acp_stop(agent.clone()).await);
            let changed = client
                .call("updateAgent", json!({ "agentId": agent, "patch": patch }))
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
        } => match client.acp_history(agent.clone(), feed, before, turns).await {
            Ok(page) => (agent, ChatNews::Page(page)),
            Err(error) => return eprintln!("sikemux: the core gave no earlier page: {error}"),
        },
        ChatAsk::Later { agent, feed, after } => {
            let asked = json!({ "agentId": agent, "feed": feed, "after": after });
            match client.call("chatPage", asked).await {
                Ok(page) => (agent, ChatNews::Later(page)),
                Err(error) => return eprintln!("sikemux: the core gave no later page: {error}"),
            }
        }
        ChatAsk::Latest { agent, feed } => {
            let asked = json!({ "agentId": agent, "feed": feed, "before": u64::MAX });
            match client.call("chatPage", asked).await {
                Ok(page) => (agent, ChatNews::Latest(page)),
                Err(error) => return eprintln!("sikemux: the core gave no latest page: {error}"),
            }
        }
        ChatAsk::Around {
            agent,
            feed,
            around,
        } => {
            let asked = json!({ "agentId": agent, "feed": feed, "around": around });
            match client.call("chatPage", asked).await {
                Ok(page) => (agent, ChatNews::Around(page)),
                Err(error) => return eprintln!("sikemux: the core gave no page there: {error}"),
            }
        }
        ChatAsk::Find {
            agent,
            params,
            asked,
        } => match client.call("chatFind", params).await {
            Ok(found) => (agent, ChatNews::Found { asked, found }),
            Err(error) => return eprintln!("sikemux: the core's find failed: {error}"),
        },
        ChatAsk::Detach { agent } => return report("detach", client.acp_detach(agent).await),
        ChatAsk::Prompt {
            agent,
            text,
            paths,
            context,
            attach: attaching,
        } => match client
            .acp_prompt(agent.clone(), None, text.clone(), paths, context)
            .await
        {
            Ok(()) if !attaching => return,
            Ok(()) => match attach(client, &agent, None).await {
                Ok(attachment) => (agent, ChatNews::Attached(attachment)),
                Err(error) => {
                    return eprintln!("sikemux: the core refused to attach chat {agent}: {error}")
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
            .acp_edit(agent.clone(), message, text.clone(), paths, Vec::new())
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
        ChatAsk::Cancel { agent } => return report("stop", client.acp_cancel(agent).await),
        ChatAsk::Permission {
            agent,
            request,
            option,
        } => {
            let replied = client
                .acp_permission_reply(agent.clone(), request.clone(), option)
                .await;
            let error = replied.err().map(|error| error.to_string());
            (agent, ChatNews::Replied { request, error })
        }
        ChatAsk::Config {
            agent,
            config,
            value,
        } => {
            return report(
                "change the settings of",
                client.acp_set_config(agent, config, value).await.map(drop),
            )
        }
        ChatAsk::StopTask { agent, task } => {
            let stopped = client.acp_stop_task(agent.clone(), task.clone()).await;
            let error = stopped.err().map(|error| error.to_string());
            (agent, ChatNews::TaskStopped { task, error })
        }
        ChatAsk::Detail { agent, tool } => {
            let asked = json!({ "agentId": agent, "toolCallId": tool });
            match client.call("chatToolDetail", asked).await {
                Ok(detail) => (agent, ChatNews::Detail { tool, detail }),
                Err(error) => {
                    return eprintln!("sikemux: the core refused a tool's detail: {error}")
                }
            }
        }
        ChatAsk::Mode { agent, mode } => {
            return report(
                "switch the mode of",
                client.acp_set_permission_mode(agent, mode).await,
            )
        }
    };
    let _ = updates.send(Update::Chat(agent, news));
}

/// Starts `agent`'s session again and attaches to it.
/// Starts `agent` again and attaches to it; one `held` waits for the person before its agent starts.
async fn restarted(client: &CoreClient, agent: String, held: bool) -> Option<(String, ChatNews)> {
    let started = client
        .call(
            "startChat",
            json!({ "agentId": agent, "waitsForPerson": held }),
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
    client.acp_attach_since(agent.to_owned(), since).await
}

fn report(what: &str, done: Result<(), ClientError>) {
    if let Err(error) = done {
        eprintln!("sikemux: the core refused to {what} a chat: {error}");
    }
}

enum Settled {
    Attached(SessionId, Attached),
    Detached(StreamId),
}

/// The terminals attached on one connection: their streams, and frames that
/// arrived before the attach that started them was answered.
struct Streams {
    /// The host the terminals run on, by its key; none for this host.
    host: Option<String>,
    client: Arc<CoreClient>,
    updates: mpsc::UnboundedSender<Update>,
    sessions: HashMap<StreamId, SessionId>,
    streams: HashMap<SessionId, StreamId>,
    early: HashMap<StreamId, Vec<Vec<u8>>>,
    settle_tx: mpsc::UnboundedSender<(Option<String>, Settled)>,
}

impl Streams {
    fn new(
        host: Option<String>,
        client: &Arc<CoreClient>,
        updates: &mpsc::UnboundedSender<Update>,
        settle_tx: &mpsc::UnboundedSender<(Option<String>, Settled)>,
    ) -> Self {
        Self {
            host,
            client: client.clone(),
            updates: updates.clone(),
            sessions: HashMap::new(),
            streams: HashMap::new(),
            early: HashMap::new(),
            settle_tx: settle_tx.clone(),
        }
    }

    fn send(&self, session: SessionId, screenful: Screenful) {
        let _ = self
            .updates
            .send(from_host(&self.host, Update::Terminal(session, screenful)));
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
                self.frame(attached.stream, attached.frame);
                for bytes in self.early.remove(&attached.stream).unwrap_or_default() {
                    self.frame(attached.stream, bytes);
                }
            }
            Settled::Detached(stream) => {
                self.sessions.remove(&stream);
                self.early.remove(&stream);
            }
        }
    }

    fn ask(&mut self, session: SessionId, ask: TerminalAsk) {
        let client = self.client.clone();
        let stream = self.streams.get(&session).copied();
        match ask {
            TerminalAsk::Attach => {
                let (settle, host) = (self.settle_tx.clone(), self.host.clone());
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
                let (settle, host) = (self.settle_tx.clone(), self.host.clone());
                tokio::spawn(async move {
                    let _ = client.detach(stream).await;
                    let _ = settle.send((host, Settled::Detached(stream)));
                });
            }
            TerminalAsk::Ack(frame) => {
                if let Some(stream) = stream {
                    client.ack(stream, frame);
                }
            }
            TerminalAsk::Input(input) => {
                drop(tokio::spawn(
                    async move { client.input(session, input).await },
                ))
            }
            TerminalAsk::Resize(cols, rows) => drop(tokio::spawn(async move {
                client.resize(session, cols, rows).await
            })),
            TerminalAsk::Lines { start, count } => {
                let (updates, host) = (self.updates.clone(), self.host.clone());
                tokio::spawn(async move {
                    if let Some(lines) = client
                        .lines(session, start, count)
                        .await
                        .ok()
                        .and_then(|bytes| decode_lines(&bytes))
                    {
                        let _ = updates.send(from_host(
                            &host,
                            Update::Terminal(session, Screenful::Lines(lines)),
                        ));
                    }
                });
            }
            TerminalAsk::Copy(range) => {
                let (updates, host) = (self.updates.clone(), self.host.clone());
                tokio::spawn(async move {
                    if let Ok(text) = client.text(session, range).await {
                        let _ = updates.send(from_host(
                            &host,
                            Update::Terminal(session, Screenful::Copied(text)),
                        ));
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
                let (updates, host) = (self.updates.clone(), self.host.clone());
                let request = Request::TerminalFind {
                    id: session,
                    query,
                    options,
                    from,
                    backwards,
                    inclusive,
                };
                tokio::spawn(async move {
                    let found = client.find(request).await.unwrap_or_default();
                    let _ = updates.send(from_host(
                        &host,
                        Update::Terminal(session, Screenful::Found(found)),
                    ));
                });
            }
            TerminalAsk::Clear => drop(tokio::spawn(async move { client.clear(session).await })),
            TerminalAsk::Text { range, reply } => drop(tokio::spawn(async move {
                if let Ok(text) = client.text(session, range).await {
                    let _ = reply.send(text);
                }
            })),
            TerminalAsk::ResetModes => {
                drop(tokio::spawn(
                    async move { client.reset_modes(session).await },
                ))
            }
            TerminalAsk::Kill => drop(tokio::spawn(async move { client.kill(session).await })),
        }
    }
}

fn read(
    client: &Arc<CoreClient>,
    key: &'static str,
    watch: &Watch,
    current: &Arc<Mutex<HashMap<&'static str, u64>>>,
    updates: &mpsc::UnboundedSender<Update>,
) {
    let (client, updates, watch, current) = (
        client.clone(),
        updates.clone(),
        watch.clone(),
        current.clone(),
    );
    tokio::spawn(async move {
        let value = match client.call(watch.method, watch.params).await {
            Ok(value) => value,
            Err(error) => return eprintln!("sikemux: the core refused {}: {error}", watch.method),
        };
        if current.lock().expect("watch generations").get(key) == Some(&watch.generation) {
            #[cfg(feature = "drive")]
            crate::drive::trace(format_args!("link read {key}"));
            let _ = updates.send(Update::Read(key, value));
        }
    });
}

pub fn utc_offset_minutes() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}
