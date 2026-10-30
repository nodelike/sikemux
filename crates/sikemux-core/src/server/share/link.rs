//! The core's one connection to the accounts server's `/v1/share`, which
//! carries every chat it shares. It runs while there is a share to carry and
//! reconnects when it drops, opening each share again and sending a fresh
//! snapshot, since the server may have lost what it held.

use std::sync::{Arc, Weak};
use std::time::Duration;

use futures_util::SinkExt;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::mpsc;

use super::super::Core;

use super::{answer, published, save, take_snapshot, Running, ShareState, ToLink};
use crate::accounts::live::{rfc3339, socket_url};
use crate::accounts::share::{
    ShareClientMessage, ShareClose, ShareCloseCode, ShareEndReason, ShareFrame, ShareHello,
    ShareOpen, SharePong, SharePublish, ShareRefusal, ShareRole, ShareServerMessage,
};
use crate::accounts::socket::{self, Closed, Failures, Frame, Socket};

/// How often expiries and chats that started again are looked for.
const TICK: Duration = Duration::from_secs(3);
/// A send the server has not taken by then means the connection is stuck, so it is made again.
const SEND_LIMIT: Duration = socket::HEARTBEAT;

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
    let mut failures = Failures::default();
    loop {
        let ended = match core.upgrade() {
            Some(core) => session(&core, &url, &mut queue).await,
            None => return,
        };
        let Some(alive) = core.upgrade() else {
            return;
        };
        {
            let mut inner = alive.shares.inner.locked();
            for running in inner.running.values_mut() {
                if running.state == ShareState::Open {
                    running.state = ShareState::Connecting;
                }
            }
            super::changed(&alive);
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
        let wait = failures.wait(hint, was_open);
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
                        failures.forget();
                        break;
                    }
                    None => return,
                },
            }
        }
    }
}

fn open_message(id: &str, expires_at: u64, viewers: &str) -> ShareClientMessage {
    ShareClientMessage::Open(ShareOpen {
        share: id.to_owned(),
        expires_at: rfc3339(expires_at),
        viewers: Some(viewers.to_owned()),
    })
}

fn close_message(id: &str) -> ShareClientMessage {
    ShareClientMessage::Close(ShareClose {
        share: id.to_owned(),
    })
}

/// Seals what a share's client heard and numbers it, if it is something to
/// publish and the share is open.
async fn send(socket: &mut Socket, message: &ShareClientMessage) -> bool {
    tokio::time::timeout(SEND_LIMIT, socket::send(socket, message))
        .await
        .unwrap_or(false)
}

