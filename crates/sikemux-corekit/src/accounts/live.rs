//! A host's live connection to its account at `/v1/live`. The server pushes
//! what changed on the account; the host proves its key on every connection,
//! applies each event once, acknowledges it, and reconnects when the
//! connection drops.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::SinkExt;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::OffsetDateTime;
use tokio::sync::{watch, Notify};
use tokio::time::Instant;

use super::protocol::{
    AccountEvent, LiveAck, LiveApp, LiveCloseCode, LiveDeviceMessage, LiveHello, LiveLeave,
    LivePong, LivePush, LiveRole, LiveServerMessage, PushKind, PushResult,
};
use super::socket::{self, send, Closed, Failures, Frame, Socket};
use sikemux_wire::accounts::protocol::RevokeReason;

const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// The server closes a connection that sends more than 20 messages a second,
/// so pushes leave room for acknowledgements and pongs.
const PUSHES_PER_SECOND: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// Signed in: stay connected.
    Stay,
    /// Signed out, but the server has not heard yet: connect, send `leave`.
    Leave,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Live,
    /// The last attempt failed and another is scheduled.
    Offline,
}

pub trait Account: Send + Sync + 'static {
    /// The device key and its signature over [`super::live_message`] for
    /// `nonce`, which must pass [`super::check_live`].
    fn sign_live(&self, nonce: &str) -> Option<(String, String)>;
    /// Answering [`Want::Stop`] ends the connection for good.
    fn want(&self) -> Want;
    /// The id of the last event applied.
    fn cursor(&self) -> i64;
    /// Applies `events`, oldest first, all newer than the cursor, and moves
    /// the cursor to the last.
    fn apply(&self, events: &[AccountEvent]);
    /// The account's newest event id is `latest`. A cursor beyond it belongs
    /// to history the server no longer has, so it starts over from nothing;
    /// the server sends only what is newer than its own record of this host.
    fn rewind(&self, latest: i64);
    fn link(&self, link: Link);
    /// The server took this device off the account. `None` when it no longer
    /// knows the key at all.
    fn removed(&self, reason: Option<RevokeReason>);
    fn pushed(&self, push: Push, result: PushResult);
}

/// A notification for one of the account's phones, sealed by the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub to: String,
    pub kind: PushKind,
    pub collapse_id: String,
    /// For a Live Activity's update or end: the activity's own push token.
    pub activity_token: Option<String>,
    pub blob: String,
    /// Milliseconds since the Unix epoch. Until then a push waits for the
    /// connection; after it, it is dropped.
    pub expires_at: u64,
}

/// Pushes waiting for the live connection, and those sent but not yet
/// answered. Each goes out with a `ref` one higher than the last, and its
/// answer carries it back. Each keeps the id it was posted with when it goes
/// out again, so the server delivers it only once.
#[derive(Default)]
pub struct Outbox {
    state: Mutex<OutboxState>,
    ready: Notify,
}

#[derive(Default)]
struct OutboxState {
    last_ref: i64,
    waiting: VecDeque<Posted>,
    sent: BTreeMap<i64, Posted>,
}

struct Posted {
    id: String,
    push: Push,
}

impl Outbox {
    /// Queues `push`, replacing an alert for the same card that has not gone
    /// out yet.
    pub fn post(&self, push: Push) {
        let now = epoch_ms();
        let mut state = self.state.locked();
        state.waiting.retain(|Posted { push: waiting, .. }| {
            waiting.expires_at > now
                && !(waiting.to == push.to && waiting.collapse_id == push.collapse_id)
        });
        state.waiting.push_back(Posted {
            id: uuid::Uuid::new_v4().simple().to_string(),
            push,
        });
        drop(state);
        self.ready.notify_one();
    }

    /// Drops the push for this card that has not gone out yet. True when
    /// there was one.
    pub fn withdraw(&self, to: &str, collapse_id: &str) -> bool {
        let mut state = self.state.locked();
        let before = state.waiting.len();
        state.waiting.retain(|Posted { push: waiting, .. }| {
            !(waiting.to == to && waiting.collapse_id == collapse_id)
        });
        state.waiting.len() != before
    }

    pub fn waiting(&self) -> Vec<Push> {
        let state = self.state.locked();
        state
            .waiting
            .iter()
            .map(|posted| posted.push.clone())
            .collect()
    }

