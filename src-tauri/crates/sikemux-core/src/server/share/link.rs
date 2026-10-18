//! The core's one connection to the accounts server's `/v1/share`, which
//! carries every chat it shares. It runs while there is a share to carry and
//! reconnects when it drops, opening each share again and sending a fresh
//! snapshot, since the server may have lost what it held.

use std::sync::{Arc, Weak};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_websockets::Message;

use super::super::remote::unix_ms;
use super::super::Core;
use serde_json::json;

use super::{answer, published, save, take_snapshot, waiting_on, Running, ShareState, ToLink};
use crate::accounts::live::{backoff, connect, rfc3339, socket_url, Socket};
use crate::accounts::share::{
    ShareClientMessage, ShareClose, ShareCloseCode, ShareEndReason, ShareFrame, ShareHello,
    ShareOpen, SharePong, SharePublish, ShareRefusal, ShareRole, ShareServerMessage,
};

const CHALLENGE_WAIT: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(25);
/// How often expiries and chats that started again are looked for.
const TICK: Duration = Duration::from_secs(3);
const REPLACED_WAIT: Duration = Duration::from_secs(60);
const OVER_LIMIT_WAIT: Duration = Duration::from_secs(30);

enum Ended {
    /// Nothing is shared any more.
    Done,
    Retry {
        hint: Option<Duration>,
        was_open: bool,
    },
}

pub(super) async fn run(
    core: Weak<Core>,
    base: String,
    mut queue: mpsc::UnboundedReceiver<ToLink>,
) {
    let url = socket_url(&base, "/v1/share");
    let mut failures: u32 = 0;
    loop {
        let ended = match core.upgrade() {
            Some(core) => session(&core, &url, &mut queue).await,
            None => return,
        };
        let Some(alive) = core.upgrade() else {
            return;
        };
        {
            let mut inner = alive.shares.lock();
            for running in inner.running.values_mut() {
                if running.state == ShareState::Open {
                    running.state = ShareState::Connecting;
                }
            }
            if matches!(ended, Ended::Done)
                || (inner.running.is_empty() && inner.closing.is_empty())
            {
                inner.link = None;
                return;
            }
        }
        drop(alive);
        let Ended::Retry { hint, was_open } = ended else {
            return;
        };
        if was_open {
            failures = 0;
        }
        failures = failures.saturating_add(1);
        let wait = backoff(failures).max(hint.unwrap_or_default());
        let deadline = tokio::time::Instant::now() + wait;
        // What arrives while offline is let go of; the snapshot after the
        // reconnect carries it.
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                message = queue.recv() => match message {
                    Some(ToLink::Heard(..) | ToLink::Send(_)) => {}
                    Some(ToLink::Open(_) | ToLink::Close(_)) => break,
                    Some(ToLink::Retry) => {
                        failures = 0;
                        break;
                    }
                    None => return,
                },
            }
        }
    }
}

async fn send(socket: &mut Socket, message: ShareClientMessage) -> bool {
    let Ok(text) = serde_json::to_string(&message) else {
        return false;
    };
    socket.send(Message::text(text)).await.is_ok()
}

enum Frame {
    Server(ShareServerMessage),
    Closed(Option<u16>),
    Other,
}

async fn next_frame(socket: &mut Socket, wait: Duration) -> Frame {
    let message = match tokio::time::timeout(wait, socket.next()).await {
        Ok(Some(Ok(message))) => message,
        _ => return Frame::Closed(None),
    };
    if let Some((code, _)) = message.as_close() {
        return Frame::Closed(Some(code.into()));
    }
    match message.as_text().map(serde_json::from_str) {
        Some(Ok(message)) => Frame::Server(message),
        _ => Frame::Other,
    }
}

fn open_message(id: &str, expires_at: u64) -> ShareClientMessage {
    ShareClientMessage::Open(ShareOpen {
        share: id.to_owned(),
        expires_at: rfc3339(expires_at),
    })
}

fn close_message(id: &str) -> ShareClientMessage {
    ShareClientMessage::Close(ShareClose {
        share: id.to_owned(),
    })
}

/// Seals what a share's client heard and numbers it, if it is something to
/// publish and the share is open.
fn publish(core: &Core, id: &str, frame: &[u8]) -> Option<ShareClientMessage> {
    let (plain, snapshot) = published(core, id, frame)?;
    let mut inner = core.shares.lock();
    numbered(inner.running.get_mut(id)?, &plain, snapshot)
}

