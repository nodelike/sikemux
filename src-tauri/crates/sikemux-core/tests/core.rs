#![cfg(unix)]

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sikemux_core::client::{ensure_running, probe, ClientEvent, CoreClient, EventSink, Reply};
use sikemux_core::protocol::{
    encode_control, read_frame_sync, AgentStateEvent, BuildIdentity, ClientMessage, Continuation,
    Event, LaunchIdentity, Request, Response, ServerMessage, SessionExit, SessionId, SpawnTarget,
    TerminalSpawn, PROTOCOL, PROTOCOL_VERSION,
};
use sikemux_core::server::{self, ServerConfig, ServerError};
use sikemux_pty::launch::PtyContext;
use sikemux_pty::output_log::OutputQuery;
use sikemux_pty::task::{TaskSource, TaskSpawnRequest};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(10);

fn test_build() -> BuildIdentity {
    BuildIdentity {
        version: "0.0.0-test".into(),
        commit: "abc1234".into(),
        built_at: 7,
        source: "f00d".into(),
    }
}

/// Every shell in these tests is a plain `/bin/sh` with a known prompt, so no
/// user dotfiles run.
fn init_env() {
    static ENV: Once = Once::new();
    ENV.call_once(|| {
        std::env::set_var("SHELL", "/bin/sh");
        std::env::set_var("PS1", "$ ");
        std::env::remove_var("ENV");
        std::env::remove_var("SIKEMUX_SHELL");
    });
}

struct TestCore {
    dir: tempfile::TempDir,
    socket: PathBuf,
    thread: Option<JoinHandle<Result<(), ServerError>>>,
}

fn start_core_at(dir: tempfile::TempDir, socket: PathBuf, idle_exit: Duration) -> TestCore {
    init_env();
    let config = ServerConfig {
        idle_exit,
        build: test_build(),
        ..ServerConfig::new(socket.clone())
    };
    let thread = std::thread::spawn(move || server::run(config));
    let deadline = Instant::now() + WAIT;
    while probe(&socket, Duration::from_secs(1)).is_err() {
        assert!(Instant::now() < deadline, "the core never answered");
        assert!(!thread.is_finished(), "the core exited during startup");
        std::thread::sleep(Duration::from_millis(5));
    }
    TestCore {
        dir,
        socket,
        thread: Some(thread),
    }
}

fn start_core_with(idle_exit: Duration) -> TestCore {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("core.sock");
    start_core_at(dir, socket, idle_exit)
}

fn start_core() -> TestCore {
    start_core_with(Duration::from_secs(600))
}

impl TestCore {
    async fn connect(&self) -> (CoreClient, Stream) {
        let (client, events) = CoreClient::connect(&self.socket).await.expect("connect");
        (client, Stream::new(events))
    }

    fn wait_exit(&mut self, limit: Duration) -> Result<(), ServerError> {
        let thread = self.thread.take().expect("core thread");
        let deadline = Instant::now() + limit;
        while !thread.is_finished() {
            assert!(Instant::now() < deadline, "the core did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        thread.join().expect("core thread panicked")
    }
}

/// Stops a core that a test left running, with every session in it.
fn shutdown_sync(socket: &Path) {
    let Ok(mut stream) = StdUnixStream::connect(socket) else {
        return;
    };
    let _ = stream.set_read_timeout(Some(WAIT));
    let hello = ClientMessage::Hello {
        protocol: PROTOCOL.into(),
        version: PROTOCOL_VERSION,
    };
    let shutdown = ClientMessage::Request {
        request_id: 1,
        request: Request::Shutdown { stop_all: true },
    };
    let _ = stream.write_all(&encode_control(&hello).expect("hello"));
    let _ = stream.write_all(&encode_control(&shutdown).expect("shutdown"));
    while let Ok(Some(_)) = read_frame_sync(&mut stream) {}
}

impl Drop for TestCore {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            shutdown_sync(&self.socket);
            let _ = thread.join();
        }
    }
}

struct Stream {
    events: UnboundedReceiver<ClientEvent>,
    output: HashMap<SessionId, Vec<u8>>,
    exits: HashMap<SessionId, Event>,
    task_notices: Vec<SessionId>,
    agent_states: Vec<AgentStateEvent>,
}

