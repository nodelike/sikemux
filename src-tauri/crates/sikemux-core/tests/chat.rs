#![cfg(unix)]

//! Chat agents in the core, driven through a stand-in agent that speaks just
//! enough ACP.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sikemux_core::client::{probe, ClientEvent, CoreClient};
use sikemux_core::protocol::{
    encode_control, read_frame_sync, BuildIdentity, ChatAccount, ChatAttachment, ChatEvent,
    ChatEventKind, ChatLaunch, ChatState, ClientMessage, Event, Request, PROTOCOL,
    PROTOCOL_VERSION,
};
use sikemux_core::server::{self, ServerConfig, ServerError};
use tokio::sync::mpsc::UnboundedReceiver;

const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");
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

/// The events a replay keeps, with streamed text joined up the way the chat
/// joins it, so what a client watched and what another replays compare.
fn as_replayed(events: &[ChatEvent]) -> Vec<(ChatEventKind, Value)> {
    let mut folded: Vec<(ChatEventKind, Value)> = Vec::new();
    for event in events {
        match event.kind {
            ChatEventKind::Status | ChatEventKind::Ready => continue,
            ChatEventKind::SessionUpdate => {
                for update in updates(std::slice::from_ref(event)) {
                    let joins = folded.last().is_some_and(|(kind, last)| {
                        *kind == ChatEventKind::SessionUpdate
                            && last["update"]["sessionUpdate"] == "agent_message_chunk"
                            && update["update"]["sessionUpdate"] == "agent_message_chunk"
                    });
                    if joins {
                        if let Some((_, last)) = folded.last_mut() {
                            let joined = format!(
                                "{}{}",
                                last["update"]["content"]["text"]
                                    .as_str()
                                    .unwrap_or_default(),
                                update["update"]["content"]["text"]
                                    .as_str()
                                    .unwrap_or_default()
                            );
                            last["update"]["content"]["text"] = json!(joined);
                        }
                    } else {
                        folded.push((ChatEventKind::SessionUpdate, update));
                    }
                }
            }
            kind => folded.push((kind, event.payload.clone())),
        }
    }
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
        permission_mode: permission_mode.into(),
        model: Some("slow".into()),
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_starts_and_streams_a_turn() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let start = client
        .acp_start(chat_launch("agent-a", "workspace-write"))
        .await
        .expect("start");
    assert!(start.session_id.starts_with("fake-"), "{start:?}");
    assert_eq!(start.setup["configOptions"][0]["currentValue"], "slow");
    chat.until(|event| {
        event.kind == ChatEventKind::Status && event.payload["state"] == "initializing"
    })
    .await;
    chat.until_kind(ChatEventKind::Ready).await;

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
    chat.until_kind(ChatEventKind::TurnStarted).await;
    let completed = chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(completed.payload["stopReason"], "end_turn");
    let expected: String = (0..40).map(|index| format!("w{index} ")).collect();
    assert_eq!(chat.text(), expected);
    let batches = chat
        .heard
        .iter()
        .filter(|event| event.kind == ChatEventKind::SessionUpdate)
        .count();
    assert!(
        batches < 40,
        "streamed text was not batched: {batches} events"
    );

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
async fn a_prompt_turned_away_mid_turn_is_not_kept() {
    let core = TestCore::start();
    let (sender, mut heard) = core.connect().await;
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
    heard.until_kind(ChatEventKind::TurnStarted).await;
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
    heard.until_kind(ChatEventKind::Error).await;
    heard.until_kind(ChatEventKind::TurnCompleted).await;

    let (late, _) = core.connect().await;
    let ChatAttachment::Live { replay, .. } =
        late.acp_attach("agent-c".into()).await.expect("attach")
    else {
        panic!("the chat was not live");
    };
    let sent: Vec<_> = replay
        .iter()
        .filter(|event| event.kind == ChatEventKind::Prompt)
        .map(|event| event.payload["text"].clone())
        .collect();
    assert_eq!(sent, ["hold 400"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_attaches_replays_what_another_watched() {
    let core = TestCore::start();
    let (watcher, mut watched) = core.connect().await;
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
        watched.until_kind(ChatEventKind::TurnCompleted).await;
    }

    let (late, _) = core.connect().await;
    let ChatAttachment::Live {
        start,
        running,
        turned,
        replay,
        permission_mode,
        ..
    } = late.acp_attach("agent-b".into()).await.expect("attach")
    else {
        panic!("the chat was not live");
    };
    assert!(!running);
    assert!(turned);
    assert_eq!(permission_mode, "workspace-write");
    assert!(start.session_id.starts_with("fake-"));
    let (prompts, rest): (Vec<_>, Vec<_>) = replay
        .iter()
        .cloned()
        .partition(|event| event.kind == ChatEventKind::Prompt);
    let sent: Vec<_> = prompts
        .iter()
        .map(|event| event.payload["text"].clone())
        .collect();
    assert_eq!(
        sent,
        ["stream 25", "hello there", "stream 7"],
        "the watcher sent these, so it never heard them back"
    );
    assert_eq!(as_replayed(&rest), as_replayed(&watched.heard));
    assert!(said(&replay).contains("echo: hello there"));
    assert!(
        replay.len() < watched.heard.len(),
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
        let (first, mut chat) = core.connect().await;
        first
            .acp_start(chat_launch("agent-c", "workspace-write"))
            .await
            .expect("start");
        first
            .acp_prompt("agent-c".into(), None, "ask".into(), Vec::new(), Vec::new())
            .await
            .expect("prompt");
        let request = chat.until_kind(ChatEventKind::PermissionRequest).await;
        request.payload["requestId"]
            .as_str()
            .expect("request id")
            .to_owned()
    };
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (second, mut chat) = core.connect().await;
    let listed = second.acp_list().await.expect("list");
    assert_eq!(listed[0].pending_permissions, vec![request_id.clone()]);
    assert!(listed[0].running);
    let ChatAttachment::Live {
        running, replay, ..
    } = second.acp_attach("agent-c".into()).await.expect("attach")
    else {
        panic!("the chat was not live");
    };
    assert!(running);
    let asked = replay
        .iter()
        .find(|event| event.kind == ChatEventKind::PermissionRequest)
        .expect("the request is replayed");
    assert_eq!(asked.payload["requestId"], request_id.as_str());
    assert_eq!(asked.payload["toolCall"]["title"], "Touch a file");

    assert!(second
        .acp_permission_reply("agent-c".into(), request_id.clone(), Some("nope".into()))
        .await
        .is_err());
    second
        .acp_permission_reply("agent-c".into(), request_id.clone(), Some("allow".into()))
        .await
        .expect("reply");
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(chat.text(), "answered allow");

    let ChatAttachment::Live { replay, .. } =
        second.acp_attach("agent-c".into()).await.expect("attach")
    else {
        panic!("the chat was not live");
    };
    assert!(
        replay
            .iter()
            .all(|event| event.kind != ChatEventKind::PermissionRequest),
        "an answered request was replayed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn yolo_answers_permission_requests_for_the_person() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    client
        .acp_start(chat_launch("agent-d", "bypass"))
        .await
        .expect("start");
    client
        .acp_prompt("agent-d".into(), None, "ask".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(chat.text(), "answered allow");
    assert!(chat
        .heard
        .iter()
        .all(|event| event.kind != ChatEventKind::PermissionRequest));

    client
        .acp_set_permission_mode("agent-d".into(), "workspace-write".into())
        .await
        .expect("permission mode");
    let ChatAttachment::Live {
        permission_mode, ..
    } = client.acp_attach("agent-d".into()).await.expect("attach")
    else {
        panic!("the chat was not live");
    };
    assert_eq!(permission_mode, "workspace-write");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_config_change_is_what_a_later_client_sees() {
    let core = TestCore::start();
    let (client, _chat) = core.connect().await;
    client
        .acp_start(chat_launch("agent-e", "workspace-write"))
        .await
        .expect("start");
    let changed = client
        .acp_set_config("agent-e".into(), "model".into(), "fast".into())
        .await
        .expect("config");
    assert_eq!(changed["configOptions"][0]["currentValue"], "fast");
    let ChatAttachment::Live { start, .. } =
        client.acp_attach("agent-e".into()).await.expect("attach")
    else {
        panic!("the chat was not live");
    };
    assert_eq!(start.setup["configOptions"][0]["currentValue"], "fast");
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_chat_ends_it() {
    let core = TestCore::start();
    let (client, _chat) = core.connect().await;
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
    let refused = client
        .acp_prompt(
            "agent-f".into(),
            None,
            "hello".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect_err("prompt after stop");
    assert!(refused.to_string().contains("not running"), "{refused}");

    client
        .acp_start(chat_launch("agent-g", "workspace-write"))
        .await
        .expect("start again");
    client.stop_all().await.expect("stop all");
    assert!(client.acp_list().await.expect("list").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_dies_says_it_exited() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    client
        .acp_start(chat_launch("agent-h", "workspace-write"))
        .await
        .expect("start");
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
    let ended = chat
        .until(|event| {
            event.kind == ChatEventKind::Status
                && matches!(event.payload["state"].as_str(), Some("stopped" | "error"))
        })
        .await;
    assert_eq!(ended.payload["reason"], "exited", "{ended:?}");
    let deadline = Instant::now() + WAIT;
    while !client.acp_list().await.expect("list").is_empty() {
        assert!(Instant::now() < deadline, "the dead chat stayed listed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_that_cannot_start_says_why() {
    let core = TestCore::start();
    let (client, _chat) = core.connect().await;
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

fn turns_completed(events: &[ChatEvent]) -> usize {
    events
        .iter()
        .filter(|event| event.kind == ChatEventKind::TurnCompleted)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_made_elsewhere_reaches_the_chat_on_its_next_prompt() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
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
    let refused = chat.until_kind(ChatEventKind::Error).await;
    assert_eq!(refused.payload["failure"]["kind"], "signIn", "{refused:?}");
    assert_eq!(refused.payload["failure"]["account"], "personal");

    // The agent read the sign-in once, when it started, so only a new one
    // sees the person sign in.
    std::fs::write(&sign_in, "me").expect("sign in");
    client
        .acp_prompt("agent-k".into(), None, "who".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(chat.text(), "account personal");
    let after = &chat.heard[chat.cursor..];
    assert!(after.iter().all(|event| event.kind != ChatEventKind::Error));
    let listed = client.acp_list().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].running);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_that_runs_out_carries_on_with_the_next_account() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let history = tempfile::tempdir().expect("history");
    let mut launch = account_launch("agent-l", history.path(), "personal");
    launch.fallbacks = vec![account("work", "Work")];
    let start = client.acp_start(launch).await.expect("start");

    client
        .acp_prompt("agent-l".into(), None, "who".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    let switched = chat
        .until(|event| {
            event.kind == ChatEventKind::SessionUpdate
                && updates(std::slice::from_ref(event))
                    .iter()
                    .any(|update| update["update"]["sessionUpdate"] == "account_switched")
        })
        .await;
    let notice = updates(std::slice::from_ref(&switched))
        .into_iter()
        .find(|update| update["update"]["sessionUpdate"] == "account_switched")
        .expect("notice");
    assert_eq!(notice["sessionId"], start.session_id.as_str());
    assert_eq!(notice["update"]["account"], "work");
    assert_eq!(notice["update"]["from"], "Personal");
    assert_eq!(notice["update"]["reason"], "limit");

    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(chat.text(), "account work");
    assert_eq!(turns_completed(&chat.heard), 1);
    assert!(chat
        .heard
        .iter()
        .all(|event| event.kind != ChatEventKind::Error));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_out_of_accounts_says_so_and_takes_one_the_person_picks() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
    let history = tempfile::tempdir().expect("history");
    let mut launch = account_launch("agent-m", history.path(), "personal,work");
    launch.fallbacks = vec![account("work", "Work")];
    client.acp_start(launch).await.expect("start");

    client
        .acp_prompt("agent-m".into(), None, "who".into(), Vec::new(), Vec::new())
        .await
        .expect("prompt");
    let failed = chat.until_kind(ChatEventKind::Error).await;
    assert_eq!(failed.payload["failure"]["kind"], "limit", "{failed:?}");
    assert_eq!(failed.payload["failure"]["account"], "work");
    assert_eq!(failed.payload["message"], "work hit its usage limit");
    assert_eq!(turns_completed(&chat.heard), 1);

    client
        .acp_switch_account("agent-m".into(), account("spare", "Spare"))
        .await
        .expect("switch");
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(chat.text(), "account spare");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_chat_signed_in_after_its_agent_started_answers_on_a_fresh_session() {
    let core = TestCore::start();
    let (client, mut chat) = core.connect().await;
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
    let ready = chat
        .until(|event| {
            event.kind == ChatEventKind::Ready
                && event.payload["sessionId"] != start.session_id.as_str()
        })
        .await;
    assert_ne!(ready.payload["sessionId"], start.session_id.as_str());
    chat.until_kind(ChatEventKind::TurnCompleted).await;
    assert_eq!(chat.text(), "account personal");
    assert!(chat
        .heard
        .iter()
        .all(|event| event.kind != ChatEventKind::Error));
}
