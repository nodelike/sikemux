#![cfg(unix)]

//! A real core and fake agents: what the person types to answer an access call, or types while
//! one is open, never reaches the project's chief.

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
    async fn open(socket: &Path, id: &str) -> Self {
        let (client, events) = CoreClient::connect(socket).await.expect("connect");
        let mut chat = Chat::new(events, id);
        chat.wait = WAIT;
        chat.attached(&client.acp_attach(id.to_owned()).await.expect("attach"));
        Self {
            client,
            chat,
            id: id.to_owned(),
        }
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

    async fn tool(&mut self, method: &str, params: Value) -> String {
        self.says(&format!("tool {method} {params}"), "tool ").await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn words_that_answer_an_access_call_never_reach_the_chief() {
    let root = tempfile::Builder::new()
        .prefix("chief")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let project = root.join("project");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir(&project).expect("project");
    let socket = root.join("core.sock");
    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
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
    let opened = app
        .call("openChief", json!({ "project": "widget" }))
        .await
        .expect("the chief opens");
    let chief_id = opened["agentId"].as_str().expect("its id").to_owned();
    let mut chief = Agent::open(&socket, &chief_id).await;
    chief.chat.until(|chat| chat.turns_ended() >= 1).await;

    let (worker_id, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("a worker starts");
    let mut worker = Agent::open(&socket, &worker_id).await;
    let asked = worker
        .tool(
            "agent.ask",
            json!({
                "question": "Which token does staging take?",
                "reason": "access",
                "breaks": "the deploy",
            }),
        )
        .await;
    assert!(asked.contains("tool answered"), "{asked}");
    worker
        .says("the password is hunter2 for staging", "echo:")
        .await;

    let still_open = worker
        .tool(
            "agent.ask",
            json!({
                "question": "Which key signs the release?",
                "reason": "access",
                "breaks": "the release",
                "blocked": false,
                "assumption": "none yet",
            }),
        )
        .await;
    assert!(still_open.contains("tool answered"), "{still_open}");
    worker.says("meanwhile the key is swordfish", "echo:").await;

    let answered = std::fs::read_dir(root.join("data").join("person-calls"))
        .expect("the calls' log")
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .any(|log| {
            log.lines()
                .any(|line| line.contains("\"event\":\"answered\""))
        });
    assert!(answered, "the access call typed to is recorded as answered");
    for words in ["hunter2", "swordfish"] {
        let marked = prompt_holding(&root, words);
        assert!(marked.contains("\"answered\":\"hiddenCall\""), "{marked}");
    }
    let digest = chief.says("context", "What happened").await;
    assert!(
        digest.contains("while a secret or access call was open there"),
        "{digest}"
    );
    assert!(!digest.contains("hunter2"), "{digest}");
    assert!(!digest.contains("swordfish"), "{digest}");
}

/// The line of the chats' records that holds the prompt the person typed with `words`.
fn prompt_holding(root: &Path, words: &str) -> String {
    std::fs::read_dir(root.join("data").join("chat-history"))
        .expect("the chats' records")
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .flat_map(|record| record.lines().map(str::to_owned).collect::<Vec<_>>())
        .find(|line| line.contains(words) && line.contains("\"kind\":\"prompt\""))
        .unwrap_or_else(|| panic!("no prompt holding {words} in the records"))
}
