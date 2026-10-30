#![cfg(unix)]

//! A core replacing itself with another binary in the same process. These run
//! the standalone core binary, since the core under test has to `exec`.

mod folded;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use folded::Folded;
use serde_json::{json, Value};
use sikemux_core::cli::auth::new_nonce;
use sikemux_core::cli::protocol::{
    CliClientCommand, CliClientHello, CliEndpointDescriptor, CliServerResponse, HarnessRequest,
    CLI_PROTOCOL_VERSION,
};
use sikemux_core::client::{
    await_deferred_upgrade, await_upgrade, frozen_request, probe, ClientEvent, CoreClient,
};
use sikemux_core::frozen::{FrozenReply, FrozenRequest};
use sikemux_core::grid::{decode_frame, Screen};
use sikemux_core::protocol::{
    BuildIdentity, ChatAttachment, ChatLaunch, ChatMark, Event, LaunchIdentity, PermissionMode,
    SessionId, SpawnTarget, StreamId, TerminalSpawn, BUILD_ID_OVERRIDE_ENV,
};
use sikemux_pty::output_log::OutputQuery;
use sikemux_wire::chat::{PermissionKind, StopReason};
use tokio::sync::mpsc::UnboundedReceiver;

const CORE: &str = env!("CARGO_BIN_EXE_sikemux-core");
const WAIT: Duration = Duration::from_secs(30);

struct CoreProcess {
    child: Child,
    dir: tempfile::TempDir,
    socket: PathBuf,
    endpoint: PathBuf,
    project: PathBuf,
}

impl CoreProcess {
    fn start(build: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("core.sock");
        let endpoint = dir.path().join("cli.json");
        let project = std::fs::canonicalize(dir.path())
            .expect("canonical")
            .join("project");
        std::fs::create_dir(&project).expect("project");
        let child = Self::spawn(dir.path(), &socket, &endpoint, build);
        let core = Self {
            child,
            dir,
            socket,
            endpoint,
            project,
        };
        core.until_answering();
        core
    }

    fn spawn(dir: &Path, socket: &Path, endpoint: &Path, build: &str) -> Child {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("core.log"))
            .expect("log");
        sikemux_process::user_environment::command(CORE)
            .arg("core")
            .arg("--socket")
            .arg(socket)
            .arg("--cli-endpoint")
            .arg(endpoint)
            .arg("--data-dir")
            .arg(dir.join("data"))
            .arg("--idle-exit-secs")
            .arg("600")
            .env(BUILD_ID_OVERRIDE_ENV, build)
            .env("SHELL", "/bin/sh")
            .env("PS1", "$ ")
            .env_remove("ENV")
            .env_remove("SIKEMUX_SHELL")
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("log"))
            .stderr(log)
            .spawn()
            .expect("start the core")
    }

    fn until_answering(&self) {
        let deadline = Instant::now() + WAIT;
        while probe(&self.socket, Duration::from_secs(1)).is_err() || !self.endpoint.exists() {
            assert!(Instant::now() < deadline, "the core never answered");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Kills the core with no warning, as a crash does, and starts another on
    /// the same socket and data folder, as launchd or the app would.
    fn crash_and_start_again(&mut self) {
        self.child.kill().expect("kill the core");
        self.child.wait().expect("reap the core");
        self.child = Self::spawn(self.dir.path(), &self.socket, &self.endpoint, "first");
        self.until_answering();
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("core.log")).unwrap_or_default()
    }

    /// A script that runs the same core binary as another build, after
    /// `before`, which sees the core's arguments.
    fn binary(&self, name: &str, build: &str, before: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n{before}\n{BUILD_ID_OVERRIDE_ENV}='{build}' exec '{CORE}' \"$@\"\n"
            ),
        )
        .expect("write the binary");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    async fn frozen(&self, request: FrozenRequest) -> FrozenReply {
        let socket = self.socket.clone();
        tokio::task::spawn_blocking(move || frozen_request(&socket, &request, WAIT))
            .await
            .expect("join")
            .expect("the core answered")
    }

    async fn upgrade(&self, binary: &Path, from: &str) -> BuildIdentity {
        let reply = self
            .frozen(FrozenRequest::Upgrade {
                binary: binary.to_path_buf(),
                force: false,
            })
            .await;
        assert_eq!(reply, FrozenReply::Accepted, "{}", self.log());
        let socket = self.socket.clone();
        let pid = self.pid();
        let old = BuildIdentity {
            version: env!("CARGO_PKG_VERSION").into(),
            commit: from.into(),
            built_at: 0,
            source: from.into(),
        };
        let hello =
            tokio::task::spawn_blocking(move || await_upgrade(&socket, pid, Some(&old), WAIT))
                .await
                .expect("join")
                .unwrap_or_else(|error| panic!("{error}\n{}", self.log()));
        assert_eq!(hello.pid, self.pid());
        hello.build
    }
}