impl Stream {
    fn new(events: UnboundedReceiver<ClientEvent>) -> Self {
        Self {
            events,
            output: HashMap::new(),
            exits: HashMap::new(),
            task_notices: Vec::new(),
            agent_states: Vec::new(),
        }
    }

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
            ClientEvent::Event(event @ Event::Exited { id, .. }) => {
                self.exits.insert(id, event);
            }
            ClientEvent::Event(Event::TaskOutput { id }) => self.task_notices.push(id),
            ClientEvent::Event(Event::AgentState(state)) => self.agent_states.push(state),
            ClientEvent::Event(_) | ClientEvent::WindowCall { .. } => {}
        }
    }

    fn output(&self, id: SessionId) -> &[u8] {
        self.output.get(&id).map(Vec::as_slice).unwrap_or_default()
    }

    async fn until_output(&mut self, client: &CoreClient, id: SessionId, needle: &str) {
        while !contains(self.output(id), needle) {
            self.pump(client).await;
        }
    }

    async fn until_agent_state(&mut self, client: &CoreClient, state: &str) -> AgentStateEvent {
        loop {
            if let Some(event) = self.agent_states.iter().find(|event| event.state == state) {
                return event.clone();
            }
            self.pump(client).await;
        }
    }

    async fn until_exit(&mut self, client: &CoreClient, id: SessionId) -> (Option<u32>, bool) {
        loop {
            if let Some(Event::Exited { code, killed, .. }) = self.exits.get(&id) {
                return (*code, *killed);
            }
            self.pump(client).await;
        }
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn launch() -> LaunchIdentity {
    LaunchIdentity {
        version: "0.0.0-test".into(),
        ..LaunchIdentity::default()
    }
}

fn terminal(startup: Option<&str>) -> SpawnTarget {
    SpawnTarget::Terminal(TerminalSpawn {
        cols: 80,
        rows: 24,
        cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        startup: startup.map(str::to_string),
        ..TerminalSpawn::default()
    })
}

fn task(dir: &Path, command: &str) -> SpawnTarget {
    let dir = dir.to_string_lossy().into_owned();
    SpawnTarget::Task {
        request: TaskSpawnRequest {
            execution_id: "exec-1".into(),
            terminal_key: "terminal-1".into(),
            task_id: "test".into(),
            label: "Test".into(),
            project: dir.clone(),
            source: TaskSource::Project,
            command: command.into(),
            cwd: dir,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
            agent_id: Some("agent-1".into()),
        },
    }
}

/// A task that waits for the test to create `go` in its directory, so the
/// test can attach before the command prints anything.
fn gated(dir: &Path, command: &str) -> SpawnTarget {
    task(
        dir,
        &format!("while [ ! -e go ]; do sleep 0.01; done; {command}"),
    )
}

fn open_gate(dir: &Path) {
    std::fs::write(dir.join("go"), b"").expect("open the gate");
}

/// Attaches repeatedly until the replay holds `needle`, then stays attached.
async fn attach_once_printed(client: &CoreClient, id: SessionId, needle: &str) -> Vec<u8> {
    let deadline = Instant::now() + WAIT;
    loop {
        let attached = client.attach(id).await.expect("attach");
        if contains(&attached.replay, needle) {
            return attached.replay;
        }
        client.detach(id).await.expect("detach");
        assert!(Instant::now() < deadline, "{needle:?} was never printed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists; nothing is delivered.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

async fn until_dead(pid: u32) {
    let deadline = Instant::now() + WAIT;
    while process_alive(pid) {
        assert!(Instant::now() < deadline, "process {pid} is still alive");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn number_after(text: &[u8], marker: &str) -> u32 {
    let text = String::from_utf8_lossy(text);
    let start = text.find(marker).expect("marker") + marker.len();
    text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("number after marker")
}

#[tokio::test]
async fn handshake_succeeds_and_a_wrong_version_is_rejected() {
    let core = start_core();
    let (client, _stream) = core.connect().await;
    assert_eq!(client.core_pid(), std::process::id());
    assert_eq!(client.hello().build, test_build());

    let mut stream = StdUnixStream::connect(&core.socket).expect("connect");
    stream.set_read_timeout(Some(WAIT)).expect("timeout");
    let hello = ClientMessage::Hello {
        protocol: PROTOCOL.into(),
        version: PROTOCOL_VERSION + 1,
    };
    stream
        .write_all(&encode_control(&hello).expect("hello"))
        .expect("send hello");
    let frame = read_frame_sync(&mut stream)
        .expect("read")
        .expect("a reply");
    let reply: ServerMessage = serde_json::from_slice(&frame.payload).expect("json");
    assert!(matches!(
        reply,
        ServerMessage::HelloRejected { version, pid, .. }
            if version == PROTOCOL_VERSION && pid == std::process::id()
    ));
    assert!(read_frame_sync(&mut stream).expect("read").is_none());

    assert!(
        matches!(
            CoreClient::connect(&core.dir.path().join("missing.sock")).await,
            Err(sikemux_core::client::ClientError::Io(_))
        ),
        "connecting to a missing socket fails"
    );
}

#[tokio::test]
async fn output_arrives_byte_for_byte() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = client
        .spawn(launch(), gated(dir.path(), "printf hello"))
        .await
        .expect("spawn");
    client.attach(id).await.expect("attach");
    open_gate(dir.path());
    let (code, killed) = stream.until_exit(&client, id).await;
    assert_eq!(stream.output(id), b"hello");
    assert_eq!(code, Some(0));
    assert!(!killed);
}

#[tokio::test]
async fn acked_output_flows_past_the_unacked_budget() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = client
        .spawn(
            launch(),
            gated(dir.path(), "head -c 2000000 /dev/zero | tr '\\0' x"),
        )
        .await
        .expect("spawn");
    client.attach(id).await.expect("attach");
    open_gate(dir.path());
    let started = Instant::now();
    stream.until_exit(&client, id).await;
    assert_eq!(stream.output(id).len(), 2_000_000);
    assert!(stream.output(id).iter().all(|byte| *byte == b'x'));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "an acking client never waits for the write-off"
    );
}

#[tokio::test]
async fn a_client_that_falls_behind_holds_the_program_until_it_acks() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = client
        .spawn(
            launch(),
            gated(dir.path(), "head -c 3000000 /dev/zero | tr '\\0' x"),
        )
        .await
        .expect("spawn");
    client.attach(id).await.expect("attach");
    open_gate(dir.path());
    let mut owed = 0;
    let deadline = Instant::now() + Duration::from_millis(500);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline.into(), stream.events.recv()).await
    {
        if let ClientEvent::Output { bytes, .. } = event {
            owed += bytes.len();
        }
    }
    assert!(owed > 0, "nothing arrived");
    assert!(owed < 1_000_000, "{owed} bytes arrived without an ack");
    client.ack(id, owed);
    stream.output.insert(id, vec![b'x'; owed]);
    stream.until_exit(&client, id).await;
    assert_eq!(stream.output(id).len(), 3_000_000);
}

#[tokio::test]
async fn a_client_that_never_acks_does_not_hold_the_program_forever() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = client
        .spawn(
            launch(),
            gated(dir.path(), "head -c 1500000 /dev/zero | tr '\\0' x"),
        )
        .await
        .expect("spawn");
    client.attach(id).await.expect("attach");
    open_gate(dir.path());
    let mut received = 0;
    loop {
        match tokio::time::timeout(WAIT, stream.events.recv()).await {
            Ok(Some(ClientEvent::Output { bytes, .. })) => received += bytes.len(),
            Ok(Some(ClientEvent::Event(Event::Exited { id: exited, .. }))) if exited == id => break,
            Ok(Some(_)) => {}
            _ => panic!("the program stayed held after {received} bytes"),
        }
    }
    assert_eq!(received, 1_500_000);
}

