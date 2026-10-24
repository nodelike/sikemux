#![cfg(unix)]

//! Chat agents the core drives in their own protocol, through stand-ins for
//! their binaries.

mod folded;

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use folded::{prompts, said, Chat, Folded};
use serde_json::Value;
use sikemux_core::client::{probe, CoreClient};
use sikemux_core::protocol::{
    encode_control, read_frame_sync, BuildIdentity, ChatAccount, ChatAttachment, ChatLaunch,
    ClientMessage, Request, PROTOCOL, PROTOCOL_VERSION,
};
use sikemux_core::server::{self, ServerConfig, ServerError};
use sikemux_wire::chat::{ChatPage, ChatRole, FailureKind, PermissionAsk};

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
            data_dir: Some(dir.path().join("data")),
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

    async fn connect(&self, agent_id: &str) -> (CoreClient, Chat) {
        let (client, events) = CoreClient::connect(&self.socket).await.expect("connect");
        (client, Chat::new(events, agent_id))
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

/// The allow-once answer to the question the chat asks, and its request.
fn allow_once(asked: &PermissionAsk) -> (String, String) {
    let allow = asked
        .options
        .iter()
        .find(|option| option.kind == "allow_once")
        .expect("an allow option");
    (asked.request_id.clone(), allow.option_id.clone())
}

async fn until_asked(chat: &mut Chat) -> PermissionAsk {
    chat.until(|chat| !chat.head().permissions.is_empty())
        .await
        .head()
        .permissions[0]
        .clone()
}

async fn until_failed(chat: &mut Chat) -> Folded {
    chat.until(|chat| chat.head().failure.is_some())
        .await
        .clone()
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
    let (client, mut chat) = core.connect("claude-a").await;
    let start = client
        .acp_start(claude_launch("claude-a", config.path()))
        .await
        .expect("start");
    assert_eq!(start.capabilities["editing"], true);
    assert_eq!(start.setup["configOptions"][0]["id"], "model");
    assert_eq!(start.setup["configOptions"][1]["category"], "thought_level");
    assert_eq!(start.setup["configOptions"][1]["currentValue"], "high");
    chat.until_ready().await;

    prompt(&client, "claude-a", &message_id(), "stream 5").await;
    chat.turn_started().await;
    assert_eq!(chat.turn_ended().await.as_deref(), Some("end_turn"));
    assert_eq!(chat.text(), "w0 w1 w2 w3 w4 ");
    assert_eq!(
        chat.head()
            .commands
            .first()
            .map(|command| command.name.as_str()),
        Some("review")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_chat_says_when_claude_retries_its_request() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-retry").await;
    client
        .acp_start(claude_launch("claude-retry", config.path()))
        .await
        .expect("start");
    chat.until_ready().await;
    prompt(&client, "claude-retry", &message_id(), "retry 2").await;
    chat.turn_ended().await;

    let attempts: Vec<Value> = chat
        .heads
        .iter()
        .filter_map(|set| set.get("retrying").filter(|retrying| !retrying.is_null()))
        .map(|retrying| retrying["attempt"].clone())
        .collect();
    assert_eq!(
        attempts.last(),
        Some(&Value::from(2)),
        "the retries were never shown: {:?}",
        chat.heads
    );
    assert_eq!(chat.head().retrying, None);
    assert_eq!(chat.text(), "back");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_network_cut_short_says_so_plainly_and_goes_again_on_retry() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-offline").await;
    client
        .acp_start(claude_launch("claude-offline", config.path()))
        .await
        .expect("start");
    chat.until_ready().await;
    prompt(&client, "claude-offline", &message_id(), "offline").await;

    let head = until_failed(&mut chat).await.view.head;
    let failure = head.failure.expect("a failure");
    assert_eq!(failure.kind, FailureKind::Network);
    assert_eq!(
        head.error.as_deref(),
        Some("Lost the connection to the model provider")
    );
    assert_eq!(
        failure.details.as_deref(),
        Some("API Error: Connection error.")
    );

    client
        .call(
            "retryFailed",
            serde_json::json!({ "agentId": "claude-offline" }),
        )
        .await
        .expect("retry");
    chat.until(|chat| chat.text().contains("back online")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_tool_waits_for_the_persons_answer() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-b").await;
    let mut launch = claude_launch("claude-b", config.path());
    launch.permission_mode = "workspace-write".into();
    client.acp_start(launch).await.expect("start");

    prompt(&client, "claude-b", &message_id(), "ask").await;
    let asked = until_asked(&mut chat).await;
    assert_eq!(asked.tool_call.title, "echo hi");
    let (request, allow) = allow_once(&asked);
    client
        .acp_permission_reply("claude-b".into(), request, Some(allow))
        .await
        .expect("reply");
    chat.turn_ended().await;
    assert!(chat.text().ends_with("allowed"), "{}", chat.text());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_claude_turn_ends_cancelled() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-c").await;
    client
        .acp_start(claude_launch("claude-c", config.path()))
        .await
        .expect("start");
    prompt(&client, "claude-c", &message_id(), "hold 10000").await;
    chat.until(|chat| chat.text().contains("holding")).await;
    client.acp_cancel("claude-c".into()).await.expect("cancel");
    assert_eq!(chat.turn_ended().await.as_deref(), Some("cancelled"));
    assert!(!chat.text().contains("held"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_claude_message_takes_the_chat_back_to_before_it() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-d").await;
    let (watcher, mut watching) = core.connect("claude-d").await;
    client
        .acp_start(claude_launch("claude-d", config.path()))
        .await
        .expect("start");
    let first = message_id();
    let second = message_id();
    prompt(&client, "claude-d", &first, "first").await;
    chat.turn_ended().await;
    let attached = watcher.acp_attach("claude-d".into()).await.expect("attach");
    assert!(matches!(attached, ChatAttachment::Live { .. }));
    watching.attached(&attached);
    prompt(&client, "claude-d", &second, "second").await;
    chat.turn_ended().await;
    watching
        .until(|chat| chat.text().contains("echo: second"))
        .await;

    client
        .acp_edit(
            "claude-d".into(),
            second.clone(),
            "count".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("edit");
    chat.until(|chat| !chat.head().running && chat.text().contains("earlier"))
        .await;
    assert!(chat.text().ends_with("1 earlier"), "{}", chat.text());

    let watched = watching
        .until(|chat| !chat.head().running && chat.text().contains("earlier"))
        .await;
    assert_eq!(watched.prompts(), ["first", "count"]);
    assert!(
        !watched.text().contains("echo: second"),
        "{}",
        watched.text()
    );
    let (late, _) = core.connect("claude-d").await;
    let ChatAttachment::Live { view, .. } = late
        .acp_attach("claude-d".into())
        .await
        .expect("attach late")
    else {
        panic!("the chat is gone");
    };
    let replayed = Folded::of_view(&view).text();
    assert!(replayed.contains("echo: first"), "{replayed}");
    assert!(!replayed.contains("echo: second"), "{replayed}");
    assert!(replayed.ends_with("1 earlier"), "{replayed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_claude_chat_replays_what_was_said() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-e").await;
    let start = client
        .acp_start(claude_launch("claude-e", config.path()))
        .await
        .expect("start");
    prompt(&client, "claude-e", &message_id(), "hello there").await;
    chat.turn_ended().await;
    client.acp_stop("claude-e".into()).await.expect("stop");

    let (again, mut resumed) = core.connect("claude-f").await;
    let mut launch = claude_launch("claude-f", config.path());
    launch.resume_id = Some(start.session_id.clone());
    let restarted = again.acp_start(launch).await.expect("resume");
    assert_eq!(restarted.session_id, start.session_id);
    resumed
        .until(|chat| chat.ready() && !chat.view.rows.is_empty())
        .await;
    assert_eq!(resumed.prompts(), ["hello there"]);
    assert_eq!(resumed.text(), "echo: hello there");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_account_out_of_usage_fails_the_turn_as_a_limit() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-g").await;
    client
        .acp_start(claude_launch("claude-g", config.path()))
        .await
        .expect("start");
    prompt(&client, "claude-g", &message_id(), "limit").await;
    let head = until_failed(&mut chat).await.view.head;
    assert_eq!(
        head.failure.as_ref().map(|failure| failure.kind),
        Some(FailureKind::Limit)
    );
    assert!(
        head.error
            .as_deref()
            .is_some_and(|error| error.starts_with("You've hit your limit · resets 3pm")),
        "{head:?}"
    );
    assert!(!chat.text().contains("hit your limit"));
    let in_an_hour = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64
        + 3_600_000;
    let resumes_at = |chat: &Folded| {
        chat.head()
            .failure
            .as_ref()
            .and_then(|failure| failure.resumes_at)
    };
    let resumes_at = resumes_at(chat.until(|chat| resumes_at(chat).is_some()).await).expect("time");
    assert!(resumes_at.abs_diff(in_an_hour) < 30_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_prompt_stopped_by_a_limit_is_answered_on_the_account_it_moves_to() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-l").await;
    let mut launch = claude_launch("claude-l", config.path());
    launch
        .env
        .insert("FAKE_CLAUDE_OUT_OF_USAGE".into(), "1".into());
    client.acp_start(launch).await.expect("start");
    prompt(&client, "claude-l", &message_id(), "hello there").await;
    let failure = until_failed(&mut chat).await.view.head.failure;
    assert_eq!(
        failure.map(|failure| failure.kind),
        Some(FailureKind::Limit)
    );

    let spare = ChatAccount {
        id: "spare".into(),
        label: "Spare".into(),
        env: BTreeMap::from([
            (
                "CLAUDE_CONFIG_DIR".to_owned(),
                config.path().to_string_lossy().into_owned(),
            ),
            ("FAKE_CLAUDE_OUT_OF_USAGE".to_owned(), "0".to_owned()),
        ]),
    };
    client
        .acp_switch_account("claude-l".into(), spare)
        .await
        .expect("switch");
    chat.until(|chat| !chat.head().running && chat.text() == "echo: hello there")
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_chat_loads_the_sikemux_skills_library() {
    let home = tempfile::tempdir().expect("home");
    let library = tempfile::tempdir().expect("library");
    std::fs::create_dir_all(library.path().join("deploy")).expect("skill folder");
    std::fs::write(
        library.path().join("deploy/SKILL.md"),
        "---\nname: deploy\ndescription: Deploys\n---\n",
    )
    .expect("skill");
    let mut launch = codex_launch("codex-skills", home.path());
    launch.env.insert(
        sikemux_core::skills::LIBRARY_ENV.to_owned(),
        library.path().to_string_lossy().into_owned(),
    );
    let core = TestCore::start();
    let (client, mut chat) = core.connect("codex-skills").await;
    client.acp_start(launch).await.expect("start");
    chat.until_ready().await;
    prompt(&client, "codex-skills", &message_id(), "hello").await;
    chat.turn_ended().await;

    let offered = &chat.head().commands;
    assert!(
        offered.iter().any(|command| command.name == "$deploy"),
        "{offered:?}"
    );
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
    let (client, mut chat) = core.connect("codex-a").await;
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
    assert_eq!(chat.turn_ended().await.as_deref(), Some("end_turn"));
    assert_eq!(chat.text(), "w0 w1 w2 w3 w4 ");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_command_waits_for_the_persons_answer() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("codex-b").await;
    let mut launch = codex_launch("codex-b", home.path());
    launch.permission_mode = "workspace-write".into();
    client.acp_start(launch).await.expect("start");

    prompt(&client, "codex-b", &message_id(), "ask").await;
    let (request, allow) = allow_once(&until_asked(&mut chat).await);
    client
        .acp_permission_reply("codex-b".into(), request, Some(allow))
        .await
        .expect("reply");
    chat.turn_ended().await;
    assert!(chat.text().ends_with("allowed"), "{}", chat.text());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_codex_turn_ends_cancelled() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("codex-c").await;
    client
        .acp_start(codex_launch("codex-c", home.path()))
        .await
        .expect("start");
    prompt(&client, "codex-c", &message_id(), "hold 10000").await;
    chat.until(|chat| chat.text().contains("holding")).await;
    client.acp_cancel("codex-c".into()).await.expect("cancel");
    assert_eq!(chat.turn_ended().await.as_deref(), Some("cancelled"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_codex_message_takes_the_thread_back_to_before_it() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("codex-d").await;
    client
        .acp_start(codex_launch("codex-d", home.path()))
        .await
        .expect("start");
    let second = message_id();
    prompt(&client, "codex-d", &message_id(), "first").await;
    chat.turn_ended().await;
    prompt(&client, "codex-d", &second, "second").await;
    chat.turn_ended().await;

    client
        .acp_edit(
            "codex-d".into(),
            second,
            "count".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("edit");
    chat.until(|chat| !chat.head().running && chat.text().contains("earlier"))
        .await;
    assert!(chat.text().ends_with("1 earlier"), "{}", chat.text());
    assert_eq!(chat.prompts(), ["first", "count"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_codex_thread_replays_what_was_said() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("codex-e").await;
    let start = client
        .acp_start(codex_launch("codex-e", home.path()))
        .await
        .expect("start");
    prompt(&client, "codex-e", &message_id(), "hello there").await;
    chat.turn_ended().await;
    client.acp_stop("codex-e".into()).await.expect("stop");

    let (again, mut resumed) = core.connect("codex-f").await;
    let mut launch = codex_launch("codex-f", home.path());
    launch.resume_id = Some(start.session_id.clone());
    let restarted = again.acp_start(launch).await.expect("resume");
    assert_eq!(restarted.session_id, start.session_id);
    resumed
        .until(|chat| chat.ready() && !chat.view.rows.is_empty())
        .await;
    assert_eq!(resumed.prompts(), ["hello there"]);
    assert_eq!(resumed.text(), "echo: hello there");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_account_out_of_usage_fails_the_turn_as_a_limit() {
    let home = tempfile::tempdir().expect("home");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("codex-g").await;
    client
        .acp_start(codex_launch("codex-g", home.path()))
        .await
        .expect("start");
    prompt(&client, "codex-g", &message_id(), "limit").await;
    let failure = until_failed(&mut chat).await.view.head.failure;
    assert_eq!(
        failure.map(|failure| failure.kind),
        Some(FailureKind::Limit)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_the_claude_model_starts_no_turn() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, mut chat) = core.connect("claude-model").await;
    client
        .acp_start(claude_launch("claude-model", config.path()))
        .await
        .expect("start");
    chat.until_ready().await;
    let options = client
        .acp_set_config("claude-model".into(), "model".into(), "small".into())
        .await
        .expect("set model");
    assert_eq!(options["configOptions"][0]["currentValue"], "small");
    prompt(&client, "claude-model", &message_id(), "after").await;
    chat.turn_ended().await;
    assert_eq!(chat.turns_started, 1, "only the prompt started a turn");
    assert!(!chat.text().contains("Set model"), "{}", chat.text());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_claude_chat_shows_the_model_its_settings_choose() {
    let config = tempfile::tempdir().expect("config");
    let core = TestCore::start();
    let (client, _chat) = core.connect("claude-settings").await;
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
    let (client, mut chat) = core.connect("live").await;
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
    chat.turn_ended().await;
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
    chat.turn_ended().await;

    client
        .acp_edit(
            "live".into(),
            second,
            "How many messages did I send you before this one? Answer with just the digit.".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("edit");
    assert_eq!(chat.turn_ended().await.as_deref(), Some("end_turn"));
    let answer = chat.text();
    assert!(answer.trim_end().ends_with('1'), "{answer}");

    client.acp_stop("live".into()).await.expect("stop");
    let (again, mut resumed) = core.connect("live-again").await;
    let mut resume = launch;
    resume.agent_id = "live-again".into();
    resume.resume_id = Some(start.session_id.clone());
    again.acp_start(resume).await.expect("resume");
    let users = resumed
        .until(|chat| chat.ready() && !chat.view.rows.is_empty())
        .await
        .prompts();
    assert!(users.iter().any(|text| text.contains("alpha")), "{users:?}");
    assert!(!users.iter().any(|text| text.contains("beta")), "{users:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_claude_chat_comes_back_as_its_last_turns_and_pages_back_whole() {
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
    let (client, mut chat) = core.connect("claude-long").await;
    let mut launch = claude_launch("claude-long", config.path());
    launch.resume_id = Some(session);
    client.acp_start(launch).await.expect("resume");
    chat.until(|chat| chat.ready() && !chat.view.rows.is_empty())
        .await;
    let answers = |rows: &[sikemux_wire::chat::ChatMessage]| {
        rows.iter()
            .filter(|row| row.role == ChatRole::Assistant)
            .count()
    };
    let shown = answers(&chat.view.rows);
    assert!(shown <= 50, "{shown} answers were sent at once");
    assert_eq!(said(&chat.view.rows).len(), shown * 100_000);
    let ChatAttachment::Live { mark, .. } = client
        .acp_attach("claude-long".into())
        .await
        .expect("attach")
    else {
        panic!("the chat is gone");
    };
    let mut before = chat.view.older_before;
    assert!(
        before.is_some(),
        "the earlier turns were not said to be there"
    );
    let mut paged = 0;
    let mut asked = Vec::new();
    while let Some(from) = before {
        let page: ChatPage = serde_json::from_value(
            client
                .acp_history("claude-long".into(), mark.feed.clone(), from, 20)
                .await
                .expect("a page"),
        )
        .expect("a chat page");
        paged += answers(&page.rows);
        asked.splice(0..0, prompts(&page.rows));
        before = page.older_before;
    }
    assert_eq!(shown + paged, 200);
    asked.extend(chat.prompts());
    let questions: Vec<String> = (0..200).map(|turn| format!("question {turn}")).collect();
    assert_eq!(asked, questions);
}