/// `plain` sealed as the share's next frame.
fn numbered(running: &mut Running, plain: &[u8], snapshot: bool) -> Option<ShareClientMessage> {
    if running.state != ShareState::Open {
        return None;
    }
    let id = running.stored.id.clone();
    let sealed = running.seal.seal(plain)?;
    running.seq += 1;
    Some(ShareClientMessage::Publish(SharePublish {
        share: id,
        frame: ShareFrame {
            seq: running.seq as i64,
            snapshot,
            sealed,
        },
    }))
}

fn refusal_text(reason: ShareRefusal) -> &'static str {
    match reason {
        ShareRefusal::Limit => "This account already shares as many chats as it may.",
        ShareRefusal::Exists => "The server already has a share with this link.",
        ShareRefusal::Expiry => "The share's end is not one the server accepts.",
        ShareRefusal::Unknown => "The server would not open the share.",
    }
}

/// Ends shares past their time, sends a snapshot of each chat that started
/// again since its last one, and answers with what to tell viewers
/// of the permission requests each chat now waits on.
fn tick(core: &Arc<Core>) -> Vec<ShareClientMessage> {
    let now = unix_ms();
    let mut stale = Vec::new();
    let mut told = Vec::new();
    {
        let mut inner = core.shares.lock();
        let before = inner.running.len();
        inner
            .running
            .retain(|_, running| running.stored.expires_at > now);
        inner.closing.retain(|stored| stored.expires_at > now);
        if inner.running.len() != before {
            save(core, &inner);
        }
        for running in inner.running.values_mut() {
            if running.state != ShareState::Open {
                continue;
            }
            let current = core
                .chats
                .get(&running.stored.agent_id)
                .map(|chat| Arc::as_ptr(&chat) as usize);
            if current.is_some() && current != running.chat {
                stale.push(running.stored.id.clone());
            }
            let waiting = waiting_on(core, &running.stored.agent_id);
            if waiting != running.waiting {
                running.waiting = waiting;
                let text = json!({ "type": "waiting", "requests": running.waiting });
                if let Ok(plain) = serde_json::to_vec(&text) {
                    told.extend(numbered(running, &plain, false));
                }
            }
        }
    }
    for id in stale {
        take_snapshot(core, &id);
    }
    told
}