impl Drop for CoreProcess {
    fn drop(&mut self) {
        let _ = frozen_request(
            &self.socket,
            &FrozenRequest::StopEverything,
            Duration::from_secs(5),
        );
        let deadline = Instant::now() + WAIT;
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

struct Stream {
    events: UnboundedReceiver<ClientEvent>,
    /// What each attached session shows now.
    output: HashMap<SessionId, Vec<u8>>,
    screens: HashMap<StreamId, (SessionId, Screen)>,
    exits: HashMap<SessionId, Option<u32>>,
}

fn screen_of(frame: &[u8]) -> Screen {
    let mut screen = Screen::default();
    screen.apply(decode_frame(frame).expect("a frame"));
    screen
}

impl Stream {
    async fn attach(&mut self, client: &CoreClient, id: SessionId) {
        let attached = client.attach(id).await.expect("attach");
        let screen = screen_of(&attached.frame);
        client.ack(attached.stream, screen.header.frame);
        self.output.insert(id, screen.text().into_bytes());
        self.screens.insert(attached.stream, (id, screen));
    }

    async fn pump(&mut self, client: &CoreClient) {
        let event = tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("timed out waiting for the core")
            .expect("the core disconnected");
        match event {
            ClientEvent::Grid { stream, frame } => {
                if let Some((id, screen)) = self.screens.get_mut(&stream) {
                    screen.apply(decode_frame(&frame).expect("a frame"));
                    client.ack(stream, screen.header.frame);
                    self.output.insert(*id, screen.text().into_bytes());
                }
            }
            ClientEvent::Event(Event::Exited { id, code, .. }) => {
                self.exits.insert(id, code);
            }
            _ => {}
        }
    }

    async fn until_output(&mut self, client: &CoreClient, id: SessionId, needle: &str) {
        while !String::from_utf8_lossy(self.output.get(&id).map(Vec::as_slice).unwrap_or_default())
            .contains(needle)
        {
            self.pump(client).await;
        }
    }

    async fn until_exit(&mut self, client: &CoreClient, id: SessionId) -> Option<u32> {
        loop {
            if let Some(code) = self.exits.get(&id) {
                return *code;
            }
            self.pump(client).await;
        }
    }
}

async fn connect(core: &CoreProcess) -> (CoreClient, Stream) {
    let (client, events) = CoreClient::connect(&core.socket).await.expect("connect");
    (
        client,
        Stream {
            events,
            output: HashMap::new(),
            screens: HashMap::new(),
            exits: HashMap::new(),
        },
    )
}

async fn spawn_shell(client: &CoreClient) -> SessionId {
    client
        .spawn(
            LaunchIdentity::default(),
            SpawnTarget::Terminal(TerminalSpawn {
                cols: 80,
                rows: 24,
                ..TerminalSpawn::default()
            }),
        )
        .await
        .expect("spawn a shell")
}

fn descriptor(endpoint: &Path) -> CliEndpointDescriptor {
    serde_json::from_slice(&std::fs::read(endpoint).expect("endpoint file")).expect("endpoint")
}

fn send(reader: &mut BufReader<TcpStream>, message: &impl serde::Serialize) {
    let mut line = serde_json::to_vec(message).expect("encode");
    line.push(b'\n');
    reader.get_mut().write_all(&line).expect("send");
}

fn receive(reader: &mut BufReader<TcpStream>) -> CliServerResponse {
    let mut line = Vec::new();
    reader
        .by_ref()
        .take(8 * 1024 * 1024)
        .read_until(b'\n', &mut line)
        .expect("receive");
    serde_json::from_slice(&line).expect("readable answer")
}

fn tool(endpoint: &Path, project: &Path, method: &str, params: Value) -> Result<Value, String> {
    let descriptor = descriptor(endpoint);
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, descriptor.port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(70)))
        .expect("timeout");
    let mut reader = BufReader::new(stream);
    send(
        &mut reader,
        &CliClientHello::Hello {
            protocol: CLI_PROTOCOL_VERSION,
            nonce: new_nonce(),
        },
    );
    assert!(matches!(
        receive(&mut reader),
        CliServerResponse::Hello { .. }
    ));
    send(
        &mut reader,
        &CliClientCommand::Harness {
            protocol: CLI_PROTOCOL_VERSION,
            token: descriptor.token,
            request: HarnessRequest {
                id: uuid::Uuid::new_v4().to_string(),
                project: project.to_string_lossy().into_owned(),
                agent_id: Some("agent-1".into()),
                method: method.into(),
                params,
            },
        },
    );
    match receive(&mut reader) {
        CliServerResponse::Result { value } => Ok(value),
        CliServerResponse::Error { message } => Err(message),
        other => panic!("unexpected answer: {other:?}"),
    }
}

