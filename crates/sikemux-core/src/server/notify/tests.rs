use std::sync::atomic::{AtomicBool, Ordering};

use iroh::SecretKey;
use serde_json::json;

use super::*;
use crate::accounts::protocol::PushResult;
use crate::protocol::{
    AgentState, AttentionKind, DeviceAccess, DeviceInfo, NotificationState, NotifyMute, NotifyPrefs,
};
use crate::push::NotificationKey;
use crate::server::access::Peer;
use crate::server::chat::permission;
use crate::server::connection::ClientConn;

const T: u64 = 1_791_000_000_000;

struct Here(AtomicBool);

impl Presence for Here {
    fn away(&self) -> Option<bool> {
        Some(!self.0.load(Ordering::Acquire))
    }
}

struct Host {
    core: Arc<Core>,
    here: Arc<Here>,
    host: String,
    phone: String,
    key: NotificationKey,
}

fn prefs(when: NotifyWhen) -> NotifyPrefs {
    NotifyPrefs {
        needs_you: true,
        finished: true,
        problems: true,
        when,
        muted: Vec::new(),
    }
}

/// A signed-in host with one phone that asked for notifications, and a chat
/// agent `chat-1` working in `/work/sikemux`.
fn host(when: NotifyWhen) -> Host {
    let core = Core::new(crate::protocol::BuildIdentity::default(), None).unwrap();
    let secret = SecretKey::generate();
    let host = secret.public().to_string();
    let phone = SecretKey::generate().public().to_string();
    core.remote.stand_in(
        secret,
        Some("user_2abc"),
        vec![DeviceInfo {
            id: phone.clone(),
            name: "Pixel".into(),
            platform: "android".into(),
            access: DeviceAccess::Watch,
            paired_at: 1,
            last_seen: None,
        }],
    );
    let key = NotificationKey::new(7, [5; 32]);
    core.remote
        .set_notifications(&phone, key.id, &key.to_hex(), prefs(when))
        .unwrap();
    let here = Arc::new(Here(AtomicBool::new(true)));
    core.notify.set_presence(here.clone());
    let _ = core.notify.host_name.set("MacBook Pro".into());
    super::super::chat::idle(&core, "chat-1", "claude", "/work/sikemux");
    Host {
        core,
        here,
        host,
        phone,
        key,
    }
}

impl Host {
    fn leave(&self) {
        self.here.0.store(false, Ordering::Release);
    }

    fn come_back(&self) {
        self.here.0.store(true, Ordering::Release);
    }

    fn signal(&self, signal: Signal, now: u64) {
        handle(&self.core, signal, now);
    }

    /// What went out to the server since the last look.
    fn sent(&self) -> Vec<(Push, Notification)> {
        let outbox = self.core.remote.outbox();
        let pushes = outbox.waiting();
        outbox.take();
        pushes
            .into_iter()
            .map(|push| {
                let plaintext = push::open(&self.key, &self.host, &self.phone, &push.blob)
                    .expect("sealed for this phone");
                let notification = serde_json::from_slice(&plaintext).unwrap();
                (push, notification)
            })
            .collect()
    }

    fn set_prefs(&self, prefs: NotifyPrefs) {
        self.core
            .remote
            .set_notifications(&self.phone, self.key.id, &self.key.to_hex(), prefs)
            .unwrap();
    }
}

fn attention(id: &str) -> Attention {
    let request = json!({
        "toolCall": { "title": "pnpm test src/settings\nand more" },
        "options": [
            { "optionId": "always", "name": "Always allow", "kind": "allow_always" },
            { "optionId": "once", "name": "Allow", "kind": "allow_once" },
            { "optionId": "no", "name": "Reject", "kind": "reject_once" },
        ],
    });
    Attention {
        id: id.into(),
        kind: AttentionKind::Permission,
        agent_id: "chat-1".into(),
        provider: "claude".into(),
        cwd: "/work/sikemux".into(),
        choices: permission::choices(&request),
        request,
        at: T,
    }
}