#[tokio::test]
async fn bytes_that_are_not_utf8_pass_through_unchanged() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = client
        .spawn(
            launch(),
            gated(dir.path(), r"printf '\377\376\000\200\033[0mabc'"),
        )
        .await
        .expect("spawn");
    client.attach(id).await.expect("attach");
    open_gate(dir.path());
    stream.until_exit(&client, id).await;
    assert_eq!(stream.output(id), b"\xff\xfe\x00\x80\x1b[0mabc");
}

#[tokio::test]
async fn exit_codes_are_reported() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client
        .spawn(launch(), terminal(Some("exit 7")))
        .await
        .expect("spawn");
    assert_eq!(stream.until_exit(&client, id).await, (Some(7), false));
}

#[tokio::test]
async fn an_interactive_shell_runs_what_is_written_to_it() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client.spawn(launch(), terminal(None)).await.expect("spawn");
    client.attach(id).await.expect("attach");
    client
        .write(id, b"echo answer-$((6*7))\n")
        .await
        .expect("write");
    stream.until_output(&client, id, "answer-42").await;
}

#[tokio::test]
async fn resize_reaches_the_terminal() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client.spawn(launch(), terminal(None)).await.expect("spawn");
    client.attach(id).await.expect("attach");
    client.resize(id, 100, 40).await.expect("resize");
    client.write(id, b"stty size\n").await.expect("write");
    stream.until_output(&client, id, "40 100").await;
    let session = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == id)
        .expect("listed");
    assert_eq!((session.cols, session.rows), (100, 40));
    assert!(client.resize(id, 0, 40).await.is_err());
}

