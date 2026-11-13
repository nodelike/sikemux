#![cfg(unix)]

//! A real core and fake agents: what the person types to answer an access call, or types while
//! one is open, never reaches the project's chief; and only the chief retires a rule of the
//! person's as now a check, while other agents change one only in a turn the person started.

mod folded;

use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use folded::Chat;
use serde_json::{json, Value};
use sikemux_client::client::{ClientEvent, CoreClient};
use sikemux_core::client::probe;
use sikemux_view::chat::{ChatMessage, ChatPart, ChatRole};
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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = sikemux_process::user_environment::command("/usr/bin/git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
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

#[tokio::test(flavor = "multi_thread")]
async fn only_the_chief_retires_a_persons_rule_as_now_a_check_and_agents_only_on_their_word() {
    let root = tempfile::Builder::new()
        .prefix("chief")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let project = root.join("project");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir(&project).expect("project");
    git(&project, &["init", "--quiet", "-b", "main"]);
    git(
        &project,
        &["commit", "--quiet", "--allow-empty", "-m", "Start"],
    );
    git(
        &project,
        &["commit", "--quiet", "--allow-empty", "-m", "Lint expect"],
    );
    let older = git(&project, &["rev-parse", "HEAD"]);

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
    let keep = |text: &str| json!({ "project": project, "entry": { "kind": "rule", "text": text, "sensitive": false } });
    let mut kept = Value::Null;
    for text in [UNWRAP, PUSH] {
        kept = app
            .call("memoryAdd", keep(text))
            .await
            .expect("the person keeps a rule");
    }
    let unwrap = rule_id(&kept, UNWRAP);
    let push = rule_id(&kept, PUSH);
    let later = |args: &[&str]| {
        let at = format!("@{} +0000", now_secs() + 60);
        let output = sikemux_process::user_environment::command("/usr/bin/git")
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .env("GIT_COMMITTER_DATE", &at)
            .env("GIT_AUTHOR_DATE", &at)
            .current_dir(&project)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}");
    };
    later(&["commit", "--quiet", "--allow-empty", "-m", "Lint unwrap"]);
    let landed = git(&project, &["rev-parse", "HEAD"]);
    git(&project, &["checkout", "--quiet", "-b", "side"]);
    later(&["commit", "--quiet", "--allow-empty", "-m", "Lint push"]);
    let unlanded = git(&project, &["rev-parse", "HEAD"]);
    git(&project, &["checkout", "--quiet", "main"]);

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

    let card = worker
        .tool("board.add", json!({ "title": "Cache", "claim": true }))
        .await;
    let card = number_after(&card, "\"card\":").unwrap_or_else(|| panic!("{card}"));
    let decided = worker
        .tool(
            "board.decide",
            json!({ "text": "Keep the cache in sqlite", "why": "It is small" }),
        )
        .await;
    let decision = number_after(&decided, "Decision #").unwrap_or_else(|| panic!("{decided}"));
    app.call(
        "overruleDecision",
        json!({ "project": project, "goal": card, "decision": decision }),
    )
    .await
    .expect("the person overrules");
    worker.chat.until_said("overruled this").await;
    let rules = || app.call("memory", json!({ "project": project }));
    let overrule = rule_id(
        &rules().await.expect("the rules"),
        "Decisions like the person's overruled",
    );

    let wrong = |id: u64, said: String| json!({ "id": id, "wrong": said });
    let check = |id: u64, commit: &str| wrong(id, format!("now a check: {commit}"));
    let refused = |said: String, why: &str| {
        assert!(said.contains("tool failed") && said.contains(why), "{said}");
    };
    refused(
        worker.tool("memory.update", check(unwrap, &landed)).await,
        "Only this project's chief",
    );
    refused(
        chief.tool("memory.update", check(overrule, &landed)).await,
        "the rule an Overrule kept",
    );
    refused(
        worker
            .tool("memory.update", json!({ "id": overrule, "retire": true }))
            .await,
        "no agent changes it",
    );
    refused(
        chief.tool("memory.update", check(unwrap, &unlanded)).await,
        "has not landed",
    );
    refused(
        chief.tool("memory.update", check(unwrap, &older)).await,
        "is older than",
    );
    refused(
        chief
            .tool(
                "memory.update",
                wrong(
                    unwrap,
                    format!("superseded by the move to pnpm in {landed}"),
                ),
            )
            .await,
        "is a rule",
    );
    let letter = json!({
        "to": worker_id,
        "text": format!("tool memory.update {}", json!({ "id": push, "retire": true })),
    });
    let sent = chief.tool("agent.send", letter).await;
    assert!(sent.contains("tool answered"), "{sent}");
    let told = tokio::time::timeout(
        WAIT,
        worker
            .chat
            .until_said("only in a turn the person's own words started"),
    )
    .await;
    assert!(told.is_ok(), "{:#?}", worker.chat.view.rows);
    rule_id(&rules().await.expect("the rules"), PUSH);

    let counted = chief
        .tool("memory.update", wrong(unwrap, "said again".into()))
        .await;
    assert!(counted.contains("as said again"), "{counted}");
    let retired = chief.tool("memory.update", check(unwrap, &landed)).await;
    assert!(retired.contains("tool answered"), "{retired}");
    let removed = worker
        .tool("memory.update", json!({ "id": push, "retire": true }))
        .await;
    assert!(removed.contains("tool answered"), "{removed}");
    let left = rules().await.expect("the rules");
    assert!(find_rule(&left, UNWRAP).is_none(), "{left}");
    assert!(find_rule(&left, PUSH).is_none(), "{left}");
    for (text, id) in [(UNWRAP, unwrap), (PUSH, push)] {
        let back = app
            .call("memoryAdd", keep(text))
            .await
            .expect("Undo keeps the rule again");
        assert_eq!(rule_id(&back, text), id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_production_question_is_answered_only_on_its_call_where_it_was_drawn() {
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
    let (app, mut events) = CoreClient::connect(&socket).await.expect("connect");
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
    let (worker_id, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("a worker starts");
    let mut worker = Agent::open(&socket, &worker_id).await;
    worker.says("get ready", "echo:").await;
    app.ask(ask::WatchView).await.expect("watch the view");

    let asked = worker
        .tool(
            "agent.ask",
            json!({
                "question": "Release 2.0 to production now?",
                "reason": "irreversible",
                "breaks": "the release reaches every customer",
                "choices": ["Release it", "Hold it"],
                "pick": "Hold it",
            }),
        )
        .await;
    let question = asked
        .split_once("\"questionId\":\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(id, _)| id.to_owned())
        .unwrap_or_else(|| panic!("{asked}"));
    let refused = |answered: Result<Value, _>| {
        let error = format!("{:?}", answered.expect_err("refused"));
        assert!(error.contains("only by a tap on its call"), "{error}");
    };
    refused(
        app.call("answerQuestion", json!({ "id": question, "choice": 0 }))
            .await,
    );
    let call = loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("the view arrives")
            .expect("the core disconnected");
        let ClientEvent::DeviceView(view) = event else {
            continue;
        };
        let view: sikemux_view::device::DeviceView =
            serde_json::from_str(&view).expect("the view reads back");
        if let Some(call) = view.calls.into_iter().next() {
            break call;
        }
    };
    let listed = app
        .call("questions", json!({}))
        .await
        .expect("the questions");
    assert!(
        listed.to_string().contains("\"production\":true"),
        "{listed}"
    );
    refused(
        app.call("answerQuestion", json!({ "id": question, "choice": 0 }))
            .await,
    );
    refused(
        app.call(
            "answerQuestion",
            json!({ "id": question, "answer": "release it" }),
        )
        .await,
    );
    let hold = call.options.last().expect("an option").id.clone();
    app.call("callSeen", json!({ "id": call.id }))
        .await
        .expect("the call is drawn here");
    app.call("answerCall", json!({ "id": call.id, "option": hold }))
        .await
        .expect("the call drawn here is answered");

    let asked = worker
        .tool(
            "agent.ask",
            json!({
                "question": "Pay for the larger build machine?",
                "reason": "money",
                "breaks": "builds stay slow",
                "choices": ["Pay for it", "Keep the small one"],
                "pick": "Keep the small one",
            }),
        )
        .await;
    assert!(asked.contains("tool answered"), "{asked}");
    let money = loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("the view arrives")
            .expect("the core disconnected");
        let ClientEvent::DeviceView(view) = event else {
            continue;
        };
        let view: sikemux_view::device::DeviceView =
            serde_json::from_str(&view).expect("the view reads back");
        let found = (view.calls.into_iter()).find(|call| call.question.contains("build machine"));
        if let Some(call) = found {
            break call;
        }
    };
    let keep = money.options.last().expect("an option").id.clone();
    app.call("answerCall", json!({ "id": money.id, "option": keep }))
        .await
        .expect("a money call answered on its card replies");

    let (round, square) = (project.join("round.png"), project.join("square.png"));
    for picture in [&round, &square] {
        std::fs::write(picture, b"\x89PNG\r\n\x1a\n").expect("a picture");
    }
    let asked = worker
        .tool(
            "agent.ask",
            json!({
                "question": "Should the avatars in the rail be round or square?",
                "reason": "taste",
                "breaks": "the avatars change shape",
                "choices": [
                    { "label": "Round", "picture": round },
                    { "label": "Square", "picture": square },
                ],
                "pick": "Round",
            }),
        )
        .await;
    assert!(asked.contains("tool answered"), "{asked}");
    let taste = loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("the view arrives")
            .expect("the core disconnected");
        let ClientEvent::DeviceView(view) = event else {
            continue;
        };
        let view: sikemux_view::device::DeviceView =
            serde_json::from_str(&view).expect("the view reads back");
        let found = (view.calls.into_iter()).find(|call| call.question.contains("avatars"));
        if let Some(call) = found {
            break call;
        }
    };
    app.call("callNotMine", json!({ "id": taste.id }))
        .await
        .expect("a call handed back replies");
}

#[tokio::test(flavor = "multi_thread")]
async fn why_answers_a_broad_question_with_the_newest_decisions_and_a_specific_miss_with_none() {
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
    let (worker_id, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("a worker starts");
    let mut worker = Agent::open(&socket, &worker_id).await;
    worker
        .tool("board.add", json!({ "title": "Share", "claim": true }))
        .await;
    let decided = worker
        .tool(
            "board.decide",
            json!({ "text": "Shared links land in Inbox", "why": "The person reads Inbox first" }),
        )
        .await;
    assert!(decided.contains("Decision #"), "{decided}");

    let words = "what was decided in this project today and what each decision rests on";
    let answered = worker.tool("agent.why", json!({ "words": words })).await;
    assert!(
        answered.contains("Shared links land in Inbox"),
        "{answered}"
    );
    assert!(
        answered.contains("The person reads Inbox first"),
        "{answered}"
    );

    let missed = worker
        .tool(
            "agent.why",
            json!({ "words": "why did we drop the rail's model names" }),
        )
        .await;
    assert!(!missed.contains("newest decisions"), "{missed}");
    assert!(!missed.contains("Shared links land in Inbox"), "{missed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_overrule_undone_while_the_chief_sleeps_after_a_restart_shows_undone_in_its_chat() {
    let root = tempfile::Builder::new()
        .prefix("chief")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let project = root.join("project");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir(&project).expect("project");
    let socket = root.join("core.sock");
    let core = start_core(&root, &socket);
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
        .expect("the chief");
    let chief = opened["agentId"].as_str().expect("its id").to_owned();
    let mut boss = Agent::open(&socket, &chief).await;
    boss.chat.until(|chat| chat.turns_ended() >= 1).await;
    let (worker_id, _) = app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("a worker starts");
    let mut worker = Agent::open(&socket, &worker_id).await;
    worker
        .tool("board.add", json!({ "title": "Totals", "claim": true }))
        .await;
    for text in ["Use the old path", "Use the staging database"] {
        let decided = worker
            .tool("board.decide", json!({ "text": text, "why": "faster" }))
            .await;
        assert!(decided.contains("Decision #"), "{decided}");
    }
    let decision = json!({ "project": project, "goal": 1, "decision": 2 });
    app.call("overruleDecision", decision.clone())
        .await
        .expect("the person overrules");
    let overruled = |chat: &folded::Folded, undone: bool| {
        chat.parts().any(|part| {
            matches!(part, ChatPart::AgentNotice { notice, .. }
                if notice.overruled.as_ref().is_some_and(|line| line.decision == 2 && line.undone == undone))
        })
    };
    boss.chat.until(|chat| overruled(chat, false)).await;
    app.call("stopChatIfIdle", json!({ "agentId": chief }))
        .await
        .expect("the chief sleeps");
    tokio::time::sleep(Duration::from_secs(2)).await;
    drop((boss, worker, app, core));

    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect again");
    app.ask(ask::WatchView).await.expect("watch");
    let deadline = Instant::now() + WAIT;
    while app.call("undoOverrule", decision.clone()).await.is_err() {
        assert!(Instant::now() < deadline, "the Overrule was never undone");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut boss = Agent::open(&socket, &chief).await;
    let shown = tokio::time::timeout(
        Duration::from_secs(20),
        boss.chat.until(|chat| overruled(chat, true)),
    )
    .await;
    assert!(shown.is_ok(), "{:?}", boss.chat.notices());
    assert!(!overruled(&boss.chat, false), "{:?}", boss.chat.notices());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chiefs_chat_is_titled_only_from_what_the_person_wrote() {
    let root = tempfile::Builder::new()
        .prefix("chief")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let project = root.join("project");
    let claude = root.join("claude");
    std::fs::create_dir_all(root.join("home")).expect("home");
    std::fs::create_dir_all(&claude).expect("claude");
    std::fs::write(
        claude.join("fake-account.json"),
        json!({ "loggedIn": true, "authMethod": "claude.ai", "email": "chief@example.com", "subscriptionType": "max" }).to_string(),
    )
    .expect("an account");
    std::fs::create_dir(&project).expect("project");
    let folder: String = (project.to_string_lossy().chars())
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let sessions = claude.join("projects").join(folder);
    std::fs::create_dir_all(&sessions).expect("its sessions");
    let socket = root.join("core.sock");
    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
    let accounts = json!({ "profiles": [{
        "id": "builtin-claude",
        "name": "Claude",
        "provider": "claude",
        "accent": "#d97757",
        "executablePath": FAKE_CLAUDE,
        "configPath": claude,
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
        .expect("the chief");
    let chief = opened["agentId"].as_str().expect("its id").to_owned();
    let mut boss = Agent::open(&socket, &chief).await;
    boss.chat.until(|chat| chat.turns_ended() >= 1).await;
    let (worker, _) = app
        .start_chat("claude:builtin-claude".into(), "widget".into(), None)
        .await
        .expect("a worker starts");
    let mut working = Agent::open(&socket, &worker).await;
    working.says("Sort the inbox", "Sort the inbox").await;
    boss.says("Plan the launch week", "Plan the launch week")
        .await;

    let agent = |id: String| {
        let app = &app;
        async move {
            let workspace = app
                .call("workspace", json!({}))
                .await
                .expect("the workspace");
            (workspace["agents"].as_array().into_iter().flatten())
                .find(|agent| agent["id"] == id.as_str())
                .cloned()
                .unwrap_or_default()
        }
    };
    let titled = |agent: &Value| {
        (agent["titled"] == true)
            .then(|| agent["title"].as_str().map(str::to_owned))
            .flatten()
    };
    let until_titled = |id: String, wanted: &'static str| {
        let agent = &agent;
        async move {
            let deadline = Instant::now() + WAIT;
            while titled(&agent(id.clone()).await).as_deref() != Some(wanted) {
                assert!(Instant::now() < deadline, "{id} was never titled {wanted}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    until_titled(chief.clone(), "Plan the launch week").await;

    for (id, named) in [
        (&chief, "Chief of widget first message"),
        (&worker, "Inbox sorted by Claude"),
    ] {
        let session = agent(id.clone()).await["resumeId"]
            .as_str()
            .expect("its session")
            .to_owned();
        let line = json!({ "type": "ai-title", "aiTitle": named, "sessionId": session });
        std::fs::write(
            sessions.join(format!("{session}.jsonl")),
            format!("{line}\n"),
        )
        .expect("Claude's title");
    }
    until_titled(worker.clone(), "Inbox sorted by Claude").await;
    assert_eq!(
        titled(&agent(chief.clone()).await).as_deref(),
        Some("Plan the launch week")
    );
}

const UNWRAP: &str = "Never call unwrap outside tests";
const PUSH: &str = "Never push to main without a review";

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_secs()
}

fn find_rule(memory: &Value, starts: &str) -> Option<u64> {
    (memory["folders"].as_array()?.iter())
        .flat_map(|folder| folder["rules"].as_array().into_iter().flatten())
        .map(|listed| &listed["rule"])
        .find(|rule| {
            rule["text"]
                .as_str()
                .is_some_and(|text| text.starts_with(starts))
        })
        .and_then(|rule| rule["id"].as_u64())
}

fn rule_id(memory: &Value, starts: &str) -> u64 {
    find_rule(memory, starts).unwrap_or_else(|| panic!("no rule {starts} in {memory}"))
}

fn number_after(text: &str, key: &str) -> Option<u64> {
    let digits = text.split_once(key)?.1.trim_start();
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse().ok()
}