#[tokio::test]
async fn a_permission_request_reaches_the_phone_sealed_with_what_it_needs_to_answer() {
    let host = host(NotifyWhen::Always);
    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    let sent = host.sent();
    assert_eq!(sent.len(), 1);
    let (push, notification) = &sent[0];
    assert_eq!(push.to, host.phone);
    assert_eq!(push.kind, PushKind::Alert);
    assert_eq!(
        push.collapse_id,
        host.key.collapse_id("permission|chat-1|r1")
    );
    assert_eq!(push.expires_at, T + PERMISSION_LIFE_MS);
    assert_eq!(notification.kind, NotificationKind::Permission);
    assert_eq!(
        notification.category,
        Some(NotificationCategory::Permission)
    );
    assert_eq!(notification.collapse_id, push.collapse_id);
    assert_eq!(notification.title, "Claude needs permission");
    assert_eq!(notification.body, "sikemux on MacBook Pro");
    assert_eq!(
        notification.detail.as_deref(),
        Some("pnpm test src/settings")
    );
    assert_eq!(notification.request_id.as_deref(), Some("r1"));
    assert_eq!(notification.allow_option_id.as_deref(), Some("once"));
    assert_eq!(notification.reject_option_id.as_deref(), Some("no"));
    assert_eq!(
        notification.url,
        format!("sikemux://device/{}/chat/chat-1", host.host)
    );
    assert_eq!(notification.thread, format!("{}/chat-1", host.host));

    host.signal(Signal::Attention(Box::new(attention("r1"))), T + 1);
    assert!(host.sent().is_empty(), "a request is announced once");
}

#[tokio::test]
async fn a_request_without_both_answers_shows_no_buttons() {
    let host = host(NotifyWhen::Always);
    let mut asked = attention("r1");
    asked.request = json!({ "options": [{ "optionId": "ok", "kind": "allow_once" }] });
    asked.choices = permission::choices(&asked.request);
    host.signal(Signal::Attention(Box::new(asked)), T);
    let (_, notification) = host.sent().remove(0);
    assert_eq!(notification.category, Some(NotificationCategory::NeedsYou));
    assert_eq!(notification.reject_option_id, None);
    assert_eq!(notification.detail, None);
}

#[tokio::test]
async fn while_the_person_is_here_a_request_waits_until_they_leave() {
    let host = host(NotifyWhen::Away);
    look_around(&host.core, T);
    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    host.signal(Signal::Attention(Box::new(attention("r2"))), T);
    host.signal(
        Signal::Cleared {
            id: "r2".into(),
            agent_id: "chat-1".into(),
        },
        T,
    );
    assert!(host.sent().is_empty());

    host.leave();
    look_around(&host.core, T + 1000);
    let sent = host.sent();
    assert_eq!(sent.len(), 1, "only the request still waiting");
    assert_eq!(sent[0].1.request_id.as_deref(), Some("r1"));

    host.come_back();
    look_around(&host.core, T + 2000);
    host.leave();
    look_around(&host.core, T + 3000);
    assert!(host.sent().is_empty(), "never twice for one request");
}

#[tokio::test]
async fn a_phone_that_turned_this_host_off_hears_nothing() {
    let host = host(NotifyWhen::Off);
    host.leave();
    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    assert!(host.sent().is_empty());
    let status = host.core.remote.status();
    assert_eq!(status.notifications[0].state, NotificationState::Off);
}

#[tokio::test]
async fn an_answered_request_takes_its_card_off_the_phone() {
    let host = host(NotifyWhen::Always);
    let cleared = |id: &str| Signal::Cleared {
        id: id.into(),
        agent_id: "chat-1".into(),
    };

    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    host.signal(cleared("r1"), T + 1);
    assert!(
        host.sent().is_empty(),
        "a card that never went out is withdrawn"
    );

    host.signal(Signal::Attention(Box::new(attention("r2"))), T + 2);
    let (alert, _) = host.sent().remove(0);
    host.signal(cleared("r2"), T + 3);
    let sent = host.sent();
    assert_eq!(sent.len(), 1);
    let (clear, notification) = &sent[0];
    assert_eq!(clear.kind, PushKind::Clear);
    assert_eq!(clear.collapse_id, alert.collapse_id);
    assert_eq!(notification.kind, NotificationKind::Clear);
    assert_eq!(notification.channel, None);
    assert_eq!(notification.request_id.as_deref(), Some("r2"));

    host.signal(cleared("r2"), T + 4);
    assert!(host.sent().is_empty(), "cleared once");
}

#[tokio::test]
async fn a_long_turn_finishing_notifies_at_most_every_five_minutes() {
    let host = host(NotifyWhen::Always);
    let turn = |started: u64, took: u64, cancelled: bool| {
        host.signal(
            Signal::TurnStarted {
                agent_id: "chat-1".into(),
            },
            started,
        );
        host.signal(
            Signal::TurnCompleted {
                agent_id: "chat-1".into(),
                cancelled,
            },
            started + took,
        );
        host.sent()
    };
    assert!(
        turn(T, 29_000, false).is_empty(),
        "short turns pass quietly"
    );
    let sent = turn(T, 31_000, false);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.kind, NotificationKind::Finished);
    assert_eq!(sent[0].1.title, "Claude finished");
    assert_eq!(sent[0].0.expires_at, T + 31_000 + FINISHED_LIFE_MS);
    assert!(turn(T + 60_000, 40_000, false).is_empty());
    assert!(
        turn(T + 400_000, 40_000, true).is_empty(),
        "a stopped turn did not finish"
    );
    assert_eq!(turn(T + 400_000, 40_000, false).len(), 1);
}

