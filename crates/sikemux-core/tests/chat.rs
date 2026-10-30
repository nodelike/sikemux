#![cfg(unix)]

//! Chat agents in the core, driven through a stand-in agent that speaks just
//! enough ACP.

mod folded;

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use folded::{Chat, Folded};
use serde_json::json;
use sikemux_core::client::{probe, CoreClient};
use sikemux_core::protocol::{
    ask, encode_control, read_frame_sync, BuildIdentity, ChatAccount, ChatAttachment, ChatLaunch,
    ChatState, ClientMessage, PermissionMode, Request, PROTOCOL, PROTOCOL_VERSION,
};
use sikemux_core::server::{self, ServerConfig, ServerError};
use sikemux_wire::chat::{
    ChatConnection, ChatPart, FailureKind, MoveReason, PermissionAsk, StopReason,
};

const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");
const WAIT: Duration = Duration::from_secs(30);

struct TestCore {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    thread: Option<JoinHandle<Result<(), ServerError>>>,
}

impl TestCore {
    fn start() -> Self {
        sikemux_process::user_environment::search_path_only_from_now();
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
                    request: Request::Shutdown(ask::Shutdown { stop_all: true }),
                },
            ] {
                let _ = stream.write_all(&encode_control(&message).expect("encode"));
            }
            while let Ok(Some(_)) = read_frame_sync(&mut stream) {}
        }
        let _ = thread.join();
    }
}

/// The chat a client attaching is sent.
async fn attach(client: &CoreClient, agent_id: &str) -> Folded {
    let attachment = client.acp_attach(agent_id.into()).await.expect("attach");
    assert!(
        matches!(attachment, ChatAttachment::Live { .. }),
        "the chat was not live: {attachment:?}"
    );
    let mut folded = Folded::default();
    folded.attached(&attachment);
    folded
}

