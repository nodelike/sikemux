#![cfg(unix)]

//! A core replacing itself with another binary in the same process. These run
//! the standalone core binary, since the core under test has to `exec`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sikemux_core::cli::auth::new_nonce;
use sikemux_core::cli::protocol::{
    CliClientCommand, CliClientHello, CliEndpointDescriptor, CliServerResponse, HarnessRequest,
    CLI_PROTOCOL_VERSION,
};
use sikemux_core::client::{
    await_deferred_upgrade, await_upgrade, frozen_request, probe, ClientEvent, CoreClient,
};
use sikemux_core::protocol::frozen::{FrozenReply, FrozenRequest};
use sikemux_core::protocol::{
    BuildIdentity, ChatAttachment, ChatEvent, ChatEventKind, ChatLaunch, Event, LaunchIdentity,
    SessionId, SpawnTarget, TerminalSpawn, WindowCall, BUILD_ID_OVERRIDE_ENV,
};
use sikemux_pty::output_log::OutputQuery;
use sikemux_pty::task::{TaskSource, TaskSpawnRequest};
use tokio::sync::mpsc::UnboundedReceiver;

const CORE: &str = env!("CARGO_BIN_EXE_sikemux-core");
const WAIT: Duration = Duration::from_secs(15);

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
        let log = std::fs::File::create(dir.path().join("core.log")).expect("log");
        let child = sikemux_process::user_environment::command(CORE)
            .arg("core")
            .arg("--socket")
            .arg(&socket)
            .arg("--cli-endpoint")
            .arg(&endpoint)
            .arg("--data-dir")
            .arg(dir.path().join("data"))
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
            .expect("start the core");
        let core = Self {
            child,
            dir,
            socket,
            endpoint,
            project,
        };
        let deadline = Instant::now() + WAIT;
        while probe(&core.socket, Duration::from_secs(1)).is_err() || !core.endpoint.exists() {
            assert!(Instant::now() < deadline, "the core never answered");
            std::thread::sleep(Duration::from_millis(10));
        }
        core
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
    output: HashMap<SessionId, Vec<u8>>,
    exits: HashMap<SessionId, Option<u32>>,
}

