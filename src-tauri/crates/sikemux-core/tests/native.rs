#![cfg(unix)]

//! Chat agents the core drives in their own protocol, through stand-ins for
//! their binaries.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use sikemux_core::client::{probe, ClientEvent, CoreClient};
use sikemux_core::protocol::{
    encode_control, read_frame_sync, BuildIdentity, ChatAttachment, ChatEvent, ChatEventKind,
    ChatLaunch, ClientMessage, Event, Request, PROTOCOL, PROTOCOL_VERSION,
};
use sikemux_core::server::{self, ServerConfig, ServerError};
use tokio::sync::mpsc::UnboundedReceiver;

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_sikemux-fake-claude");
const FAKE_CODEX: &str = env!("CARGO_BIN_EXE_sikemux-fake-codex");
const WAIT: Duration = Duration::from_secs(30);

struct TestCore {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    thread: Option<JoinHandle<Result<(), ServerError>>>,
}

impl TestCore {
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("core.sock");
        let config = ServerConfig {
            idle_exit: Duration::from_secs(600),
            build: BuildIdentity {
                version: "0.0.0-test".into(),
                commit: "chat".into(),
                built_at: 0,
                source: "chat".into(),
            },
            ..ServerConfig::new(socket.clone())
        };
        let thread = std::thread::spawn(move || server::run(config));
        let deadline = Instant::now() + WAIT;
        while probe(&socket, Duration::from_secs(1)).is_err() {
            assert!(Instant::now() < deadline, "the core never answered");
            std::thread::sleep(Duration::from_millis(5));
        }
        Self {
            _dir: dir,
            socket,
            thread: Some(thread),
        }
    }

    async fn connect(&self) -> (CoreClient, Chat) {
        let (client, events) = CoreClient::connect(&self.socket).await.expect("connect");
        (client, Chat::new(events))
    }
}

impl Drop for TestCore {
    fn drop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        if let Ok(mut stream) = StdUnixStream::connect(&self.socket) {
            let _ = stream.set_read_timeout(Some(WAIT));
            for message in [
                ClientMessage::Hello {
                    protocol: PROTOCOL.into(),
                    version: PROTOCOL_VERSION,
                    newest: None,
                },
                ClientMessage::Request {
                    request_id: 1,
                    request: Request::Shutdown { stop_all: true },
                },
            ] {
                let _ = stream.write_all(&encode_control(&message).expect("encode"));
            }
            while let Ok(Some(_)) = read_frame_sync(&mut stream) {}
        }
        let _ = thread.join();
    }
}

/// Every chat event one client heard, in order.
struct Chat {
    events: UnboundedReceiver<ClientEvent>,
    heard: Vec<ChatEvent>,
    /// Waiting looks only at events after the last one waited for.
    cursor: usize,
}

impl Chat {
    fn new(events: UnboundedReceiver<ClientEvent>) -> Self {
        Self {
            events,
            heard: Vec::new(),
            cursor: 0,
        }
    }

    async fn pump(&mut self) {
        let event = tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("timed out waiting for the core")
            .expect("the core disconnected");
        if let ClientEvent::Event(Event::Chat { event, .. }) = event {
            self.heard.push(event);
        }
    }

    async fn until(&mut self, mut found: impl FnMut(&ChatEvent) -> bool) -> ChatEvent {
        loop {
            if let Some(offset) = self.heard[self.cursor..].iter().position(&mut found) {
                let index = self.cursor + offset;
                self.cursor = index + 1;
                return self.heard[index].clone();
            }
            self.pump().await;
        }
    }

    async fn until_kind(&mut self, kind: ChatEventKind) -> ChatEvent {
        self.until(|event| event.kind == kind).await
    }

    fn text(&self) -> String {
        said(&self.heard)
    }
}

