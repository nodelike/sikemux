#![cfg(unix)]

//! A real core, git shim and fake agent: an agent's commits stay its own after
//! a rebase by hand, a landing and a push, a footer copied onto another commit
//! still counts for nothing, an agent can wake one held by a stop, and a file a shell command
//! wrote reads as the turn's uncommitted work.

mod folded;

use std::path::{Path, PathBuf};
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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = sikemux_process::user_environment::command("/usr/bin/git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit_file(dir: &Path, file: &str, text: &str, subject: &str) {
    std::fs::write(dir.join(file), text).expect("write");
    git(dir, &["add", file]);
    git(dir, &["commit", "--quiet", "-m", subject]);
}

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

/// What the agent said to each message that reads `prompt`, in order. A lead's chat also holds
/// what it says to its worker's reports, which quote the worker's own answers.
fn answers<'a>(rows: &'a [ChatMessage], prompt: &'a str) -> impl Iterator<Item = String> + 'a {
    rows.chunk_by(|_, next| next.role != ChatRole::User)
        .filter(move |turn| folded::prompts(turn) == [prompt])
        .map(folded::said)
}

/// One agent's chat, which a test prompts as the person does.
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

    /// Sends `text` and answers what the agent said to it, once that holds `reply`.
    async fn says(&mut self, text: String, reply: &str) -> String {
        let before = answers(&self.chat.view.rows, &text).count();
        self.client
            .ask(ask::AcpPrompt {
                agent_id: self.id.clone(),
                message_id: None,
                text: text.clone(),
                paths: Vec::new(),
                context: Vec::new(),
            })
            .await
            .expect("prompt");
        let mut said = String::new();
        self.chat
            .until(|chat| {
                said = answers(&chat.view.rows, &text)
                    .nth(before)
                    .unwrap_or_default();
                said.contains(reply)
            })
            .await;
        said
    }

    async fn run(&mut self, command: &str) {
        let said = self.says(format!("run {command}"), "ran with").await;
        assert!(said.contains("status 0"), "{command}: {said}");
    }

    async fn tool(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let said = self.says(format!("tool {method} {params}"), "tool ").await;
        match said.split_once("tool answered: ") {
            Some((_, answer)) => Ok(serde_json::Deserializer::from_str(answer)
                .into_iter()
                .next()
                .expect("an answer")
                .expect("the answer is JSON")),
            None => Err(said),
        }
    }
}

