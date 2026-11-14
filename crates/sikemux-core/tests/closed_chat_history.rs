#![cfg(unix)]

//! Card #239: what a chat shows when taken up again from Recent after it was closed, with and
//! without the core handing over to a new build of itself in between.

mod folded;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use folded::Chat;
use serde_json::{json, Value};
use sikemux_client::client::CoreClient;
use sikemux_core::build::BUILD_ID_OVERRIDE_ENV;
use sikemux_core::client::{await_upgrade, frozen_request, probe};
use sikemux_core::frozen::{FrozenReply, FrozenRequest};
use sikemux_view::chat::ChatPart;
use sikemux_wire::protocol::{ask, BuildIdentity};

const CORE: &str = env!("CARGO_BIN_EXE_sikemux-core");
const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_sikemux-fake-claude");
const WAIT: Duration = Duration::from_secs(90);
const RULE: &str = "In widget, commit messages say what changed in the present tense";

struct Sandbox {
    root: PathBuf,
    child: Option<Child>,
    socket: PathBuf,
    report: String,
}

impl Sandbox {
    fn start() -> Self {
        let root = tempfile::Builder::new()
            .prefix("qa239")
            .tempdir_in("/tmp")
            .expect("temp dir")
            .keep();
        let root = std::fs::canonicalize(root).expect("canonical");
        let claude = root.join("claude");
        let bin = root.join("bin");
        for dir in [
            root.join("home"),
            claude.clone(),
            bin.clone(),
            root.join("project"),
        ] {
            std::fs::create_dir_all(dir).expect("dir");
        }
        std::os::unix::fs::symlink(FAKE_CLAUDE, bin.join("claude")).expect("claude on PATH");
        std::fs::write(
            claude.join("fake-account.json"),
            json!({ "loggedIn": true, "authMethod": "claude.ai", "email": "qa@example.com", "subscriptionType": "max" }).to_string(),
        )
        .expect("account");
        std::fs::write(root.join("script.json"), json!([
            { "when": "take the totals card", "steps": [
                { "tool": "mcp__sikemux-tools__board_add", "call": "board.add", "input": { "title": "Totals", "claim": true } },
                { "say": "Card taken." } ] },
            { "when": "decide the inbox", "steps": [
                { "tool": "mcp__sikemux-tools__board_decide", "call": "board.decide", "input": { "text": "Inbox for shared links", "why": "they land untriaged" } },
                { "say": "Decided." } ] },
            { "when": "keep my rule", "steps": [
                { "tool": "mcp__sikemux-tools__memory_add", "call": "memory.add", "input": { "kind": "rule", "person": true, "text": format!("{RULE}.") } },
                { "say": "Rule kept." } ] },
        ]).to_string()).expect("script");
        let socket = root.join("core.sock");
        let mut sandbox = Self {
            root,
            child: None,
            socket,
            report: String::new(),
        };
        sandbox.child = Some(sandbox.spawn());
        sandbox
    }