#[tokio::test]
async fn a_problem_says_what_went_wrong_once_a_minute() {
    let host = host(NotifyWhen::Always);
    let error = |at: u64| {
        host.signal(
            Signal::Problem {
                agent_id: "chat-1".into(),
                message: "the agent's process ended\nstack".into(),
            },
            at,
        );
        host.sent()
    };
    let sent = error(T);
    assert_eq!(sent[0].1.kind, NotificationKind::Problem);
    assert_eq!(
        sent[0].1.channel,
        Some(crate::push::NotificationChannel::Problems)
    );
    assert_eq!(
        sent[0].1.detail.as_deref(),
        Some("the agent's process ended")
    );
    assert!(error(T + 30_000).is_empty());
    assert_eq!(error(T + 61_000).len(), 1);
}

#[tokio::test]
async fn waiting_for_input_notifies_and_clears_when_the_agent_moves_on() {
    let host = host(NotifyWhen::Always);
    let state = |state: AgentState, at: u64| {
        host.signal(
            Signal::AgentState {
                agent_id: "chat-1".into(),
                state,
            },
            at,
        );
        host.sent()
    };
    let sent = state(AgentState::Blocked, T);
    assert_eq!(sent[0].1.kind, NotificationKind::Input);
    assert_eq!(sent[0].1.title, "Claude needs your input");
    assert!(state(AgentState::Blocked, T + 1).is_empty());
    let sent = state(AgentState::Working, T + 2);
    assert_eq!(sent[0].0.kind, PushKind::Clear);
    assert!(state(AgentState::Idle, T + 3).is_empty());
}

#[tokio::test]
async fn the_phone_s_choices_and_mutes_hold() {
    let host = host(NotifyWhen::Always);
    host.set_prefs(NotifyPrefs {
        needs_you: false,
        ..prefs(NotifyWhen::Always)
    });
    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    assert!(host.sent().is_empty());

    host.set_prefs(NotifyPrefs {
        muted: vec![NotifyMute {
            agent_id: "chat-1".into(),
            until: Some(T + 1000),
        }],
        ..prefs(NotifyWhen::Always)
    });
    host.signal(Signal::Attention(Box::new(attention("r2"))), T);
    assert!(host.sent().is_empty());
    host.signal(Signal::Attention(Box::new(attention("r3"))), T + 1000);
    assert_eq!(host.sent().len(), 1, "the mute ran out");

    host.set_prefs(NotifyPrefs {
        muted: vec![NotifyMute {
            agent_id: "chat-1".into(),
            until: None,
        }],
        ..prefs(NotifyWhen::Always)
    });
    host.signal(Signal::Attention(Box::new(attention("r4"))), T + 2000);
    assert!(host.sent().is_empty());
}

#[tokio::test]
async fn nothing_reaches_a_phone_showing_the_chat() {
    let host = host(NotifyWhen::Always);
    let client = ClientConn::stand_in(
        &host.core,
        Peer::Device {
            id: host.phone.clone(),
        },
    );
    let other = ClientConn::stand_in(
        &host.core,
        Peer::Device {
            id: "someone-else".into(),
        },
    );
    let chat = host.core.chats.get("chat-1").unwrap();
    chat.feed.subscribe(&client);
    chat.feed.subscribe(&other);
    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    assert!(host.sent().is_empty());

    client.set_foreground(false);
    host.signal(Signal::Attention(Box::new(attention("r2"))), T);
    assert_eq!(host.sent().len(), 1, "the app went to the background");
}

#[tokio::test]
async fn a_host_signed_out_sends_nothing() {
    let host = host(NotifyWhen::Always);
    let phones = host.core.remote.status().devices;
    host.core
        .remote
        .stand_in(SecretKey::generate(), None, phones);
    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    assert!(host.sent().is_empty());
    let status = host.core.remote.status();
    assert_eq!(status.notifications[0].state, NotificationState::SignedOut);
}

