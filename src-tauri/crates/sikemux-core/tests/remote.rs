#![cfg(unix)]

mod folded;

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use folded::{prompts, Chat, Folded};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr};
use serde_json::json;
use sikemux_core::client::{probe, ClientError, ClientEvent, CoreClient};
use sikemux_core::grid::{decode_frame, Screen};
use sikemux_core::protocol::{
    Attention, AttentionKind, BackdropImage, BuildIdentity, ChatAttachment, ChatLaunch, ChatMark,
    ChatState, DeviceAccess, DeviceView, Event, LaunchIdentity, RemoteStatus, SessionId,
    SpawnTarget, TerminalSpawn, MAX_ATTACHMENT_BYTES,
};
use sikemux_core::remote::{self, SecretKey};
use sikemux_core::server::{self, ServerConfig, ServerError};
use sikemux_wire::chat::{ChatChange, ChatPage, ChatRole};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(30);

fn init_env() {
    static ENV: Once = Once::new();
    ENV.call_once(|| {
        stand_in_ssh();
        std::env::set_var("SHELL", "/bin/sh");
        std::env::set_var("PS1", "$ ");
        std::env::remove_var("ENV");
        std::env::remove_var("SIKEMUX_SHELL");
    });
}

/// Puts an `ssh` that only echoes what is typed first on the PATH the core's
/// terminals get, so no test reaches a real host.
fn stand_in_ssh() {
    use std::os::unix::fs::PermissionsExt;
    let bin = std::env::temp_dir().join(format!("sikemux-remote-test-{}", std::process::id()));
    std::fs::create_dir_all(&bin).expect("a folder for the stand-in ssh");
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\nexec cat\n").expect("the stand-in ssh");
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755))
        .expect("the stand-in ssh runs");
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{path}", bin.display()));
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
    server::trust_remote_file(&socket, core_key).expect("host key");
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