/// Waits for `rev` in `dir` to leave `from`, as a queued land or push does once its check passes.
async fn moved(dir: &Path, rev: &str, from: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let now = git(dir, &["rev-parse", rev]);
        if now != from {
            return now;
        }
        assert!(Instant::now() < deadline, "{rev} never moved from {from}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn owners(answer: &Value) -> Vec<(String, String)> {
    answer["commits"][0]["owners"]
        .as_array()
        .unwrap_or_else(|| panic!("no owners in {answer}"))
        .iter()
        .map(|owner| {
            (
                owner["kind"].as_str().unwrap_or_default().to_owned(),
                owner["name"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_landed_commit_stays_its_agents_after_a_rebase_by_hand() {
    let root = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let project = root.join("project");
    let origin = root.join("origin.git");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir(&project).expect("project");
    git(
        &root,
        &["init", "--quiet", "--bare", "-b", "main", "origin.git"],
    );
    git(&project, &["init", "--quiet", "-b", "main"]);
    git(&project, &["config", "user.name", "Test"]);
    git(&project, &["config", "user.email", "test@example.com"]);
    git(
        &project,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    commit_file(&project, "base.txt", "base\n", "base");
    git(&project, &["push", "--quiet", "origin", "main"]);

    let socket: PathBuf = root.join("core.sock");
    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
    let accounts = json!({
        "profiles": [{
            "id": "fake",
            "name": "Fake",
            "provider": "opencode",
            "accent": "#a277ff",
            "executablePath": FAKE_AGENT,
        }],
    });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("accounts");
    let added = json!({ "id": "widget", "name": "widget", "path": project });
    app.call("addProject", added).await.expect("the project");
    app.call("setAgentDefaults", json!({ "permissionMode": "bypass" }))
        .await
        .expect("the defaults");
    let (lead, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("the lead starts");
    let mut lead = Agent::open(&socket, &lead).await;
    let spawned = lead
        .tool(
            "agent.spawn",
            json!({ "cli": "opencode", "checkout": "own", "prompt": "hello" }),
        )
        .await
        .expect("a worker");
    let worker_id = spawned["agentId"].as_str().expect("its id");
    let worker_name = spawned["name"].as_str().expect("its name").to_owned();
    let mut worker = Agent::open(&socket, worker_id).await;
    worker.chat.until_said("hello").await;

    worker
        .run("printf 'one\\n' > one.txt && git add one.txt && git commit -qm 'worker: one'")
        .await;
    worker
        .run("printf 'worker\\n' > shared.txt && git add shared.txt && git commit -qm 'worker: clash'")
        .await;
    commit_file(&project, "shared.txt", "main\n", "main moves on");
    git(&project, &["push", "--quiet", "origin", "main"]);

    worker
        .run("git rebase main; printf 'both\\n' > shared.txt && git add shared.txt && GIT_EDITOR=true git rebase --continue")
        .await;
    worker
        .run(
            "git switch -qc side && printf 'picked\\n' > picked.txt && git add picked.txt && git commit -qm 'worker: picked' \
             && git switch -q - && git cherry-pick side",
        )
        .await;
    worker
        .run(
            "printf 'fixed\\n' > one.txt && git add one.txt && git commit -q --fixup HEAD~2 \
             && GIT_SEQUENCE_EDITOR=true git rebase -q -i --autosquash main",
        )
        .await;
    let before = git(&project, &["rev-parse", "main"]);
    let queued = worker
        .tool("git.commit", json!({ "land": true }))
        .await
        .expect("it lands");
    assert_eq!(
        queued["queued"]["commits"].as_array().map(Vec::len),
        Some(3),
        "{queued}"
    );
    moved(&project, "main", &before).await;

    for commit in ["main", "main~1", "main~2"] {
        let found = git(&project, &["rev-parse", commit]);
        let wrote = lead
            .tool("agent.wrote", json!({ "target": found }))
            .await
            .expect("who wrote it");
        assert_eq!(
            owners(&wrote),
            [("agent".to_owned(), worker_name.clone())],
            "{commit}: {wrote}"
        );
    }

    let led = lead
        .tool("git.commit", json!({ "push": true }))
        .await
        .expect("the lead pushes what its worker landed");
    assert_eq!(
        led["pushed"]["pushed"].as_array().map(Vec::len),
        Some(3),
        "{led}"
    );
    assert_eq!(
        git(&origin, &["log", "--format=%s", "main"]),
        "worker: picked\nworker: clash\nworker: one\nmain moves on\nbase"
    );
    commit_file(
        &project,
        "theirs.txt",
        "theirs\n",
        "someone else's, not pushed",
    );
    worker
        .run("printf 'last\\n' > last.txt && git add last.txt && git commit -qm 'worker: last'")
        .await;
    let before = git(&origin, &["rev-parse", "main"]);
    let queued = worker
        .tool("git.commit", json!({ "land": true, "push": true }))
        .await
        .expect("it pushes");
    assert_eq!(
        queued["queued"]["commits"].as_array().map(Vec::len),
        Some(1),
        "{queued}"
    );
    moved(&origin, "main", &before).await;
    assert_eq!(
        git(&origin, &["log", "--format=%s", "main"]),
        "worker: last\nworker: picked\nworker: clash\nworker: one\nmain moves on\nbase"
    );
    let tip = git(&origin, &["rev-parse", "main"]);
    let wrote = lead
        .tool("agent.wrote", json!({ "target": tip }))
        .await
        .expect("who wrote it");
    assert_eq!(
        owners(&wrote),
        [("agent".to_owned(), worker_name.clone())],
        "{wrote}"
    );

    let signed = git(&project, &["log", "-1", "--format=%B", &tip]);
    let footer = signed.rsplit("\n\n").next().expect("a footer");
    let forged = format!("forged\n\n{footer}");
    std::fs::write(project.join("forged.txt"), "forged\n").expect("write");
    git(&project, &["add", "forged.txt"]);
    git(&project, &["commit", "--quiet", "-m", &forged]);
    let found = git(&project, &["rev-parse", "main"]);
    let wrote = lead
        .tool("agent.wrote", json!({ "target": found }))
        .await
        .expect("who wrote it");
    assert_eq!(owners(&wrote)[0].0, "person", "{wrote}");
}

/// Everything stopped, the person writes to one agent, and that agent messages another that is
/// still held asleep: it wakes for the message rather than waiting for the person to open it.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_the_person_set_going_again_wakes_one_held_by_stopping_everything() {
    let root = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let project = root.join("project");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir(&project).expect("project");
    let socket: PathBuf = root.join("core.sock");

    let mut stopping = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
    let accounts = json!({
        "profiles": [{
            "id": "fake",
            "name": "Fake",
            "provider": "opencode",
            "accent": "#a277ff",
            "executablePath": FAKE_AGENT,
        }],
    });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("accounts");
    let added = json!({ "id": "widget", "name": "widget", "path": project });
    app.call("addProject", added).await.expect("the project");
    let (lead, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("the lead starts");
    let (held, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("the other starts");
    let _ = app.ask(ask::Shutdown { stop_all: true }).await;
    stopping.0.wait().expect("the core stops");
    drop(app);

    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
    app.ask(ask::RegisterWindow).await.expect("the window");
    let mut lead = Agent::open(&socket, &lead).await;
    let answer = lead
        .tool("agent.send", json!({ "to": held, "text": "carry on" }))
        .await
        .expect("the message");
    assert!(answer.to_string().contains("woke it first"), "{answer}");
}

/// A file a shell command wrote is credited to its agent only once the write settles, after the
/// turn ended, and the turn read "done, no changes" for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_whose_shell_command_wrote_a_file_reads_uncommitted() {
    let root = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let project = root.join("project");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir(&project).expect("project");
    git(&project, &["init", "--quiet", "-b", "main"]);
    commit_file(&project, "base.txt", "base\n", "base");

    let socket: PathBuf = root.join("core.sock");
    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
    let accounts = json!({
        "profiles": [{
            "id": "fake",
            "name": "Fake",
            "provider": "opencode",
            "accent": "#a277ff",
            "executablePath": FAKE_AGENT,
        }],
    });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("accounts");
    let added = json!({ "id": "widget", "name": "widget", "path": project });
    app.call("addProject", added).await.expect("the project");
    app.call("setAgentDefaults", json!({ "permissionMode": "bypass" }))
        .await
        .expect("the defaults");
    let (writer, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("the writer starts");
    let (onlooker, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("the onlooker starts");
    let mut writer = Agent::open(&socket, &writer).await;
    let mut onlooker = Agent::open(&socket, &onlooker).await;
    writer.run("echo hello > notes.txt").await;
    writer.chat.until(|chat| chat.turns_ended() >= 1).await;

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let listed = onlooker
            .tool("agent.list", json!({}))
            .await
            .expect("the agents");
        let turn = (listed["agents"].as_array().into_iter().flatten())
            .find(|agent| agent["agentId"] == writer.id.as_str())
            .map(|agent| agent["turn"].as_str().unwrap_or_default().to_owned())
            .unwrap_or_default();
        if turn.starts_with("done, uncommitted") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the writer's turn reads {turn:?}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