#[tokio::test]
async fn what_the_server_answers_shows_in_settings_and_a_refusal_stops_pushing() {
    let host = host(NotifyWhen::Always);
    let push = |result| {
        let push = Push {
            to: host.phone.clone(),
            kind: PushKind::Alert,
            collapse_id: "0".repeat(32),
            blob: "AAAA".into(),
            expires_at: T,
        };
        host.core.remote.note_pushed(&push, result)
    };
    let state = || host.core.remote.status().notifications[0].state;
    assert_eq!(state(), NotificationState::On);
    assert!(push(PushResult::NoToken));
    assert_eq!(state(), NotificationState::PhoneOff);
    assert!(!push(PushResult::NoToken));
    assert!(push(PushResult::Failed));
    assert_eq!(state(), NotificationState::NotReaching);
    assert!(!push(PushResult::Throttled));
    assert!(push(PushResult::Sent));
    assert_eq!(state(), NotificationState::On);

    assert!(push(PushResult::NotAllowed));
    assert_eq!(state(), NotificationState::OtherAccount);
    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    assert!(host.sent().is_empty());

    host.set_prefs(prefs(NotifyWhen::Always));
    host.signal(Signal::Attention(Box::new(attention("r2"))), T);
    assert_eq!(host.sent().len(), 1, "sending its key again tries again");
}

#[tokio::test]
async fn only_a_paired_phone_with_a_real_key_is_kept() {
    let host = host(NotifyWhen::Always);
    let remote = &host.core.remote;
    let key = host.key.to_hex();
    assert!(remote
        .set_notifications("stranger", 1, &key, prefs(NotifyWhen::Always))
        .is_err());
    assert!(remote
        .set_notifications(&host.phone, 1, "abcd", prefs(NotifyWhen::Always))
        .is_err());
    let flood = NotifyPrefs {
        muted: (0..300)
            .map(|n| NotifyMute {
                agent_id: n.to_string(),
                until: None,
            })
            .collect(),
        ..prefs(NotifyWhen::Always)
    };
    assert!(remote
        .set_notifications(&host.phone, 1, &key, flood)
        .is_err());
    assert_eq!(remote.status().notifications.len(), 1);
}

#[tokio::test]
async fn revoking_a_phone_forgets_its_key() {
    let host = host(NotifyWhen::Always);
    super::super::remote::revoke(&host.core, &host.phone).unwrap();
    assert!(host.core.remote.status().notifications.is_empty());
    assert!(host
        .core
        .remote
        .notify_targets()
        .is_some_and(|(_, targets)| targets.is_empty()));
}

#[tokio::test]
async fn clearing_stops_notifications() {
    let host = host(NotifyWhen::Always);
    host.core.remote.clear_notifications(&host.phone).unwrap();
    host.signal(Signal::Attention(Box::new(attention("r1"))), T);
    assert!(host.sent().is_empty());
    assert!(host.core.remote.status().notifications.is_empty());
}

/// Against a stand-in for the accounts server: the push goes out on the live
/// connection and the server's answer shows in the phone's state.
#[tokio::test]
async fn a_push_reaches_the_server_and_its_answer_comes_back() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_websockets::{Message, ServerBuilder};

    let host = host(NotifyWhen::Always);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    host.core
        .remote
        .set_accounts_api(&format!("http://{}", listener.local_addr().unwrap()));
    super::super::remote::ensure_live(&host.core);
    let (stream, _) = listener.accept().await.unwrap();
    let (_, mut socket) = ServerBuilder::new().accept(stream).await.unwrap();
    let send = |value: serde_json::Value| Message::text(value.to_string());
    let read = |message: Message| -> serde_json::Value {
        serde_json::from_str(message.as_text().unwrap()).unwrap()
    };
    socket
        .send(send(json!({ "type": "challenge", "nonce": "ab".repeat(32), "expiresAt": "2026-10-03T00:00:30Z" })))
        .await
        .unwrap();
    socket.next().await.unwrap().unwrap();
    socket
        .send(send(
            json!({ "type": "ready", "latest": 0, "heartbeatMs": 25000 }),
        ))
        .await
        .unwrap();

    let mut asked = attention("r1");
    asked.at = epoch_ms();
    host.signal(Signal::Attention(Box::new(asked)), epoch_ms());
    let push = read(socket.next().await.unwrap().unwrap());
    assert_eq!(push["type"], "push");
    assert_eq!(push["to"], host.phone);
    assert_eq!(push["kind"], "alert");
    assert_eq!(
        push["collapseId"],
        host.key.collapse_id("permission|chat-1|r1")
    );
    let blob = push["blob"].as_str().unwrap();
    assert!(blob.len() <= crate::push::MAX_BLOB_CHARS);
    assert!(crate::push::open(&host.key, &host.host, &host.phone, blob).is_some());

    socket
        .send(send(
            json!({ "type": "pushed", "ref": push["ref"], "result": "no_token" }),
        ))
        .await
        .unwrap();
    socket.send(send(json!({ "type": "ping" }))).await.unwrap();
    assert_eq!(read(socket.next().await.unwrap().unwrap())["type"], "pong");
    assert_eq!(
        host.core.remote.status().notifications[0].state,
        NotificationState::PhoneOff
    );
    super::super::remote::stop_live(&host.core);
}