    fn spawn(&self) -> Child {
        let root = &self.root;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("core.log"))
            .expect("log");
        let endpoint = root.join("cli.json");
        let child = sikemux_process::user_environment::command(CORE)
            .arg("core")
            .arg("--socket")
            .arg(&self.socket)
            .arg("--cli-endpoint")
            .arg(&endpoint)
            .arg("--data-dir")
            .arg(root.join("data"))
            .arg("--idle-exit-secs")
            .arg("600")
            .arg("--handover-settle-ms")
            .arg("500")
            .env(BUILD_ID_OVERRIDE_ENV, "first")
            .env("HOME", root.join("home"))
            .env("SHELL", "/bin/sh")
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            )
            .env("FAKE_CLAUDE_SCRIPT", root.join("script.json"))
            .env(sikemux_process::vars::AGENT_SEARCH, "path")
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("log"))
            .stderr(log)
            .spawn()
            .expect("core");
        let deadline = Instant::now() + WAIT;
        while probe(&self.socket, Duration::from_secs(1)).is_err() || !endpoint.exists() {
            assert!(Instant::now() < deadline, "the core never answered");
            std::thread::sleep(Duration::from_millis(10));
        }
        child
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.root.join("core.log")).unwrap_or_default()
    }

    fn note(&mut self, line: impl AsRef<str>) {
        println!("{}", line.as_ref());
        self.report.push_str(line.as_ref());
        self.report.push('\n');
    }

    fn disk(&mut self, when: &str) {
        self.note(format!("-- on disk {when}"));
        for folder in ["chat-history", "chats"] {
            let dir = self.root.join("data").join(folder);
            let mut files: Vec<String> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| {
                    let bytes = entry.metadata().map_or(0, |meta| meta.len());
                    format!(
                        "{folder}/{} ({bytes} B)",
                        entry.file_name().to_string_lossy()
                    )
                })
                .collect();
            files.sort();
            if files.is_empty() {
                self.note(format!("   {folder}/ empty"));
            }
            for file in files {
                self.note(format!("   {file}"));
            }
        }
    }

    /// Kills the core with no warning, as a crash does, and starts another on the same socket
    /// and data folder.
    fn crash_and_start_again(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().expect("kill the core");
            child.wait().expect("reap the core");
        }
        self.child = Some(self.spawn());
        self.note("-- crashed and started again");
    }

    async fn hand_over(&mut self) {
        let binary = self.root.join("second-build");
        std::fs::write(
            &binary,
            format!("#!/bin/sh\n{BUILD_ID_OVERRIDE_ENV}='second' exec '{CORE}' \"$@\"\n"),
        )
        .expect("binary");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let socket = self.socket.clone();
        let reply = tokio::task::spawn_blocking(move || {
            frozen_request(
                &socket,
                &FrozenRequest::Upgrade {
                    binary,
                    force: false,
                },
                WAIT,
            )
        })
        .await
        .expect("join")
        .expect("answered");
        assert_eq!(reply, FrozenReply::Accepted, "{}", self.log());
        let socket = self.socket.clone();
        let pid = self.child.as_ref().expect("core").id();
        let old = BuildIdentity {
            version: env!("CARGO_PKG_VERSION").into(),
            commit: "first".into(),
            built_at: 0,
            source: "first".into(),
        };
        let hello =
            tokio::task::spawn_blocking(move || await_upgrade(&socket, pid, Some(&old), WAIT))
                .await
                .expect("join")
                .unwrap_or_else(|error| panic!("{error}\n{}", self.log()));
        assert_eq!(hello.build.commit, "second");
        self.note(format!("-- handed over to build {}", hello.build.commit));
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = frozen_request(
            &self.socket,
            &FrozenRequest::StopEverything,
            Duration::from_secs(5),
        );
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_secs(10);
            while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(out) = std::env::var_os("QA239_OUT") {
            let name = std::thread::current()
                .name()
                .unwrap_or("qa239")
                .replace("::", "-");
            let _ = std::fs::write(Path::new(&out).join(format!("{name}.txt")), &self.report);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn rows(chat: &folded::Folded) -> Vec<String> {
    let short = |text: &str| text.chars().take(60).collect::<String>().replace('\n', " ");
    chat.view
        .rows
        .iter()
        .flat_map(|row| {
            let role = format!("{:?}", row.role);
            row.parts.iter().map(move |part| {
                let shown = match part {
                    ChatPart::Text { text, .. } => format!("text {:?}", short(text)),
                    ChatPart::AgentNotice { notice, .. } => format!(
                        "notice {:?}{}{}",
                        notice.title,
                        if notice.decided.is_some() {
                            " [decided]"
                        } else {
                            ""
                        },
                        if notice.rule.is_some() { " [rule]" } else { "" },
                    ),
                    other => {
                        let value = serde_json::to_value(other).unwrap_or_default();
                        let kind = value["kind"].as_str().unwrap_or("?").to_owned();
                        let name = (value.get("title").or(value.get("name")))
                            .and_then(Value::as_str)
                            .map(|name| format!(" {:?}", short(name)))
                            .unwrap_or_default();
                        format!("{kind}{name}")
                    }
                };
                format!("{role}: {shown}")
            })
        })
        .collect()
}

async fn open(socket: &Path, id: &str) -> (CoreClient, Chat) {
    let (client, events) = CoreClient::connect(socket).await.expect("connect");
    let mut chat = Chat::new(events, id);
    chat.wait = WAIT;
    chat.attached(&client.acp_attach(id.to_owned()).await.expect("attach"));
    (client, chat)
}

async fn says(client: &CoreClient, chat: &mut Chat, id: &str, text: &str, reply: &str) {
    let times = chat.text().matches(reply).count() + 1;
    client
        .ask(ask::AcpPrompt {
            agent_id: id.to_owned(),
            message_id: Some(uuid::Uuid::new_v4().to_string()),
            text: text.to_owned(),
            paths: Vec::new(),
            context: Vec::new(),
        })
        .await
        .expect("prompt");
    chat.until(|chat| chat.text().matches(reply).count() >= times)
        .await;
}

/// Starts a Claude chat in a fresh sandbox, gives it turns and the rows only Sikemux holds,
/// and answers the sandbox, the chat's agent and its rows.
async fn chat_with_sikemux_rows() -> (Sandbox, CoreClient, String, Vec<String>) {
    let mut sandbox = Sandbox::start();
    let (app, _events) = CoreClient::connect(&sandbox.socket).await.expect("connect");
    app.call(
        "setAgentAccounts",
        json!({ "profiles": [{
            "id": "builtin-claude", "name": "Claude", "provider": "claude", "accent": "#d97757",
            "executablePath": FAKE_CLAUDE, "configPath": sandbox.root.join("claude"),
        }] }),
    )
    .await
    .expect("accounts");
    app.call(
        "addProject",
        json!({ "id": "widget", "name": "widget", "path": sandbox.root.join("project") }),
    )
    .await
    .expect("project");
    let (id, _) = app
        .start_chat("claude:builtin-claude".into(), "widget".into(), None)
        .await
        .expect("chat");
    let (client, mut chat) = open(&sandbox.socket, &id).await;
    says(&client, &mut chat, &id, "hello there", "echo: hello there").await;
    says(
        &client,
        &mut chat,
        &id,
        "take the totals card",
        "Card taken.",
    )
    .await;
    says(&client, &mut chat, &id, "decide the inbox", "Decided.").await;
    says(
        &client,
        &mut chat,
        &id,
        &format!("keep my rule: {RULE}"),
        "Rule kept.",
    )
    .await;
    chat.until(|chat| {
        let notices = chat.notices();
        notices.iter().any(|title| title == "Kept as a rule")
            && chat.parts().any(|part| {
                matches!(part, ChatPart::AgentNotice { notice, .. } if notice.decided.is_some())
            })
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let before = rows(&chat);
    sandbox.note("== rows while open");
    for row in &before {
        sandbox.note(format!("   {row}"));
    }
    sandbox.disk("while open");
    (sandbox, app, id, before)
}

/// Takes the closed chat up again, as the app does from its closed tabs or Recent: a chat
/// resuming the agent's saved session. Answers its agent.
async fn reopen(sandbox: &Sandbox, app: &CoreClient, closed: &str) -> String {
    let deadline = Instant::now() + WAIT;
    let taken = loop {
        match app.call("reopenAgent", json!({ "agentId": closed })).await {
            Ok(taken) => break taken,
            Err(error)
                if Instant::now() < deadline && error.to_string().contains("no longer start") =>
            {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(error) => panic!("reopen: {error}\n{}", sandbox.log()),
        }
    };
    taken["agentId"].as_str().expect("its agent").to_owned()
}

async fn wake(sandbox: &Sandbox, agent_id: &str) {
    let (waker, _events) = CoreClient::connect(&sandbox.socket).await.expect("connect");
    waker
        .ask(ask::AcpWake {
            agent_id: agent_id.to_owned(),
        })
        .await
        .unwrap_or_else(|error| panic!("wake: {error}\n{}", sandbox.log()));
}

/// The chat's rows once it shows `last` and has settled.
async fn settled(sandbox: &mut Sandbox, agent_id: &str, last: &str) -> Vec<String> {
    let (_client, mut chat) = open(&sandbox.socket, agent_id).await;
    chat.until(|chat| chat.text().contains(last)).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (_client, chat) = open(&sandbox.socket, agent_id).await;
    rows(&chat)
}

/// Writes down `after` and answers the rows of `before` it lacks and those it shows more often.
fn compare(
    sandbox: &mut Sandbox,
    case: &str,
    before: &[String],
    after: &[String],
) -> (Vec<String>, Vec<String>) {
    sandbox.note(format!("== rows after {case}"));
    for row in after {
        sandbox.note(format!("   {row}"));
    }
    let count = |rows: &[String], row: &String| rows.iter().filter(|held| *held == row).count();
    let mut lost = Vec::new();
    let mut doubled = Vec::new();
    for row in before {
        let (was, is) = (count(before, row), count(after, row));
        if is < was && !lost.contains(row) {
            lost.push(row.clone());
        }
        if is > was && !doubled.contains(row) {
            doubled.push(row.clone());
        }
    }
    sandbox.note(format!(
        "== {case}: {}",
        if lost.is_empty() && doubled.is_empty() {
            "nothing lost".to_owned()
        } else {
            format!("lost {lost:?}, doubled {doubled:?}")
        }
    ));
    (lost, doubled)
}

#[derive(Clone, Copy, PartialEq)]
enum Between {
    Moment,
    Minute,
    HandOver,
    Crash,
}

impl Between {
    fn named(self) -> &'static str {
        match self {
            Between::Moment => "",
            Between::Minute => ", 65 s",
            Between::HandOver => ", handover",
            Between::Crash => ", crash",
        }
    }

    async fn pass(self, sandbox: &mut Sandbox) {
        match self {
            Between::Moment => tokio::time::sleep(Duration::from_secs(2)).await,
            Between::Minute => tokio::time::sleep(Duration::from_secs(65)).await,
            Between::HandOver => sandbox.hand_over().await,
            Between::Crash => sandbox.crash_and_start_again(),
        }
        sandbox.disk(&format!("after{}", self.named()));
    }
}

fn transcript(sandbox: &Sandbox) -> PathBuf {
    let folder = sandbox.root.join("claude").join("projects").join("fake");
    (std::fs::read_dir(&folder).expect("its sessions").flatten())
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .expect("the session")
}

/// Adds a turn to the agent's own record of the session, as one taken up in a terminal does.
fn grow_outside(sandbox: &Sandbox) {
    let path = transcript(sandbox);
    let session = path
        .file_stem()
        .expect("named")
        .to_string_lossy()
        .into_owned();
    let text = std::fs::read_to_string(&path).expect("the record");
    let leaf: Value = text
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .expect("a record");
    let (asked, said) = (
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    );
    let lines = [
        json!({ "type": "user", "uuid": asked, "parentUuid": leaf["uuid"], "sessionId": session, "isSidechain": false,
            "message": { "role": "user", "content": "typed in a terminal" } }),
        json!({ "type": "assistant", "uuid": said, "parentUuid": asked, "sessionId": session, "isSidechain": false,
            "message": { "id": "msg-outside", "model": "fake", "role": "assistant", "content": [{ "type": "text", "text": "grown outside" }] } }),
    ];
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open");
    for line in lines {
        use std::io::Write;
        writeln!(file, "{line}").expect("append");
    }
}

/// Closes the chat as the person does, and waits for the workspace to have written it down.
async fn close(sandbox: &mut Sandbox, app: &CoreClient, agent_id: &str) {
    app.call("closeChat", json!({ "agentId": agent_id }))
        .await
        .expect("close");
    let workspace = sandbox.root.join("data").join("workspace.json");
    let deadline = Instant::now() + WAIT;
    while std::fs::read_to_string(&workspace).is_ok_and(|saved| saved.contains(agent_id)) {
        assert!(Instant::now() < deadline, "the close was never saved");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    sandbox.disk("after closing");
}

async fn closed_then_reopened(between: Between, grown: bool) {
    let (mut sandbox, app, id, before) = chat_with_sikemux_rows().await;
    close(&mut sandbox, &app, &id).await;
    between.pass(&mut sandbox).await;
    if grown {
        grow_outside(&sandbox);
    }
    let app = CoreClient::connect(&sandbox.socket)
        .await
        .expect("connect")
        .0;
    let reopened = reopen(&sandbox, &app, &id).await;
    let last = if grown { "grown outside" } else { "Rule kept." };
    let after = settled(&mut sandbox, &reopened, last).await;
    sandbox.disk("after reopening");
    let case = format!(
        "close{}{}, reopen",
        between.named(),
        if grown { ", session grows outside" } else { "" }
    );
    let (lost, doubled) = compare(&mut sandbox, &case, &before, &after);
    assert!(
        lost.is_empty() && doubled.is_empty(),
        "{case}: lost {lost:?}, doubled {doubled:?}"
    );
    if grown {
        let outside =
            |row: &&String| row.contains("grown outside") || row.contains("typed in a terminal");
        assert_eq!(after.iter().filter(outside).count(), 2, "{after:#?}");
        assert!(
            after.iter().rposition(|row| row.contains("Rule kept."))
                < after.iter().position(|row| row.contains("grown outside"))
        );
    }
}

async fn asleep_then_woken(between: Between) {
    let (mut sandbox, app, id, before) = chat_with_sikemux_rows().await;
    app.call("sleepAgent", json!({ "agentId": id }))
        .await
        .expect("sleep");
    tokio::time::sleep(Duration::from_secs(2)).await;
    sandbox.disk("asleep");
    between.pass(&mut sandbox).await;
    let (_client, chat) = open(&sandbox.socket, &id).await;
    let asleep = rows(&chat);
    let case = format!("sleep{}, read asleep", between.named());
    let (lost, doubled) = compare(&mut sandbox, &case, &before, &asleep);
    assert!(
        lost.is_empty() && doubled.is_empty(),
        "{case}: lost {lost:?}, doubled {doubled:?}"
    );
    wake(&sandbox, &id).await;
    let after = settled(&mut sandbox, &id, "Rule kept.").await;
    sandbox.disk("after waking");
    let case = format!("sleep{}, wake", between.named());
    let (lost, doubled) = compare(&mut sandbox, &case, &before, &after);
    assert!(
        lost.is_empty() && doubled.is_empty(),
        "{case}: lost {lost:?}, doubled {doubled:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn qa239_close_then_reopen() {
    closed_then_reopened(Between::Moment, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qa239_close_wait_a_minute_then_reopen() {
    closed_then_reopened(Between::Minute, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qa239_close_hand_over_then_reopen() {
    closed_then_reopened(Between::HandOver, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qa239_close_crash_then_reopen() {
    closed_then_reopened(Between::Crash, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qa239_close_session_grows_outside_then_reopen() {
    closed_then_reopened(Between::Moment, true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qa239_sleep_then_wake() {
    asleep_then_woken(Between::Moment).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qa239_sleep_hand_over_then_wake() {
    asleep_then_woken(Between::HandOver).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qa239_sleep_crash_then_wake() {
    asleep_then_woken(Between::Crash).await;
}

/// A closed chat whose agent no longer keeps its session is gone from Recent, and so is the
/// history Sikemux kept for it.
#[tokio::test(flavor = "multi_thread")]
async fn qa239_a_closed_chat_gone_from_recent_lets_go_of_its_history() {
    let (mut sandbox, app, id, _) = chat_with_sikemux_rows().await;
    close(&mut sandbox, &app, &id).await;
    let history = sandbox.root.join("data").join("chat-history");
    let kept = |history: &Path| {
        (std::fs::read_dir(history).into_iter().flatten().flatten())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .count()
    };
    assert_eq!(kept(&history), 1, "{}", sandbox.log());
    std::fs::remove_file(transcript(&sandbox)).expect("the session goes");
    sandbox.crash_and_start_again();
    let deadline = Instant::now() + WAIT;
    while kept(&history) > 0 {
        assert!(
            Instant::now() < deadline,
            "the kept history stayed\n{}",
            sandbox.log()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let chiefs = sandbox.root.join("data").join("chiefs.json");
    while std::fs::read_to_string(&chiefs).is_ok_and(|kept| kept.contains(&id)) {
        assert!(
            Instant::now() < deadline,
            "the chief kept its lines\n{}",
            sandbox.log()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    sandbox.disk("after the session left Recent");
}

/// Opens a turn the agent holds open until `gate` is gone, and answers the rows once it shows
/// the turn started.
async fn holding_turn(sandbox: &Sandbox, agent_id: &str, gate: &Path) -> Vec<String> {
    std::fs::write(gate, b"").expect("gate");
    let (client, mut chat) = open(&sandbox.socket, agent_id).await;
    client
        .ask(ask::AcpPrompt {
            agent_id: agent_id.to_owned(),
            message_id: Some(uuid::Uuid::new_v4().to_string()),
            text: format!("hold {}", gate.display()),
            paths: Vec::new(),
            context: Vec::new(),
        })
        .await
        .expect("prompt");
    chat.until(|chat| chat.text().contains("holding")).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    rows(&chat)
}

fn once(sandbox: &mut Sandbox, after: &[String], said: &str) {
    let times = after.iter().filter(|row| row.contains(said)).count();
    sandbox.note(format!("== {said:?} shown {times} times"));
    assert_eq!(times, 1, "{said}: {after:#?}");
}

/// Review of #239: a chat closed in the middle of a turn showed that turn twice once reopened,
/// read again from the agent's record past where the kept rows already reached.
#[tokio::test(flavor = "multi_thread")]
async fn qa239_close_mid_turn_then_reopen() {
    let (mut sandbox, app, id, _) = chat_with_sikemux_rows().await;
    let gate = sandbox.root.join("gate");
    let before = holding_turn(&sandbox, &id, &gate).await;
    close(&mut sandbox, &app, &id).await;
    let reopened = reopen(&sandbox, &app, &id).await;
    let after = settled(&mut sandbox, &reopened, "holding").await;
    let (lost, doubled) = compare(&mut sandbox, "close mid-turn, reopen", &before, &after);
    assert!(
        lost.is_empty() && doubled.is_empty(),
        "lost {lost:?}, doubled {doubled:?}"
    );
    once(&mut sandbox, &after, "holding");
}

/// A chat whose core is killed in the middle of a turn comes back with every row, the cut turn
/// once, and again so once closed and reopened.
#[tokio::test(flavor = "multi_thread")]
async fn qa239_crash_mid_turn_then_reopen() {
    let (mut sandbox, _, id, _) = chat_with_sikemux_rows().await;
    let gate = sandbox.root.join("gate");
    let before = holding_turn(&sandbox, &id, &gate).await;
    sandbox.crash_and_start_again();
    std::fs::remove_file(&gate).expect("the turn may end");
    let carried = settled(&mut sandbox, &id, "Rule kept.").await;
    let (lost, doubled) = compare(&mut sandbox, "crash mid-turn", &before, &carried);
    assert!(
        lost.is_empty() && doubled.is_empty(),
        "lost {lost:?}, doubled {doubled:?}"
    );
    once(&mut sandbox, &carried, "holding");
    let app = CoreClient::connect(&sandbox.socket)
        .await
        .expect("connect")
        .0;
    close(&mut sandbox, &app, &id).await;
    let reopened = reopen(&sandbox, &app, &id).await;
    let after = settled(&mut sandbox, &reopened, "Rule kept.").await;
    let case = "crash mid-turn, close, reopen";
    let (lost, doubled) = compare(&mut sandbox, case, &before, &after);
    assert!(
        lost.is_empty() && doubled.is_empty(),
        "lost {lost:?}, doubled {doubled:?}"
    );
    once(&mut sandbox, &after, "holding");
}
