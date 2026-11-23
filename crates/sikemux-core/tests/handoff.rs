#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

//! Switching a chat's agent between messages: what its next message hands over.

mod folded;

use sikemux_process::command::{Child, Stdio};
use std::path::Path;
use std::time::{Duration, Instant};

use folded::Chat;
use serde_json::{json, Value};
use sikemux_client::client::CoreClient;
use sikemux_core::client::probe;
use sikemux_wire::protocol::ask;

const CORE: &str = env!("CARGO_BIN_EXE_sikemux-core");
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");
const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_sikemux-fake-claude");
const WAIT: Duration = Duration::from_secs(90);

struct KilledOnDrop(Child);

impl Drop for KilledOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_core(dir: &Path, socket: &Path) -> KilledOnDrop {
    let log = std::fs::File::create(dir.join("core.log")).expect("log");
    let endpoint = dir.join("cli.json");
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
        .env("PATH", "/usr/bin:/bin")
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

async fn open(socket: &Path, id: &str) -> (CoreClient, Chat) {
    let (client, events) = CoreClient::connect(socket).await.expect("connect");
    let mut chat = Chat::new(events, id);
    chat.wait = WAIT;
    chat.attached(&client.acp_attach(id.to_owned()).await.expect("attach"));
    (client, chat)
}

async fn agent(app: &CoreClient, id: &str) -> Value {
    let workspace = app.call("workspace", json!({})).await.expect("workspace");
    (workspace["agents"].as_array().into_iter().flatten())
        .find(|agent| agent["id"] == id)
        .cloned()
        .expect("the chat's agent")
}

async fn switch(app: &CoreClient, id: &str, provider: &str) {
    app.call("handOff", json!({ "agentId": id, "provider": provider }))
        .await
        .expect("handed over");
}

/// The person's report, 24 Nov: a chat switched to another agent and back before anything was
/// sent still showed "Hands over to Claude" over the composer.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_switched_away_and_back_hands_nothing_over_and_resumes_its_session() {
    let root = tempfile::Builder::new()
        .prefix("handoff")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = sikemux_process::files::real_path(root.path()).expect("canonical");
    let (project, claude) = (root.join("project"), root.join("claude"));
    for dir in [root.join("home"), claude.clone(), project.clone()] {
        std::fs::create_dir_all(dir).expect("dir");
    }
    std::fs::write(
        claude.join("fake-account.json"),
        json!({ "loggedIn": true, "authMethod": "claude.ai", "email": "qa@example.com", "subscriptionType": "max" }).to_string(),
    )
    .expect("an account");
    let socket = root.join("core.sock");
    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
    let accounts = json!({ "profiles": [
        { "id": "builtin-claude", "name": "Claude", "provider": "claude", "accent": "#d97757",
          "executablePath": FAKE_CLAUDE, "configPath": claude },
        { "id": "fake", "name": "Fake", "provider": "omp", "accent": "#a277ff",
          "executablePath": FAKE_AGENT },
    ] });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("accounts");
    app.call(
        "addProject",
        json!({ "id": "widget", "name": "widget", "path": project }),
    )
    .await
    .expect("the project");
    let (id, _) = app
        .start_chat("claude:builtin-claude".into(), "widget".into(), None)
        .await
        .expect("a chat");
    let (_watching, mut chat) = open(&socket, &id).await;
    app.ask(ask::AcpPrompt {
        agent_id: id.clone(),
        message_id: Some(uuid::Uuid::new_v4().to_string()),
        text: "hello there".into(),
        paths: Vec::new(),
        context: Vec::new(),
    })
    .await
    .expect("prompt");
    chat.until(|chat| chat.text().contains("echo: hello there"))
        .await;
    let session = agent(&app, &id).await["resumeId"].clone();
    assert!(session.is_string(), "the Claude session is noted");

    switch(&app, &id, "omp").await;
    let (_watching, mut chat) = open(&socket, &id).await;
    chat.until(|chat| chat.head().handoff.is_some()).await;

    switch(&app, &id, "claude").await;
    let (_watching, chat) = open(&socket, &id).await;
    assert!(chat.head().handoff.is_none(), "nothing is handed over");
    let back = agent(&app, &id).await;
    assert_eq!(back["provider"], "claude");
    assert_eq!(
        back["resumeId"], session,
        "the chat resumes its Claude session"
    );

    switch(&app, &id, "omp").await;
    let (_watching, mut chat) = open(&socket, &id).await;
    chat.until(|chat| {
        (chat.head().handoff.as_ref()).is_some_and(|handoff| handoff.from == "claude")
    })
    .await;
}