/// Everything the agent said, read the way the chat reads it.
fn said(events: &[ChatEvent]) -> String {
    updates(events)
        .iter()
        .filter(|update| update["update"]["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["update"]["content"]["text"].as_str())
        .collect()
}

/// Streamed updates one at a time, whether they came batched or not.
fn updates(events: &[ChatEvent]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event.kind == ChatEventKind::SessionUpdate)
        .flat_map(|event| match event.payload.get("updates") {
            Some(Value::Array(batch)) => batch.clone(),
            _ => vec![event.payload.clone()],
        })
        .collect()
}

fn claude_launch(agent_id: &str, config: &Path) -> ChatLaunch {
    ChatLaunch {
        agent_id: agent_id.into(),
        provider: "claude".into(),
        cwd: std::env::temp_dir(),
        program: PathBuf::from(FAKE_CLAUDE),
        args: Vec::new(),
        env: BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".to_owned(),
            config.to_string_lossy().into_owned(),
        )]),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: "bypass".into(),
        model: None,
        effort: Some("high".into()),
        account: None,
        fallbacks: Vec::new(),
    }
}

async fn prompt(client: &CoreClient, agent_id: &str, message_id: &str, text: &str) {
    client
        .acp_prompt(
            agent_id.into(),
            Some(message_id.into()),
            text.into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
}

fn message_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_chat_streams_a_turn_with_its_options_and_commands() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let start = client
        .acp_start(claude_launch("claude-a", config.path()))
        .await
        .expect("start");
    assert_eq!(start.capabilities["editing"], true);
    assert_eq!(start.setup["configOptions"][0]["id"], "model");
    assert_eq!(start.setup["configOptions"][1]["category"], "thought_level");
    assert_eq!(start.setup["configOptions"][1]["currentValue"], "high");
    chat.until_kind(ChatEventKind::Ready).await;

    prompt(&client, "claude-a", &message_id(), "stream 5").await;
    chat.until_kind(ChatEventKind::TurnStarted).await;
    let completed = chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(completed.payload["stopReason"], "end_turn");
    assert_eq!(chat.text(), "w0 w1 w2 w3 w4 ");
    let commands: Vec<Value> = updates(&chat.heard)
        .into_iter()
        .filter(|update| update["update"]["sessionUpdate"] == "available_commands_update")
        .collect();
    assert_eq!(
        commands[0]["update"]["availableCommands"][0]["name"],
        "review"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_tool_waits_for_the_persons_answer() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let mut launch = claude_launch("claude-b", config.path());
    launch.permission_mode = "workspace-write".into();
    client.acp_start(launch).await.expect("start");

    prompt(&client, "claude-b", &message_id(), "ask").await;
    let asked = chat.until_kind(ChatEventKind::PermissionRequest).await;
    assert_eq!(asked.payload["toolCall"]["title"], "echo hi");
    let allow = asked.payload["options"]
        .as_array()
        .and_then(|options| options.iter().find(|option| option["kind"] == "allow_once"))
        .and_then(|option| option["optionId"].as_str())
        .expect("an allow option")
        .to_owned();
    client
        .acp_permission_reply(
            "claude-b".into(),
            asked.payload["requestId"]
                .as_str()
                .expect("request id")
                .into(),
            Some(allow),
        )
        .await
        .expect("reply");
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert!(chat.text().ends_with("allowed"), "{}", chat.text());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_claude_turn_ends_cancelled() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    client
        .acp_start(claude_launch("claude-c", config.path()))
        .await
        .expect("start");
    prompt(&client, "claude-c", &message_id(), "hold 10000").await;
    chat.until(|event| said(std::slice::from_ref(event)).contains("holding"))
        .await;
    client.acp_cancel("claude-c".into()).await.expect("cancel");
    let completed = chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(completed.payload["stopReason"], "cancelled");
    assert!(!chat.text().contains("held"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_claude_message_takes_the_chat_back_to_before_it() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let (watcher, mut watching) = core.connect().await;
    client
        .acp_start(claude_launch("claude-d", config.path()))
        .await
        .expect("start");
    let first = message_id();
    let second = message_id();
    prompt(&client, "claude-d", &first, "first").await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    let attached = watcher.acp_attach("claude-d".into()).await.expect("attach");
    assert!(matches!(attached, ChatAttachment::Live { .. }));
    prompt(&client, "claude-d", &second, "second").await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;

    client
        .acp_edit(
            "claude-d".into(),
            second.clone(),
            "count".into(),
            Vec::new(),
            Vec::new(),
            false,
        )
        .await
        .expect("edit");
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert!(chat.text().ends_with("1 earlier"), "{}", chat.text());

    watching
        .until(|event| {
            updates(std::slice::from_ref(event)).iter().any(|update| {
                update["update"]["sessionUpdate"] == "message_rewound"
                    && update["update"]["messageId"] == second.as_str()
            })
        })
        .await;
    let (late, _) = core.connect().await;
    let ChatAttachment::Live { replay, .. } = late
        .acp_attach("claude-d".into())
        .await
        .expect("attach late")
    else {
        panic!("the chat is gone");
    };
    let replayed = said(&replay);
    assert!(replayed.contains("echo: first"), "{replayed}");
    assert!(!replayed.contains("echo: second"), "{replayed}");
    assert!(replayed.ends_with("1 earlier"), "{replayed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_claude_chat_replays_what_was_said() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let start = client
        .acp_start(claude_launch("claude-e", config.path()))
        .await
        .expect("start");
    prompt(&client, "claude-e", &message_id(), "hello there").await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    client.acp_stop("claude-e".into()).await.expect("stop");

    let (again, mut resumed) = core.connect().await;
    let mut launch = claude_launch("claude-f", config.path());
    launch.resume_id = Some(start.session_id.clone());
    let restarted = again.acp_start(launch).await.expect("resume");
    assert_eq!(restarted.session_id, start.session_id);
    resumed.until_kind(ChatEventKind::Ready).await;
    let replayed = updates(&resumed.heard);
    assert!(replayed.iter().any(|update| {
        update["update"]["sessionUpdate"] == "user_message_chunk"
            && update["update"]["content"]["text"] == "hello there"
    }));
    assert_eq!(resumed.text(), "echo: hello there");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_account_out_of_usage_fails_the_turn_as_a_limit() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    client
        .acp_start(claude_launch("claude-g", config.path()))
        .await
        .expect("start");
    prompt(&client, "claude-g", &message_id(), "limit").await;
    let error = chat.until_kind(ChatEventKind::Error).await;
    assert_eq!(error.payload["failure"]["kind"], "limit");
    assert_eq!(
        error.payload["failure"]["title"],
        "You've hit your limit · resets 3pm"
    );
    assert!(!chat.text().contains("hit your limit"));
}

fn codex_launch(agent_id: &str, home: &Path) -> ChatLaunch {
    ChatLaunch {
        provider: "codex".into(),
        program: PathBuf::from(FAKE_CODEX),
        env: BTreeMap::from([("CODEX_HOME".to_owned(), home.to_string_lossy().into_owned())]),
        ..claude_launch(agent_id, home)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_chat_streams_a_turn_with_its_options() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let start = client
        .acp_start(codex_launch("codex-a", home.path()))
        .await
        .expect("start");
    assert_eq!(start.capabilities["editing"], true);
    let options = start.setup["configOptions"].as_array().expect("options");
    assert!(options.iter().any(|option| option["id"] == "model"));
    assert!(options
        .iter()
        .any(|option| option["category"] == "thought_level" && option["currentValue"] == "high"));

    prompt(&client, "codex-a", &message_id(), "stream 5").await;
    let completed = chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(completed.payload["stopReason"], "end_turn");
    assert_eq!(chat.text(), "w0 w1 w2 w3 w4 ");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_command_waits_for_the_persons_answer() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let mut launch = codex_launch("codex-b", home.path());
    launch.permission_mode = "workspace-write".into();
    client.acp_start(launch).await.expect("start");

    prompt(&client, "codex-b", &message_id(), "ask").await;
    let asked = chat.until_kind(ChatEventKind::PermissionRequest).await;
    let allow = asked.payload["options"]
        .as_array()
        .and_then(|options| options.iter().find(|option| option["kind"] == "allow_once"))
        .and_then(|option| option["optionId"].as_str())
        .expect("an allow option")
        .to_owned();
    client
        .acp_permission_reply(
            "codex-b".into(),
            asked.payload["requestId"]
                .as_str()
                .expect("request id")
                .into(),
            Some(allow),
        )
        .await
        .expect("reply");
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert!(chat.text().ends_with("allowed"), "{}", chat.text());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_codex_turn_ends_cancelled() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    client
        .acp_start(codex_launch("codex-c", home.path()))
        .await
        .expect("start");
    prompt(&client, "codex-c", &message_id(), "hold 10000").await;
    chat.until(|event| said(std::slice::from_ref(event)).contains("holding"))
        .await;
    client.acp_cancel("codex-c".into()).await.expect("cancel");
    let completed = chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(completed.payload["stopReason"], "cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_codex_message_takes_the_thread_back_to_before_it() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    client
        .acp_start(codex_launch("codex-d", home.path()))
        .await
        .expect("start");
    let second = message_id();
    prompt(&client, "codex-d", &message_id(), "first").await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    prompt(&client, "codex-d", &second, "second").await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;

    client
        .acp_edit(
            "codex-d".into(),
            second,
            "count".into(),
            Vec::new(),
            Vec::new(),
            false,
        )
        .await
        .expect("edit");
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert!(chat.text().ends_with("1 earlier"), "{}", chat.text());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_codex_thread_replays_what_was_said() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let start = client
        .acp_start(codex_launch("codex-e", home.path()))
        .await
        .expect("start");
    prompt(&client, "codex-e", &message_id(), "hello there").await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    client.acp_stop("codex-e".into()).await.expect("stop");

    let (again, mut resumed) = core.connect().await;
    let mut launch = codex_launch("codex-f", home.path());
    launch.resume_id = Some(start.session_id.clone());
    let restarted = again.acp_start(launch).await.expect("resume");
    assert_eq!(restarted.session_id, start.session_id);
    resumed.until_kind(ChatEventKind::Ready).await;
    let replayed = updates(&resumed.heard);
    assert!(replayed.iter().any(|update| {
        update["update"]["sessionUpdate"] == "user_message_chunk"
            && update["update"]["content"]["text"] == "hello there"
    }));
    assert_eq!(resumed.text(), "echo: hello there");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_account_out_of_usage_fails_the_turn_as_a_limit() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    client
        .acp_start(codex_launch("codex-g", home.path()))
        .await
        .expect("start");
    prompt(&client, "codex-g", &message_id(), "limit").await;
    let error = chat.until_kind(ChatEventKind::Error).await;
    assert_eq!(error.payload["failure"]["kind"], "limit");
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_the_claude_model_starts_no_turn() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    client
        .acp_start(claude_launch("claude-model", config.path()))
        .await
        .expect("start");
    chat.until_kind(ChatEventKind::Ready).await;
    let options = client
        .acp_set_config("claude-model".into(), "model".into(), "small".into())
        .await
        .expect("set model");
    assert_eq!(options["configOptions"][0]["currentValue"], "small");
    prompt(&client, "claude-model", &message_id(), "after").await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    let turns = chat
        .heard
        .iter()
        .filter(|event| event.kind == ChatEventKind::TurnStarted)
        .count();
    assert_eq!(turns, 1, "only the prompt started a turn");
    assert!(!chat.text().contains("Set model"), "{}", chat.text());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_claude_chat_shows_the_model_its_settings_choose() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, _chat) = core.connect().await;
    let mut launch = claude_launch("claude-settings", config.path());
    launch.effort = None;
    launch
        .env
        .insert("FAKE_CLAUDE_SETTINGS_MODEL".into(), "small".into());
    let start = client.acp_start(launch).await.expect("start");
    assert_eq!(start.setup["configOptions"][0]["currentValue"], "small");
}

/// Runs the real `claude` named by `SIKEMUX_LIVE_CLAUDE` on a few short Haiku
/// turns: `cargo test -p sikemux-core --test native live_claude -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to Claude with the person's own account"]
async fn live_claude_streams_edits_and_resumes() {
    let Some(program) = std::env::var_os("SIKEMUX_LIVE_CLAUDE") else {
        return;
    };
    let folder = tempfile::tempdir().expect("folder");
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let launch = ChatLaunch {
        agent_id: "live".into(),
        provider: "claude".into(),
        cwd: folder.path().canonicalize().expect("folder"),
        program: PathBuf::from(program),
        args: Vec::new(),
        env: BTreeMap::new(),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: "bypass".into(),
        model: Some("haiku".into()),
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    };
    let start = client.acp_start(launch.clone()).await.expect("start");
    assert_eq!(start.setup["configOptions"][0]["currentValue"], "haiku");

    prompt(
        &client,
        "live",
        &message_id(),
        "Reply with exactly the word: alpha",
    )
    .await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert!(
        chat.text().to_lowercase().contains("alpha"),
        "{}",
        chat.text()
    );

    let second = message_id();
    prompt(
        &client,
        "live",
        &second,
        "Reply with exactly the word: beta",
    )
    .await;
    chat.until_kind(ChatEventKind::TurnCompleted).await;

    client
        .acp_edit(
            "live".into(),
            second,
            "How many messages did I send you before this one? Answer with just the digit.".into(),
            Vec::new(),
            Vec::new(),
            false,
        )
        .await
        .expect("edit");
    let completed = chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(completed.payload["stopReason"], "end_turn");
    let answer = chat.text();
    assert!(answer.trim_end().ends_with('1'), "{answer}");

    client.acp_stop("live".into()).await.expect("stop");
    let (again, mut resumed) = core.connect().await;
    let mut resume = launch;
    resume.agent_id = "live-again".into();
    resume.resume_id = Some(start.session_id.clone());
    again.acp_start(resume).await.expect("resume");
    resumed.until_kind(ChatEventKind::Ready).await;
    let replayed = updates(&resumed.heard);
    let users: Vec<&str> = replayed
        .iter()
        .filter(|update| update["update"]["sessionUpdate"] == "user_message_chunk")
        .filter_map(|update| update["update"]["content"]["text"].as_str())
        .collect();
    assert!(users.iter().any(|text| text.contains("alpha")), "{users:?}");
    assert!(!users.iter().any(|text| text.contains("beta")), "{users:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_claude_chat_comes_back_whole() {
    let config = tempfile::tempdir().expect("config");
    let session = uuid::Uuid::new_v4().to_string();
    let project = config.path().join("projects").join("fake");
    std::fs::create_dir_all(&project).expect("project");
    let mut transcript = String::new();
    let mut parent = Value::Null;
    for turn in 0..200 {
        let (asked, answered) = (format!("q{turn}"), format!("a{turn}"));
        for (uuid, record) in [
            (
                asked.clone(),
                serde_json::json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "text", "text": format!("question {turn}") }] } }),
            ),
            (
                answered.clone(),
                serde_json::json!({ "type": "assistant", "message": { "id": format!("msg{turn}"), "role": "assistant", "model": "fake", "content": [{ "type": "text", "text": "x".repeat(100_000) }] } }),
            ),
        ] {
            let mut record = record;
            record["uuid"] = Value::String(uuid.clone());
            record["parentUuid"] = parent.clone();
            record["sessionId"] = Value::String(session.clone());
            transcript.push_str(&record.to_string());
            transcript.push('\n');
            parent = Value::String(uuid);
        }
    }
    std::fs::write(project.join(format!("{session}.jsonl")), transcript).expect("transcript");

    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let mut launch = claude_launch("claude-long", config.path());
    launch.resume_id = Some(session);
    client.acp_start(launch).await.expect("resume");
    chat.until_kind(ChatEventKind::Ready).await;
    let answers = updates(&chat.heard)
        .into_iter()
        .filter(|update| update["update"]["sessionUpdate"] == "agent_message_chunk")
        .count();
    assert_eq!(answers, 200);
}