#[tokio::test]
async fn attach_replays_earlier_output_then_streams_live_bytes() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client
        .spawn(launch(), terminal(Some("printf 'early-%s\\n' marker")))
        .await
        .expect("spawn");
    attach_once_printed(&client, id, "early-marker").await;
    client
        .write(id, b"echo live-$((1+1))\n")
        .await
        .expect("write");
    stream.until_output(&client, id, "live-2").await;
    assert!(
        !contains(stream.output(id), "early-marker"),
        "the replay is not sent again as live output"
    );
}

#[tokio::test]
async fn two_attached_clients_receive_the_same_bytes() {
    let core = start_core();
    let (first, mut first_stream) = core.connect().await;
    let (second, mut second_stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = first
        .spawn(
            launch(),
            gated(
                dir.path(),
                "i=0; while [ $i -lt 300 ]; do printf 'line %d\\n' $i; i=$((i+1)); done",
            ),
        )
        .await
        .expect("spawn");
    first.attach(id).await.expect("first attach");
    second.attach(id).await.expect("second attach");
    open_gate(dir.path());
    first_stream.until_exit(&first, id).await;
    second_stream.until_exit(&second, id).await;
    assert!(contains(first_stream.output(id), "line 299"));
    assert_eq!(first_stream.output(id), second_stream.output(id));
}

#[tokio::test]
async fn a_session_outlives_its_client_and_can_be_reattached() {
    let core = start_core();
    let id = {
        let (client, _stream) = core.connect().await;
        let id = client.spawn(launch(), terminal(None)).await.expect("spawn");
        client.attach(id).await.expect("attach");
        id
    };
    let (client, mut stream) = core.connect().await;
    let session = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == id)
        .expect("the session survived its client");
    assert!(session.running);
    client.attach(id).await.expect("reattach");
    client
        .write(id, b"echo still-$((2+3))\n")
        .await
        .expect("write");
    stream.until_output(&client, id, "still-5").await;
}

#[tokio::test]
async fn kill_terminates_the_whole_process_tree() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client
        .spawn(launch(), terminal(Some("sleep 1000 & echo bg-pid:$!")))
        .await
        .expect("spawn");
    let replay = attach_once_printed(&client, id, "bg-pid:").await;
    let background = number_after(&replay, "bg-pid:");
    let shell = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == id)
        .and_then(|session| session.pid)
        .expect("shell pid");
    assert!(process_alive(background));

    client.kill(id).await.expect("kill");
    let (_, killed) = stream.until_exit(&client, id).await;
    assert!(killed);
    until_dead(background).await;
    until_dead(shell).await;
    assert!(client.list().await.expect("list").is_empty());
    client
        .kill(id)
        .await
        .expect("killing a gone session is not an error");
    assert!(client.write(id, b"x").await.is_err());
}

