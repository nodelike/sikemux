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

fn write_claude_account(dir: &Path, uuid: &str, used_percent: f64) {
    std::fs::create_dir_all(dir).expect("account dir");
    std::fs::write(
        dir.join(".claude.json"),
        json!({ "oauthAccount": { "accountUuid": uuid } }).to_string(),
    )
    .expect("claude json");
    std::fs::write(
        dir.join("fake-account.json"),
        json!({
            "loggedIn": true,
            "authMethod": "claude.ai",
            "email": format!("{uuid}@example.com"),
            "subscriptionType": "max"
        })
        .to_string(),
    )
    .expect("fake account");
    let resets_at = "2099-01-01T00:00:00Z";
    std::fs::write(
        dir.join("fake-usage.json"),
        json!({
            "rate_limits_available": true,
            "subscription_type": "max",
            "rate_limits": {
                "seven_day": { "utilization": used_percent, "resets_at": resets_at }
            }
        })
        .to_string(),
    )
    .expect("fake usage");
}

async fn move_fresh(app: &CoreClient, id: &str, provider: &str, profile: Option<&str>) {
    app.ask(ask::AcpStop {
        agent_id: id.to_owned(),
    })
    .await
    .expect("stop");
    app.call(
        "updateAgent",
        json!({
            "agentId": id,
            "patch": {
                "provider": provider,
                "profileId": profile,
                "model": null,
                "modelProvider": null,
                "effort": null,
                "resumeId": null,
            }
        }),
    )
    .await
    .expect("updateAgent");
    app.call(
        "startChat",
        json!({ "agentId": id, "waitsForPerson": true }),
    )
    .await
    .expect("startChat");
}

/// The person's report, 24 Nov: switching a chat on Kishore (12% used) to another harness and
/// back must keep Kishore (and its CLAUDE_CONFIG_DIR), and when no prior account can be kept it
/// must choose an account with quota, never Kishore Pers at 100%.
#[tokio::test(flavor = "multi_thread")]
async fn switching_harness_and_back_keeps_account_and_avoids_full_account() {
    let root = tempfile::Builder::new()
        .prefix("account-switch")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = sikemux_process::files::real_path(root.path()).expect("canonical");
    let project = root.join("project");
    let kishore = root.join("claude-kishore");
    let pers = root.join("claude-pers");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir_all(&project).expect("project");
    write_claude_account(&kishore, "kishore-uuid", 12.0);
    write_claude_account(&pers, "pers-uuid", 100.0);

    let socket = root.join("core.sock");
    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
    let accounts = json!({
        "profiles": [
            { "id": "kishore", "name": "Kishore", "provider": "claude", "accent": "#e0a030",
              "executablePath": FAKE_CLAUDE, "configPath": kishore },
            { "id": "kishore-pers", "name": "Kishore Pers", "provider": "claude", "accent": "#d97757",
              "executablePath": FAKE_CLAUDE, "configPath": pers },
            { "id": "fake", "name": "Fake", "provider": "omp", "accent": "#a277ff",
              "executablePath": FAKE_AGENT },
        ],
        "selected": { "claude": "kishore-pers" },
        "smart": true,
    });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("accounts");
    app.call(
        "addProject",
        json!({ "id": "widget", "name": "widget", "path": project }),
    )
    .await
    .expect("the project");

    // Seed usage readings so the core knows Kishore is at 12% and Kishore Pers is at 100%.
    for dir in [&kishore, &pers] {
        app.call(
            "agentUsage",
            json!({ "provider": "claude", "configPath": dir }),
        )
        .await
        .expect("usage");
    }

    // Fresh chat on Kishore -> switch to omp -> switch back to claude: must stay on Kishore.
    let (fresh_id, _) = app
        .start_chat("claude:kishore".into(), "widget".into(), None)
        .await
        .expect("fresh chat");
    assert_eq!(agent(&app, &fresh_id).await["profileId"], "kishore");

    move_fresh(&app, &fresh_id, "omp", None).await;
    move_fresh(&app, &fresh_id, "claude", None).await;
    assert_eq!(
        agent(&app, &fresh_id).await["profileId"],
        "kishore",
        "switching a fresh chat away and back must keep Kishore"
    );

    // Prompt it and confirm the process ran in Kishore's CLAUDE_CONFIG_DIR.
    let (_watching, mut chat) = open(&socket, &fresh_id).await;
    app.ask(ask::AcpPrompt {
        agent_id: fresh_id.clone(),
        message_id: Some(uuid::Uuid::new_v4().to_string()),
        text: "ping fresh".into(),
        paths: Vec::new(),
        context: Vec::new(),
    })
    .await
    .expect("prompt");
    chat.until(|chat| chat.text().contains("echo: ping fresh"))
        .await;
    let session = agent(&app, &fresh_id).await["resumeId"]
        .as_str()
        .expect("session")
        .to_owned();
    assert!(
        kishore
            .join("projects/fake")
            .join(format!("{session}.jsonl"))
            .exists(),
        "Claude must have run in Kishore's CLAUDE_CONFIG_DIR"
    );
    assert!(
        !pers
            .join("projects/fake")
            .join(format!("{session}.jsonl"))
            .exists(),
        "Claude must not have run in Kishore Pers's CLAUDE_CONFIG_DIR"
    );

    // Started chat on Kishore -> handOff to omp -> handOff back to claude: must stay on Kishore
    // and run in Kishore's CLAUDE_CONFIG_DIR (not corrupted by omp's kept account).
    switch(&app, &fresh_id, "omp").await;
    switch(&app, &fresh_id, "claude").await;
    assert_eq!(
        agent(&app, &fresh_id).await["profileId"],
        "kishore",
        "switching a started chat away and back must keep Kishore"
    );
    let (_watching, mut chat) = open(&socket, &fresh_id).await;
    app.ask(ask::AcpPrompt {
        agent_id: fresh_id.clone(),
        message_id: Some(uuid::Uuid::new_v4().to_string()),
        text: "ping back".into(),
        paths: Vec::new(),
        context: Vec::new(),
    })
    .await
    .expect("prompt back");
    chat.until(|chat| chat.text().contains("echo: ping back"))
        .await;
    let session_back = agent(&app, &fresh_id).await["resumeId"]
        .as_str()
        .expect("session back")
        .to_owned();
    assert_eq!(session_back, session, "resumed the same Claude session");
    assert!(
        !pers
            .join("projects/fake")
            .join(format!("{session_back}.jsonl"))
            .exists(),
        "Claude must still run in Kishore's CLAUDE_CONFIG_DIR after handoff back"
    );

    // Chat starting on omp (no prior Claude account) switches to claude with smart off and
    // Kishore Pers (100%) as the selected default: must still pick Kishore (12%), never 100%.
    app.call("setSmartAccounts", json!({ "on": false }))
        .await
        .expect("smart off");
    let (other_id, _) = app
        .start_chat("omp".into(), "widget".into(), None)
        .await
        .expect("omp chat");
    move_fresh(&app, &other_id, "claude", None).await;
    assert_eq!(
        agent(&app, &other_id).await["profileId"],
        "kishore",
        "when no prior Claude account exists, must pick one with quota, never one at 100%"
    );
}