    /// Up to `most` pushes to send now, oldest first.
    pub fn take(&self, most: usize) -> Vec<LivePush> {
        let now = epoch_ms();
        let mut state = self.state.locked();
        let mut taken = Vec::new();
        while taken.len() < most {
            let Some(posted) = state.waiting.pop_front() else {
                break;
            };
            let push = &posted.push;
            if push.expires_at <= now {
                continue;
            }
            state.last_ref += 1;
            let r#ref = state.last_ref;
            taken.push(LivePush {
                r#ref,
                id: Some(posted.id.clone()),
                to: push.to.clone(),
                kind: push.kind,
                collapse_id: push.collapse_id.clone(),
                activity_token: push.activity_token.clone(),
                blob: push.blob.clone(),
                expires_at: rfc3339(push.expires_at),
            });
            state.sent.insert(r#ref, posted);
        }
        taken
    }

    fn has_waiting(&self) -> bool {
        !self.state.locked().waiting.is_empty()
    }

    fn answered(&self, r#ref: i64) -> Option<Push> {
        Some(self.state.locked().sent.remove(&r#ref)?.push)
    }

    /// The connection dropped before these were answered, so they go out
    /// again on the next one, ahead of anything newer.
    fn unanswered(&self) {
        let mut state = self.state.locked();
        let sent = std::mem::take(&mut state.sent);
        for posted in sent.into_values().rev() {
            let push = &posted.push;
            if !state.waiting.iter().any(|Posted { push: waiting, .. }| {
                waiting.to == push.to && waiting.collapse_id == push.collapse_id
            }) {
                state.waiting.push_front(posted);
            }
        }
    }
}

/// `2026-10-03T00:00:30.000Z` for milliseconds since the Unix epoch.
pub fn rfc3339(ms: u64) -> String {
    let at = OffsetDateTime::UNIX_EPOCH + time::Duration::milliseconds(ms as i64);
    at.format(format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    ))
    .unwrap_or_default()
}

/// Milliseconds since the Unix epoch for an RFC 3339 time such as the server
/// writes, `2026-10-03T00:00:30.000Z`.
pub fn parse_rfc3339(text: &str) -> Option<u64> {
    let at = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()
}

/// `base` is the accounts API, `http(s)://host`.
pub fn url(base: &str) -> String {
    socket_url(base, "/v1/live")
}

pub fn socket_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    };
    format!("{base}{path}")
}

/// Keeps the connection while [`Account::want`] asks for one. `changed`
/// fires when that answer may have changed.
pub async fn run<A: Account>(
    account: Arc<A>,
    url: String,
    app: LiveApp,
    outbox: Arc<Outbox>,
    mut changed: watch::Receiver<()>,
) {
    let mut failures = Failures::default();
    account.link(Link::Connecting);
    loop {
        if changed.has_changed().is_err() || account.want() == Want::Stop {
            return;
        }
        let ended = session(&*account, &url, &app, &outbox, &mut changed).await;
        outbox.unanswered();
        let wait = match ended {
            Ended::Done => continue,
            Ended::Retry {
                hint,
                was_live,
                why,
            } => {
                eprintln!("sikemux core: the account's live connection ended: {why}");
                failures.wait(hint, was_live)
            }
        };
        account.link(Link::Offline);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            gone = changed.changed() => {
                if gone.is_err() {
                    return;
                }
                failures.forget();
            }
        }
    }
}

enum Ended {
    Done,
    Retry {
        hint: Option<Duration>,
        was_live: bool,
        why: String,
    },
}

/// Holds pushes to [`PUSHES_PER_SECOND`], so a burst of them, or those going
/// out again after a reconnect, never gets the connection closed.
struct Pace {
    since: Instant,
    sent: usize,
}

impl Pace {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            sent: 0,
        }
    }

    fn allowed(&mut self) -> usize {
        if self.since.elapsed() >= Duration::from_secs(1) {
            *self = Self::new();
        }
        PUSHES_PER_SECOND.saturating_sub(self.sent)
    }

    fn next(&self) -> Instant {
        self.since + Duration::from_secs(1)
    }
}

/// Sends what the pace allows. False when a send failed.
async fn flush(socket: &mut Socket, outbox: &Outbox, pace: &mut Pace) -> bool {
    for push in outbox.take(pace.allowed()) {
        pace.sent += 1;
        if !send(socket, &LiveDeviceMessage::Push(push)).await {
            return false;
        }
    }
    true
}