async fn session(
    core: &Arc<Core>,
    url: &str,
    queue: &mut mpsc::UnboundedReceiver<ToLink>,
) -> Ended {
    let retry = |hint, was_open| Ended::Retry { hint, was_open };
    let Some(mut socket) = connect(url).await else {
        return retry(None, false);
    };
    let nonce = loop {
        match next_frame(&mut socket, CHALLENGE_WAIT).await {
            Frame::Server(ShareServerMessage::Challenge(challenge)) => break challenge.nonce,
            Frame::Server(_) | Frame::Other => continue,
            Frame::Closed(_) => return retry(None, false),
        }
    };
    let Some((key, signature)) = core.remote.sign_share(&nonce) else {
        return retry(None, false);
    };
    let hello = ShareClientMessage::Hello(ShareHello {
        role: ShareRole::Host,
        key: Some(key),
        signature: Some(signature),
        token: None,
        share: None,
        since: None,
    });
    if !send(&mut socket, hello).await {
        return retry(None, false);
    }

    let mut open = false;
    let mut hint: Option<Duration> = None;
    let mut silence = HEARTBEAT * 2;
    let mut ticker = tokio::time::interval(TICK);
    loop {
        let message = tokio::select! {
            frame = next_frame(&mut socket, silence) => match frame {
                Frame::Server(message) => message,
                Frame::Other => continue,
                Frame::Closed(code) => {
                    let code = code.and_then(|code| ShareCloseCode::from_code(code.into()));
                    return match code {
                        Some(ShareCloseCode::Replaced) => retry(Some(REPLACED_WAIT), open),
                        Some(ShareCloseCode::OverLimit) => {
                            retry(Some(hint.unwrap_or_default().max(OVER_LIMIT_WAIT)), open)
                        }
                        _ => retry(hint, open),
                    };
                }
            },
            heard = queue.recv() => {
                let Some(heard) = heard else {
                    return Ended::Done;
                };
                if !open {
                    continue;
                }
                let outgoing = match heard {
                    ToLink::Open(id) => core
                        .shares
                        .lock()
                        .running
                        .get(&id)
                        .map(|running| open_message(&id, running.stored.expires_at)),
                    ToLink::Close(id) => Some(close_message(&id)),
                    ToLink::Heard(id, frame) => publish(core, &id, &frame),
                    ToLink::Send(message) => Some(message),
                    ToLink::Retry => None,
                };
                if let Some(outgoing) = outgoing {
                    if !send(&mut socket, outgoing).await {
                        return retry(hint, open);
                    }
                }
                continue;
            }
            _ = ticker.tick() => {
                if open {
                    for message in tick(core) {
                        if !send(&mut socket, message).await {
                            return retry(hint, open);
                        }
                    }
                }
                let idle = {
                    let inner = core.shares.lock();
                    inner.running.is_empty() && inner.closing.is_empty()
                };
                if idle {
                    let _ = socket.close().await;
                    return Ended::Done;
                }
                continue;
            }
        };
        let sent = match message {
            ShareServerMessage::Ready(ready) => {
                open = true;
                if let Ok(ms) = u64::try_from(ready.heartbeat_ms) {
                    silence = Duration::from_millis(ms) * 2;
                }
                let (opening, closing): (Vec<_>, Vec<_>) = {
                    let inner = core.shares.lock();
                    (
                        inner
                            .running
                            .values()
                            .filter(|running| running.state != ShareState::Refused)
                            .map(|running| {
                                open_message(&running.stored.id, running.stored.expires_at)
                            })
                            .collect(),
                        inner
                            .closing
                            .iter()
                            .map(|stored| close_message(&stored.id))
                            .collect(),
                    )
                };
                let mut sent = true;
                for message in opening.into_iter().chain(closing) {
                    sent = sent && send(&mut socket, message).await;
                }
                sent
            }
            ShareServerMessage::Opened(opened) => {
                let mut inner = core.shares.lock();
                if let Some(running) = inner.running.get_mut(&opened.share) {
                    running.state = ShareState::Open;
                    running.problem = None;
                    drop(inner);
                    take_snapshot(core, &opened.share);
                }
                true
            }
            ShareServerMessage::Refused(refused) => {
                let mut inner = core.shares.lock();
                if let Some(running) = inner.running.get_mut(&refused.share) {
                    running.state = ShareState::Refused;
                    running.problem = Some(refusal_text(refused.reason).to_owned());
                    running.client.close();
                    super::chat::detach(core, running.client.id, &running.stored.agent_id);
                }
                true
            }
            ShareServerMessage::Viewers(viewers) => {
                if let Some(running) = core.shares.lock().running.get_mut(&viewers.share) {
                    running.viewers = u32::try_from(viewers.count).unwrap_or(0);
                }
                true
            }
            ShareServerMessage::Ask(asked) => {
                let core = core.clone();
                tokio::spawn(async move {
                    if let Some(reply) =
                        answer(core.clone(), asked.share, asked.viewer, asked.sealed).await
                    {
                        let mut inner = core.shares.lock();
                        super::tell(&core, &mut inner, reply);
                    }
                });
                true
            }
            ShareServerMessage::Snapshot(wanted) => {
                take_snapshot(core, &wanted.share);
                true
            }
            ShareServerMessage::Ended(ended) => {
                let mut inner = core.shares.lock();
                inner.closing.retain(|stored| stored.id != ended.share);
                if let Some(running) = inner.running.remove(&ended.share) {
                    super::chat::detach(core, running.client.id, &running.stored.agent_id);
                    if ended.reason != ShareEndReason::Stopped {
                        eprintln!("sikemux core: a shared chat's link expired");
                    }
                }
                save(core, &inner);
                true
            }
            ShareServerMessage::Ping(_) => {
                send(&mut socket, ShareClientMessage::Pong(SharePong {})).await
            }
            ShareServerMessage::Bye(bye) => {
                hint = Some(Duration::from_millis(
                    u64::try_from(bye.reconnect_after_ms).unwrap_or_default(),
                ));
                true
            }
            _ => true,
        };
        if !sent {
            return retry(hint, open);
        }
    }
}
