#![cfg(unix)]

//! A real core and fake agents: what the person types in a lead's chat while its worker's access
//! call is open is never kept on another call the same words answer.

mod folded;

use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use folded::Chat;
use serde_json::{json, Value};
use sikemux_client::client::CoreClient;
use sikemux_core::client::probe;
use sikemux_view::chat::{ChatMessage, ChatRole};
use sikemux_wire::protocol::ask;

const CORE: &str = env!("CARGO_BIN_EXE_sikemux-core");
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");
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

/// What the agent said to each message that reads `prompt`, in order.
fn answers<'a>(rows: &'a [ChatMessage], prompt: &'a str) -> impl Iterator<Item = String> + 'a {
    rows.chunk_by(|_, next| next.role != ChatRole::User)
        .filter(move |turn| folded::prompts(turn) == [prompt])
        .map(folded::said)
}

/// One agent's chat, which the test prompts as the person does.
struct Agent {
    client: CoreClient,
    chat: Chat,
    id: String,
}

impl Agent {
    async fn start(app: &CoreClient, socket: &Path) -> Self {
        let (id, _) = app
            .start_chat("opencode:fake".into(), "widget".into(), None)
            .await
            .expect("an agent starts");
        let (client, events) = CoreClient::connect(socket).await.expect("connect");
        let mut chat = Chat::new(events, &id);
        chat.wait = WAIT;
        chat.attached(&client.acp_attach(id.clone()).await.expect("attach"));
        Self { client, chat, id }
    }

    /// Sends `text` as the person and answers what the agent said to it, once that holds `reply`.
    async fn says(&mut self, text: &str, reply: &str) -> String {
        let before = answers(&self.chat.view.rows, text).count();
        self.client
            .ask(ask::AcpPrompt {
                agent_id: self.id.clone(),
                message_id: Some(uuid::Uuid::new_v4().to_string()),
                text: text.to_owned(),
                paths: Vec::new(),
                context: Vec::new(),
            })
            .await
            .expect("prompt");
        let mut said = String::new();
        self.chat
            .until(|chat| {
                said = answers(&chat.view.rows, text)
                    .nth(before)
                    .unwrap_or_default();
                said.contains(reply)
            })
            .await;
        said
    }
}

fn raise(question: &str) -> Value {
    json!({ "question": question, "reason": "access", "breaks": "the deploy" })
}

/// The calls' log, every line the core has written so far.
fn calls_log(root: &Path) -> String {
    let Ok(files) = std::fs::read_dir(root.join("data").join("person-calls")) else {
        return String::new();
    };
    files
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .collect()
}

/// The line of `log` that raised the call asking `question`.
fn raised_in(log: &str, question: &str) -> Option<String> {
    (log.lines())
        .find(|line| line.contains("\"event\":\"raised\"") && line.contains(question))
        .map(str::to_owned)
}

/// The calls' log once it holds what `ready` looks for. The core puts a call to the person after
/// `ask` has answered, so a turn can end before its call is written.
fn log_once(root: &Path, waiting_for: &str, ready: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let log = calls_log(root);
        if ready(&log) {
            return log;
        }
        assert!(Instant::now() < deadline, "{waiting_for}: {log}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The calls file's line that raised the call asking `question`, once the core has written it.
fn raised(root: &Path, question: &str) -> String {
    let log = log_once(
        root,
        &format!("no call asking {question} was raised"),
        |log| raised_in(log, question).is_some(),
    );
    raised_in(&log, question).unwrap_or_default()
}

async fn core_with_project(root: &Path) -> (KilledOnDrop, CoreClient) {
    let project = root.join("project");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir(&project).expect("project");
    let core = start_core(root, &root.join("core.sock"));
    let (app, _events) = CoreClient::connect(&root.join("core.sock"))
        .await
        .expect("connect");
    let accounts = json!({ "profiles": [{
        "id": "fake",
        "name": "Fake",
        "provider": "opencode",
        "accent": "#a277ff",
        "executablePath": FAKE_AGENT,
    }] });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("accounts");
    app.call(
        "addProject",
        json!({ "id": "widget", "name": "widget", "path": project }),
    )
    .await
    .expect("the project");
    (core, app)
}

#[tokio::test(flavor = "multi_thread")]
async fn words_typed_to_a_lead_beside_its_workers_access_call_are_kept_nowhere() {
    let root = tempfile::Builder::new()
        .prefix("answered-in-chat")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let (_core, app) = core_with_project(&root).await;
    let socket = root.join("core.sock");

    let mut lead = Agent::start(&app, &socket).await;
    let token = "Which token does the Widget deploy take?";
    let task = format!("tool agent.ask {}", raise(token));
    let spawn = json!({ "cli": "opencode", "prompt": task });
    let spawned = lead
        .says(&format!("tool agent.spawn {spawn}"), "tool ")
        .await;
    let spawned = spawned
        .split_once("tool answered: ")
        .map(|(_, answer)| answer);
    let spawned: Value = serde_json::from_str(spawned.unwrap_or_default()).expect("a worker");
    let kept = root.join("data").join("agent-questions.json");
    let deadline = Instant::now() + WAIT;
    while !std::fs::read_to_string(&kept).is_ok_and(|kept| kept.contains(token)) {
        assert!(Instant::now() < deadline, "the worker never asked its lead");
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut passed = raise(token);
    passed["for"] = spawned["agentId"].clone();
    lead.says(&format!("tool agent.ask {passed}"), "tool ")
        .await;
    raised(&root, token);

    let plan = "Which plan should the Widget host run on?";
    let money = json!({
        "question": plan,
        "reason": "money",
        "choices": ["Small", "Large"],
        "pick": "Small",
        "breaks": "the launch",
    });
    let typed = format!("tool agent.ask {money}\nuse hunter2now for the widget deploy");
    lead.says(&typed, "hunter2now").await;
    raised(&root, plan);

    let answered = |log: &str| log.matches("\"event\":\"answered\"").count();
    let before = answered(&calls_log(&root));
    lead.says("hunter2now is the one", "hunter2now").await;
    let log = log_once(
        &root,
        "the lead's own call is answered in the chat",
        |log| answered(log) > before,
    );
    assert!(!log.contains("hunter2now"), "{log}");
}
