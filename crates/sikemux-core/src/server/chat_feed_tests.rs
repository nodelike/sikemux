//! Tests of a chat's feed that need a whole core around it.

use serde_json::json;
use sikemux_fold::ChatEventKind;
use sikemux_view::chat::ChatChange;
use sikemux_wire::protocol::{ChatStart, PermissionMode};
use std::sync::Arc;

use crate::server::connection::ClientConn;
use crate::server::ServerCore;
use sikemux_chat::server::chat::feed::replay::testing::{asks, text};
use sikemux_chat::server::chat::feed::*;
use sikemux_wire::protocol::{ChatAttachment, ChatMark, Event, ServerMessage};

fn prompts(rows: &[sikemux_view::chat::ChatMessage]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.role == sikemux_view::chat::ChatRole::User)
        .flat_map(|row| row.parts.iter())
        .filter_map(|part| match part {
            sikemux_view::chat::ChatPart::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_long_history_loaded_at_the_start_reaches_clients_as_its_last_turns() {
    let dir = tempfile::tempdir().unwrap();
    let core = crate::server::Core::new(Default::default(), None).unwrap();
    let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
    let feed = Feed::new(
        "agent".into(),
        PermissionMode::WorkspaceWrite,
        Some(dir.path()),
    );
    feed.subscribe(&client);

    feed.hold();
    for n in 0..60 {
        let (asked, answer) = (format!("ask {n}"), format!("m{n}"));
        feed.emit(
            ChatEventKind::SessionUpdate,
            text("user_message_chunk", None, &asked),
        );
        feed.emit(
            ChatEventKind::SessionUpdate,
            text("agent_message_chunk", Some(&answer), "done"),
        );
    }
    let mut heard = Vec::new();
    while let Ok(frame) = frames.try_recv() {
        heard.push(frame);
    }
    assert!(heard.is_empty(), "nothing goes out while the history loads");

    feed.let_out();
    let mut views = Vec::new();
    while let Ok(frame) = frames.try_recv() {
        let frame = sikemux_wire::protocol::read_frame_sync(&mut &frame[..])
            .unwrap()
            .unwrap();
        let Ok(ServerMessage::Event {
            event: Event::Chat { changes, .. },
        }) = serde_json::from_slice(&frame.payload)
        else {
            continue;
        };
        let changes: Vec<ChatChange> = serde_json::from_value(changes).unwrap();
        views.extend(changes.into_iter().filter_map(|change| match change {
            ChatChange::View { view } => Some(view),
            _ => None,
        }));
    }
    let [view] = &views[..] else {
        panic!(
            "the history goes out as one view, not {} of them",
            views.len()
        );
    };
    let first = 60 - sikemux_fold::TAIL_TURNS;
    assert_eq!(prompts(&view.rows), asks(first..60));
    assert_eq!(view.older_before, Some(first as u64 * 2));
}

/// A client watching a chat through its sleep holds its rows already, so its waking sends only
/// what is new. A whole view would replace them with the window, which a long last turn
/// leaves holding only the message just sent.
#[tokio::test]
async fn a_chat_waking_under_a_long_turn_keeps_the_rows_its_client_holds() {
    let dir = tempfile::tempdir().unwrap();
    let core = crate::server::Core::new(Default::default(), None).unwrap();
    let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
    let feed = Feed::new(
        "agent".into(),
        PermissionMode::WorkspaceWrite,
        Some(dir.path()),
    );
    feed.subscribe(&client);
    feed.prompted(
        Some("first"),
        "first",
        &[],
        &[],
        sikemux_chat::server::chat::Whose::PERSON,
        None,
    );
    feed.emit(
        ChatEventKind::SessionUpdate,
        text("agent_message_chunk", Some("long"), &"x".repeat(1_200_000)),
    );
    feed.emit(
        ChatEventKind::TurnCompleted,
        json!({ "stopReason": "end_turn" }),
    );
    feed.rest();
    while frames.try_recv().is_ok() {}

    feed.hold();
    feed.prompted(
        Some("second"),
        "second",
        &[],
        &[],
        sikemux_chat::server::chat::Whose::PERSON,
        None,
    );
    feed.let_out();

    let mut heard = Vec::new();
    while let Ok(frame) = frames.try_recv() {
        let frame = sikemux_wire::protocol::read_frame_sync(&mut &frame[..])
            .unwrap()
            .unwrap();
        if let Ok(ServerMessage::Event {
            event: Event::Chat { changes, .. },
        }) = serde_json::from_slice(&frame.payload)
        {
            let changes: Vec<ChatChange> = serde_json::from_value(changes).unwrap();
            heard.extend(changes);
        }
    }
    assert!(
        !heard
            .iter()
            .any(|change| matches!(change, ChatChange::View { .. })),
        "the rows it holds are not replaced"
    );
    let sent: Vec<_> = heard
        .into_iter()
        .filter_map(|change| match change {
            ChatChange::Row { row, .. } => Some(row),
            _ => None,
        })
        .collect();
    assert_eq!(prompts(&sent), ["second"]);
}

fn said_turn(feed: &Arc<Feed>, n: usize) {
    feed.prompted(
        None,
        &format!("ask {n}"),
        &[],
        &[],
        sikemux_chat::server::chat::Whose::PERSON,
        None,
    );
    feed.emit(ChatEventKind::TurnStarted, json!({}));
    for piece in ["work", "ing"] {
        feed.emit(
            ChatEventKind::SessionUpdate,
            text("agent_message_chunk", Some(&format!("m{n}")), piece),
        );
    }
    feed.emit(
        ChatEventKind::TurnCompleted,
        json!({ "stopReason": "end_turn" }),
    );
}

fn attached(
    feed: &Arc<Feed>,
    client: &Arc<ClientConn>,
    frames: &mut tokio::sync::mpsc::UnboundedReceiver<Arc<[u8]>>,
    since: Option<ChatMark>,
) -> ChatAttachment {
    feed.attach(
        client,
        1,
        Standing {
            turned: true,
            draft: None,
        },
        since,
    );
    while let Ok(frame) = frames.try_recv() {
        let frame = sikemux_wire::protocol::read_frame_sync(&mut &frame[..])
            .unwrap()
            .unwrap();
        if let Ok(ServerMessage::Response {
            response: sikemux_wire::protocol::Response::ChatAttached { attachment },
            ..
        }) = serde_json::from_slice(&frame.payload)
        {
            return attachment;
        }
    }
    panic!("the attach was not answered");
}

fn rows_sent(attachment: &ChatAttachment) -> Vec<String> {
    let ChatAttachment::Resumed { changes, .. } = attachment else {
        panic!("the chat was sent whole rather than resumed");
    };
    let changes: Vec<ChatChange> = serde_json::from_value(changes.clone()).unwrap();
    changes
        .into_iter()
        .filter_map(|change| match change {
            ChatChange::Row { row, .. } => Some(
                row.parts
                    .iter()
                    .filter_map(|part| match part {
                        sikemux_view::chat::ChatPart::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

fn mark_of(attachment: &ChatAttachment) -> ChatMark {
    match attachment {
        ChatAttachment::Live { mark, .. } | ChatAttachment::Resumed { mark, .. } => mark.clone(),
        ChatAttachment::Missing => panic!("the chat is missing"),
    }
}

#[tokio::test]
async fn a_client_back_after_the_chat_moved_on_unwatched_gets_just_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let core = crate::server::Core::new(Default::default(), None).unwrap();
    let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
    let feed = Feed::new(
        "agent".into(),
        PermissionMode::WorkspaceWrite,
        Some(dir.path()),
    );
    feed.set_start(ChatStart {
        session_id: "s".into(),
        capabilities: json!({}),
        setup: json!({}),
    });
    said_turn(&feed, 0);
    let left_at = mark_of(&attached(&feed, &client, &mut frames, None));
    feed.unsubscribe(client.id);
    assert!(feed.unfolded());

    said_turn(&feed, 1);
    said_turn(&feed, 2);
    let back = attached(&feed, &client, &mut frames, Some(left_at));
    assert_eq!(
        rows_sent(&back),
        ["ask 1", "working", "ask 2", "working"],
        "the two turns it missed, and nothing it held"
    );

    let caught_up = mark_of(&back);
    feed.unsubscribe(client.id);
    let again = attached(&feed, &client, &mut frames, Some(caught_up));
    assert!(
        rows_sent(&again).is_empty(),
        "a fold built again keeps every row's number"
    );
}

/// A reopened chat reads its saved end while a client already listens, and learns only once
/// those rows are in that the session holds turns before them (task #269).
#[tokio::test]
async fn a_listening_client_learns_of_turns_read_before_its_rows() {
    let core = crate::server::Core::new(Default::default(), None).unwrap();
    let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
    let feed = Feed::new("agent".into(), PermissionMode::WorkspaceWrite, None);
    feed.subscribe(&client);
    feed.hold();
    for n in 0..3 {
        feed.emit(
            ChatEventKind::SessionUpdate,
            text("user_message_chunk", None, &format!("ask {n}")),
        );
        feed.emit(
            ChatEventKind::SessionUpdate,
            text("agent_message_chunk", Some(&format!("m{n}")), "done"),
        );
    }
    feed.tell_earlier(Box::new(|| Ok(None)));
    feed.let_out();

    let mut older = None;
    while let Ok(frame) = frames.try_recv() {
        let frame = sikemux_wire::protocol::read_frame_sync(&mut &frame[..])
            .unwrap()
            .unwrap();
        if let Ok(ServerMessage::Event {
            event: Event::Chat { changes, .. },
        }) = serde_json::from_slice(&frame.payload)
        {
            let changes: Vec<ChatChange> = serde_json::from_value(changes).unwrap();
            for change in changes {
                if let ChatChange::View { view } = change {
                    older = view.older_before;
                }
            }
        }
    }
    assert!(older.is_some(), "the client is told it can page back");
}

#[tokio::test]
async fn end_turn_logs_live_model_and_strips_sikemux_routing_prefix() {
    use sikemux_chat::server::chat::{complete_turn_for_test, idle};
    use sikemux_workspace::server::activity_log::ReportedTokens;
    use sikemux_workspace::server::workspace_state::WorkspaceParts;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let core = crate::server::Core::new(Default::default(), Some(dir.path())).unwrap();
    let chat = idle(&core, "agent-1", "omp", "/repo");
    chat.mark_ready(ChatStart {
        session_id: "s1".into(),
        capabilities: json!({}),
        setup: json!({
            "configOptions": [{
                "id": "model",
                "currentValue": "sikemux-local-3425/workbuddy/hy4flash-free"
            }]
        }),
    });
    complete_turn_for_test(
        chat.clone(),
        json!({
            "stopReason": "end_turn",
            "usage": { "inputTokens": 100, "outputTokens": 20 }
        }),
    );
    chat.feed.set_start(ChatStart {
        session_id: "s1".into(),
        capabilities: json!({}),
        setup: json!({
            "configOptions": [{
                "id": "model",
                "currentValue": "anthropic/claude-sonnet-4"
            }]
        }),
    });
    complete_turn_for_test(
        chat.clone(),
        json!({
            "stopReason": "end_turn",
            "usage": { "inputTokens": 50, "outputTokens": 10 }
        }),
    );

    let today = (sikemux_process::clock::epoch_ms() as i64).div_euclid(86_400_000);
    let mut tokens: Vec<ReportedTokens> = Vec::new();
    for _ in 0..100 {
        tokens = core
            .activity_log()
            .reported_days(today, today + 1)
            .into_iter()
            .flat_map(|day| day.tokens)
            .collect();
        if tokens.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let models: Vec<(&str, i64, i64)> = tokens
        .iter()
        .map(|entry| (entry.model.as_str(), entry.input, entry.output))
        .collect();
    assert_eq!(
        models,
        vec![
            ("workbuddy/hy4flash-free", 100, 20),
            ("anthropic/claude-sonnet-4", 50, 10),
        ]
    );
}

#[tokio::test]
async fn replacing_a_feed_tells_listening_clients_to_reattach() {
    let core = crate::server::Core::new(Default::default(), None).unwrap();
    let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
    let feed = Feed::new("agent".into(), PermissionMode::WorkspaceWrite, None);
    feed.set_start(ChatStart {
        session_id: "s".into(),
        capabilities: json!({}),
        setup: json!({}),
    });
    said_turn(&feed, 0);
    let mark = mark_of(&attached(&feed, &client, &mut frames, None));
    assert!(feed.has_subscriber(client.id));

    feed.reattach_subscribers();
    assert!(!feed.has_subscriber(client.id));

    let mut gap = None;
    while let Ok(frame) = frames.try_recv() {
        let frame = sikemux_wire::protocol::read_frame_sync(&mut &frame[..])
            .unwrap()
            .unwrap();
        if let Ok(ServerMessage::Event {
            event: Event::Chat { seq, changes, .. },
        }) = serde_json::from_slice(&frame.payload)
        {
            gap = Some((seq, changes));
        }
    }
    assert_eq!(gap, Some((mark.seq + 2, json!([]))));
}

#[tokio::test]
async fn reattaching_an_asleep_long_turn_sends_full_view_instead_of_mismatched_keep() {
    let dir = tempfile::tempdir().unwrap();
    let core = crate::server::Core::new(Default::default(), None).unwrap();
    let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
    let feed = Feed::new(
        "agent".into(),
        PermissionMode::WorkspaceWrite,
        Some(dir.path()),
    );
    feed.set_start(ChatStart {
        session_id: "s".into(),
        capabilities: json!({}),
        setup: json!({}),
    });
    feed.prompted(
        None,
        "build the connector",
        &[],
        &[],
        sikemux_chat::server::chat::Whose::PERSON,
        None,
    );
    feed.emit(ChatEventKind::TurnStarted, json!({}));
    let blob = "x".repeat(60_000);
    for n in 0..24 {
        feed.emit(
            ChatEventKind::SessionUpdate,
            json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": format!("call-{n}"),
                    "title": format!("Read file {n}"),
                    "rawInput": { "data": blob },
                },
            }),
        );
    }
    feed.emit(
        ChatEventKind::SessionUpdate,
        text("agent_message_chunk", None, "All done and verified."),
    );
    feed.emit(
        ChatEventKind::TurnCompleted,
        json!({ "stopReason": "end_turn" }),
    );

    let initial = attached(&feed, &client, &mut frames, None);
    let left_at = mark_of(&initial);
    let ChatAttachment::Live { view, .. } = initial else {
        panic!("initial attach sends the view");
    };
    let live_view: sikemux_view::chat::ChatView = serde_json::from_value(view).unwrap();
    assert_eq!(live_view.rows.len(), 2);

    feed.unsubscribe(client.id);
    assert!(feed.rest() > 0);
    assert!(feed.unfolded());

    let reopened = attached(&feed, &client, &mut frames, Some(left_at.clone()));
    let ChatAttachment::Live { view, .. } = reopened else {
        panic!("reattaching a mid-turn reloaded asleep chat must send Live, not Resumed");
    };
    let view: sikemux_view::chat::ChatView = serde_json::from_value(view).unwrap();
    assert!(
        !view.rows.is_empty(),
        "reopened asleep chat must show its tail rows"
    );
    let text_shown: String = view
        .rows
        .iter()
        .flat_map(|row| row.parts.iter())
        .filter_map(|part| match part {
            sikemux_view::chat::ChatPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text_shown.contains("All done and verified."));
    let before = view
        .older_before
        .expect("earlier part of the turn pages back");
    let page = feed.history(feed.id(), before, 20).unwrap();
    assert!(!page.rows.is_empty());

    feed.outlive_chat();
    let feed_id = feed.id().to_owned();
    drop(feed);

    let kept = Feed::kept(
        "agent".into(),
        PermissionMode::WorkspaceWrite,
        dir.path(),
        &feed_id,
    );
    kept.set_start(ChatStart {
        session_id: "s".into(),
        capabilities: json!({}),
        setup: json!({}),
    });
    assert!(kept.rest() > 0);
    let unshelved = attached(&kept, &client, &mut frames, Some(left_at));
    assert!(
        matches!(unshelved, ChatAttachment::Live { .. }),
        "reattaching an unshelved mid-turn chat with a prior mark must send Live"
    );
}

#[tokio::test]
async fn prompt_chat_steers_mid_turn_when_requested_and_supported() {
    use sikemux_calls::chat::PromptChatParams;
    use sikemux_chat::server::chat::calls::prompt_chat;
    use sikemux_chat::server::chat::queue::Queued;
    use sikemux_chat::server::chat::{ChatCommand, Origin, Steered, Whose};
    use sikemux_chat::server::chat_state::ChatParts;
    use sikemux_wire::protocol::ChatLaunch;

    let dir = tempfile::tempdir().unwrap();
    let core = crate::server::Core::new(Default::default(), Some(dir.path())).unwrap();
    let launch = |agent_id: &str| ChatLaunch {
        agent_id: agent_id.into(),
        provider: "antigravity".into(),
        cwd: "/repo".into(),
        program: "/bin/false".into(),
        args: Vec::new(),
        env: Default::default(),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: PermissionMode::WorkspaceWrite,
        model: None,
        model_provider: None,
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    };

    let mut steerable = core
        .chats()
        .insert(&core, launch("steerable"), Origin::default(), None)
        .unwrap();
    steerable.chat.mark_ready(ChatStart {
        session_id: "s1".into(),
        capabilities: json!({ "steering": true }),
        setup: json!({}),
    });
    steerable.chat.start_prompted();
    steerable.chat.feed.enqueue(Queued::new(
        Some("q1".into()),
        "earlier queued".into(),
        Vec::new(),
        Vec::new(),
        Whose::PERSON,
    ));

    let driver = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(command) = steerable.queue.recv().await {
            match command {
                ChatCommand::Steer { text, reply, .. } => {
                    seen.push(("steer", text));
                    let _ = reply.send(Ok(Steered::Injected));
                }
                ChatCommand::Prompt { text, .. } => {
                    seen.push(("prompt", text));
                }
                _ => {}
            }
            if seen.len() == 2 {
                break;
            }
        }
        seen
    });

    prompt_chat(
        &core,
        PromptChatParams {
            steer: true,
            ..PromptChatParams::new("steerable".into(), "isnt the bottom one redundant?".into())
        },
    )
    .await
    .unwrap();
    assert!(!steerable.chat.feed.has_queued());

    prompt_chat(
        &core,
        PromptChatParams::new("steerable".into(), "wait behind turn".into()),
    )
    .await
    .unwrap();

    assert_eq!(
        driver.await.unwrap(),
        vec![
            (
                "steer",
                "earlier queued\n\nisnt the bottom one redundant?".into()
            ),
            ("prompt", "wait behind turn".into()),
        ]
    );

    let mut unsteerable = core
        .chats()
        .insert(&core, launch("unsteerable"), Origin::default(), None)
        .unwrap();
    unsteerable.chat.mark_ready(ChatStart {
        session_id: "s2".into(),
        capabilities: json!({ "steering": false }),
        setup: json!({}),
    });
    unsteerable.chat.start_prompted();

    prompt_chat(
        &core,
        PromptChatParams {
            steer: true,
            ..PromptChatParams::new("unsteerable".into(), "fallback to prompt".into())
        },
    )
    .await
    .unwrap();

    match unsteerable.queue.recv().await {
        Some(ChatCommand::Prompt { text, .. }) => assert_eq!(text, "fallback to prompt"),
        other => panic!("expected Prompt fallback, got {}", other.is_some()),
    }
}
