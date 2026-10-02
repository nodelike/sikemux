#![cfg(unix)]

//! The agents' tool endpoint the core serves over loopback TCP, driven the way
//! the `sikemux` CLI drives it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sikemux_core::cli::auth::{new_nonce, same_secret, server_proof};
use sikemux_core::cli::protocol::{
    CliClientCommand, CliClientHello, CliCloseReason, CliEndpointDescriptor, CliOpenRequest,
    CliOpenTarget, CliServerResponse, CliTargetKind, HarnessRequest, CLI_PROTOCOL_VERSION,
};
use sikemux_core::client::{probe, ClientEvent, CoreClient};
use sikemux_core::protocol::{
    encode_control, read_frame_sync, ClientMessage, LaunchIdentity, Request, SpawnTarget,
    WindowAnswer, WindowCall, PROTOCOL, PROTOCOL_VERSION,
};
use sikemux_core::server::{self, ServerConfig, ServerError};
use sikemux_pty::task::{TaskSource, TaskSpawnRequest};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(10);

fn init_env() {
    static ENV: Once = Once::new();
    ENV.call_once(|| {
        std::env::set_var("SHELL", "/bin/sh");
        std::env::remove_var("ENV");
        std::env::remove_var("SIKEMUX_SHELL");
    });
}

struct TestCore {
    socket: PathBuf,
    endpoint: PathBuf,
    thread: Option<JoinHandle<Result<(), ServerError>>>,
}

fn start_core(dir: &Path, data_dir: &Path) -> TestCore {
    init_env();
    let socket = dir.join("core.sock");
    let endpoint = dir.join("cli.json");
    let config = ServerConfig {
        idle_exit: Duration::from_secs(600),
        cli_endpoint: Some(endpoint.clone()),
        data_dir: Some(data_dir.to_path_buf()),
        ..ServerConfig::new(socket.clone())
    };
    let thread = std::thread::spawn(move || server::run(config));
    let deadline = Instant::now() + WAIT;
    while probe(&socket, Duration::from_secs(1)).is_err() || !endpoint.exists() {
        assert!(Instant::now() < deadline, "the core never answered");
        assert!(!thread.is_finished(), "the core exited during startup");
        std::thread::sleep(Duration::from_millis(5));
    }
    TestCore {
        socket,
        endpoint,
        thread: Some(thread),
    }
}

impl TestCore {
    fn stop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        if let Ok(mut stream) = StdUnixStream::connect(&self.socket) {
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
        let _ = thread.join();
    }
}

impl Drop for TestCore {
    fn drop(&mut self) {
        self.stop();
    }
}

fn descriptor(endpoint: &Path) -> CliEndpointDescriptor {
    serde_json::from_slice(&std::fs::read(endpoint).expect("endpoint file")).expect("endpoint")
}

/// One connection, proven to reach the holder of the endpoint's token.
fn connect(endpoint: &Path) -> (BufReader<TcpStream>, CliEndpointDescriptor) {
    let descriptor = descriptor(endpoint);
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, descriptor.port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(70)))
        .expect("timeout");
    let mut reader = BufReader::new(stream);
    let nonce = new_nonce();
    send(
        &mut reader,
        &CliClientHello::Hello {
            protocol: CLI_PROTOCOL_VERSION,
            nonce: nonce.clone(),
        },
    );
    match receive(&mut reader) {
        CliServerResponse::Hello { proof } => assert!(same_secret(
            &proof,
            &server_proof(&descriptor.token, descriptor.port, &nonce)
        )),
        other => panic!("no proof: {other:?}"),
    }
    (reader, descriptor)
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
    serde_json::from_slice(&line).unwrap_or_else(|_| panic!("unreadable: {line:?}"))
}