fn publish(core: &Core, id: &str, frame: &[u8]) -> Option<ShareClientMessage> {
    let (plain, snapshot) = published(core, id, frame)?;
    let mut inner = core.shares.inner.locked();
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

/// Ends shares past their time and sends a snapshot of each chat that
/// started again since its last one.
fn tick(core: &Arc<Core>) {
    let now = epoch_ms();
    let mut stale = Vec::new();
    {
        let mut inner = core.shares.inner.locked();
        let before = inner.running.len();
        inner
            .running
            .retain(|_, running| running.stored.expires_at > now);
        inner.closing.retain(|stored| stored.expires_at > now);
        if inner.running.len() != before {
            save(core, &inner);
        }
        for running in inner.running.values() {
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
        }
    }
    for id in stale {
        take_snapshot(core, &id);
    }
}

async fn session(
    core: &Arc<Core>,
    url: &str,
    queue: &mut mpsc::UnboundedReceiver<ToLink>,
) -> Ended {
    let retry = |hint, was_open| Ended::Retry { hint, was_open };
    let Some(mut socket) = socket::connect(url).await else {
        return retry(None, false);
    };
    let nonce = socket::challenge(&mut socket, |message| match message {
        ShareServerMessage::Challenge(challenge) => Some(challenge.nonce),
        _ => None,
    });
    let Some(nonce) = nonce.await else {
        return retry(None, false);
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
        proof: None,
        since: None,
    });
    if !send(&mut socket, &hello).await {
        return retry(None, false);
    }

    let mut open = false;
    let mut hint: Option<Duration> = None;
    let mut silence = socket::HEARTBEAT * 2;
    let mut ticker = tokio::time::interval(TICK);
    loop {
        let message = tokio::select! {
            frame = socket::next_frame(&mut socket, silence) => match frame {
                Frame::Server(message) => message,
                Frame::Other => continue,
                Frame::Closed(code) => {
                    let code = code.and_then(|code| ShareCloseCode::from_code(code.into()));
                    return match code {
                        Some(ShareCloseCode::Replaced) => {
                            retry(socket::wait_after(Closed::Replaced, hint), open)
                        }
                        Some(ShareCloseCode::OverLimit) => {
                            retry(socket::wait_after(Closed::OverLimit, hint), open)
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
                let mut taken = None;
                let outgoing = match heard {
                    ToLink::Open(id) => core
                        .shares
                        .inner.locked()
                        .running
                        .get(&id)
                        .map(|running| {
                            open_message(&id, running.stored.expires_at, running.seal.viewer_check())
                        }),
                    ToLink::Close(id) => Some(close_message(&id)),
                    ToLink::Heard(id, frame, took) => {
                        taken = Some(took);
                        publish(core, &id, &frame)
                    }
                    ToLink::Send(message) => Some(message),
                    ToLink::Retry => None,
                };
                if let Some(outgoing) = outgoing {
                    if !send(&mut socket, &outgoing).await {
                        return retry(hint, open);
                    }
                }
                drop(taken);
                continue;
            }
            _ = ticker.tick() => {
                if open {
                    tick(core);
                }
                let idle = {
                    let inner = core.shares.inner.locked();
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
                silence = socket::silence(ready.heartbeat_ms);
                let (opening, closing): (Vec<_>, Vec<_>) = {
                    let inner = core.shares.inner.locked();
                    (
                        inner
                            .running
                            .values()
                            .filter(|running| running.state != ShareState::Refused)
                            .map(|running| {
                                open_message(
                                    &running.stored.id,
                                    running.stored.expires_at,
                                    running.seal.viewer_check(),
                                )
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
                    sent = sent && send(&mut socket, &message).await;
                }
                sent
            }
            ShareServerMessage::Opened(opened) => {
                let mut inner = core.shares.inner.locked();
                if let Some(running) = inner.running.get_mut(&opened.share) {
                    running.state = ShareState::Open;
                    running.problem = None;
                    drop(inner);
                    super::changed(core);
                    take_snapshot(core, &opened.share);
                }
                true
            }
            ShareServerMessage::Refused(refused) => {
                let mut inner = core.shares.inner.locked();
                if let Some(running) = inner.running.get_mut(&refused.share) {
                    running.state = ShareState::Refused;
                    running.problem = Some(refusal_text(refused.reason).to_owned());
                    super::changed(core);
                    running.client.close();
                    super::chat::detach(core, running.client.id, &running.stored.agent_id);
                }
                true
            }
            ShareServerMessage::Viewers(viewers) => {
                if let Some(running) = core.shares.inner.locked().running.get_mut(&viewers.share) {
                    running.viewers = u32::try_from(viewers.count).unwrap_or(0);
                    super::changed(core);
                }
                true
            }
            ShareServerMessage::Ask(asked) => {
                let core = core.clone();
                tokio::spawn(async move {
                    if let Some(reply) =
                        answer(core.clone(), asked.share, asked.viewer, asked.sealed).await
                    {
                        let mut inner = core.shares.inner.locked();
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
                let mut inner = core.shares.inner.locked();
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
                send(&mut socket, &ShareClientMessage::Pong(SharePong {})).await
            }
            ShareServerMessage::Bye(bye) => {
                hint = Some(socket::asked_wait(bye.reconnect_after_ms));
                true
            }
            _ => true,
        };
        if !sent {
            return retry(hint, open);
        }
    }
}