#[tokio::test]
async fn task_output_can_be_paged_after_the_task_exits() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = client
        .spawn(launch(), task(dir.path(), "printf 'alpha\\nbeta\\n'"))
        .await
        .expect("spawn");
    stream.until_exit(&client, id).await;
    let page = client
        .task_output(
            id,
            OutputQuery {
                limit: 8192,
                ..OutputQuery::default()
            },
        )
        .await
        .expect("task output");
    assert_eq!(page.bytes, b"alpha\r\nbeta\r\n");
    assert!(!page.has_more);
    while !stream.task_notices.contains(&id) {
        stream.pump(&client).await;
    }
    let session = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == id)
        .expect("a finished task is retained");
    assert!(!session.running);
    client
        .attach(id)
        .await
        .expect("a finished task is attachable");

    let shell = client.spawn(launch(), terminal(None)).await.expect("spawn");
    assert!(client
        .task_output(shell, OutputQuery::default())
        .await
        .is_err());
}

#[test]
fn the_core_exits_when_idle_but_not_while_a_client_is_connected() {
    let mut core = start_core_with(Duration::from_millis(300));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let client = runtime
        .block_on(CoreClient::connect(&core.socket))
        .expect("connect");
    std::thread::sleep(Duration::from_millis(900));
    assert!(
        core.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished()),
        "the core exited under a connected client"
    );
    drop(client);
    drop(runtime);
    core.wait_exit(WAIT).expect("idle exit");
    assert!(!core.socket.exists(), "the socket is removed on exit");
}

#[test]
fn a_second_core_on_the_same_socket_refuses_to_start() {
    let core = start_core();
    let result = server::run(ServerConfig::new(core.socket.clone()));
    assert!(
        matches!(
            &result,
            Err(ServerError::AlreadyRunning { pid: Some(pid), .. }) if *pid == std::process::id()
        ),
        "{result:?}"
    );
    assert!(probe(&core.socket, Duration::from_secs(1)).is_ok());
}

