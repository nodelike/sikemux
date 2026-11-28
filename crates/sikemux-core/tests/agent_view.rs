#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

//! Switching an `antigravity` agent to TUI uses catalog command `agy` (not `antigravity`), and
//! switching back to GUI after the terminal exits (e.g. code 127) clears `ptyId` and `terminal`
//! instead of trapping the agent in TUI.

mod folded;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use folded::Chat;
use serde_json::{json, Value};
use sikemux_client::client::{ClientEvent, CoreClient};
use sikemux_core::client::probe;
use sikemux_process::command::{Child, Stdio};
use sikemux_wire::protocol::{ask, ChatAttachment, Event, SessionId};
use tokio::sync::mpsc::UnboundedReceiver;

const CORE: &str = env!("CARGO_BIN_EXE_sikemux-core");
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");
const WAIT: Duration = Duration::from_secs(60);

struct KilledOnDrop(Child);

impl Drop for KilledOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_core(dir: &Path, bin: &Path, socket: &Path) -> KilledOnDrop {
    let log = std::fs::File::create(dir.join("core.log")).expect("log");
    let endpoint = dir.join("cli.json");
    let path_env = format!("{}:/usr/bin:/bin", bin.display());
    let child = sikemux_process::user_environment::command(CORE)
        .arg("core")
        .arg("--socket")
        .arg(socket)
        .arg("--cli-endpoint")
        .arg(&endpoint)
        .arg("--data-dir")
        .arg(dir.join("data"))
        .arg("--idle-exit-secs")
        .arg("600")
        .env("HOME", dir.join("home"))
        .env("SHELL", "/bin/sh")
        .env("PATH", path_env)
        .env("FAKE_ACP_DIR", dir.join("acp"))
        .env(sikemux_process::vars::AGENT_SEARCH, "path")
        .stdin(Stdio::null())
        .stdout(log.try_clone().expect("log"))
        .stderr(log)
        .spawn()
        .expect("start the core");
    let core = KilledOnDrop(child);
    let deadline = Instant::now() + WAIT;
    while probe(socket, Duration::from_secs(1)).is_err() || !endpoint.exists() {
        assert!(Instant::now() < deadline, "the core never answered");
        std::thread::sleep(Duration::from_millis(10));
    }
    core
}

fn write_agy(bin: &Path, marker: &Path) -> PathBuf {
    let agy = bin.join("agy");
    std::fs::write(
        &agy,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "agy 0.0.0-test"
  exit 0
fi
if [ "$1" = "acp" ]; then
  log=
  prev=
  for arg in "$@"; do
    if [ "$prev" = "--log-file" ]; then
      log=$arg
      break
    fi
    prev=$arg
  done
  [ -z "$log" ] || echo 'Using file-based token storage' > "$log"
  exec '{agent}'
fi
printf '%s %s\n' "$0" "$*" >> '{marker}'
exit 127
"#,
            agent = FAKE_AGENT,
            marker = marker.display(),
        ),
    )
    .expect("write agy");
    std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).expect("chmod agy");
    agy
}