fn tool(endpoint: &Path, project: &Path, method: &str, params: Value) -> Result<Value, String> {
    let (mut reader, descriptor) = connect(endpoint);
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

async fn call(
    endpoint: &Path,
    project: &Path,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let (endpoint, project, method) = (
        endpoint.to_path_buf(),
        project.to_path_buf(),
        method.to_owned(),
    );
    tokio::task::spawn_blocking(move || tool(&endpoint, &project, &method, params))
        .await
        .expect("tool call")
}

fn window_open(endpoint: &Path) -> bool {
    let (mut reader, descriptor) = connect(endpoint);
    send(
        &mut reader,
        &CliClientCommand::Ping {
            protocol: CLI_PROTOCOL_VERSION,
            token: descriptor.token,
        },
    );
    match receive(&mut reader) {
        CliServerResponse::Pong { window, .. } => window,
        other => panic!("no pong: {other:?}"),
    }
}

fn project_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("project");
    let path = std::fs::canonicalize(dir.path()).expect("canonical");
    (dir, path)
}

fn kinds(events: &Value) -> Vec<String> {
    events["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["kind"].as_str().expect("kind").to_owned())
        .collect()
}

/// Answers window calls the way the app does, starting task sessions in the
/// core under the execution id the core chose.
async fn play_window(
    client: std::sync::Arc<CoreClient>,
    mut events: UnboundedReceiver<ClientEvent>,
) {
    while let Some(event) = events.recv().await {
        let ClientEvent::WindowCall { call_id, call } = event else {
            continue;
        };
        let answer = match call {
            WindowCall::Harness { request } => match request.method.as_str() {
                "workspace.inspect" => Ok(json!({ "project": request.project, "windows": [] })),
                "ui.open" => Ok(json!({ "kind": request.params["kind"] })),
                "task.start" => {
                    let params = &request.params;
                    let text = |key: &str| params[key].as_str().unwrap_or_default().to_owned();
                    let spawned = client
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
                        .await;
                    spawned
                        .map(|_| json!({ "previewUrl": "http://localhost:1" }))
                        .map_err(|error| error.to_string())
                }
                method => Err(format!("{method} is not played here")),
            },
            WindowCall::Open { request } => {
                let opened: Vec<String> = request
                    .targets
                    .iter()
                    .map(|target| target.id.clone())
                    .collect();
                client.answer_window(
                    call_id,
                    WindowAnswer::Value {
                        value: json!({ "opened": opened, "failed": [] }),
                    },
                );
                client.window_open_closed(call_id);
                continue;
            }
        };
        client.answer_window(call_id, answer.into());
    }
}

async fn open_window(core: &TestCore) -> (std::sync::Arc<CoreClient>, tokio::task::JoinHandle<()>) {
    let (client, events) = CoreClient::connect(&core.socket).await.expect("connect");
    let client = std::sync::Arc::new(client);
    client.register_window().await.expect("register");
    let player = tokio::spawn(play_window(client.clone(), events));
    (client, player)
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_answer_without_a_window_and_say_when_they_need_one() {
    let dir = tempfile::tempdir().expect("dir");
    let (_project_dir, project) = project_dir();
    let core = start_core(dir.path(), &dir.path().join("data"));
    let endpoint = core.endpoint.clone();

    assert!(!window_open(&endpoint));
    assert!(
        call(&endpoint, &project, "task.read", json!({ "taskId": "dev" }))
            .await
            .unwrap_err()
            .contains("has not been started")
    );
    assert_eq!(
        call(
            &endpoint,
            &project,
            "events.wait",
            json!({ "cursor": "0", "timeoutMs": 0 })
        )
        .await
        .unwrap(),
        json!({ "events": [], "cursor": "0", "truncated": false })
    );
    assert!(call(
        &endpoint,
        &project,
        "events.wait",
        json!({ "cursor": "7", "timeoutMs": 0 })
    )
    .await
    .unwrap_err()
    .contains("invalid"));
    let inspect = call(&endpoint, &project, "workspace.inspect", json!({}))
        .await
        .unwrap();
    assert_eq!(inspect["project"], project.to_string_lossy().as_ref());
    assert_eq!(inspect["window"], Value::Null);
    assert_eq!(inspect["runs"], json!([]));
    assert_eq!(inspect["cursor"], "0");
    assert!(inspect["note"]
        .as_str()
        .unwrap()
        .contains("need the window open"));
    assert!(call(
        &endpoint,
        &project,
        "task.start",
        json!({ "command": "true", "idempotencyKey": "first" })
    )
    .await
    .unwrap_err()
    .contains("open Sikemux to start tasks"));

    let (mut reader, _) = connect(&endpoint);
    send(
        &mut reader,
        &CliClientCommand::Ping {
            protocol: CLI_PROTOCOL_VERSION,
            token: "wrong".into(),
        },
    );
    assert_eq!(
        receive(&mut reader),
        CliServerResponse::Error {
            message: "CLI authentication failed".into()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_started_through_the_window_is_read_and_stopped_after_the_window_closes() {
    let dir = tempfile::tempdir().expect("dir");
    let (_project_dir, project) = project_dir();
    let core = start_core(dir.path(), &dir.path().join("data"));
    let endpoint = core.endpoint.clone();
    let (window, player) = open_window(&core).await;
    assert!(window_open(&endpoint));

    let inspect = call(&endpoint, &project, "workspace.inspect", json!({}))
        .await
        .unwrap();
    assert_eq!(inspect["runs"], json!([]));
    assert_eq!(inspect["cursor"], json!("0"));

    let params = json!({
        "command": "printf 'READY\\n'; sleep 30",
        "idempotencyKey": "first",
        "readyWhen": "READY",
    });
    let started = call(&endpoint, &project, "task.start", params.clone())
        .await
        .unwrap();
    assert_eq!(started["status"], "running", "{started}");
    assert_eq!(started["ready"], true, "{started}");
    assert!(started["taskId"]
        .as_str()
        .unwrap()
        .starts_with("sh:printf-ready-"));
    let execution_id = started["executionId"].as_str().unwrap().to_owned();
    let again = call(&endpoint, &project, "task.start", params)
        .await
        .unwrap();
    assert_eq!(again["executionId"], execution_id);

    player.abort();
    drop(window);
    let deadline = Instant::now() + WAIT;
    while window_open(&endpoint) {
        assert!(Instant::now() < deadline, "the window never left");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let read = call(
        &endpoint,
        &project,
        "task.read",
        json!({ "executionId": execution_id, "plain": true }),
    )
    .await
    .unwrap();
    assert!(read["output"].as_str().unwrap().contains("READY"), "{read}");
    assert_eq!(read["previewUrl"], "http://localhost:1");
    let history = call(
        &endpoint,
        &project,
        "events.wait",
        json!({ "cursor": "0", "timeoutMs": 0, "executionId": execution_id }),
    )
    .await
    .unwrap();
    assert_eq!(&kinds(&history)[..2], ["task.starting", "task.running"]);

    let cursor = history["cursor"].as_str().unwrap().to_owned();
    let waiting = {
        let (endpoint, project, cursor) = (endpoint.clone(), project.clone(), cursor.clone());
        tokio::spawn(async move {
            call(
                &endpoint,
                &project,
                "events.wait",
                json!({ "cursor": cursor, "timeoutMs": 10_000 }),
            )
            .await
        })
    };
    let stopped = call(
        &endpoint,
        &project,
        "task.stop",
        json!({ "taskId": started["taskId"] }),
    )
    .await
    .unwrap();
    assert_eq!(stopped["status"], "stopped");
    let woken = waiting.await.unwrap().unwrap();
    assert!(
        kinds(&woken).contains(&"task.stopping".to_owned()),
        "{woken}"
    );
    let inspect = call(&endpoint, &project, "workspace.inspect", json!({}))
        .await
        .unwrap();
    assert_eq!(inspect["window"], Value::Null);
    assert_eq!(inspect["runs"][0]["executionId"], execution_id.as_str());
    assert_eq!(inspect["runs"][0]["status"], "stopped");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_waiting_on_a_window_that_leaves_fails_and_a_new_window_takes_over() {
    let dir = tempfile::tempdir().expect("dir");
    let (_project_dir, project) = project_dir();
    let core = start_core(dir.path(), &dir.path().join("data"));
    let endpoint = core.endpoint.clone();

    let (silent, mut silent_events) = CoreClient::connect(&core.socket).await.expect("connect");
    silent.register_window().await.expect("register");
    let pending = {
        let (endpoint, project) = (endpoint.clone(), project.clone());
        tokio::spawn(async move { call(&endpoint, &project, "workspace.inspect", json!({})).await })
    };
    let asked = tokio::time::timeout(WAIT, silent_events.recv())
        .await
        .expect("asked");
    assert!(matches!(asked, Some(ClientEvent::WindowCall { .. })));
    let (replacement, _player) = open_window(&core).await;
    assert_eq!(
        pending.await.unwrap(),
        Err("Sikemux's window closed before it answered".into())
    );
    let inspect = call(&endpoint, &project, "workspace.inspect", json!({}))
        .await
        .unwrap();
    assert_eq!(inspect["project"], project.to_string_lossy().as_ref());
    drop(silent);
    assert!(
        window_open(&endpoint),
        "the old window leaving does not close the new one"
    );
    drop(replacement);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_open_answers_twice() {
    let dir = tempfile::tempdir().expect("dir");
    let (_project_dir, project) = project_dir();
    let core = start_core(dir.path(), &dir.path().join("data"));
    let (_window, _player) = open_window(&core).await;
    let endpoint = core.endpoint.clone();
    let file = project.join("file.rs");
    std::fs::write(&file, "").expect("file");
    let responses = tokio::task::spawn_blocking(move || {
        let (mut reader, descriptor) = connect(&endpoint);
        send(
            &mut reader,
            &CliClientCommand::Open {
                protocol: CLI_PROTOCOL_VERSION,
                token: descriptor.token,
                request: CliOpenRequest {
                    id: "request".into(),
                    cwd: project.to_string_lossy().into_owned(),
                    wait: true,
                    targets: vec![CliOpenTarget {
                        id: "target".into(),
                        kind: CliTargetKind::File,
                        path: file.to_string_lossy().into_owned(),
                        project_root: project.to_string_lossy().into_owned(),
                        line: None,
                        column: None,
                    }],
                },
            },
        );
        (receive(&mut reader), receive(&mut reader))
    })
    .await
    .unwrap();
    assert_eq!(
        responses,
        (
            CliServerResponse::Accepted {
                request_id: "request".into(),
                opened: vec!["target".into()],
                failed: vec![],
            },
            CliServerResponse::Closed {
                request_id: "request".into(),
                reason: CliCloseReason::TabsClosed,
            }
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn event_cursors_outlive_the_core_and_the_endpoint_file_goes_with_it() {
    let dir = tempfile::tempdir().expect("dir");
    let data = dir.path().join("data");
    let (_project_dir, project) = project_dir();
    let mut first = start_core(dir.path(), &data);
    let endpoint = first.endpoint.clone();
    let (_window, _player) = open_window(&first).await;
    call(&endpoint, &project, "ui.open", json!({ "kind": "diff" }))
        .await
        .unwrap();
    let before = call(
        &endpoint,
        &project,
        "events.wait",
        json!({ "cursor": "0", "timeoutMs": 0 }),
    )
    .await
    .unwrap();
    assert_eq!(kinds(&before), ["ui.opened"]);
    assert_eq!(before["cursor"], "1");
    tokio::task::block_in_place(|| first.stop());
    assert!(!endpoint.exists(), "a core that exits removes its endpoint");

    let second = start_core(dir.path(), &data);
    let after = call(
        &second.endpoint,
        &project,
        "events.wait",
        json!({ "cursor": "0", "timeoutMs": 0 }),
    )
    .await
    .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        call(
            &second.endpoint,
            &project,
            "events.wait",
            json!({ "cursor": "1", "timeoutMs": 0 })
        )
        .await
        .unwrap()["events"],
        json!([])
    );
    assert!(data.join("journal").read_dir().unwrap().count() == 1);
    assert!(data.join("agent-tool-calls.json").exists());
}