#[test]
fn a_dead_socket_file_is_replaced_and_permissions_are_private() {
    let dir = tempfile::tempdir().expect("temp dir");
    let parent = dir.path().join("fresh");
    std::fs::create_dir(&parent).expect("parent");
    let stale = parent.join("core.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale).expect("stale socket"));
    assert!(stale.exists());
    let core = start_core_at(dir, stale.clone(), Duration::from_secs(600));
    assert!(probe(&core.socket, Duration::from_secs(1)).is_ok());
    let mode = std::fs::metadata(&stale)
        .expect("socket")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    drop(core);

    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("missing/core.sock");
    let core = start_core_at(dir, socket, Duration::from_secs(600));
    let mode = std::fs::metadata(core.socket.parent().expect("parent"))
        .expect("parent dir")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[tokio::test]
async fn shutdown_refuses_with_sessions_unless_told_to_stop_them() {
    let mut core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client.spawn(launch(), terminal(None)).await.expect("spawn");
    let pid = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == id)
        .and_then(|session| session.pid)
        .expect("pid");
    assert!(client.shutdown(false).await.is_err());
    client.shutdown(true).await.expect("shutdown");
    let (_, killed) = stream.until_exit(&client, id).await;
    assert!(killed);
    until_dead(pid).await;
    core.wait_exit(WAIT).expect("core exits");
}

const SERVE_ENV: &str = "SIKEMUX_CORE_TEST_SERVE";

/// Not a test on its own: `ensure_running` below starts this test binary as
/// its core, and this is the core it runs.
#[test]
fn serve_core_for_ensure_running() {
    if let Some(socket) = std::env::var_os(SERVE_ENV) {
        init_env();
        let _ = server::run(ServerConfig {
            idle_exit: Duration::from_millis(500),
            ..ServerConfig::new(socket.into())
        });
    }
}

#[test]
fn ensure_running_starts_one_detached_core() {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("core.sock");
    let log = dir.path().join("core.log");
    let binary = dir.path().join("sikemux");
    let test_binary = std::env::current_exe().expect("test binary");
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\n[ \"$1\" = core ] && [ \"$2\" = --socket ] || exit 9\n{SERVE_ENV}=\"$3\" exec '{}' --exact serve_core_for_ensure_running --test-threads 1\n",
            test_binary.display()
        ),
    )
    .expect("launcher");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let hello = ensure_running(&socket, &binary, &log, &[]).expect("core started");
    assert_ne!(hello.pid, std::process::id());
    // SAFETY: getsid only reads the session id of the given pid.
    let session = unsafe { libc::getsid(hello.pid as libc::pid_t) };
    assert_eq!(
        session, hello.pid as libc::pid_t,
        "the core leads its own session"
    );
    let again = ensure_running(&socket, &binary, &log, &[]).expect("core found");
    assert_eq!(again.pid, hello.pid);

    shutdown_sync(&socket);
    let deadline = Instant::now() + WAIT;
    while process_alive(hello.pid) {
        assert!(Instant::now() < deadline, "the started core did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn agent_terminal(agent_id: &str, startup: &str) -> SpawnTarget {
    SpawnTarget::Terminal(TerminalSpawn {
        cols: 80,
        rows: 24,
        cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        startup: Some(startup.into()),
        context: Some(PtyContext {
            session_id: "session-1".into(),
            session_name: "test".into(),
            session_kind: "project".into(),
            project: None,
            window_id: None,
            pane_id: None,
            agent_id: Some(agent_id.into()),
            agent_type: Some("claude".into()),
            initial_prompt_submitted: false,
            shell_integration: false,
        }),
        ..TerminalSpawn::default()
    })
}

#[tokio::test]
async fn an_agent_terminal_reports_ready_then_working_on_a_submitted_line_then_stopped() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client
        .spawn(launch(), agent_terminal("agent-7", "read line; exit 3"))
        .await
        .expect("spawn");
    let ready = stream.until_agent_state(&client, "idle").await;
    assert_eq!(ready.agent_id, "agent-7");
    assert_eq!(ready.reason, "agent ready; no prompt submitted");
    let listed = client.list().await.expect("list");
    let info = listed.iter().find(|info| info.id == id).expect("listed");
    assert_eq!(info.agent_state.as_deref(), Some("idle"));

    client.write(id, b"go").await.expect("typing");
    client.write(id, b"\r").await.expect("submit");
    let working = stream.until_agent_state(&client, "working").await;
    assert_eq!(working.reason, "command submitted");
    assert!(working.sequence > ready.sequence);

    let stopped = stream.until_agent_state(&client, "stopped").await;
    assert_eq!(stopped.reason, "agent process stopped with code 3");
    assert!(stopped.sequence > working.sequence);
}

#[tokio::test]
async fn a_killed_agent_reports_nothing_more() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client
        .spawn(launch(), agent_terminal("agent-8", "sleep 1000"))
        .await
        .expect("spawn");
    stream.until_agent_state(&client, "idle").await;
    client.kill(id).await.expect("kill");
    stream.until_exit(&client, id).await;
    client.list().await.expect("list");
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(200), stream.events.recv()).await
    {
        assert!(
            !matches!(event, ClientEvent::Event(Event::AgentState(_))),
            "a killed agent published {event:?}"
        );
    }
}

#[tokio::test]
async fn manifests_are_listed_reloaded_and_explained() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let manifests = tempfile::tempdir().expect("manifest dir");
    let configured = client
        .configure(Some(manifests.path().to_path_buf()))
        .await
        .expect("configure");
    assert!(!configured.manifests.is_empty());
    assert_eq!(
        client.list_manifests().await.expect("list"),
        client.reload_manifests().await.expect("reload")
    );

    assert!(client
        .explain_agent_detection("nobody".into())
        .await
        .is_err());
    let id = client
        .spawn(launch(), agent_terminal("agent-9", "sleep 1000"))
        .await
        .expect("spawn");
    stream.until_agent_state(&client, "idle").await;
    let explain = client
        .explain_agent_detection("agent-9".into())
        .await
        .expect("explain");
    assert!(!explain.manifest_version.is_empty());
    client.kill(id).await.expect("kill");
}

#[tokio::test]
async fn subscribe_streams_only_what_comes_after_it() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client
        .spawn(
            launch(),
            terminal(Some("printf 'before\\n'; read line; echo after-$line")),
        )
        .await
        .expect("spawn");
    attach_once_printed(&client, id, "before").await;
    client.detach(id).await.expect("detach");
    client.subscribe(id).await.expect("subscribe");
    client.write(id, b"x\n").await.expect("write");
    stream.until_output(&client, id, "after-x").await;
    assert!(!contains(stream.output(id), "before"));
}

