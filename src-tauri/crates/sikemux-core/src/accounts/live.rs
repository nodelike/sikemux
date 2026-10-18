//! A host's live connection to its account at `/v1/live`. The server pushes
//! what changed on the account; the host proves its key on every connection,
//! applies each event once, acknowledges it, and reconnects when the
//! connection drops.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::OffsetDateTime;
use tokio::net::TcpStream;
use tokio::sync::{watch, Notify};
use tokio_websockets::{ClientBuilder, Connector, MaybeTlsStream, Message, WebSocketStream};

use super::protocol::{
    AccountEvent, LiveAck, LiveApp, LiveCloseCode, LiveDeviceMessage, LiveHello, LiveLeave,
    LivePong, LivePush, LiveRole, LiveServerMessage, PushKind, PushResult, RevokeReason,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const CHALLENGE_WAIT: Duration = Duration::from_secs(10);
/// How often the server pings until its `ready` says. Two of these without
/// a frame means the connection is dead even if the network has not said so.
const HEARTBEAT: Duration = Duration::from_secs(25);
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(5 * 60);
/// Another connection with this key took over, which happens while a core
/// hands over to its replacement. Coming straight back would push it off.
const REPLACED_WAIT: Duration = Duration::from_secs(60);
/// The server's limits count over a minute, so coming back sooner only
/// prolongs the lockout.
const OVER_LIMIT_WAIT: Duration = Duration::from_secs(30);

const CLOSE_WAIT: Duration = Duration::from_secs(2);

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

/// What the live connection needs from the host it speaks for.
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
    /// What the server did with a push from the [`Outbox`].
    fn pushed(&self, push: Push, result: PushResult);
}

/// A notification for one of the account's phones, sealed by the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub to: String,
    pub kind: PushKind,
    pub collapse_id: String,
    pub blob: String,
    /// Milliseconds since the Unix epoch. Until then a push waits for the
    /// connection; after it, it is dropped.
    pub expires_at: u64,
}

/// Pushes waiting for the live connection, and those sent but not yet
/// answered. Each goes out with a `ref` one higher than the last, and its
/// answer carries it back.
#[derive(Default)]
pub struct Outbox {
    state: Mutex<OutboxState>,
    ready: Notify,
}

#[derive(Default)]
struct OutboxState {
    last_ref: i64,
    waiting: VecDeque<Push>,
    sent: BTreeMap<i64, Push>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

impl Outbox {
    fn lock(&self) -> MutexGuard<'_, OutboxState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Queues `push`, replacing an alert for the same card that has not gone
    /// out yet.
    pub fn post(&self, push: Push) {
        let now = now_ms();
        let mut state = self.lock();
        state.waiting.retain(|waiting| {
            waiting.expires_at > now
                && !(waiting.to == push.to && waiting.collapse_id == push.collapse_id)
        });
        state.waiting.push_back(push);
        drop(state);
        self.ready.notify_one();
    }

    /// Drops the push for this card that has not gone out yet. True when
    /// there was one.
    pub fn withdraw(&self, to: &str, collapse_id: &str) -> bool {
        let mut state = self.lock();
        let before = state.waiting.len();
        state
            .waiting
            .retain(|waiting| !(waiting.to == to && waiting.collapse_id == collapse_id));
        state.waiting.len() != before
    }

    pub fn waiting(&self) -> Vec<Push> {
        self.lock().waiting.iter().cloned().collect()
    }