/// Attaches to the terminal and waits until its screen shows `needle`.
async fn until_output(
    client: &CoreClient,
    events: &mut UnboundedReceiver<ClientEvent>,
    id: SessionId,
    needle: &str,
) {
    let attached = client.attach(id).await.expect("the device attaches");
    let mut screen = Screen::default();
    screen.apply(decode_frame(&attached.frame).expect("a frame"));
    client.ack(attached.stream, screen.header.frame);
    while !screen.text().contains(needle) {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("timed out waiting for output")
            .expect("the device's connection closed");
        if let ClientEvent::Grid { stream, frame } = event {
            if stream == attached.stream {
                screen.apply(decode_frame(&frame).expect("a frame"));
                client.ack(stream, screen.header.frame);
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

/// A host's set-up terminal shows the sign-in code the core approves for the
/// account, and echoes what is typed into it, so a device that could type
/// there could print a code of its own and get its host onto the account.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_app_types_into_a_setup_terminal() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (device, _device_events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let locked = |result: Result<(), ClientError>| {
        result.is_err_and(|error| error.to_string().contains("setting a host up"))
    };

    let started = app
        .call(
            "installHost",
            json!({ "alias": "build-box", "work": "start" }),
        )
        .await
        .expect("the app sets a host up");
    let id = started["terminal"].as_u64().expect("its terminal");
    let fake = b"  and enter the code BCDF-GHJK, or enter it in Sikemux\n";
    assert!(
        locked(device.write(id, fake).await),
        "a device types into it"
    );
    let drafted = device
        .call(
            "typeWhenReady",
            json!({ "id": id, "draft": "enter the code BCDF-GHJK" }),
        )
        .await;
    assert!(drafted.is_err_and(|error| error.to_string().contains("setting a host up")));
    app.write(id, b"yes\n")
        .await
        .expect("the app types into it");
    assert!(
        locked(device.write(id, fake).await),
        "the lock outlives the app's typing"
    );

    app.kill(id).await.expect("the set-up terminal ends");
    let after = device.write(id, fake).await;
    assert!(
        !locked(after),
        "an ended set-up terminal still reads as locked"
    );
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
    let accounts = json!({
        "profiles": [{
            "id": "fake",
            "name": "Fake",
            "provider": "opencode",
            "accent": "#a277ff",
            "executablePath": FAKE_AGENT,
        }],
    });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("the accounts");
    let project = json!({ "id": "sess-tmp", "name": "tmp", "path": std::env::temp_dir() });
    app.call("addProject", project).await.expect("the project");
    app.call("setAgentDefaults", json!({ "permissionMode": "bypass" }))
        .await
        .expect("the defaults");
}

/// Keeps an agent in the workspace, as the app does when it opens one.
async fn keep_agent(app: &CoreClient, agent_id: &str, title: &str, asleep: bool) {
    let agent = json!({
        "id": agent_id,
        "provider": "opencode",
        "project": "sess-tmp",
        "cwd": std::env::temp_dir(),
        "title": title,
        "renamed": true,
        "permissionMode": "bypass",
        "profileId": "fake",
        "asleep": asleep,
    });
    app.call("createAgent", json!({ "agent": agent }))
        .await
        .expect("the agent");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_reaches_everyone_watching_and_stays_in_the_replay() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = client
        .start_chat("opencode:fake".into(), "sess-tmp".into(), None)
        .await
        .expect("the phone starts a chat");
    let mut watching = Chat::new(app_events, &agent_id);
    watching.attached(
        &app.acp_attach(agent_id.clone())
            .await
            .expect("the app watches"),
    );
    let mut sending = Chat::new(events, &agent_id);

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
    watching
        .until(|chat| chat.prompts() == ["from the phone"])
        .await;

    sending.until_said("echo: from the phone").await;
    assert_eq!(
        sending.prompts(),
        ["from the phone"],
        "the sender is sent its prompt as a row too"
    );

    let mut replayed = Folded::default();
    replayed.attached(&app.acp_attach(agent_id).await.expect("attach"));
    assert_eq!(replayed.prompts(), ["from the phone"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_is_sent_a_long_chat_s_last_turns_and_pages_back_to_its_start() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Watch);
    let core = start_core_with(&core_key, &[&phone], true);
    let (app, app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let start = app
        .acp_start(fake_launch("agent-long"))
        .await
        .expect("the app starts a chat");
    assert!(!start.session_id.is_empty());
    let mut chat = Chat::new(app_events, "agent-long");
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
        chat.until_turns_ended(n + 1).await;
    }
    let turns = |range: std::ops::Range<usize>| -> Vec<String> {
        range.map(|n| format!("turn {n}")).collect()
    };
    let page = |page: serde_json::Value| -> ChatPage {
        serde_json::from_value(page).expect("a chat page")
    };

    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, _events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let attachment = client
        .acp_attach("agent-long".into())
        .await
        .expect("attach");
    let ChatAttachment::Live { mark, .. } = &attachment else {
        panic!("the phone is sent the chat as it stands");
    };
    let mut shown = Folded::default();
    shown.attached(&attachment);
    let before = shown
        .view
        .older_before
        .expect("the phone is sent the chat's end, with more before it");
    let opened = shown.prompts();
    assert!(!opened.is_empty() && opened.len() < 55, "{opened:?}");
    assert_eq!(opened, turns(55 - opened.len()..55));
    assert_eq!(shown.view.rows[0].role, ChatRole::User);

    let mut paged = opened;
    let mut older = Some(before);
    while let Some(from) = older {
        let earlier = page(
            client
                .acp_history("agent-long".into(), mark.feed.clone(), from, 3)
                .await
                .expect("a watching phone reads the history"),
        );
        let found = prompts(&earlier.rows);
        assert_eq!(found.len(), 3.min(55 - paged.len()), "{found:?}");
        paged.splice(0..0, found);
        older = earlier.older_before;
    }
    assert_eq!(paged, turns(0..55));

    let mut taken = Folded::default();
    taken.attached(&app.acp_attach("agent-long".into()).await.expect("attach"));
    assert_eq!(
        taken.drawn(),
        shown.drawn(),
        "the app opens the chat at the same end the phone does"
    );
    assert_eq!(taken.view.older_before, Some(before));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_lists_the_app_s_chats_and_wakes_a_sleeping_one() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    app.register_window()
        .await
        .expect("the app opens its window");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, _events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = client
        .start_chat("opencode:fake".into(), "sess-tmp".into(), None)
        .await
        .expect("the phone starts a chat");

    app.call(
        "updateAgent",
        json!({ "agentId": agent_id, "patch": { "title": "Fix the replay test", "renamed": true } }),
    )
    .await
    .expect("rename the chat");
    keep_agent(&app, "agent-sleepy", "Tidy the docs", true).await;
    keep_agent(&app, "agent-failed", "Try Codex", false).await;
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

    client
        .acp_wake("agent-sleepy".into())
        .await
        .expect("the core wakes the chat itself");
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

    let workspace = client.workspace().await.expect("workspace");
    assert_eq!(
        workspace.palette.get("ground").map(String::as_str),
        Some("#0f0f13"),
        "a device draws in the core's own colours for the person's theme"
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
    let refused = client.publish_on_screen(Vec::new(), true).await;
    assert!(refusal(refused).contains("only Sikemux on this host"));
    let refused = client.watch_view().await;
    assert!(refusal(refused).contains("only Sikemux on this host"));
    let refused = client.acp_wake("agent-gone".into()).await;
    assert!(refusal(refused).contains("no longer on this host"));
}

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_sikemux-fake-claude");

/// The chat a window just opened sees, once it holds `text`.
async fn shown_with(core: &TestCore, agent_id: &str, text: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let (window, _events) = CoreClient::connect(&core.socket).await.expect("window");
        if let Ok(ChatAttachment::Live { view, .. }) = window.acp_attach(agent_id.into()).await {
            if view.to_string().contains(text) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn ready_after_writing(app: &CoreClient, agent_id: &str) -> bool {
    app.call("composing", json!({ "agentId": agent_id }))
        .await
        .expect("the person begins writing");
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let chats = app.acp_list().await.expect("list chats");
        if chats
            .iter()
            .any(|chat| chat.agent_id == agent_id && chat.state == ChatState::Ready)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// A sleeping chat shown in a window shows its history, from the record the core kept or
/// else from Claude's saved transcript, and the person's first keystroke wakes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_sleeping_chat_shows_its_history_and_wakes_when_the_person_writes() {
    let claude_config = tempfile::tempdir().expect("claude config");
    let accounts = json!({ "profiles": [
        { "id": "fake", "name": "Fake", "provider": "opencode", "accent": "#a277ff",
          "executablePath": FAKE_AGENT },
        { "id": "claude", "name": "Claude", "provider": "claude", "accent": "#d97757",
          "executablePath": FAKE_CLAUDE, "configPath": claude_config.path() },
    ] });
    let open = |core: &TestCore| {
        let (socket, accounts) = (core.socket.clone(), accounts.clone());
        async move {
            let (app, events) = CoreClient::connect(&socket).await.expect("app");
            app.register_window().await.expect("the window");
            app.call("setAgentAccounts", accounts)
                .await
                .expect("accounts");
            let project = json!({ "id": "sess-tmp", "name": "tmp", "path": std::env::temp_dir() });
            app.call("addProject", project).await.expect("project");
            (app, events)
        }
    };
    let core = start_core(&SecretKey::generate(), &[]);
    let (app, _events) = open(&core).await;

    let (acp, _) = app
        .start_chat("opencode:fake".into(), "sess-tmp".into(), None)
        .await
        .expect("a chat");
    app.acp_prompt(
        acp.clone(),
        None,
        "hello acp".into(),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("a message");
    assert!(shown_with(&core, &acp, "echo").await, "the agent answered");
    app.call("sleepAgent", json!({ "agentId": acp }))
        .await
        .expect("the chat sleeps");
    assert!(
        shown_with(&core, &acp, "hello acp").await,
        "the record it kept shows"
    );
    assert!(ready_after_writing(&app, &acp).await, "writing wakes it");

    let (claude, session) = {
        let elsewhere = start_core(&SecretKey::generate(), &[]);
        let (other, _events) = open(&elsewhere).await;
        let (claude, _) = other
            .start_chat("claude:claude".into(), "sess-tmp".into(), None)
            .await
            .expect("a Claude chat");
        other
            .acp_prompt(
                claude.clone(),
                None,
                "hello claude".into(),
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("a message");
        assert!(
            shown_with(&elsewhere, &claude, "echo").await,
            "Claude answered"
        );
        let session = other
            .workspace()
            .await
            .expect("workspace")
            .agents
            .into_iter()
            .find(|agent| agent.id == claude)
            .and_then(|agent| agent.resume_id)
            .expect("its session");
        (claude, session)
    };
    let asleep = json!({ "id": claude, "provider": "claude", "project": "sess-tmp",
        "cwd": std::env::temp_dir(), "title": "Claude", "renamed": true, "permissionMode": "bypass",
        "profileId": "claude", "resumeId": session, "asleep": true });
    app.call("createAgent", json!({ "agent": asleep }))
        .await
        .expect("a sleeping Claude chat this core has no record of");
    assert!(
        shown_with(&core, &claude, "hello claude").await,
        "its saved transcript shows"
    );
    assert!(ready_after_writing(&app, &claude).await, "writing wakes it");
}

/// A window can ask to close a tab again before it hears the first close, as with a
/// second click on its ×; the tab is closed already, so that is no failure.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_tab_already_closed_does_nothing() {
    let core_key = SecretKey::generate();
    let core = start_core(&core_key, &[]);
    let (app, _app_events) = CoreClient::connect(&core.socket).await.expect("app");
    app.register_window()
        .await
        .expect("the app opens its window");
    publish_fake_agent(&app).await;
    let file = std::env::temp_dir().join("closing-twice.txt");
    std::fs::write(&file, "hi").expect("the file");
    let tab = app
        .call(
            "openTab",
            json!({ "project": "sess-tmp", "kind": "files", "target": file }),
        )
        .await
        .expect("the tab opens");
    let tab = tab.as_str().expect("its id").to_owned();
    for _ in 0..2 {
        app.call("closeTab", json!({ "project": "sess-tmp", "tab": tab }))
            .await
            .expect("closing it, once or again");
    }
    app.call("closeTabs", json!({ "project": "sess-tmp", "tabs": [tab] }))
        .await
        .expect("closing it among others");
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

#[tokio::test(flavor = "multi_thread")]
async fn a_device_starts_a_chat_agent_the_app_published_and_talks_to_it() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let core = start_core(&core_key, &[&phone]);
    let (app, mut app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;

    let endpoint = phone.endpoint().await;
    let (client, events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let workspace = client.workspace().await.expect("workspace");
    assert_eq!(workspace.projects[0].name, "tmp");
    let fake = workspace
        .launchers
        .iter()
        .find(|launcher| launcher.id == "opencode:fake");
    assert_eq!(
        fake.map(|launcher| launcher.label.as_str()),
        Some("OpenCode")
    );
    assert!(!format!("{workspace:?}").contains(FAKE_AGENT));

    let (agent_id, start) = client
        .start_chat("opencode:fake".into(), "sess-tmp".into(), None)
        .await
        .expect("the phone starts a chat");
    // Its agent starts with the first message, so the chat has no session yet.
    assert!(start.session_id.is_empty());
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
    assert_eq!(begun.launcher.as_deref(), Some("opencode:fake"));
    assert_eq!(begun.permission_mode, "workspace-write");
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
    Chat::new(events, &agent_id)
        .until_said("echo: hello from the phone")
        .await;

    let chats = client.acp_list().await.expect("list chats");
    let chat = chats
        .iter()
        .find(|chat| chat.agent_id == agent_id)
        .expect("the chat");
    assert_eq!(chat.started_by, Some(phone.id()));
    assert_eq!(
        chat.cwd,
        std::fs::canonicalize(std::env::temp_dir()).expect("the temp folder")
    );
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
        .start_chat("opencode:fake".into(), "sess-tmp".into(), None)
        .await;
    assert!(refusal(refused).contains("watch"));
    assert!(refusal(
        client
            .call("setAgentDefaults", json!({ "permissionMode": "read-only" }))
            .await
    )
    .contains("only a device on this host's account"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_sends_a_file_for_a_chat_and_the_agent_is_given_its_path() {
    let core_key = SecretKey::generate();
    let phone = Device::new("Phone", DeviceAccess::Full);
    let watcher = Device::new("Watcher", DeviceAccess::Watch);
    let core = start_core(&core_key, &[&phone, &watcher]);
    let (app, app_events) = CoreClient::connect(&core.socket).await.expect("app");
    publish_fake_agent(&app).await;
    let status = listening(&app).await;
    let endpoint = phone.endpoint().await;
    let (client, events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = client
        .start_chat("opencode:fake".into(), "sess-tmp".into(), None)
        .await
        .expect("the phone starts a chat");
    let mut app_chat = Chat::new(app_events, &agent_id);
    app_chat.attached(
        &app.acp_attach(agent_id.clone())
            .await
            .expect("the app watches"),
    );

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
    let prompted = app_chat
        .until(|chat| !chat.prompts().is_empty())
        .await
        .view
        .rows
        .iter()
        .find(|row| row.role == ChatRole::User)
        .expect("the prompt")
        .clone();
    assert_eq!(prompted.attachments, [sent]);
    Chat::new(events, &agent_id)
        .until_said("look at this")
        .await;
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
                return *view;
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
    publish_fake_agent(&app).await;
    let status = listening(&app).await;

    let watch_endpoint = watch.endpoint().await;
    let (watcher, mut watcher_events) = remote::connect(&watch_endpoint, core_addr(&status))
        .await
        .expect("the watch connects");
    let phone_endpoint = phone.endpoint().await;
    let (driver, driver_events) = remote::connect(&phone_endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = driver
        .start_chat("opencode:fake".into(), "sess-tmp".into(), None)
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
    Chat::new(driver_events, &agent_id)
        .until_said(&option)
        .await;
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
    assert!(changed
        .workspace
        .launchers
        .iter()
        .any(|launcher| launcher.id == "opencode:fake"));

    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut last = changed;
    while let Ok(event) = events.try_recv() {
        if let ClientEvent::Event(Event::DeviceView { view }) = event {
            assert_ne!(*view, last, "nothing changed, so nothing was sent");
            last = *view;
        }
    }
    while let Ok(event) = app_events.try_recv() {
        assert!(!matches!(
            event,
            ClientEvent::Event(Event::DeviceView { .. })
        ));
    }
}

/// The numbers of the chat frames `chat` hears until its agent has said
/// `needle` and the turn has ended.
async fn numbered_until(chat: &mut Chat, needle: &str) -> Vec<u64> {
    let mut heard = Vec::new();
    while !chat.text().contains(needle) || chat.head().running {
        heard.push(chat.next_frame().await);
    }
    heard
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
    let (first, first_events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    let (agent_id, _) = first
        .start_chat("opencode:fake".into(), "sess-tmp".into(), None)
        .await
        .expect("start");
    app.acp_attach(agent_id.clone())
        .await
        .expect("the app watches, which keeps the chat folded");
    let mut chat = Chat::new(first_events, &agent_id);
    let attachment = first.acp_attach(agent_id.clone()).await.expect("attach");
    let ChatAttachment::Live { mark, .. } = &attachment else {
        panic!("the chat is running");
    };
    let mark = mark.clone();
    chat.attached(&attachment);
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
    let seqs = numbered_until(&mut chat, "echo: say one").await;
    assert!(
        seqs.windows(2).all(|pair| pair[1] > pair[0]),
        "changes are numbered in the order they are sent"
    );
    assert!(
        seqs.iter().all(|seq| *seq > mark.seq),
        "the prompt's turn follows the attach answer's mark"
    );
    let seen = ChatMark {
        feed: mark.feed.clone(),
        seq: *seqs.last().unwrap(),
    };
    let held = chat.folded.clone();
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
    numbered_until(&mut chat, "echo: say four").await;
    drop(first);
    drop(chat);
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
    let resumed = second
        .acp_attach_since(agent_id.clone(), Some(seen.clone()))
        .await
        .expect("resume");
    let ChatAttachment::Resumed { changes, mark } = &resumed else {
        panic!("the core still had what the phone missed");
    };
    assert!(mark.seq > seen.seq);
    let changes: Vec<ChatChange> =
        serde_json::from_value(changes.clone()).expect("a list of chat changes");
    let sent: Vec<String> = changes
        .iter()
        .filter_map(|change| match change {
            ChatChange::Row { row, .. } if row.role == ChatRole::User => {
                Some(prompts(std::slice::from_ref(row)).concat())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        ["say four", "say two"],
        "the phone is sent only the rows it missed"
    );
    let mut caught_up = held;
    caught_up.attached(&resumed);
    assert_eq!(caught_up.prompts(), ["say one", "say four", "say two"]);
    let text = caught_up.text();
    assert!(text.contains("echo: say four"), "{text}");
    assert!(text.contains("echo: say two"), "{text}");

    let stale = ChatMark {
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
    let (client, events) = remote::connect(&endpoint, core_addr(&status))
        .await
        .expect("the phone connects");
    app.acp_start(fake_launch("agent-again"))
        .await
        .expect("the app starts a chat");
    let mut chat = Chat::new(events, "agent-again");
    let attachment = client
        .acp_attach("agent-again".into())
        .await
        .expect("attach");
    let ChatAttachment::Live { mark: before, .. } = &attachment else {
        panic!("the chat is running");
    };
    let before = before.clone();
    chat.attached(&attachment);
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
    let heard = numbered_until(&mut chat, "first run").await;
    let seen = ChatMark {
        feed: before.feed.clone(),
        seq: heard.last().copied().unwrap_or(before.seq),
    };

    app.acp_stop("agent-again".into()).await.expect("stop");
    app.acp_start(fake_launch("agent-again"))
        .await
        .expect("the app starts it again");
    let ChatAttachment::Live { mark, view } = client
        .acp_attach_since("agent-again".into(), Some(seen))
        .await
        .expect("attach")
    else {
        panic!("a mark from the run before is answered with the whole new run");
    };
    assert_ne!(mark.feed, before.feed);
    assert!(!view.to_string().contains("first run"), "{view}");
}