#[tokio::test]
async fn reset_modes_reach_the_screen_and_every_subscriber() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client
        .spawn(
            launch(),
            terminal(Some("printf '\\033[?1049h\\033[?1000h'; sleep 1000")),
        )
        .await
        .expect("spawn");
    let deadline = Instant::now() + WAIT;
    loop {
        let attached = client.attach(id).await.expect("attach");
        if attached.alternate_screen {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the program never switched screens"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    client.reset_modes(id).await.expect("reset");
    stream.until_output(&client, id, "\x1b[?1049l").await;
    assert!(!client.attach(id).await.expect("attach").alternate_screen);
    assert!(client.reset_modes(id + 1_000_000).await.is_err());
    client.kill(id).await.expect("kill");
}

#[tokio::test]
async fn writes_larger_than_a_frame_arrive_whole() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = client
        .spawn(
            launch(),
            task(dir.path(), "stty raw -echo; head -c 17000000 | wc -c"),
        )
        .await
        .expect("spawn");
    client.attach(id).await.expect("attach");
    let payload = vec![b'y'; 17_000_000];
    client.write(id, &payload).await.expect("write");
    stream.until_exit(&client, id).await;
    assert!(contains(stream.output(id), "17000000"));
}

#[tokio::test]
async fn stop_all_kills_every_session_and_keeps_the_core() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let shell = client.spawn(launch(), terminal(None)).await.expect("spawn");
    let task = client
        .spawn(launch(), task(dir.path(), "sleep 1000"))
        .await
        .expect("task");
    client.stop_all().await.expect("stop all");
    for id in [shell, task] {
        let (_, killed) = stream.until_exit(&client, id).await;
        assert!(killed);
    }
    assert!(client.list().await.expect("list").is_empty());
    client
        .spawn(launch(), terminal(None))
        .await
        .expect("the core still spawns");
}

#[derive(Default)]
struct OrderSink(std::sync::Mutex<Vec<String>>);

impl OrderSink {
    fn note(&self, entry: String) {
        self.0.lock().expect("order").push(entry);
    }

    fn entries(&self) -> Vec<String> {
        self.0.lock().expect("order").clone()
    }
}

impl EventSink for OrderSink {
    fn output(&self, id: SessionId, _bytes: &[u8]) {
        self.note(format!("output {id}"));
    }

    fn event(&self, event: Event) {
        if let Event::Exited { id, .. } = event {
            self.note(format!("exited {id}"));
        }
    }

    fn closed(&self) {}
}

#[tokio::test]
async fn a_spawn_reply_comes_before_the_session_exits() {
    let core = start_core();
    let sink = Arc::new(OrderSink::default());
    let client = CoreClient::connect_with(&core.socket, sink.clone())
        .await
        .expect("connect");
    let dir = tempfile::tempdir().expect("task dir");
    for _ in 0..20 {
        let noted = sink.clone();
        let id = client
            .submit(
                Request::Spawn {
                    launch: launch(),
                    target: Box::new(task(dir.path(), "true")),
                },
                move |reply| match reply {
                    Ok(Reply::Response(Response::Spawned { id })) => {
                        noted.note(format!("spawned {id}"));
                        id
                    }
                    other => panic!("unexpected reply {other:?}"),
                },
            )
            .expect("submit")
            .await
            .expect("reply");
        let deadline = Instant::now() + WAIT;
        while !sink.entries().contains(&format!("exited {id}")) {
            assert!(Instant::now() < deadline, "the task never exited");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let entries = sink.entries();
        let position = |entry: String| entries.iter().position(|seen| *seen == entry);
        assert!(position(format!("spawned {id}")) < position(format!("exited {id}")));
    }
}

#[tokio::test]
async fn session_ids_start_from_the_clock() {
    let core = start_core();
    let (client, _stream) = core.connect().await;
    let id = client.spawn(launch(), terminal(None)).await.expect("spawn");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    assert!(id > now_ms - 600_000 && id <= now_ms);
    assert!(id < 1 << 53);
}

#[tokio::test]
async fn a_terminal_that_ends_unwatched_stays_to_show_how_it_ended() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client
        .spawn(launch(), terminal(Some("echo farewell-$((40+2)); exit 3")))
        .await
        .expect("spawn");
    let (code, killed) = stream.until_exit(&client, id).await;
    assert_eq!((code, killed), (Some(3), false));

    let (later, _stream) = core.connect().await;
    let session = later
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == id)
        .expect("the ended terminal is kept");
    assert!(!session.running);
    assert_eq!(
        session.exit,
        Some(SessionExit {
            code: Some(3),
            signal: None
        })
    );
    let attached = later.attach(id).await.expect("attach the ended terminal");
    assert!(attached.exited);
    assert!(contains(&attached.replay, "farewell-42"));

    later.kill(id).await.expect("close it");
    assert!(later.list().await.expect("list").is_empty());
}