async fn agent_record(app: &CoreClient, id: &str) -> Value {
    let workspace = app.call("workspace", json!({})).await.expect("workspace");
    (workspace["agents"].as_array().into_iter().flatten())
        .find(|agent| agent["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("agent {id} missing from workspace: {workspace}"))
}

async fn say(app: &CoreClient, socket: &Path, id: &str, text: &str) {
    let (_watching, mut chat) = open(socket, id).await;
    app.ask(ask::AcpPrompt {
        agent_id: id.to_owned(),
        message_id: Some(uuid::Uuid::new_v4().to_string()),
        text: text.to_owned(),
        paths: Vec::new(),
        context: Vec::new(),
    })
    .await
    .expect("prompt");
    chat.until(|chat| chat.text().contains(&format!("echo: {text}")) && !chat.head().running)
        .await;
}

async fn open(socket: &Path, id: &str) -> (CoreClient, Chat) {
    let (client, events) = CoreClient::connect(socket).await.expect("connect");
    let mut chat = Chat::new(events, id);
    chat.wait = WAIT;
    let attachment = client.acp_attach(id.to_owned()).await.expect("attach");
    assert!(
        matches!(attachment, ChatAttachment::Live { .. }),
        "expected live chat attachment for {id}: {attachment:?}"
    );
    chat.attached(&attachment);
    (client, chat)
}

async fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.trim().is_empty() {
                return text;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_exit(
    events: &mut UnboundedReceiver<ClientEvent>,
    pty_id: SessionId,
) -> Option<u32> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("timed out waiting for SessionExit")
            .expect("core event stream closed");
        if let ClientEvent::Event(Event::Exited { id, code, .. }) = event {
            if id == pty_id {
                return code;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn antigravity_tui_runs_agy_and_exited_terminal_switches_back_to_chat() {
    let root = tempfile::Builder::new()
        .prefix("agent-view")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = sikemux_process::files::real_path(root.path()).expect("canonical");
    let project = root.join("project");
    let bin = root.join("bin");
    for dir in [
        root.join("home"),
        root.join("acp"),
        project.clone(),
        bin.clone(),
    ] {
        std::fs::create_dir_all(dir).expect("dir");
    }
    let marker = root.join("terminal.marker");
    let agy = write_agy(&bin, &marker);
    assert!(!bin.join("antigravity").exists());

    let socket = root.join("core.sock");
    let _core = start_core(&root, &bin, &socket);
    let (app, mut events) = CoreClient::connect(&socket).await.expect("connect");

    app.call(
        "addProject",
        json!({ "id": "widget", "name": "widget", "path": project }),
    )
    .await
    .expect("addProject");

    let (agent_id, _) = app
        .start_chat("antigravity".into(), "widget".into(), None)
        .await
        .expect("start_chat antigravity");
    say(&app, &socket, &agent_id, "hello").await;

    let switched = app
        .call(
            "switchAgentView",
            json!({ "agentId": agent_id, "to": "terminal" }),
        )
        .await
        .expect("switch to terminal");
    let pty_id = switched["ptyId"]
        .as_u64()
        .expect("switchAgentView to terminal returns ptyId");

    let recorded = wait_for_file(&marker).await;
    assert!(
        recorded.contains(&agy.to_string_lossy().into_owned()),
        "expected terminal to run {} rather than `antigravity`, got: {recorded}",
        agy.display()
    );

    let exit_code = wait_for_exit(&mut events, pty_id).await;
    assert_eq!(exit_code, Some(127));

    let restarted = app
        .call(
            "switchAgentView",
            json!({ "agentId": agent_id, "to": "terminal" }),
        )
        .await
        .expect("re-enter terminal after exit 127");
    let restarted_pty = restarted["ptyId"]
        .as_u64()
        .expect("re-entering terminal returns a new ptyId");
    assert_ne!(restarted_pty, pty_id);
    let sessions = app.ask(ask::List).await.expect("list sessions");
    assert!(
        !sessions.iter().any(|session| session.id == pty_id),
        "previous exited PTY {pty_id} should be removed when replaced by {restarted_pty}: {sessions:?}"
    );
    assert_eq!(wait_for_exit(&mut events, restarted_pty).await, Some(127));

    let back = app
        .call(
            "switchAgentView",
            json!({ "agentId": agent_id, "to": "chat" }),
        )
        .await
        .expect("switch back to chat after exit 127");
    assert_eq!(back, Value::Null);
    assert!(
        app.ask(ask::List).await.expect("list sessions").is_empty(),
        "switching back to chat should remove the exited PTY session"
    );

    let record = agent_record(&app, &agent_id).await;
    assert!(
        record["ptyId"].is_null(),
        "ptyId should be cleared after switching back to chat: {record}"
    );
    assert_eq!(
        record["terminal"], false,
        "terminal should be false after switching back to chat: {record}"
    );

    let back_again = app
        .call(
            "switchAgentView",
            json!({ "agentId": agent_id, "to": "chat" }),
        )
        .await
        .expect("second switchAgentView to chat is idempotent");
    assert_eq!(back_again, Value::Null);
    let record_again = agent_record(&app, &agent_id).await;
    assert!(record_again["ptyId"].is_null(), "{record_again}");
    assert_eq!(record_again["terminal"], false, "{record_again}");

    say(&app, &socket, &agent_id, "after switch").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_chief_after_exited_terminal_still_reopens_as_same_chief() {
    let root = tempfile::Builder::new()
        .prefix("chief-reopen")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = sikemux_process::files::real_path(root.path()).expect("canonical");
    let project = root.join("project");
    let bin = root.join("bin");
    for dir in [
        root.join("home"),
        root.join("acp"),
        project.clone(),
        bin.clone(),
    ] {
        std::fs::create_dir_all(dir).expect("dir");
    }
    let marker = root.join("terminal.marker");
    let _agy = write_agy(&bin, &marker);

    let socket = root.join("core.sock");
    let _core = start_core(&root, &bin, &socket);
    let (app, mut events) = CoreClient::connect(&socket).await.expect("connect");

    app.call(
        "addProject",
        json!({ "id": "widget", "name": "widget", "path": project }),
    )
    .await
    .expect("addProject");
    app.call("installedAgents", json!({}))
        .await
        .expect("installedAgents");

    let opened = app
        .call("openChief", json!({ "project": "widget" }))
        .await
        .expect("openChief");
    let chief_id = opened["agentId"]
        .as_str()
        .expect("openChief returns agentId")
        .to_owned();
    let (_chief_watch, mut chief_chat) = open(&socket, &chief_id).await;
    chief_chat.until(folded::Folded::has_ended_a_turn).await;
    say(&app, &socket, &chief_id, "hello chief").await;

    let switched = app
        .call(
            "switchAgentView",
            json!({ "agentId": chief_id, "to": "terminal" }),
        )
        .await
        .expect("switch chief to terminal");
    let pty_id = switched["ptyId"].as_u64().expect("ptyId");
    assert_eq!(wait_for_exit(&mut events, pty_id).await, Some(127));

    app.call("closeAgents", json!({ "agentIds": [chief_id] }))
        .await
        .expect("close chief");
    let workspace = app.call("workspace", json!({})).await.expect("workspace");
    assert!(
        !(workspace["agents"].as_array().into_iter().flatten())
            .any(|agent| agent["id"] == chief_id),
        "closed chief should not remain in workspace.agents: {workspace}"
    );

    let reopened = app
        .call("openChief", json!({ "project": "widget" }))
        .await
        .expect("reopen chief");
    assert_eq!(
        reopened["agentId"].as_str(),
        Some(chief_id.as_str()),
        "openChief should reopen the exact same Chief agentId: {reopened}"
    );

    let reopened_pty = agent_record(&app, &chief_id).await["ptyId"]
        .as_u64()
        .expect("reopened terminal chief has ptyId");
    assert_eq!(wait_for_exit(&mut events, reopened_pty).await, Some(127));

    let back = app
        .call(
            "switchAgentView",
            json!({ "agentId": chief_id, "to": "chat" }),
        )
        .await
        .expect("switch reopened chief back to chat");
    assert_eq!(back, Value::Null);

    let record = agent_record(&app, &chief_id).await;
    assert!(record["ptyId"].is_null(), "{record}");
    assert_eq!(record["terminal"], false, "{record}");

    let names = app.call("agentNames", json!({})).await.expect("agentNames");
    let chief_name = (names.as_array().into_iter().flatten())
        .find(|entry| entry["agentId"] == chief_id)
        .unwrap_or_else(|| panic!("chief missing from agentNames: {names}"));
    assert_eq!(chief_name["name"], "Chief", "{chief_name}");

    say(&app, &socket, &chief_id, "chief back").await;
}