async fn session<A: Account>(
    account: &A,
    url: &str,
    app: &LiveApp,
    outbox: &Outbox,
    changed: &mut watch::Receiver<()>,
) -> Ended {
    let retry = |hint, was_live, why: &str| Ended::Retry {
        hint,
        was_live,
        why: why.to_owned(),
    };
    let Some(mut socket) = socket::connect(url).await else {
        return retry(None, false, "could not connect");
    };
    let nonce = socket::challenge(&mut socket, |message| match message {
        LiveServerMessage::Challenge(challenge) => Some(challenge.nonce),
        _ => None,
    });
    let Some(nonce) = nonce.await else {
        return retry(None, false, "no challenge came");
    };
    let Some((key, signature)) = account.sign_live(&nonce) else {
        return retry(None, false, "no key to sign the hello with");
    };
    let hello = LiveDeviceMessage::Hello(LiveHello {
        role: LiveRole::Host,
        key: Some(key),
        signature: Some(signature),
        token: None,
        app: Some(app.clone()),
    });
    if !send(&mut socket, &hello).await {
        return retry(None, false, "could not send the hello");
    }

    let mut ready = false;
    let mut leaving = false;
    let mut hint = None;
    let mut silence = socket::HEARTBEAT * 2;
    let mut pace = Pace::new();
    let mut held = false;
    loop {
        let frame = tokio::select! {
            frame = socket::next_frame(&mut socket, silence) => frame,
            _ = outbox.ready.notified(), if ready && !leaving && !held => {
                if !flush(&mut socket, outbox, &mut pace).await {
                    return retry(hint, ready, "could not send a push");
                }
                held = outbox.has_waiting();
                continue;
            }
            _ = tokio::time::sleep_until(pace.next()), if held && !leaving => {
                if !flush(&mut socket, outbox, &mut pace).await {
                    return retry(hint, ready, "could not send a push");
                }
                held = outbox.has_waiting();
                continue;
            }
            gone = changed.changed() => {
                match if gone.is_err() { Want::Stop } else { account.want() } {
                    Want::Stop => {
                        let _ = tokio::time::timeout(CLOSE_WAIT, socket.close()).await;
                        return Ended::Done;
                    }
                    Want::Leave if ready && !leaving => {
                        leaving = send(&mut socket, &LiveDeviceMessage::Leave(LiveLeave {})).await;
                    }
                    _ => {}
                }
                continue;
            }
        };
        let message = match frame {
            Frame::Server(message) => message,
            Frame::Other => continue,
            Frame::Closed(close) => {
                let why = close.to_string();
                return match close
                    .code()
                    .and_then(|code| LiveCloseCode::from_code(code.into()))
                {
                    Some(LiveCloseCode::Unauthenticated) if !ready => {
                        account.removed(None);
                        Ended::Done
                    }
                    Some(LiveCloseCode::Revoked) => {
                        account.removed(None);
                        Ended::Done
                    }
                    Some(LiveCloseCode::Replaced) => {
                        retry(socket::wait_after(Closed::Replaced, hint), ready, &why)
                    }
                    Some(LiveCloseCode::OverLimit) => {
                        retry(socket::wait_after(Closed::OverLimit, hint), ready, &why)
                    }
                    _ => retry(hint, ready, &why),
                };
            }
        };
        let answered = match message {
            LiveServerMessage::Ready(server) => {
                ready = true;
                silence = socket::silence(server.heartbeat_ms);
                account.rewind(server.latest);
                account.link(Link::Live);
                if account.want() == Want::Leave {
                    leaving = true;
                    send(&mut socket, &LiveDeviceMessage::Leave(LiveLeave {})).await
                } else {
                    let flushed = flush(&mut socket, outbox, &mut pace).await;
                    held = outbox.has_waiting();
                    flushed
                }
            }
            LiveServerMessage::Events(batch) => {
                let Some(last) = batch.events.iter().map(|event| event.id).max() else {
                    continue;
                };
                let cursor = account.cursor();
                let mut fresh: Vec<AccountEvent> = batch
                    .events
                    .into_iter()
                    .filter(|event| event.id > cursor)
                    .collect();
                fresh.sort_by_key(|event| event.id);
                if !fresh.is_empty() {
                    account.apply(&fresh);
                }
                send(&mut socket, &LiveDeviceMessage::Ack(LiveAck { id: last })).await
            }
            LiveServerMessage::Reset(reset) => {
                account.rewind(reset.latest);
                true
            }
            LiveServerMessage::Ping(_) => {
                send(&mut socket, &LiveDeviceMessage::Pong(LivePong {})).await
            }
            LiveServerMessage::Bye(bye) => {
                hint = Some(socket::asked_wait(bye.reconnect_after_ms));
                true
            }
            LiveServerMessage::Pushed(pushed) => {
                if let Some(push) = outbox.answered(pushed.r#ref) {
                    account.pushed(push, pushed.result);
                }
                true
            }
            LiveServerMessage::Revoked(revoked) => {
                account.removed(Some(revoked.reason));
                return Ended::Done;
            }
            _ => true,
        };
        if !answered {
            return retry(hint, ready, "could not answer the server");
        }
    }
}

#[cfg(test)]
mod tests;