fn chat_launch(agent_id: &str, permission_mode: &str) -> ChatLaunch {
    ChatLaunch {
        agent_id: agent_id.into(),
        provider: "opencode".into(),
        cwd: std::env::temp_dir(),
        program: PathBuf::from(FAKE_AGENT),
        args: vec!["acp".into()],
        env: BTreeMap::new(),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: PermissionMode::parse(permission_mode).unwrap(),
        model: Some("slow".into()),
        model_provider: None,
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_starts_and_streams_a_turn() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-a").await;
    let start = client
        .acp_start(chat_launch("agent-a", "workspace-write"))
        .await
        .expect("start");
    assert!(start.session_id.starts_with("fake-"), "{start:?}");
    assert_eq!(start.setup["configOptions"][0]["currentValue"], "slow");
    chat.until(|chat| chat.head().connection == ChatConnection::Initializing)
        .await;
    chat.until_ready().await;

    let frames = chat.frames;
    client
        .acp_prompt(
            "agent-a".into(),
            None,
            "stream 40".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    chat.turn_started().await;
    assert_eq!(chat.turn_ended().await, Some(StopReason::EndTurn));
    let expected: String = (0..40).map(|index| format!("w{index} ")).collect();
    assert_eq!(chat.text(), expected);
    let answer = chat.view.rows.last().expect("the answer");
    assert!(
        matches!(&answer.parts[..], [ChatPart::Text { text, .. }] if *text == expected),
        "{answer:?}"
    );
    let sent = chat.frames - frames;
    assert!(sent < 40, "streamed text was not batched: {sent} frames");

    let listed = client.acp_list().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].agent_id, "agent-a");
    assert_eq!(listed[0].state, ChatState::Ready);
    assert_eq!(
        listed[0].session_id.as_deref(),
        Some(start.session_id.as_str())
    );
    assert!(!listed[0].running);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_knows_how_its_agent_can_be_signed_in() {
    let core = TestCore::start();
    let (client, _chat) = core.connect("agent-sign-in").await;
    let start = client
        .acp_start(chat_launch("agent-sign-in", "workspace-write"))
        .await
        .expect("start");
    assert_eq!(
        start.capabilities["signInMethods"],
        json!([
            { "kind": "terminal", "id": "fake-login", "name": "Log in", "description": null, "args": ["login"], "env": [] },
            { "kind": "agent", "id": "fake-key", "name": "API key", "description": "Use a key from the environment" },
        ])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_sent_mid_turn_is_kept_only_once_the_agent_takes_it() {
    let core = TestCore::start();
    let (sender, mut heard) = core.connect("agent-c").await;
    sender
        .acp_start(chat_launch("agent-c", "workspace-write"))
        .await
        .expect("start");
    sender
        .acp_prompt(
            "agent-c".into(),
            None,
            "hold 400".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    heard.turn_started().await;
    sender
        .acp_prompt(
            "agent-c".into(),
            None,
            "too soon".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");

    let (late, _) = core.connect("agent-c").await;
    assert_eq!(attach(&late, "agent-c").await.prompts(), ["hold 400"]);

    heard.turn_ended().await;
    heard.turn_started().await;
    assert_eq!(
        attach(&late, "agent-c").await.prompts(),
        ["hold 400", "too soon"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_attaches_replays_what_another_watched() {
    let core = TestCore::start();
    let (watcher, mut watched) = core.connect("agent-b").await;
    watcher
        .acp_start(chat_launch("agent-b", "workspace-write"))
        .await
        .expect("start");
    for prompt in ["stream 25", "hello there", "stream 7"] {
        watcher
            .acp_prompt(
                "agent-b".into(),
                None,
                prompt.into(),
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("prompt");
        watched.turn_ended().await;
    }

    let (late, _) = core.connect("agent-b").await;
    let replayed = attach(&late, "agent-b").await;
    let head = replayed.head();
    assert!(!head.running);
    assert!(head.turned);
    assert_eq!(head.permission_mode, PermissionMode::WorkspaceWrite);
    assert!(head
        .session_id
        .as_deref()
        .is_some_and(|session| session.starts_with("fake-")));
    let sent = ["stream 25", "hello there", "stream 7"];
    assert_eq!(replayed.prompts(), sent);
    assert_eq!(
        watched.prompts(),
        sent,
        "the watcher hears its own prompts as rows too"
    );
    assert_eq!(replayed.drawn(), watched.drawn());
    assert!(replayed.text().contains("echo: hello there"));
    let streamed: String = (0..25).map(|index| format!("w{index} ")).collect();
    assert!(
        replayed.parts().any(|part| matches!(
            part,
            ChatPart::Text { text, .. } if *text == streamed
        )),
        "streamed text was not joined in the replay"
    );

    assert_eq!(
        late.acp_attach("nobody".into()).await.expect("attach"),
        ChatAttachment::Missing
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permission_request_waits_for_a_client_that_comes_back() {
    let core = TestCore::start();
    let request_id = {
        let (first, mut chat) = core.connect("agent-c").await;
        first
            .acp_start(chat_launch("agent-c", "workspace-write"))
            .await
            .expect("start");
        first
            .acp_prompt("agent-c".into(), None, "ask".into(), Vec::new(), Vec::new())
            .await
            .expect("prompt");
        chat.until(|chat| !chat.head().permissions.is_empty())
            .await
            .head()
            .permissions[0]
            .request_id
            .clone()
    };
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (second, mut chat) = core.connect("agent-c").await;
    let listed = second.acp_list().await.expect("list");
    assert_eq!(listed[0].pending_permissions, vec![request_id.clone()]);
    assert!(listed[0].running);
    let attachment = second.acp_attach("agent-c".into()).await.expect("attach");
    chat.attached(&attachment);
    assert!(chat.head().running);
    let [asked]: &[PermissionAsk] = &chat.head().permissions[..] else {
        panic!("the request is not shown: {:?}", chat.head().permissions);
    };
    assert_eq!(asked.request_id, request_id);
    assert_eq!(asked.tool_call.title, "Touch a file");

    assert!(second
        .acp_permission_reply("agent-c".into(), request_id.clone(), Some("nope".into()))
        .await
        .is_err());
    second
        .acp_permission_reply("agent-c".into(), request_id.clone(), Some("allow".into()))
        .await
        .expect("reply");
    chat.turn_ended().await;
    assert_eq!(chat.text(), "answered allow");

    assert!(
        attach(&second, "agent-c")
            .await
            .head()
            .permissions
            .is_empty(),
        "an answered request was still shown"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn yolo_answers_permission_requests_for_the_person() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-d").await;
    client
        .acp_start(chat_launch("agent-d", "bypass"))
        .await
        .expect("start");
    client
        .acp_prompt("agent-d".into(), None, "ask".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    chat.turn_ended().await;
    assert_eq!(chat.text(), "answered allow");
    assert!(chat.heads.iter().all(|set| set
        .get("permissions")
        .and_then(|asks| asks.as_array())
        .is_none_or(Vec::is_empty)));

    client
        .acp_set_permission_mode("agent-d".into(), PermissionMode::WorkspaceWrite)
        .await
        .expect("permission mode");
    assert_eq!(
        attach(&client, "agent-d").await.head().permission_mode,
        PermissionMode::WorkspaceWrite
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_config_change_is_what_a_later_client_sees() {
    let core = TestCore::start();
    let (client, _chat) = core.connect("agent-e").await;
    client
        .acp_start(chat_launch("agent-e", "workspace-write"))
        .await
        .expect("start");
    let changed = client
        .acp_set_config("agent-e".into(), "model".into(), "fast".into())
        .await
        .expect("config");
    assert_eq!(changed["configOptions"][0]["currentValue"], "fast");
    assert_eq!(
        attach(&client, "agent-e").await.head().setup["configOptions"][0]["currentValue"],
        "fast"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_chat_ends_it() {
    let core = TestCore::start();
    let (client, _chat) = core.connect("agent-f").await;
    client
        .acp_start(chat_launch("agent-f", "workspace-write"))
        .await
        .expect("start");
    assert!(client
        .acp_start(chat_launch("agent-f", "workspace-write"))
        .await
        .is_err());
    client.acp_stop("agent-f".into()).await.expect("stop");
    assert!(client.acp_list().await.expect("list").is_empty());
    client
        .acp_prompt(
            "agent-f".into(),
            None,
            "hello".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("a send starts the stopped chat again, on the program it ran");
    assert_eq!(client.acp_list().await.expect("list").len(), 1);

    client
        .acp_start(chat_launch("agent-g", "workspace-write"))
        .await
        .expect("start again");
    client.stop_all().await.expect("stop all");
    assert!(client.acp_list().await.expect("list").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_at_work_is_not_stopped_by_a_stop_meant_for_idle_chats() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-s").await;
    client
        .acp_start(chat_launch("agent-s", "workspace-write"))
        .await
        .expect("start");
    client
        .acp_prompt("agent-s".into(), None, "ask".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    let request_id = chat
        .until(|chat| !chat.head().permissions.is_empty())
        .await
        .head()
        .permissions[0]
        .request_id
        .clone();

    let inbox = client.call("inbox", json!({})).await.expect("inbox");
    assert_eq!(inbox["agents"][0]["status"], "blocked", "{inbox}");
    let refused = client
        .call("stopChatIfIdle", json!({ "agentId": "agent-s" }))
        .await
        .expect("stop if idle");
    assert_eq!(refused["stopped"], false);
    assert_eq!(client.acp_list().await.expect("list").len(), 1);

    client
        .acp_permission_reply("agent-s".into(), request_id, Some("allow".into()))
        .await
        .expect("reply");
    chat.turn_ended().await;
    let inbox = client.call("inbox", json!({})).await.expect("inbox");
    assert_eq!(inbox["agents"][0]["status"], "done", "{inbox}");
    let stopped = client
        .call("stopChatIfIdle", json!({ "agentId": "agent-s" }))
        .await
        .expect("stop if idle");
    assert_eq!(stopped["stopped"], true);
    assert!(client.acp_list().await.expect("list").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_watching_a_chat_that_sleeps_hears_it_wake() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-w").await;
    client
        .acp_start(chat_launch("agent-w", "workspace-write"))
        .await
        .expect("start");
    client
        .acp_prompt(
            "agent-w".into(),
            None,
            "stream 2".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    chat.turn_ended().await;
    let slept = client
        .call("stopChatIfIdle", json!({ "agentId": "agent-w" }))
        .await
        .expect("sleep");
    assert_eq!(slept["stopped"], true);

    client
        .acp_prompt(
            "agent-w".into(),
            None,
            "stream 3".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("a send wakes the chat");
    chat.until_said("w2 ").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_dies_carries_on_until_it_has_died_too_often() {
    const RESTARTED: &str = "This agent stopped while it was working, and was started again";
    const CARRIED_ON: &str = "Your process stopped while you were working";
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-h").await;
    let history = tempfile::tempdir().expect("history");
    let mut launch = chat_launch("agent-h", "workspace-write");
    launch.env.insert(
        "FAKE_ACP_DIR".into(),
        history.path().to_string_lossy().into_owned(),
    );
    client.acp_start(launch).await.expect("start");
    for restart in 0..4 {
        client
            .acp_prompt(
                "agent-h".into(),
                None,
                "exit 3".into(),
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("prompt");
        if restart == 3 {
            break;
        }
        let restarts = restart + 1;
        chat.until(|chat| {
            chat.notices()
                .iter()
                .filter(|title| *title == RESTARTED)
                .count()
                == restarts
        })
        .await;
        chat.until(|chat| {
            !chat.head().running
                && chat.head().stop_reason == Some(StopReason::EndTurn)
                && chat.text().matches(CARRIED_ON).count() >= restarts
        })
        .await;
    }
    let ended = chat
        .until(|chat| {
            matches!(
                chat.head().connection,
                ChatConnection::Stopped | ChatConnection::Error
            )
        })
        .await
        .head()
        .ended
        .clone();
    assert_eq!(
        ended.as_ref().map(|ended| ended.reason.as_str()),
        Some("exited"),
        "{ended:?}"
    );
    let deadline = Instant::now() + WAIT;
    while !client.acp_list().await.expect("list").is_empty() {
        assert!(Instant::now() < deadline, "the dead chat stayed listed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_that_cannot_start_says_why() {
    let core = TestCore::start();
    let (client, _chat) = core.connect("agent-i").await;
    let mut launch = chat_launch("agent-i", "workspace-write");
    launch.program = PathBuf::from("/nonexistent/agent");
    let error = client.acp_start(launch).await.expect_err("start");
    assert!(!error.to_string().is_empty());
    let mut relative = chat_launch("agent-j", "workspace-write");
    relative.cwd = PathBuf::from("relative");
    let error = client.acp_start(relative).await.expect_err("start");
    assert!(error.to_string().contains("absolute"), "{error}");
    assert!(client.acp_list().await.expect("list").is_empty());
}

fn account(id: &str, label: &str) -> ChatAccount {
    ChatAccount {
        id: id.into(),
        label: label.into(),
        env: BTreeMap::from([("FAKE_ACP_ACCOUNT".to_string(), id.to_string())]),
    }
}

/// A launch whose sessions outlive its agent process, on the `personal`
/// account, with `limited` accounts out of usage.
fn account_launch(agent_id: &str, history: &std::path::Path, limited: &str) -> ChatLaunch {
    let mut launch = chat_launch(agent_id, "workspace-write");
    let personal = account("personal", "Personal");
    launch.env.extend(personal.env.clone());
    launch.env.insert(
        "FAKE_ACP_DIR".into(),
        history.to_string_lossy().into_owned(),
    );
    launch.env.insert("FAKE_ACP_LIMITED".into(), limited.into());
    launch.account = Some(personal);
    launch
}

/// Lets chats of the fake agent move to another account at a usage limit.
async fn switch_at_limit(client: &CoreClient) {
    let book = json!({ "profiles": [], "selected": {}, "autoSwitch": { "opencode": true } });
    client
        .call("setAgentAccounts", book)
        .await
        .expect("the accounts");
}

/// Waits until the turn the agent answers with `answer` has ended.
async fn until_answered(chat: &mut Chat, answer: &str) {
    chat.until(|chat| !chat.head().running && chat.text() == answer)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_signs_in_by_its_own_method_and_the_refused_prompt_goes_again() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-auth").await;
    let history = tempfile::tempdir().expect("history");
    let sign_in = history.path().join("signed-in");
    std::fs::write(&sign_in, "").expect("sign out");
    let mut launch = account_launch("agent-auth", history.path(), "");
    launch.env.insert(
        "FAKE_ACP_SIGN_IN".into(),
        sign_in.to_string_lossy().into_owned(),
    );
    client.acp_start(launch).await.expect("start");
    client
        .acp_prompt(
            "agent-auth".into(),
            None,
            "who".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt");
    let refused = chat.until(|chat| chat.head().failure.is_some()).await;
    assert_eq!(
        refused.head().failure.as_ref().map(|failure| failure.kind),
        Some(FailureKind::SignIn),
        "{:?}",
        refused.head()
    );

    let unknown = client
        .call(
            "authenticateChat",
            json!({ "agentId": "agent-auth", "methodId": "nope" }),
        )
        .await;
    assert!(unknown.is_err(), "{unknown:?}");
    client
        .call(
            "authenticateChat",
            json!({ "agentId": "agent-auth", "methodId": "fake-key" }),
        )
        .await
        .expect("authenticate");
    client
        .call("retryFailed", json!({ "agentId": "agent-auth" }))
        .await
        .expect("retry");
    until_answered(&mut chat, "account personal").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_made_elsewhere_reaches_the_chat_on_its_next_prompt() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-k").await;
    let history = tempfile::tempdir().expect("history");
    let sign_in = history.path().join("signed-in");
    std::fs::write(&sign_in, "").expect("sign out");
    let mut launch = account_launch("agent-k", history.path(), "");
    launch.env.insert(
        "FAKE_ACP_SIGN_IN".into(),
        sign_in.to_string_lossy().into_owned(),
    );
    client.acp_start(launch).await.expect("start");

    client
        .acp_prompt("agent-k".into(), None, "who".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    let failure = chat
        .until(|chat| chat.head().failure.is_some())
        .await
        .head()
        .failure
        .clone()
        .expect("a failure");
    assert_eq!(failure.kind, FailureKind::SignIn, "{failure:?}");
    assert_eq!(failure.account.as_deref(), Some("personal"));
    assert_eq!(chat.errors, 1);

    // The agent read the sign-in once, when it started, so only a new one
    // sees the person sign in.
    std::fs::write(&sign_in, "me").expect("sign in");
    client
        .acp_prompt("agent-k".into(), None, "who".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    until_answered(&mut chat, "account personal").await;
    assert_eq!(chat.errors, 1, "{:?}", chat.head());
    let listed = client.acp_list().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].running);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_that_runs_out_carries_on_with_the_next_account() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-l").await;
    switch_at_limit(&client).await;
    let history = tempfile::tempdir().expect("history");
    let mut launch = account_launch("agent-l", history.path(), "personal");
    launch.fallbacks = vec![account("work", "Work")];
    client.acp_start(launch).await.expect("start");

    client
        .acp_prompt("agent-l".into(), None, "who".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    let moved = chat
        .until(|chat| {
            chat.parts()
                .any(|part| matches!(part, ChatPart::Account { .. }))
        })
        .await
        .parts()
        .find_map(|part| match part {
            ChatPart::Account { moved, .. } => Some(moved.clone()),
            _ => None,
        })
        .expect("the move");
    assert_eq!(moved.label, "Work");
    assert_eq!(moved.from.as_deref(), Some("Personal"));
    assert_eq!(moved.reason, MoveReason::Limit);

    until_answered(&mut chat, "account work").await;
    assert_eq!(chat.turns_ended(), 1, "{:?}", chat.ends);
    assert_eq!(chat.errors, 0, "{:?}", chat.head());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_out_of_accounts_says_so_and_takes_one_the_person_picks() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-m").await;
    switch_at_limit(&client).await;
    let history = tempfile::tempdir().expect("history");
    let mut launch = account_launch("agent-m", history.path(), "personal,work");
    launch.fallbacks = vec![account("work", "Work")];
    client.acp_start(launch).await.expect("start");

    client
        .acp_prompt("agent-m".into(), None, "who".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    let head = chat
        .until(|chat| chat.head().failure.is_some())
        .await
        .head()
        .clone();
    let failure = head.failure.expect("a failure");
    assert_eq!(failure.kind, FailureKind::Limit, "{failure:?}");
    assert_eq!(failure.account.as_deref(), Some("work"));
    assert_eq!(head.error.as_deref(), Some("work hit its usage limit"));
    assert!(!head.running);
    assert_eq!(chat.turns_ended(), 1, "{:?}", chat.ends);

    client
        .acp_switch_account("agent-m".into(), account("spare", "Spare"))
        .await
        .expect("switch");
    until_answered(&mut chat, "account spare").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_chat_signed_in_after_its_agent_started_answers_on_a_fresh_session() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect("agent-n").await;
    let history = tempfile::tempdir().expect("history");
    let sign_in = history.path().join("signed-in");
    std::fs::write(&sign_in, "").expect("sign out");
    let mut launch = account_launch("agent-n", history.path(), "");
    launch.env.insert(
        "FAKE_ACP_SIGN_IN".into(),
        sign_in.to_string_lossy().into_owned(),
    );
    launch.env.insert("FAKE_ACP_STRICT_LOAD".into(), "1".into());
    let start = client.acp_start(launch).await.expect("start");

    // The agent read the sign-in before this, and its session saved nothing.
    std::fs::write(&sign_in, "me").expect("sign in");
    client
        .acp_prompt("agent-n".into(), None, "who".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    chat.until(|chat| {
        chat.ready()
            && chat
                .head()
                .session_id
                .as_deref()
                .is_some_and(|session| session != start.session_id)
    })
    .await;
    until_answered(&mut chat, "account personal").await;
    assert_eq!(chat.errors, 0, "{:?}", chat.head());
}