    pub(crate) fn take(&self) -> Vec<LivePush> {
        let now = now_ms();
        let mut state = self.lock();
        let mut taken = Vec::new();
        while let Some(push) = state.waiting.pop_front() {
            if push.expires_at <= now {
                continue;
            }
            state.last_ref += 1;
            let r#ref = state.last_ref;
            taken.push(LivePush {
                r#ref,
                to: push.to.clone(),
                kind: push.kind,
                collapse_id: push.collapse_id.clone(),
                blob: push.blob.clone(),
                expires_at: rfc3339(push.expires_at),
            });
            state.sent.insert(r#ref, push);
        }
        taken
    }

    fn answered(&self, r#ref: i64) -> Option<Push> {
        self.lock().sent.remove(&r#ref)
    }

    /// The connection dropped before these were answered, so they go out
    /// again on the next one, ahead of anything newer.
    fn unanswered(&self) {
        let mut state = self.lock();
        let sent = std::mem::take(&mut state.sent);
        for push in sent.into_values().rev() {
            if !state
                .waiting
                .iter()
                .any(|waiting| waiting.to == push.to && waiting.collapse_id == push.collapse_id)
            {
                state.waiting.push_front(push);
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

/// The WebSocket at `path` on the accounts API at `base`.
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
    let mut failures: u32 = 0;
    account.link(Link::Connecting);
    loop {
        if changed.has_changed().is_err() || account.want() == Want::Stop {
            return;
        }
        let ended = session(&*account, &url, &app, &outbox, &mut changed).await;
        outbox.unanswered();
        let wait = match ended {
            Ended::Done => continue,
            Ended::Retry { hint, was_live } => {
                if was_live {
                    failures = 0;
                }
                failures = failures.saturating_add(1);
                backoff(failures).max(hint.unwrap_or_default())
            }
        };
        account.link(Link::Offline);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            gone = changed.changed() => {
                if gone.is_err() {
                    return;
                }
                // Something changed, such as the network coming back: start the backoff over.
                failures = 0;
            }
        }
    }
}

/// Full jitter: anywhere from nothing to the doubled delay, capped.
pub(crate) fn backoff(failures: u32) -> Duration {
    let ceiling = BACKOFF_BASE
        .saturating_mul(1u32 << failures.saturating_sub(1).min(16))
        .min(BACKOFF_CAP);
    let fraction = (uuid::Uuid::new_v4().as_u128() % 1000) as u32;
    ceiling.mul_f64(f64::from(fraction) / 1000.0)
}

enum Ended {
    Done,
    Retry {
        hint: Option<Duration>,
        was_live: bool,
    },
}

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn connector() -> &'static Connector {
    static CONNECTOR: OnceLock<Connector> = OnceLock::new();
    CONNECTOR.get_or_init(|| match super::tls_config() {
        Some(config) => Connector::Rustls(tokio_rustls::TlsConnector::from(config)),
        None => Connector::Plain,
    })
}

pub(crate) async fn connect(url: &str) -> Option<Socket> {
    let builder = ClientBuilder::new().uri(url).ok()?.connector(connector());
    match tokio::time::timeout(CONNECT_TIMEOUT, builder.connect()).await {
        Ok(Ok((socket, _))) => Some(socket),
        _ => None,
    }
}

async fn send(socket: &mut Socket, message: LiveDeviceMessage) -> bool {
    let Ok(text) = serde_json::to_string(&message) else {
        return false;
    };
    socket.send(Message::text(text)).await.is_ok()
}

enum Frame {
    Server(LiveServerMessage),
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

async fn flush(socket: &mut Socket, outbox: &Outbox) -> bool {
    for push in outbox.take() {
        if !send(socket, LiveDeviceMessage::Push(push)).await {
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
    let retry = |hint, was_live| Ended::Retry { hint, was_live };
    let Some(mut socket) = connect(url).await else {
        return retry(None, false);
    };
    let nonce = loop {
        match next_frame(&mut socket, CHALLENGE_WAIT).await {
            Frame::Server(LiveServerMessage::Challenge(challenge)) => break challenge.nonce,
            Frame::Server(_) | Frame::Other => continue,
            Frame::Closed(_) => return retry(None, false),
        }
    };
    let Some((key, signature)) = account.sign_live(&nonce) else {
        return retry(None, false);
    };
    let hello = LiveDeviceMessage::Hello(LiveHello {
        role: LiveRole::Host,
        key: Some(key),
        signature: Some(signature),
        token: None,
        app: Some(app.clone()),
    });
    if !send(&mut socket, hello).await {
        return retry(None, false);
    }

    let mut ready = false;
    let mut leaving = false;
    let mut hint = None;
    let mut silence = HEARTBEAT * 2;
    loop {
        let frame = tokio::select! {
            frame = next_frame(&mut socket, silence) => frame,
            _ = outbox.ready.notified(), if ready && !leaving => {
                if !flush(&mut socket, outbox).await {
                    return retry(hint, ready);
                }
                continue;
            }
            gone = changed.changed() => {
                match if gone.is_err() { Want::Stop } else { account.want() } {
                    Want::Stop => {
                        let _ = tokio::time::timeout(CLOSE_WAIT, socket.close()).await;
                        return Ended::Done;
                    }
                    Want::Leave if ready && !leaving => {
                        leaving = send(&mut socket, LiveDeviceMessage::Leave(LiveLeave {})).await;
                    }
                    _ => {}
                }
                continue;
            }
        };
        let message = match frame {
            Frame::Server(message) => message,
            Frame::Other => continue,
            Frame::Closed(code) => {
                return match code.and_then(|code| LiveCloseCode::from_code(code.into())) {
                    Some(LiveCloseCode::Unauthenticated) if !ready => {
                        account.removed(None);
                        Ended::Done
                    }
                    Some(LiveCloseCode::Revoked) => {
                        account.removed(None);
                        Ended::Done
                    }
                    Some(LiveCloseCode::Replaced) => retry(Some(REPLACED_WAIT), ready),
                    Some(LiveCloseCode::OverLimit) => {
                        retry(Some(hint.unwrap_or_default().max(OVER_LIMIT_WAIT)), ready)
                    }
                    _ => retry(hint, ready),
                };
            }
        };
        let answered = match message {
            LiveServerMessage::Ready(server) => {
                ready = true;
                if let Ok(ms) = u64::try_from(server.heartbeat_ms) {
                    silence = Duration::from_millis(ms) * 2;
                }
                account.rewind(server.latest);
                account.link(Link::Live);
                if account.want() == Want::Leave {
                    leaving = true;
                    send(&mut socket, LiveDeviceMessage::Leave(LiveLeave {})).await
                } else {
                    flush(&mut socket, outbox).await
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
                send(&mut socket, LiveDeviceMessage::Ack(LiveAck { id: last })).await
            }
            LiveServerMessage::Reset(reset) => {
                account.rewind(reset.latest);
                true
            }
            LiveServerMessage::Ping(_) => {
                send(&mut socket, LiveDeviceMessage::Pong(LivePong {})).await
            }
            LiveServerMessage::Bye(bye) => {
                hint = Some(Duration::from_millis(
                    u64::try_from(bye.reconnect_after_ms).unwrap_or_default(),
                ));
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
            return retry(hint, ready);
        }
    }
}

#[cfg(test)]
mod tests;