async fn call(core: &CoreProcess, method: &str, params: Value) -> Result<Value, String> {
    let (endpoint, project, method) = (
        core.endpoint.clone(),
        core.project.clone(),
        method.to_owned(),
    );
    tokio::task::spawn_blocking(move || tool(&endpoint, &project, &method, params))
        .await
        .expect("tool call")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_keeps_every_session_and_the_harness() {
    let core = CoreProcess::start("first");
    let pid = core.pid();

    let (client, mut stream) = connect(&core).await;
    let shell = spawn_shell(&client).await;
    stream.attach(&client, shell).await;
    client
        .write(shell, b"echo before-$((6*7))\r")
        .await
        .expect("write");
    stream.until_output(&client, shell, "before-42").await;

    let running = call(
        &core,
        "task.start",
        json!({ "command": "printf 'task-out\\n'; sleep 30", "idempotencyKey": "keep", "readyWhen": "task-out" }),
    )
    .await
    .expect("start a task");
    assert_eq!(running["ready"], true, "{running}");
    let running_id = running["executionId"].as_str().expect("id").to_owned();
    let finished = call(
        &core,
        "task.start",
        json!({ "command": "printf 'done-out\\n'; exit 3", "idempotencyKey": "done" }),
    )
    .await
    .expect("start a task that ends");
    let finished_id = finished["executionId"].as_str().expect("id").to_owned();
    let deadline = Instant::now() + WAIT;
    while call(&core, "task.read", json!({ "executionId": finished_id }))
        .await
        .expect("read")["exitCode"]
        != 3
    {
        assert!(Instant::now() < deadline, "the task never ended");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let cursor = call(&core, "workspace.inspect", json!({}))
        .await
        .expect("inspect")["cursor"]
        .as_str()
        .expect("cursor")
        .to_owned();
    let before = client.list().await.expect("list");
    let shell_pid = before
        .iter()
        .find(|session| session.id == shell)
        .and_then(|session| session.pid)
        .expect("shell pid");
    let endpoint_file = std::fs::read(&core.endpoint).expect("endpoint file");

    let build = core
        .upgrade(&core.binary("second-build", "second", ""), "first")
        .await;
    assert_eq!(build.commit, "second");
    assert!(core.child.id() == pid && alive(pid));
    assert!(alive(shell_pid), "the shell died in the update");

    let (client, mut stream) = connect(&core).await;
    let after = client.list().await.expect("list");
    assert_eq!(
        after
            .iter()
            .map(|session| (
                session.id,
                session.pid,
                session.running,
                session.exit.clone()
            ))
            .collect::<Vec<_>>(),
        before
            .iter()
            .map(|session| (
                session.id,
                session.pid,
                session.running,
                session.exit.clone()
            ))
            .collect::<Vec<_>>()
    );
    let attached = client.attach(shell).await.expect("attach after the update");
    let text = screen_of(&attached.frame).text();
    assert!(text.contains("before-42"), "{text:?}");
    client.detach(attached.stream).await.expect("detach");
    stream.attach(&client, shell).await;
    client
        .write(shell, b"echo after-$((6*7))\r")
        .await
        .expect("write");
    stream.until_output(&client, shell, "after-42").await;

    let running_pty = after
        .iter()
        .find(|session| session.task_execution_id.as_deref() == Some(running_id.as_str()))
        .expect("the running task")
        .id;
    let page = client
        .task_output(
            running_pty,
            OutputQuery {
                limit: 8192,
                ..OutputQuery::default()
            },
        )
        .await
        .expect("task output");
    assert!(String::from_utf8_lossy(&page.bytes).contains("task-out"));

    assert_eq!(
        descriptor(&core.endpoint),
        serde_json::from_slice(&endpoint_file).expect("old endpoint")
    );
    assert_eq!(
        std::fs::read(&core.endpoint).expect("endpoint"),
        endpoint_file
    );
    let read = call(
        &core,
        "task.read",
        json!({ "executionId": running_id, "plain": true }),
    )
    .await
    .expect("read after the update");
    assert!(read["output"]
        .as_str()
        .expect("output")
        .contains("task-out"));
    assert_eq!(read["status"], "running");
    let ended = call(
        &core,
        "task.read",
        json!({ "executionId": finished_id, "plain": true }),
    )
    .await
    .expect("read the ended task");
    assert_eq!(ended["exitCode"], 3);
    assert!(ended["output"]
        .as_str()
        .expect("output")
        .contains("done-out"));
    let again = call(
        &core,
        "task.start",
        json!({ "command": "printf 'task-out\\n'; sleep 30", "idempotencyKey": "keep" }),
    )
    .await
    .expect("the key still names its run");
    assert_eq!(again["executionId"], running_id.as_str());
    let events = call(
        &core,
        "events.wait",
        json!({ "cursor": cursor, "timeoutMs": 0 }),
    )
    .await
    .expect("an old cursor still works");
    assert_eq!(events["truncated"], false, "{events}");
    let resumed_at: u64 = events["cursor"]
        .as_str()
        .expect("cursor")
        .parse()
        .expect("number");
    assert!(
        resumed_at >= cursor.parse::<u64>().expect("number"),
        "{events}"
    );
    let history = call(
        &core,
        "events.wait",
        json!({ "cursor": "0", "timeoutMs": 0, "executionId": running_id }),
    )
    .await
    .expect("history");
    assert_eq!(history["events"][1]["kind"], "task.running");

    client.write(shell, b"exit 5\r").await.expect("write");
    assert_eq!(stream.until_exit(&client, shell).await, Some(5));
    assert!(core.log().contains("took over from"), "{}", core.log());
    let prefix = format!("sikemux-core-handover-{pid}-");
    let left: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .expect("temp dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .collect();
    assert!(left.is_empty(), "the hand-over was left behind: {left:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_is_refused_for_a_binary_that_cannot_take_over() {
    let core = CoreProcess::start("first");
    let refusal = |reply: FrozenReply| match reply {
        FrozenReply::Refused { message } => message,
        other => panic!("{other:?}"),
    };
    let plain = core.dir.path().join("plain");
    std::fs::write(&plain, "not a program").expect("write");
    let cases = [
        (plain.clone(), "not an executable"),
        (core.dir.path().join("missing"), "cannot be read"),
        (PathBuf::from("relative/core"), "not an absolute path"),
        (core.binary("same", "first", ""), "already runs"),
        (
            core.binary("chatty", "other", "echo nonsense; exit 0"),
            "unreadable",
        ),
    ];
    for (binary, expected) in cases {
        let message = refusal(
            core.frozen(FrozenRequest::Upgrade {
                binary,
                force: false,
            })
            .await,
        );
        assert!(message.contains(expected), "{message}");
    }
    let hello = probe(&core.socket, Duration::from_secs(1)).expect("still serving");
    assert_eq!(hello.pid, core.pid());
    assert_eq!(hello.build.commit, "first");

    let (client, _stream) = connect(&core).await;
    let shell = spawn_shell(&client).await;
    let shell_pid = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == shell)
        .and_then(|session| session.pid)
        .expect("pid");
    assert_eq!(
        core.frozen(FrozenRequest::StopEverything).await,
        FrozenReply::Accepted
    );
    let deadline = Instant::now() + WAIT;
    while alive(shell_pid) {
        assert!(
            Instant::now() < deadline,
            "stopping everything left the shell"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_hand_over_keeps_the_sessions_it_can_adopt() {
    let core = CoreProcess::start("first");
    let (client, mut stream) = connect(&core).await;
    let shell = spawn_shell(&client).await;
    stream.attach(&client, shell).await;
    client.write(shell, b"echo kept\r").await.expect("write");
    stream.until_output(&client, shell, "kept").await;
    let shell_pid = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == shell)
        .and_then(|session| session.pid)
        .expect("pid");
    let old_token = descriptor(&core.endpoint).token;

    let damaging = core.binary("damaging", "second", "printf garbage > \"$3/state.json\"");
    assert_eq!(core.upgrade(&damaging, "first").await.commit, "second");
    assert!(alive(shell_pid));
    assert!(
        core.log().contains("UPDATE STATE DAMAGED"),
        "{}",
        core.log()
    );

    let (client, mut stream) = connect(&core).await;
    let kept = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == shell)
        .expect("the shell was kept");
    assert_eq!(kept.pid, Some(shell_pid));
    assert!(kept.running);
    stream.attach(&client, shell).await;
    client
        .write(shell, b"echo still-$((2*21))\r")
        .await
        .expect("write");
    stream.until_output(&client, shell, "still-42").await;
    let republished = descriptor(&core.endpoint);
    assert_ne!(republished.token, old_token);
    assert_eq!(republished.pid, core.pid());
}

const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");

fn chat_launch(core: &CoreProcess, agent_id: &str) -> ChatLaunch {
    let history = core.dir.path().join("agent-history");
    std::fs::create_dir_all(&history).expect("history");
    ChatLaunch {
        agent_id: agent_id.into(),
        provider: "opencode".into(),
        cwd: core.project.clone(),
        program: PathBuf::from(FAKE_AGENT),
        args: vec!["acp".into()],
        env: [(
            "FAKE_ACP_DIR".to_owned(),
            history.to_string_lossy().into_owned(),
        )]
        .into(),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: PermissionMode::WorkspaceWrite,
        model: None,
        model_provider: None,
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    }
}

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_sikemux-fake-claude");
const FAKE_CODEX: &str = env!("CARGO_BIN_EXE_sikemux-fake-codex");

/// A chat of `provider`'s stand-in agent, asking before it acts.
fn agent_launch(core: &CoreProcess, provider: &str, agent_id: &str) -> ChatLaunch {
    let home = core.dir.path().join(format!("{provider}-home"));
    std::fs::create_dir_all(&home).expect("home");
    let home = home.to_string_lossy().into_owned();
    let (program, variable) = match provider {
        "claude" => (FAKE_CLAUDE, "CLAUDE_CONFIG_DIR"),
        "codex" => (FAKE_CODEX, "CODEX_HOME"),
        _ => return chat_launch(core, agent_id),
    };
    ChatLaunch {
        provider: provider.into(),
        program: PathBuf::from(program),
        args: Vec::new(),
        env: [(variable.to_owned(), home)].into(),
        ..chat_launch(core, agent_id)
    }
}

/// The stand-in agents `pid` started that are still its children.
fn children(pid: u32) -> Vec<u32> {
    let listed = sikemux_process::user_environment::command("ps")
        .args(["-A", "-o", "pid=,ppid=,comm="])
        .output()
        .expect("ps");
    let mut pids: Vec<u32> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let child = fields.next()?.parse().ok()?;
            let parent: u32 = fields.next()?.parse().ok()?;
            let command = fields.next()?;
            (parent == pid && command.contains("sikemux-fake")).then_some(child)
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// Folds what each of `chats` says until `found` accepts every one of them.
async fn until_chats(
    events: &mut UnboundedReceiver<ClientEvent>,
    chats: &mut HashMap<String, Folded>,
    mut found: impl FnMut(&str, &Folded) -> bool,
) {
    let mut waiting: Vec<String> = chats
        .iter()
        .filter(|(agent_id, chat)| !found(agent_id, chat))
        .map(|(agent_id, _)| agent_id.clone())
        .collect();
    while !waiting.is_empty() {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {waiting:?}"))
            .expect("the core disconnected");
        let ClientEvent::Event(Event::Chat {
            agent_id, changes, ..
        }) = event
        else {
            continue;
        };
        let Some(chat) = chats.get_mut(&agent_id) else {
            continue;
        };
        chat.apply(&changes);
        if found(&agent_id, chat) {
            waiting.retain(|waiting| *waiting != agent_id);
        }
    }
}

/// The chat a client attaching is sent, when it is still running there.
async fn attached(client: &CoreClient, agent_id: &str, since: Option<ChatMark>) -> Option<Folded> {
    let attachment = client
        .acp_attach_since(agent_id.into(), since)
        .await
        .expect("attach");
    if !matches!(attachment, ChatAttachment::Live { .. }) {
        return None;
    }
    let mut chat = Folded::default();
    chat.attached(&attachment);
    Some(chat)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_carries_running_chats_over_with_their_agents() {
    let core = CoreProcess::start("first");
    let (client, mut events) = CoreClient::connect(&core.socket).await.expect("connect");
    let providers = ["opencode", "claude", "codex"];
    let asking: Vec<String> = providers.iter().map(|p| format!("{p}-ask")).collect();
    let holding: Vec<String> = providers.iter().map(|p| format!("{p}-hold")).collect();
    let mut sessions = HashMap::new();
    for provider in providers {
        for agent_id in [format!("{provider}-ask"), format!("{provider}-hold")] {
            let started = client
                .acp_start(agent_launch(&core, provider, &agent_id))
                .await
                .unwrap_or_else(|error| panic!("start {agent_id}: {error}\n{}", core.log()));
            sessions.insert(agent_id, started.session_id);
        }
    }
    for agent_id in &asking {
        client
            .acp_prompt(agent_id.clone(), None, "ask".into(), Vec::new(), Vec::new())
            .await
            .expect("prompt");
    }
    for agent_id in &holding {
        client
            .acp_prompt(
                agent_id.clone(),
                None,
                "hold 8000".into(),
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("prompt");
    }
    let mut asked: HashMap<String, Folded> = asking
        .iter()
        .chain(&holding)
        .map(|agent_id| (agent_id.clone(), Folded::default()))
        .collect();
    until_chats(&mut events, &mut asked, |agent_id, chat| {
        if asking.iter().any(|asking| asking == agent_id) {
            !chat.head().permissions.is_empty()
        } else {
            chat.text().contains("holding")
        }
    })
    .await;
    let agents = children(core.pid());
    assert_eq!(agents.len(), 6, "{agents:?}");
    drop(client);
    drop(events);

    let binary = core.binary("second-build", "second", "");
    let build = core.upgrade(&binary, "first").await;
    assert_eq!(build.commit, "second");
    assert_eq!(children(core.pid()), agents, "{}", core.log());

    let (client, mut events) = CoreClient::connect(&core.socket).await.expect("connect");
    let chats = client.acp_list().await.expect("list");
    assert_eq!(chats.len(), 6, "{}", core.log());
    for chat in &chats {
        assert!(chat.running, "{} stopped in the update", chat.agent_id);
        assert_eq!(
            chat.session_id.as_ref(),
            sessions.get(&chat.agent_id),
            "{} moved to another session",
            chat.agent_id
        );
    }
    let mut carried = HashMap::new();
    for agent_id in asking.iter().chain(&holding) {
        let chat = attached(&client, agent_id, None)
            .await
            .unwrap_or_else(|| panic!("{agent_id} was not carried over\n{}", core.log()));
        assert!(chat.head().running, "{agent_id}");
        if holding.contains(agent_id) {
            assert!(chat.text().contains("holding"), "{}", chat.text());
        } else {
            assert!(
                !chat.head().permissions.is_empty(),
                "{agent_id} lost its question"
            );
        }
        carried.insert(agent_id.clone(), chat);
    }
    for agent_id in &asking {
        let request = &asked[agent_id].head().permissions[0];
        let allow = request
            .options
            .iter()
            .find(|option| option.kind == PermissionKind::AllowOnce)
            .expect("an allow option")
            .option_id
            .clone();
        client
            .acp_permission_reply(agent_id.clone(), request.request_id.clone(), Some(allow))
            .await
            .unwrap_or_else(|error| panic!("{agent_id}: {error}\n{}", core.log()));
    }
    until_chats(&mut events, &mut carried, |_, chat| chat.turns_ended() >= 1).await;
    for (agent_id, chat) in &carried {
        assert_eq!(chat.ends[0], Some(StopReason::EndTurn), "{agent_id}");
    }
    for agent_id in &holding {
        let text = carried[agent_id].text();
        assert!(text.contains("held"), "{agent_id}: {text}");
    }
    for agent_id in &asking {
        let answer = carried[agent_id].text();
        assert!(
            answer.contains("allow"),
            "{agent_id} was not told the answer: {answer}"
        );
    }
    assert_eq!(children(core.pid()), agents);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_core_that_crashes_mid_turn_carries_the_turn_on_when_it_starts_again() {
    let mut core = CoreProcess::start("first");
    let (client, mut events) = CoreClient::connect(&core.socket).await.expect("connect");
    let mut chats = Vec::new();
    for provider in ["opencode", "claude", "codex"] {
        let agent_id = format!("{provider}-crash");
        client
            .acp_start(agent_launch(&core, provider, &agent_id))
            .await
            .unwrap_or_else(|error| panic!("start {agent_id}: {error}\n{}", core.log()));
        chats.push(agent_id);
    }
    for agent_id in &chats {
        client
            .acp_prompt(
                agent_id.clone(),
                None,
                "hold 600000".into(),
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("prompt");
    }
    let mut holding: HashMap<String, Folded> = chats
        .iter()
        .map(|agent_id| (agent_id.clone(), Folded::default()))
        .collect();
    until_chats(&mut events, &mut holding, |_, chat| {
        chat.text().contains("holding")
    })
    .await;
    let mut marks = HashMap::new();
    for agent_id in &chats {
        let ChatAttachment::Live { mark, .. } =
            client.acp_attach(agent_id.clone()).await.expect("attach")
        else {
            panic!("{agent_id} is not running");
        };
        marks.insert(agent_id.clone(), mark);
    }
    drop(client);
    drop(events);
    // A crash loses what the chat said in its last second.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    core.crash_and_start_again();

    let deadline = Instant::now() + WAIT * 2;
    loop {
        let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
        let listed = client.acp_list().await.expect("list");
        let finished = chats.iter().all(|agent_id| {
            listed.iter().any(|chat| {
                chat.agent_id == *agent_id && chat.session_id.is_some() && !chat.running
            })
        });
        if finished {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the turns never finished: {listed:?}\n{}",
            core.log()
        );
        drop(client);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
    for agent_id in &chats {
        let chat = attached(&client, agent_id, marks.remove(agent_id))
            .await
            .unwrap_or_else(|| panic!("{agent_id} was not taken up again\n{}", core.log()));
        let head = chat.head();
        assert!(!head.running, "{agent_id}");
        let text = chat.text();
        assert_eq!(
            text.matches("holding").count(),
            1,
            "{agent_id}: {:#?}\n{}",
            chat.view,
            core.log()
        );
        assert!(text.contains("Sikemux restarted"), "{agent_id}: {text}");
        assert_eq!(
            chat.prompts(),
            ["hold 600000"],
            "{agent_id}: {:#?}",
            chat.view
        );
        assert_eq!(head.stop_reason, Some(StopReason::EndTurn), "{agent_id}");
        assert_eq!(
            chat.notices()
                .iter()
                .filter(|title| *title == "Sikemux restarted while this agent was working")
                .count(),
            1,
            "{agent_id}"
        );
    }
}

/// A chat shown on its saved session, still waiting for the person when its core crashes,
/// waits again once the core is back: nothing starts its agent but the person.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_waiting_for_the_person_still_waits_after_a_crash() {
    let mut core = CoreProcess::start("first");
    let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
    let accounts = json!({
        "profiles": [{
            "id": "fake",
            "name": "Fake",
            "provider": "opencode",
            "accent": "#a277ff",
            "executablePath": FAKE_AGENT,
        }],
    });
    client
        .call("setAgentAccounts", accounts)
        .await
        .expect("the accounts");
    let project = json!({ "id": "held", "name": "held", "path": core.project });
    client
        .call("addProject", project)
        .await
        .expect("the project");
    let agent_id = "agent-held".to_owned();
    let agent = json!({
        "id": agent_id,
        "provider": "opencode",
        "project": "held",
        "cwd": core.project,
        "title": "Saved",
        "renamed": true,
        "permissionMode": "bypass",
        "profileId": "fake",
        "resumeId": "ses-saved",
        "asleep": true,
    });
    client
        .call("createAgent", json!({ "agent": agent }))
        .await
        .expect("the agent");
    assert!(
        matches!(
            client.acp_attach(agent_id.clone()).await,
            Ok(ChatAttachment::Live { .. })
        ),
        "the asleep chat is shown\n{}",
        core.log()
    );
    assert_eq!(
        children(core.pid()),
        Vec::<u32>::new(),
        "showing it started nothing"
    );
    drop(client);
    // A crash loses what the chat wrote down in its last second.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    core.crash_and_start_again();

    let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
    let deadline = Instant::now() + WAIT;
    while !client
        .acp_list()
        .await
        .expect("list")
        .iter()
        .any(|chat| chat.agent_id == agent_id)
    {
        assert!(
            Instant::now() < deadline,
            "the chat was not taken up again\n{}",
            core.log()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        children(core.pid()),
        Vec::<u32>::new(),
        "no agent started\n{}",
        core.log()
    );

    client
        .call("composing", json!({ "agentId": agent_id }))
        .await
        .expect("the person begins writing");
    let deadline = Instant::now() + WAIT;
    while children(core.pid()).is_empty() {
        assert!(
            Instant::now() < deadline,
            "writing started the agent\n{}",
            core.log()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A chat whose agent does not finish starting while `gate` exists, started
/// in the background since starting waits for it.
async fn start_held_chat(core: &CoreProcess, client: &Arc<CoreClient>, gate: &Path) {
    std::fs::write(gate, b"").expect("gate");
    let mut launch = chat_launch(core, "slow");
    launch.env.insert(
        "FAKE_ACP_START_GATE".into(),
        gate.to_string_lossy().into_owned(),
    );
    let starting = client.clone();
    tokio::spawn(async move {
        let _ = starting.acp_start(launch).await;
    });
    let deadline = Instant::now() + WAIT;
    loop {
        let reply = core.frozen(FrozenRequest::UpdateBlockers).await;
        let FrozenReply::UpdateBlockers { chats } = reply else {
            panic!("{reply:?}");
        };
        if chats
            .iter()
            .any(|chat| chat.agent_id == "slow" && chat.reason == "starting")
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the starting chat was never named"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits for a deferred update to finish, and answers with the build the
/// core runs after it.
async fn after_deferred_update(core: &CoreProcess) -> BuildIdentity {
    let socket = core.socket.clone();
    let pid = core.pid();
    let old = BuildIdentity {
        version: env!("CARGO_PKG_VERSION").into(),
        commit: "first".into(),
        built_at: 0,
        source: "first".into(),
    };
    let hello =
        tokio::task::spawn_blocking(move || await_deferred_upgrade(&socket, pid, Some(&old), WAIT))
            .await
            .expect("join")
            .unwrap_or_else(|error| panic!("{error}\n{}", core.log()));
    assert_eq!(hello.pid, core.pid());
    hello.build
}

/// The update notice a chat shows when it was started again rather than
/// carried over.
fn restarted(chat: &Folded) -> bool {
    chat.notices()
        .iter()
        .any(|title| title.contains("Sikemux restarted this agent"))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_waits_for_a_starting_chat_and_then_carries_it_over() {
    let core = CoreProcess::start("first");
    let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
    let client = Arc::new(client);
    let gate = core.dir.path().join("gate");
    start_held_chat(&core, &client, &gate).await;

    let binary = core.binary("second-build", "second", "");
    let reply = core
        .frozen(FrozenRequest::Upgrade {
            binary,
            force: false,
        })
        .await;
    assert!(matches!(reply, FrozenReply::Deferred { .. }), "{reply:?}");
    // Longer than a forced update waits before it stops a chat.
    tokio::time::sleep(Duration::from_secs(12)).await;
    let (still, _) = CoreClient::connect(&core.socket).await.expect("connect");
    assert_eq!(still.hello().build.commit, "first", "{}", core.log());
    drop(still);
    std::fs::remove_file(&gate).expect("open the gate");
    assert_eq!(after_deferred_update(&core).await.commit, "second");

    let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
    let chat = attached(&client, "slow", None)
        .await
        .unwrap_or_else(|| panic!("the chat was lost\n{}", core.log()));
    assert!(!restarted(&chat), "{}", core.log());
    assert!(
        core.log().contains("1 of them still running"),
        "{}",
        core.log()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forced_update_restarts_a_starting_chat_and_says_so() {
    let core = CoreProcess::start("first");
    let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
    let client = Arc::new(client);
    let gate = core.dir.path().join("gate");
    start_held_chat(&core, &client, &gate).await;

    let binary = core.binary("second-build", "second", "");
    let reply = core
        .frozen(FrozenRequest::Upgrade {
            binary,
            force: true,
        })
        .await;
    assert!(matches!(reply, FrozenReply::Deferred { .. }), "{reply:?}");
    assert_eq!(after_deferred_update(&core).await.commit, "second");
    std::fs::remove_file(&gate).expect("open the gate");

    let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
    let deadline = Instant::now() + WAIT;
    loop {
        let chat = attached(&client, "slow", None)
            .await
            .unwrap_or_else(|| panic!("the chat was not started again\n{}", core.log()));
        if restarted(&chat) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the chat never said it was restarted\n{}",
            core.log()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_core_takes_over_from_the_oldest_hand_over_a_stable_release_wrote() {
    let core = CoreProcess::start("first");
    let (client, mut stream) = connect(&core).await;
    let shell = spawn_shell(&client).await;
    stream.attach(&client, shell).await;
    client
        .write(shell, b"echo older-$((3*3))\r")
        .await
        .expect("write");
    stream.until_output(&client, shell, "older-9").await;

    let older = core.binary(
        "older",
        "second",
        "sed -e 's/\"format\":[0-9]*/\"format\":4/' -e 's/,\"liveChats\":\\[[^]]*\\]//' \"$3/state.json\" > \"$3/older.json\" && mv \"$3/older.json\" \"$3/state.json\" && grep -q '\"format\":4' \"$3/state.json\"",
    );
    assert_eq!(core.upgrade(&older, "first").await.commit, "second");
    assert!(!core.log().contains("DAMAGED"), "{}", core.log());

    let (client, _stream) = connect(&core).await;
    let frame = client.attach(shell).await.expect("attach").frame;
    assert!(screen_of(&frame).text().contains("older-9"));
}
