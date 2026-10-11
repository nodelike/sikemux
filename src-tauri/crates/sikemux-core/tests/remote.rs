#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr};
use serde_json::json;
use sikemux_core::client::{probe, ClientError, ClientEvent, CoreClient};
use sikemux_core::protocol::{
    Attention, AttentionKind, BackdropImage, BuildIdentity, ChatAttachment, ChatEventKind,
    ChatLaunch, ChatLauncher, ChatState, DeviceAccess, DeviceView, Event, LaunchIdentity,
    ProjectInfo, PublishedChat, PublishedRecent, RemoteStatus, SessionId, SpawnTarget,
    TerminalSpawn, MAX_ATTACHMENT_BYTES,
};
use sikemux_core::remote::{self, SecretKey};
use sikemux_core::server::{self, ServerConfig, ServerError};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(30);

fn init_env() {
    static ENV: Once = Once::new();
    ENV.call_once(|| {
        std::env::set_var("SHELL", "/bin/sh");
        std::env::set_var("PS1", "$ ");
        std::env::remove_var("ENV");
        std::env::remove_var("SIKEMUX_SHELL");
    });
}

struct Device {
    key: SecretKey,
    name: &'static str,
    access: DeviceAccess,
}

impl Device {
    fn new(name: &'static str, access: DeviceAccess) -> Self {
        Self {
            key: SecretKey::generate(),
            name,
            access,
        }
    }

    fn id(&self) -> String {
        self.key.public().to_string()
    }

    async fn endpoint(&self) -> Endpoint {
        Endpoint::builder(presets::Minimal)
            .secret_key(self.key.clone())
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .expect("loopback address")
            .bind()
            .await
            .expect("device endpoint")
    }
}

struct TestCore {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    thread: Option<JoinHandle<Result<(), ServerError>>>,
}

impl Drop for TestCore {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let socket = self.socket.clone();
            let _ = std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().expect("runtime");
                runtime.block_on(async {
                    if let Ok((client, _events)) = CoreClient::connect(&socket).await {
                        let _ = client.shutdown(true).await;
                    }
                });
            })
            .join();
            let _ = thread.join();
        }
    }
}

/// A core signed in to an account, its remote access already on, trusting
/// `devices`.
fn start_core(core_key: &SecretKey, devices: &[&Device]) -> TestCore {
    start_core_with(core_key, devices, false)
}

