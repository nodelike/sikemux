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