impl Stream {
    async fn pump(&mut self, client: &CoreClient) {
        let event = tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("timed out waiting for the core")
            .expect("the core disconnected");
        match event {
            ClientEvent::Output { id, bytes } => {
                client.ack(id, bytes.len());
                self.output.entry(id).or_default().extend_from_slice(&bytes);
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

/// Answers the window calls a task start makes, the way the app does.
async fn play_window(client: Arc<CoreClient>, mut events: UnboundedReceiver<ClientEvent>) {
    while let Some(event) = events.recv().await {
        let ClientEvent::WindowCall { call_id, call } = event else {
            continue;
        };
        let WindowCall::Harness { request } = call else {
            continue;
        };
        let answer = match request.method.as_str() {
            "workspace.inspect" => Ok(json!({ "project": request.project })),
            "task.start" => {
                let params = &request.params;
                let text = |key: &str| params[key].as_str().unwrap_or_default().to_owned();
                client
                    .spawn(
                        LaunchIdentity::default(),
                        SpawnTarget::Task {
                            request: TaskSpawnRequest {
                                execution_id: text("executionId"),
                                terminal_key: "terminal".into(),
                                task_id: text("taskId"),
                                label: text("label"),
                                project: request.project.clone(),
                                source: TaskSource::Project,
                                command: text("command"),
                                cwd: text("cwd"),
                                env: HashMap::new(),
                                cols: 80,
                                rows: 24,
                                agent_id: request.agent_id.clone(),
                            },
                        },
                    )
                    .await
                    .map(|_| json!({}))
                    .map_err(|error| error.to_string())
            }
            method => Err(format!("{method} is not played here")),
        };
        client.answer_window(call_id, answer.into());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_keeps_every_session_and_the_harness() {
    let core = CoreProcess::start("first");
    let pid = core.pid();
    let (window, window_events) = CoreClient::connect(&core.socket).await.expect("window");
    let window = Arc::new(window);
    window.register_window().await.expect("register");
    let player = tokio::spawn(play_window(window.clone(), window_events));

    let (client, mut stream) = connect(&core).await;
    let shell = spawn_shell(&client).await;
    client.attach(shell).await.expect("attach");
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
    player.abort();
    drop(window);

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
    assert!(
        String::from_utf8_lossy(&attached.replay).contains("before-42"),
        "{:?}",
        String::from_utf8_lossy(&attached.replay)
    );
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
        let message = refusal(core.frozen(FrozenRequest::Upgrade { binary }).await);
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
    client.attach(shell).await.expect("attach");
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
    client.attach(shell).await.expect("attach");
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
        permission_mode: "workspace-write".into(),
        model: None,
        effort: None,
    }
}

async fn until_chat(
    events: &mut UnboundedReceiver<ClientEvent>,
    mut found: impl FnMut(&ChatEvent) -> bool,
) -> ChatEvent {
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("timed out waiting for the chat")
            .expect("the core disconnected");
        if let ClientEvent::Event(Event::Chat { event, .. }) = event {
            if found(&event) {
                return event;
            }
        }
    }
}

fn said(events: &[ChatEvent]) -> String {
    events
        .iter()
        .filter(|event| event.kind == ChatEventKind::SessionUpdate)
        .flat_map(|event| match event.payload.get("updates") {
            Some(Value::Array(batch)) => batch.clone(),
            _ => vec![event.payload.clone()],
        })
        .filter_map(|update| {
            update["update"]["content"]["text"]
                .as_str()
                .map(str::to_owned)
        })
        .collect::<Vec<_>>()
        .join("|")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_waits_for_a_chat_turn_and_resumes_the_chat_after() {
    let core = CoreProcess::start("first");
    let (client, mut events) = CoreClient::connect(&core.socket).await.expect("connect");
    let started = client
        .acp_start(chat_launch(&core, "talker"))
        .await
        .expect("start");
    client
        .acp_start(chat_launch(&core, "quiet"))
        .await
        .expect("start a chat that never talks");
    client
        .acp_prompt(
            "talker".into(),
            "hello first".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    until_chat(&mut events, |event| {
        event.kind == ChatEventKind::TurnCompleted
    })
    .await;
    client
        .acp_prompt("talker".into(), "hold 4000".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    until_chat(&mut events, |event| {
        event.kind == ChatEventKind::SessionUpdate
            && said(std::slice::from_ref(event)).contains("holding")
    })
    .await;

    let binary = core.binary("second-build", "second", "");
    let asked = Instant::now();
    let reply = core
        .frozen(FrozenRequest::Upgrade {
            binary: binary.clone(),
        })
        .await;
    assert!(
        matches!(reply, FrozenReply::Deferred { .. }),
        "{reply:?}\n{}",
        core.log()
    );
    let completed = until_chat(&mut events, |event| {
        event.kind == ChatEventKind::TurnCompleted
    })
    .await;
    assert_eq!(completed.payload["stopReason"], "end_turn");
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
    assert_eq!(hello.build.commit, "second");
    assert!(
        asked.elapsed() >= Duration::from_millis(500),
        "the update did not wait for the turn"
    );
    drop(client);

    let (client, _events) = CoreClient::connect(&core.socket).await.expect("connect");
    let chats = client.acp_list().await.expect("list");
    assert_eq!(
        chats
            .iter()
            .map(|chat| chat.agent_id.as_str())
            .collect::<Vec<_>>(),
        ["talker"],
        "{}",
        core.log()
    );
    let ChatAttachment::Live { start, replay, .. } =
        client.acp_attach("talker".into()).await.expect("attach")
    else {
        panic!("the chat was not resumed\n{}", core.log());
    };
    assert_eq!(start.session_id, started.session_id);
    let history = said(&replay);
    assert!(history.contains("hello first"), "{history}");
    assert!(history.contains("hold 4000"), "{history}");
    assert!(history.contains("holding held"), "{history}");
    client
        .acp_prompt(
            "talker".into(),
            "after update".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt after the update");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_core_takes_over_from_a_format_one_hand_over() {
    let core = CoreProcess::start("first");
    let (client, mut stream) = connect(&core).await;
    let shell = spawn_shell(&client).await;
    client.attach(shell).await.expect("attach");
    client
        .write(shell, b"echo older-$((3*3))\r")
        .await
        .expect("write");
    stream.until_output(&client, shell, "older-9").await;

    let older = core.binary(
        "older",
        "second",
        "sed -e 's/\"format\":4/\"format\":1/' -e 's/,\"chats\":\\[[^]]*\\]//' \"$3/state.json\" > \"$3/older.json\" && mv \"$3/older.json\" \"$3/state.json\" && grep -q '\"format\":1' \"$3/state.json\"",
    );
    assert_eq!(core.upgrade(&older, "first").await.commit, "second");
    assert!(!core.log().contains("DAMAGED"), "{}", core.log());

    let (client, _stream) = connect(&core).await;
    let replay = client.attach(shell).await.expect("attach").replay;
    assert!(String::from_utf8_lossy(&replay).contains("older-9"));
}