#[tokio::test]
async fn a_watched_terminal_leaves_when_it_ends_and_an_attach_before_the_exit_hears_it() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let id = client.spawn(launch(), terminal(None)).await.expect("spawn");
    let attached = client.attach(id).await.expect("attach");
    assert!(!attached.exited);
    client.write(id, b"exit 0\n").await.expect("write");
    stream.until_exit(&client, id).await;
    let deadline = Instant::now() + WAIT;
    while !client.list().await.expect("list").is_empty() {
        assert!(Instant::now() < deadline, "the watched terminal stayed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn an_agent_that_ends_while_watched_stays_and_a_continuation_starts_below_its_screen() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let ended = client
        .spawn(
            launch(),
            agent_terminal(
                "agent-9",
                "printf '\\033[?1000h'; echo before-$((1+1)); exit 7",
            ),
        )
        .await
        .expect("spawn");
    let attached = client.attach(ended).await.expect("attach");
    assert!(!attached.exited);
    assert_eq!(stream.until_exit(&client, ended).await, (Some(7), false));
    let kept = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == ended)
        .expect("a watched agent terminal is kept after it ends");
    assert!(!kept.running && !kept.killed);

    let SpawnTarget::Terminal(mut resumed) =
        agent_terminal("agent-9", "echo after-$((2+2)); sleep 100")
    else {
        unreachable!()
    };
    resumed.continues = Some(Continuation {
        session: ended,
        note: "\u{2014} Resuming Claude \u{2014}\u{7}".into(),
    });
    let id = client
        .spawn(launch(), SpawnTarget::Terminal(resumed))
        .await
        .expect("spawn the continuation");
    let replay = attach_once_printed(&client, id, "after-4").await;
    let text = String::from_utf8_lossy(&replay);
    let before = text.find("before-2").expect("the old screen is carried");
    let note = text
        .find("\u{2014} Resuming Claude \u{2014}")
        .expect("the note follows it");
    let after = text.find("after-4").expect("the new program prints below");
    assert!(before < note && note < after);
    assert!(!text.contains('\u{7}'));
    assert!(!contains(&replay, "\x1b[?1000h"));
    let sessions = client.list().await.expect("list");
    assert!(sessions.iter().all(|session| session.id != ended));
    client.kill(id).await.expect("kill");
    let (_, killed) = stream.until_exit(&client, id).await;
    assert!(killed);
}

#[tokio::test]
async fn a_task_session_says_what_it_was_started_as_and_how_it_ended() {
    let core = start_core();
    let (client, mut stream) = core.connect().await;
    let dir = tempfile::tempdir().expect("task dir");
    let id = client
        .spawn(launch(), task(dir.path(), "exit 4"))
        .await
        .expect("spawn");
    stream.until_exit(&client, id).await;
    let session = client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|session| session.id == id)
        .expect("listed");
    let task = session.task.expect("task info");
    assert_eq!(task.execution_id, "exec-1");
    assert_eq!(task.terminal_key, "terminal-1");
    assert_eq!(task.task_id, "test");
    assert_eq!(task.command, "exit 4");
    assert_eq!(task.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(session.exit.and_then(|exit| exit.code), Some(4));
}
