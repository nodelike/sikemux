#![cfg(unix)]

//! A real core and fake agents: while a worker's access call, which its lead passed on, is open,
//! what the person writes to the lead or to the chief reaches them as usual and leaves the call
//! open, since neither chat shows the call or says it keeps what is typed private.

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
        Self::open(socket, &id).await
    }

    async fn open(socket: &Path, id: &str) -> Self {
        let id = id.to_owned();
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
    json!({ "question": question, "reason": "access", "breaks": "the deploy", "choices": ["Skip, use the mock"] })
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
async fn messages_to_a_lead_and_the_chief_beside_a_workers_access_call_reach_them() {
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

    let heard = lead
        .says("what is the status of the deploy?", "echo:")
        .await;
    assert!(heard.contains("status of the deploy"), "{heard}");
    let opened = app
        .call("openChief", json!({ "project": "widget" }))
        .await
        .expect("the chief opens");
    let chief_id = opened["agentId"].as_str().expect("its id").to_owned();
    let mut chief = Agent::open(&socket, &chief_id).await;
    chief.chat.until(|chat| chat.turns_ended() >= 1).await;
    let heard = chief.says("how far along is the widget?", "echo:").await;
    assert!(heard.contains("how far along"), "{heard}");
    let log = calls_log(&root);
    assert!(!log.contains("\"event\":\"answered\""), "{log}");
    assert!(
        !root.join("data").join("agent-secrets").exists(),
        "nothing was taken as the worker's answer"
    );
}

/// Every file under `dir` whose path holds `part` and whose text holds `words`.
fn holding(dir: &Path, part: &str, words: &str, found: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            holding(&path, part, words, found);
        } else if path.to_string_lossy().contains(part)
            && std::fs::read_to_string(&path).is_ok_and(|text| text.contains(words))
        {
            found.push(path);
        }
    }
}

/// QA #233: a lead's answer to its worker's access question was kept in clear as a decision, so
/// the away block, the inbox and every reader of decisions could quote it.
#[tokio::test(flavor = "multi_thread")]
async fn a_leads_answer_to_an_access_question_is_kept_as_settled_without_its_words() {
    lead_answers_access_question(false).await;
}

/// The same, once the lead has passed the call on to the person and then answers it itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_leads_answer_to_an_access_call_it_passed_on_is_kept_without_its_words() {
    lead_answers_access_question(true).await;
}

async fn lead_answers_access_question(passed_on: bool) {
    let root = tempfile::Builder::new()
        .prefix("qa233")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let (_core, app) = core_with_project(&root).await;
    let socket = root.join("core.sock");
    let project = root.join("project");

    let mut lead = Agent::start(&app, &socket).await;
    let asked = "What is the passphrase for the staging vault?";
    let task = format!("tool agent.ask {}", raise(asked));
    let spawn = json!({ "cli": "opencode", "prompt": task });
    let spawned = lead
        .says(&format!("tool agent.spawn {spawn}"), "tool ")
        .await;
    let spawned = spawned
        .split_once("tool answered: ")
        .map(|(_, answer)| answer);
    let spawned: Value = serde_json::from_str(spawned.unwrap_or_default()).expect("a worker");
    let worker = spawned["agentId"].as_str().expect("its id").to_owned();
    let kept = root.join("data").join("agent-questions.json");
    let deadline = Instant::now() + WAIT;
    let question = loop {
        let open: Value = std::fs::read_to_string(&kept)
            .ok()
            .and_then(|kept| serde_json::from_str(&kept).ok())
            .unwrap_or_default();
        let found = (open["open"].as_array().into_iter().flatten())
            .find(|question| question["asked"] == asked)
            .and_then(|question| question["id"].as_str().map(str::to_owned));
        if let Some(id) = found {
            break id;
        }
        assert!(Instant::now() < deadline, "the worker never asked its lead");
        std::thread::sleep(Duration::from_millis(10));
    };

    if passed_on {
        let mut passed = raise(asked);
        passed["for"] = json!(worker);
        lead.says(&format!("tool agent.ask {passed}"), "tool ")
            .await;
        raised(&root, asked);
    } else {
        let mut asker = Agent::open(&socket, &worker).await;
        let heard = asker.says("how is the vault going?", "echo:").await;
        assert!(heard.contains("how is the vault going"), "{heard}");
    }

    let passphrase = "correct horse battery staple";
    let send = json!({ "to": worker, "answers": question, "text": passphrase, "why": format!("the vault takes {passphrase}") });
    lead.says(&format!("tool agent.send {send}"), "tool ").await;
    let deadline = Instant::now() + WAIT;
    let digest = loop {
        let digest = app
            .call("digest", json!({ "project": project }))
            .await
            .expect("the digest");
        if digest["decided"]
            .as_array()
            .is_some_and(|decided| !decided.is_empty())
        {
            break digest;
        }
        assert!(Instant::now() < deadline, "the answer was never kept");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let board = app
        .call("board", json!({ "project": project }))
        .await
        .expect("the board");
    let why = json!({ "words": "the passphrase for the staging vault" });
    let why = lead.says(&format!("tool agent.why {why}"), "tool ").await;
    let mut kept = Vec::new();
    holding(&root.join("data"), "board", "correct horse", &mut kept);

    let decision = &digest["decided"][0]["decision"];
    assert!(
        decision["text"]
            .as_str()
            .is_some_and(|text| text.ends_with("'s question privately")),
        "{decision}"
    );
    assert_eq!(decision["asked"], asked, "{decision}");
    assert!(!digest.to_string().contains("correct horse"), "{digest}");
    assert!(!board.to_string().contains("correct horse"), "{board}");
    assert!(!why.contains("correct horse"), "{why}");
    assert_eq!(kept, Vec::<std::path::PathBuf>::new());
}