/// With `keeps_data`, the core has a data directory, where chats keep their
/// history for phones to page through.
fn start_core_with(core_key: &SecretKey, devices: &[&Device], keeps_data: bool) -> TestCore {
    init_env();
    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("core.sock");
    let stored = json!({
        "secretKey": hex::encode(core_key.to_bytes()),
        "enabled": true,
        "owner": "user_2test",
        "devices": devices.iter().map(|device| json!({
            "id": device.id(),
            "name": device.name,
            "platform": "ios",
            "access": device.access,
            "pairedAt": 1,
            "lastSeen": null,
        })).collect::<Vec<_>>(),
    });
    std::fs::write(
        remote_file(&socket),
        serde_json::to_vec(&stored).expect("remote file"),
    )
    .expect("write the remote file");
    let config = ServerConfig {
        idle_exit: Duration::from_secs(600),
        build: BuildIdentity {
            version: "0.0.0-test".into(),
            ..BuildIdentity::default()
        },
        remote_direct_only: true,
        data_dir: keeps_data.then(|| dir.path().join("data")),
        attachment_dir: Some(dir.path().join("pasted")),
        ..ServerConfig::new(socket.clone())
    };
    let thread = std::thread::spawn(move || server::run(config));
    let deadline = Instant::now() + WAIT;
    while probe(&socket, Duration::from_secs(1)).is_err() {
        assert!(Instant::now() < deadline, "the core never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
    TestCore {
        _dir: dir,
        socket,
        thread: Some(thread),
    }
}

fn remote_file(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".remote.json");
    PathBuf::from(path)
}

async fn listening(app: &CoreClient) -> RemoteStatus {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = app.remote_status().await.expect("remote status");
        if !status.addresses.is_empty() {
            return status;
        }
        assert!(Instant::now() < deadline, "remote access never listened");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn core_addr(status: &RemoteStatus) -> EndpointAddr {
    let id = status.core_id.parse().expect("core id");
    status
        .addresses
        .iter()
        .fold(EndpointAddr::new(id), |addr, address| {
            addr.with_ip_addr(address.parse().expect("address"))
        })
}

async fn until_status(
    events: &mut UnboundedReceiver<ClientEvent>,
    done: impl Fn(&RemoteStatus) -> bool,
) -> RemoteStatus {
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("timed out waiting for a remote status")
            .expect("the app's connection closed");
        if let ClientEvent::Event(Event::Remote { status }) = event {
            if done(&status) {
                return status;
            }
        }
    }
}

async fn until_disconnected(client: &CoreClient) {
    let deadline = Instant::now() + WAIT;
    while client.is_connected() {
        assert!(Instant::now() < deadline, "the device stayed connected");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn refusal(result: Result<impl std::fmt::Debug, ClientError>) -> String {
    match result {
        Err(ClientError::Core(message)) => message,
        other => panic!("expected the core to refuse, got {other:?}"),
    }
}

fn echo_terminal() -> SpawnTarget {
    SpawnTarget::Terminal(TerminalSpawn {
        cols: 80,
        rows: 24,
        cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        startup: Some("echo remote-hello".into()),
        ..TerminalSpawn::default()
    })
}

async fn until_output(
    client: &CoreClient,
    events: &mut UnboundedReceiver<ClientEvent>,
    id: SessionId,
    needle: &str,
) {
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains(needle) {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("timed out waiting for output")
            .expect("the device's connection closed");
        if let ClientEvent::Output { id: from, bytes } = event {
            client.ack(from, bytes.len());
            if from == id {
                seen.extend_from_slice(&bytes);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paired_device_drives_a_terminal_over_the_network() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    let status = listening(&app).await;
    assert_eq!(status.core_id, core_key.public().to_string());

    let endpoint = phone.endpoint().await;
    let (client, mut events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let id = client
        .spawn(LaunchIdentity::default(), echo_terminal())
        .await
        .expect("the phone starts a terminal");
    client.attach(id).await.expect("the phone attaches");
    until_output(&client, &mut events, id, "remote-hello").await;
    let session = app
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == id)
        .expect("the phone's terminal");
    assert_eq!(session.started_by, Some(phone.id()));
    client.kill(id).await.expect("the phone ends the terminal");

    let status = app.remote_status().await.expect("status");
    assert_eq!(status.connected, vec![phone.id()]);
    let seen = status.devices[0].last_seen.expect("last seen");
    assert!(seen > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watching_device_reads_but_cannot_drive_or_reach_the_core() {
    let core_key = SecretKey::generate();
    let watcher = Device::new("Watcher", DeviceAccess::Watch);
    let core = start_core(&core_key, &[&watcher]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    let id = app
        .spawn(LaunchIdentity::default(), echo_terminal())
        .await
        .expect("the app starts a terminal");
    let status = listening(&app).await;

    let endpoint = watcher.endpoint().await;
    let (client, _events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the watcher connects");
    let sessions = client.list().await.expect("the watcher lists sessions");
    let session = sessions.iter().find(|session| session.id == id);
    assert_eq!(session.expect("the app's terminal").started_by, None);
    assert!(refusal(client.kill(id).await).contains("watch"));
    assert!(refusal(client.write(id, b"exit\n").await).contains("watch"));
    assert!(refusal(client.stop_all().await).contains("only Sikemux on this host"));
    assert!(refusal(client.set_remote_access(false).await).contains("only Sikemux on this host"));
    assert!(app.list().await.expect("list").iter().any(|s| s.id == id));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_the_core_never_paired_with_is_turned_away() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let stranger = Device::new("Stranger", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    let status = listening(&app).await;

    let endpoint = stranger.endpoint().await;
    let attempt = remote::connect(&endpoint, core_addr(&status)).await;
    assert!(attempt.is_err(), "a stranger got a session");
    assert!(app
        .remote_status()
        .await
        .expect("status")
        .connected
        .is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_forgets_the_mac_is_unpaired() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Watch);
    let core = start_core(&core_key, &[&phone]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, _events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");

    client.unpair().await.expect("unpair");
    until_disconnected(&client).await;
    let status = app.remote_status().await.expect("status");
    assert!(status.devices.is_empty());
    let again = remote::connect(&endpoint, core_addr(&status)).await;
    assert!(again.is_err(), "an unpaired device reconnected");
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_or_narrowing_a_device_takes_effect_on_its_open_connection() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let tablet = Device::new("Tablet", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone, &tablet]);
    let (app, mut app_events) = CoreClient::connect(&core.socket).await.expect("app");
    let status = listening(&app).await;
    let id = app
        .spawn(LaunchIdentity::default(), echo_terminal())
        .await
        .expect("the app starts a terminal");

    let phone_endpoint = phone.endpoint().await;
    let (phone_client, _phone_events) = remote::connect(&phone_endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let tablet_endpoint = tablet.endpoint().await;
    let (tablet_client, _tablet_events) = remote::connect(&tablet_endpoint, core_addr(&status))
        .await
        .expect("the tablet connects");
    until_status(&mut app_events, |status| status.connected.len() == 2).await;

    app.set_device_access(phone.id(), DeviceAccess::Watch)
        .await
        .expect("narrow the phone");
    assert!(refusal(phone_client.kill(id).await).contains("watch"));

    let status = app.revoke_device(tablet.id()).await.expect("revoke");
    assert_eq!(status.devices.len(), 1);
    until_disconnected(&tablet_client).await;
    let again = remote::connect(&tablet_endpoint, core_addr(&status)).await;
    assert!(again.is_err(), "a revoked device reconnected");

    let status = app.set_remote_access(false).await.expect("turn off");
    assert!(!status.enabled);
    assert!(status.addresses.is_empty());
    until_disconnected(&phone_client).await;
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(remote_file(&core.socket)).expect("remote file"))
            .expect("json");
    assert_eq!(stored["enabled"], json!(false));
    assert_eq!(stored["devices"].as_array().map(Vec::len), Some(1));
}

const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");

async fn publish_fake_agent(app: &CoreClient) {
    publish_fake_agent_asking(app, "bypass").await;
}

async fn publish_fake_agent_asking(app: &CoreClient, permission_mode: &str) {
    let launcher = ChatLauncher {
        id: "opencode".into(),
        provider: "opencode".into(),
        label: "OpenCode".into(),
        program: FAKE_AGENT.into(),
        args: vec!["acp".into()],
        env: [("SECRET_TOKEN".to_owned(), "do-not-share".to_owned())].into(),
        permission_mode: permission_mode.into(),
        account: None,
        fallbacks: Vec::new(),
        status: None,
    };
    let project = ProjectInfo {
        id: "sess-tmp".into(),
        name: "tmp".into(),
        path: std::env::temp_dir(),
    };
    app.publish_workspace(vec![project], vec![launcher])
        .await
        .expect("publish");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_reaches_everyone_watching_but_its_sender_and_stays_in_the_replay() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, mut app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, mut events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = client
        .start_chat("opencode".into(), "sess-tmp".into(), None)
        .await
        .expect("the phone starts a chat");
    app.acp_attach(agent_id.clone())
        .await
        .expect("the app watches");

    client
        .acp_prompt(
            agent_id.clone(),
            None,
            "from the phone".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    let prompt = loop {
        let event = tokio::time::timeout(WAIT, app_events.recv())
            .await
            .expect("the app never heard the prompt")
            .expect("the app's connection closed");
        if let ClientEvent::Event(Event::Chat { event, .. }) = event {
            if event.kind == ChatEventKind::Prompt {
                break event;
            }
        }
    };
    assert_eq!(prompt.payload["text"], "from the phone");

    until_said(&mut events, &agent_id, "from the phone").await;
    while let Ok(event) = events.try_recv() {
        if let ClientEvent::Event(Event::Chat { event, .. }) = event {
            assert_ne!(
                event.kind,
                ChatEventKind::Prompt,
                "the sender already shows its prompt"
            );
        }
    }

    let ChatAttachment::Live { replay, .. } = app.acp_attach(agent_id).await.expect("attach")
    else {
        panic!("the chat is running");
    };
    assert!(replay
        .iter()
        .any(|event| event.kind == ChatEventKind::Prompt
            && event.payload["text"] == "from the phone"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_is_sent_a_long_chat_s_last_turns_and_pages_back_to_its_start() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Watch);
    let core = start_core_with(&core_key, &[&phone], true);
    let (app, mut app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let start = app
        .acp_start(fake_launch("agent-long"))
        .await
        .expect("the app starts a chat");
    assert!(!start.session_id.is_empty());
    for n in 0..55 {
        app.acp_prompt(
            "agent-long".into(),
            None,
            format!("turn {n}"),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
        loop {
            let event = tokio::time::timeout(WAIT, app_events.recv())
                .await
                .expect("the turn never ended")
                .expect("the app's connection closed");
            if let ClientEvent::Event(Event::Chat { event, .. }) = event {
                if event.kind == ChatEventKind::TurnCompleted {
                    break;
                }
            }
        }
    }
    let prompts = |events: &[sikemux_core::protocol::ChatEvent]| -> Vec<String> {
        events
            .iter()
            .filter(|event| event.kind == ChatEventKind::Prompt)
            .map(|event| {
                event.payload["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    };
    let turns = |range: std::ops::Range<usize>| -> Vec<String> {
        range.map(|n| format!("turn {n}")).collect()
    };

    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, _events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let ChatAttachment::Live {
        replay,
        mark,
        older_before: Some(before),
        ..
    } = client
        .acp_attach("agent-long".into())
        .await
        .expect("attach")
    else {
        panic!("the phone is sent the chat's end, with more before it");
    };
    assert_eq!(prompts(&replay), turns(5..55));
    assert_eq!(replay[0].kind, ChatEventKind::Prompt);

    let (events, older) = client
        .acp_history("agent-long".into(), mark.feed.clone(), before, 3)
        .await
        .expect("a watching phone reads the history");
    assert_eq!(prompts(&events), turns(2..5));
    let older = older.expect("there is more");
    let (events, older) = client
        .acp_history("agent-long".into(), mark.feed, older, 3)
        .await
        .expect("history");
    assert_eq!(prompts(&events), turns(0..2));
    assert_eq!(older, None);

    let ChatAttachment::Live {
        replay,
        older_before,
        ..
    } = app.acp_attach("agent-long".into()).await.expect("attach")
    else {
        panic!("the app takes the chat up");
    };
    assert_eq!(prompts(&replay), turns(0..55));
    assert_eq!(older_before, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_lists_the_app_s_chats_and_wakes_a_sleeping_one() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, mut app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, _events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = client
        .start_chat("opencode".into(), "sess-tmp".into(), None)
        .await
        .expect("the phone starts a chat");

    let published = |agent_id: &str, title: &str, asleep: bool| PublishedChat {
        agent_id: agent_id.into(),
        provider: "opencode".into(),
        title: Some(title.into()),
        cwd: std::env::temp_dir(),
        asleep,
    };
    app.publish_agents(
        vec![
            published(&agent_id, "Fix the replay test", false),
            published("agent-sleepy", "Tidy the docs", true),
            published("agent-failed", "Try Codex", false),
        ],
        Default::default(),
    )
    .await
    .expect("publish chats");
    let chats = client.acp_list().await.expect("list chats");
    let titled = |id: &str| {
        chats
            .iter()
            .find(|chat| chat.agent_id == id)
            .expect("listed")
    };
    assert_eq!(
        titled(&agent_id).title.as_deref(),
        Some("Fix the replay test")
    );
    assert!(!titled(&agent_id).asleep);
    assert_eq!(
        titled("agent-sleepy").title.as_deref(),
        Some("Tidy the docs")
    );
    assert!(titled("agent-sleepy").asleep);
    assert_eq!(titled("agent-failed").state, ChatState::Stopped);
    assert!(!titled("agent-failed").asleep);

    let waking = tokio::spawn(async move {
        let woken = client.acp_wake("agent-sleepy".into()).await;
        (client, woken)
    });
    loop {
        let event = tokio::time::timeout(WAIT, app_events.recv())
            .await
            .expect("the app is asked to wake the chat")
            .expect("the app's connection closed");
        if matches!(&event, ClientEvent::Event(Event::WakeChat { agent_id }) if agent_id == "agent-sleepy")
        {
            break;
        }
    }
    app.acp_start(fake_launch("agent-sleepy"))
        .await
        .expect("the app starts it again");
    let (client, woken) = waking.await.expect("wake task");
    woken.expect("the chat wakes");
    let chats = client.acp_list().await.expect("list chats");
    let sleepy = chats
        .iter()
        .find(|chat| chat.agent_id == "agent-sleepy")
        .expect("listed");
    assert!(!sleepy.asleep);
    assert_eq!(
        sleepy.state,
        ChatState::Ready,
        "a wake is answered once the chat is ready"
    );
    assert!(matches!(
        client.acp_attach("agent-sleepy".into()).await,
        Ok(ChatAttachment::Live { .. })
    ));

    app.publish_palette([("ground".to_owned(), "#0f0f13".to_owned())].into())
        .await
        .expect("publish palette");
    let workspace = client.workspace().await.expect("workspace");
    assert_eq!(
        workspace.palette.get("ground").map(String::as_str),
        Some("#0f0f13")
    );
    let picture = BackdropImage {
        id: "pane-1".into(),
        data_url: "data:image/jpeg;base64,AAAA".into(),
    };
    app.publish_backdrop(true, Some(picture))
        .await
        .expect("publish backdrop");
    let backdrop = client.workspace().await.expect("workspace").backdrop;
    assert!(backdrop.texture);
    assert_eq!(backdrop.image.as_deref(), Some("pane-1"));
    let data_url = client.backdrop_image().await.expect("backdrop image");
    assert_eq!(data_url.as_deref(), Some("data:image/jpeg;base64,AAAA"));
    let not_a_picture = BackdropImage {
        id: "pane-2".into(),
        data_url: "data:text/plain,hi".into(),
    };
    let refused = app.publish_backdrop(true, Some(not_a_picture)).await;
    assert!(refusal(refused).contains("not an image"));
    let refused = app.unpair().await;
    assert!(refusal(refused).contains("only a paired device"));
    let refused = client.publish_palette(Default::default()).await;
    assert!(refusal(refused).contains("only Sikemux on this host"));

    let refused = client.publish_agents(Vec::new(), Default::default()).await;
    assert!(refusal(refused).contains("only Sikemux on this host"));
    let refused = client.publish_on_screen(Vec::new()).await;
    assert!(refusal(refused).contains("only Sikemux on this host"));
    let refused = client.watch_view().await;
    assert!(refusal(refused).contains("only Sikemux on this host"));
    let refused = client.acp_wake("agent-failed".into()).await;
    assert!(refusal(refused).contains("stopped on the host"));
    let refused = client.acp_wake("agent-gone".into()).await;
    assert!(refusal(refused).contains("no longer open"));
}

fn fake_launch(agent_id: &str) -> ChatLaunch {
    ChatLaunch {
        agent_id: agent_id.into(),
        provider: "opencode".into(),
        cwd: std::env::temp_dir(),
        program: FAKE_AGENT.into(),
        args: vec!["acp".into()],
        env: Default::default(),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: "bypass".into(),
        model: None,
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    }
}

async fn until_said(events: &mut UnboundedReceiver<ClientEvent>, agent: &str, needle: &str) {
    let mut said = String::new();
    while !said.contains(needle) {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("timed out waiting for the agent")
            .expect("the device's connection closed");
        let ClientEvent::Event(Event::Chat {
            agent_id, event, ..
        }) = event
        else {
            continue;
        };
        if agent_id != agent || event.kind != ChatEventKind::SessionUpdate {
            continue;
        }
        said.push_str(&event.payload.to_string());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_starts_a_chat_agent_the_app_published_and_talks_to_it() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, mut app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;

    let endpoint = phone.endpoint().await;
    let (client, mut events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let workspace = client.workspace().await.expect("workspace");
    assert_eq!(workspace.projects[0].name, "tmp");
    assert_eq!(workspace.launchers[0].label, "OpenCode");
    assert!(!format!("{workspace:?}").contains("do-not-share"));

    let (agent_id, start) = client
        .start_chat("opencode".into(), "sess-tmp".into(), None)
        .await
        .expect("the phone starts a chat");
    assert!(!start.session_id.is_empty());
    let begun = loop {
        let event = tokio::time::timeout(WAIT, app_events.recv())
            .await
            .expect("the app never heard the chat begin")
            .expect("the app's connection closed");
        if let ClientEvent::Event(Event::ChatBegun { chat }) = event {
            break chat;
        }
    };
    assert_eq!(begun.agent_id, agent_id);
    assert_eq!(begun.launcher.as_deref(), Some("opencode"));
    assert_eq!(begun.permission_mode, "bypass");
    assert_eq!(begun.started_by, Some(phone.id()));
    client
        .acp_prompt(
            agent_id.clone(),
            None,
            "hello from the phone".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    until_said(&mut events, &agent_id, "hello from the phone").await;

    let chats = client.acp_list().await.expect("list chats");
    let chat = chats
        .iter()
        .find(|chat| chat.agent_id == agent_id)
        .expect("the chat");
    assert_eq!(chat.started_by, Some(phone.id()));
    assert_eq!(chat.cwd, std::env::temp_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watching_device_cannot_start_a_chat() {
    let core_key = SecretKey::generate();
    let watcher = Device::new("Watcher", DeviceAccess::Watch);
    let core = start_core(&core_key, &[&watcher]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;

    let endpoint = watcher.endpoint().await;
    let (client, _events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the watcher connects");
    assert_eq!(
        client.workspace().await.expect("workspace").projects.len(),
        1
    );
    let refused = client
        .start_chat("opencode".into(), "sess-tmp".into(), None)
        .await;
    assert!(refusal(refused).contains("watch"));
    assert!(
        refusal(client.publish_workspace(Vec::new(), Vec::new()).await)
            .contains("only Sikemux on this host")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_sends_a_file_for_a_chat_and_the_agent_is_given_its_path() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let watcher = Device::new("Watcher", DeviceAccess::Watch);
    let core = start_core(&core_key, &[&phone, &watcher]);
    let (app, mut app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, mut events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = client
        .start_chat("opencode".into(), "sess-tmp".into(), None)
        .await
        .expect("the phone starts a chat");
    app.acp_attach(agent_id.clone())
        .await
        .expect("the app watches");

    let picture = vec![0xffu8, 0xd8, 0xff, 0xe0, 1, 2, 3];
    let path = client
        .attach_file(
            agent_id.clone(),
            "../../IMG_0042".into(),
            "image/jpeg".into(),
            &picture,
        )
        .await
        .expect("the phone sends a picture");
    assert!(path.ends_with("pasted/IMG_0042.jpg"), "{}", path.display());
    assert_eq!(std::fs::read(&path).expect("the host keeps it"), picture);
    let again = client
        .attach_file(
            agent_id.clone(),
            "IMG_0042.jpg".into(),
            "image/jpeg".into(),
            b"x",
        )
        .await
        .expect("the same name again");
    assert!(
        again.ends_with("pasted/IMG_0042 (1).jpg"),
        "{}",
        again.display()
    );
    let largest = vec![7u8; MAX_ATTACHMENT_BYTES];
    let kept = client
        .attach_file(agent_id.clone(), "big.bin".into(), String::new(), &largest)
        .await
        .expect("a file at the limit");
    assert_eq!(
        std::fs::metadata(kept).expect("kept").len(),
        largest.len() as u64
    );
    assert!(refusal(
        client
            .attach_file(
                agent_id.clone(),
                "bigger.bin".into(),
                String::new(),
                &[largest, vec![7]].concat()
            )
            .await
    )
    .contains("10 MB"));

    assert!(refusal(
        client
            .attach_file(
                "no-such-chat".into(),
                "a.txt".into(),
                "text/plain".into(),
                b"a"
            )
            .await
    )
    .contains("not running"));
    let watch_endpoint = watcher.endpoint().await;
    let (watching, _watch_events) = remote::connect(&watch_endpoint, core_addr(&status))
        .await
        .expect("the watcher connects");
    assert!(refusal(
        watching
            .attach_file(agent_id.clone(), "a.txt".into(), "text/plain".into(), b"a")
            .await
    )
    .contains("watch"));

    let sent = path.to_string_lossy().into_owned();
    client
        .acp_prompt(
            agent_id.clone(),
            None,
            "look at this".into(),
            vec![sent.clone()],
            Vec::new(),
        )
        .await
        .expect("prompt");
    let prompt = loop {
        let event = tokio::time::timeout(WAIT, app_events.recv())
            .await
            .expect("the app never heard the prompt")
            .expect("the app's connection closed");
        if let ClientEvent::Event(Event::Chat { event, .. }) = event {
            if event.kind == ChatEventKind::Prompt {
                break event;
            }
        }
    };
    assert_eq!(prompt.payload["paths"], json!([sent]));
    until_said(&mut events, &agent_id, "look at this").await;
}

fn recent_chat(session_id: &str) -> PublishedRecent {
    PublishedRecent {
        launcher: "opencode".into(),
        provider: "opencode".into(),
        session_id: session_id.into(),
        title: "Fix the login flake".into(),
        cwd: std::env::temp_dir(),
        active_at: 1_700_000_000_000,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_resumes_a_recent_chat_the_app_published() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let watcher = Device::new("Watcher", DeviceAccess::Watch);
    let core = start_core(&core_key, &[&phone, &watcher]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    app.publish_recent(vec![recent_chat("saved-1")])
        .await
        .expect("publish recent");
    let status = listening(&app).await;

    let endpoint = phone.endpoint().await;
    let (client, mut events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let view = until_view(&mut events, |view| !view.recent.is_empty()).await;
    let recent = &view.recent[0];
    assert_eq!(recent.title, "Fix the login flake");
    assert_eq!(recent.project, "sess-tmp");

    let watch_endpoint = watcher.endpoint().await;
    let (watching, _watch_events) = remote::connect(&watch_endpoint, core_addr(&status))
        .await
        .expect("the watcher connects");
    assert!(refusal(watching.resume_chat(recent.id.clone()).await).contains("watch"));
    assert!(refusal(client.resume_chat("opencode:unknown".into()).await).contains("recent"));

    let (agent_id, start) = client
        .resume_chat(recent.id.clone())
        .await
        .expect("the phone resumes the chat");
    assert_eq!(start.session_id, "saved-1");
    let view = until_view(&mut events, |view| view.recent.is_empty()).await;
    assert!(view.chats.iter().any(|chat| chat.agent_id == agent_id));
    assert!(refusal(client.resume_chat(recent.id.clone()).await).contains("already open"));
}

/// The next view the core sends that `wanted` accepts.
async fn until_view(
    events: &mut UnboundedReceiver<ClientEvent>,
    wanted: impl Fn(&DeviceView) -> bool,
) -> DeviceView {
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("timed out waiting for the device's view")
            .expect("the device's connection closed");
        if let ClientEvent::Event(Event::DeviceView { view }) = event {
            if wanted(&view) {
                return view;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watching_device_sees_a_permission_request_it_never_attached_to_and_answers_it() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let watch = Device::new("Watch", DeviceAccess::Watch);
    let core = start_core(&core_key, &[&phone, &watch]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent_asking(&app, "workspace-write").await;
    let status = listening(&app).await;

    let watch_endpoint = watch.endpoint().await;
    let (watcher, mut watcher_events) = remote::connect(&watch_endpoint, core_addr(&status))
        .await
        .expect("the watch connects");
    let phone_endpoint = phone.endpoint().await;
    let (driver, mut driver_events) = remote::connect(&phone_endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = driver
        .start_chat("opencode".into(), "sess-tmp".into(), None)
        .await
        .expect("start");
    driver
        .acp_prompt(agent_id.clone(), None, "ask".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");

    let view = until_view(&mut watcher_events, |view| !view.attentions.is_empty()).await;
    let attention = view.attentions[0].clone();
    let Attention {
        id,
        kind,
        agent_id: waiting,
        request,
        ..
    } = attention;
    assert_eq!(kind, AttentionKind::Permission);
    assert_eq!(waiting, agent_id);
    let listed = watcher.attentions().await.expect("attentions");
    assert_eq!(
        listed
            .iter()
            .map(|attention| attention.id.clone())
            .collect::<Vec<_>>(),
        vec![id.clone()]
    );
    let option = request["options"][0]["optionId"]
        .as_str()
        .expect("an option")
        .to_owned();

    watcher
        .acp_permission_reply(agent_id.clone(), id.clone(), Some(option.clone()))
        .await
        .expect("the watch answers");
    until_view(&mut watcher_events, |view| view.attentions.is_empty()).await;
    assert!(watcher.attentions().await.expect("attentions").is_empty());
    until_said(&mut driver_events, &agent_id, &option).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_is_sent_its_view_on_connecting_and_again_when_it_changes() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Watch);
    let core = start_core(&core_key, &[&phone]);
    let (app, mut app_events) = CoreClient::connect(&core.socket).await.expect("app");
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (_client, mut events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");

    let first = until_view(&mut events, |_| true).await;
    assert!(first.workspace.projects.is_empty());
    publish_fake_agent(&app).await;
    let changed = until_view(&mut events, |view| !view.workspace.projects.is_empty()).await;
    assert_eq!(changed.workspace.launchers[0].id, "opencode");

    tokio::time::sleep(Duration::from_secs(1)).await;
    while let Ok(event) = events.try_recv() {
        assert!(
            !matches!(event, ClientEvent::Event(Event::DeviceView { .. })),
            "nothing changed, so nothing was sent"
        );
    }
    while let Ok(event) = app_events.try_recv() {
        assert!(!matches!(
            event,
            ClientEvent::Event(Event::DeviceView { .. })
        ));
    }
}

/// The chat events the device hears, numbered, until one says `needle`.
async fn numbered_until(
    events: &mut UnboundedReceiver<ClientEvent>,
    needle: &str,
) -> Vec<(u64, ChatEventKind)> {
    let mut heard = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("timed out waiting for the agent")
            .expect("the device's connection closed");
        let ClientEvent::Event(Event::Chat { seq, event, .. }) = event else {
            continue;
        };
        heard.push((seq, event.kind));
        if event.payload.to_string().contains(needle) {
            return heard;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_reconnects_hears_only_what_it_missed_and_detaching_stops_the_events() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (first, mut first_events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = first
        .start_chat("opencode".into(), "sess-tmp".into(), None)
        .await
        .expect("start");
    let ChatAttachment::Live { mark, .. } =
        first.acp_attach(agent_id.clone()).await.expect("attach")
    else {
        panic!("the chat is running");
    };
    first
        .acp_prompt(
            agent_id.clone(),
            None,
            "say one".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    let heard = numbered_until(&mut first_events, "one").await;
    let seqs: Vec<u64> = heard.iter().map(|(seq, _)| *seq).collect();
    assert!(
        seqs.windows(2).all(|pair| pair[1] > pair[0]),
        "events are numbered in the order they are sent"
    );
    assert!(
        heard
            .iter()
            .any(|(seq, kind)| *seq > mark.seq && *kind == ChatEventKind::TurnStarted),
        "the prompt's turn follows the attach answer's mark"
    );
    let seen = sikemux_core::protocol::ChatMark {
        feed: mark.feed.clone(),
        seq: *seqs.last().unwrap(),
    };
    first
        .acp_prompt(
            agent_id.clone(),
            None,
            "say four".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    numbered_until(&mut first_events, "four").await;
    drop(first);
    drop(first_events);
    tokio::time::sleep(Duration::from_millis(300)).await;

    app.acp_prompt(
        agent_id.clone(),
        None,
        "say two".into(),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("the app prompts while the phone is away");
    let (second, mut second_events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone reconnects");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let ChatAttachment::Resumed { events, mark } = second
        .acp_attach_since(agent_id.clone(), Some(seen.clone()))
        .await
        .expect("resume")
    else {
        panic!("the core still had what the phone missed");
    };
    assert!(mark.seq > seen.seq);
    assert!(events
        .iter()
        .any(|event| event.kind == ChatEventKind::Prompt && event.payload["text"] == "say two"));
    assert!(
        !events
            .iter()
            .any(|event| event.kind == ChatEventKind::Prompt && event.payload["text"] != "say two"),
        "the phone already shows the prompts it sent"
    );
    assert!(events
        .iter()
        .any(|event| event.payload.to_string().contains("four")));

    let stale = sikemux_core::protocol::ChatMark {
        feed: "another run".into(),
        seq: seen.seq,
    };
    assert!(matches!(
        second
            .acp_attach_since(agent_id.clone(), Some(stale))
            .await
            .expect("attach"),
        ChatAttachment::Live { .. }
    ));

    second.acp_detach(agent_id.clone()).await.expect("detach");
    while second_events.try_recv().is_ok() {}
    app.acp_prompt(
        agent_id.clone(),
        None,
        "say three".into(),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("prompt");
    tokio::time::sleep(Duration::from_secs(1)).await;
    while let Ok(event) = second_events.try_recv() {
        assert!(
            !matches!(event, ClientEvent::Event(Event::Chat { .. })),
            "a detached device hears nothing of the chat"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_holding_a_mark_from_before_the_chat_restarted_is_sent_the_new_run_whole() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, mut events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    app.acp_start(fake_launch("agent-again"))
        .await
        .expect("the app starts a chat");
    let ChatAttachment::Live { mark: before, .. } = client
        .acp_attach("agent-again".into())
        .await
        .expect("attach")
    else {
        panic!("the chat is running");
    };
    client
        .acp_prompt(
            "agent-again".into(),
            None,
            "first run".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    let heard = numbered_until(&mut events, "first run").await;
    let seen = sikemux_core::protocol::ChatMark {
        feed: before.feed.clone(),
        seq: heard.last().map_or(before.seq, |(seq, _)| *seq),
    };

    app.acp_stop("agent-again".into()).await.expect("stop");
    app.acp_start(fake_launch("agent-again"))
        .await
        .expect("the app starts it again");
    let ChatAttachment::Live { mark, replay, .. } = client
        .acp_attach_since("agent-again".into(), Some(seen))
        .await
        .expect("attach")
    else {
        panic!("a mark from the run before is answered with the whole new run");
    };
    assert_ne!(mark.feed, before.feed);
    assert!(!replay
        .iter()
        .any(|event| event.payload.to_string().contains("first run")));
}
