//! What the host's WebSocket connections to the accounts server share: the
//! connection, the challenge its hello answers, frames each way, and how long
//! to wait before connecting again.

use std::sync::OnceLock;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_websockets::{ClientBuilder, Connector, MaybeTlsStream, Message, WebSocketStream};

use crate::backoff::Backoff;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const CHALLENGE_WAIT: Duration = Duration::from_secs(10);
/// How often the server pings until its `ready` says. Two of these without
/// a frame means the connection is dead even if the network has not said so.
pub const HEARTBEAT: Duration = Duration::from_secs(25);
/// Another connection with this key took over, which happens while a core
/// hands over to its replacement. Coming straight back would push it off.
const REPLACED_WAIT: Duration = Duration::from_secs(60);
/// The server's limits count over a minute, so coming back sooner only
/// prolongs the lockout.
pub(crate) const OVER_LIMIT_WAIT: Duration = Duration::from_secs(30);
const RECONNECT: Backoff = Backoff {
    first: Duration::from_secs(1),
    cap: Duration::from_secs(5 * 60),
};

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn connector() -> &'static Connector {
    static CONNECTOR: OnceLock<Connector> = OnceLock::new();
    CONNECTOR.get_or_init(|| match sikemux_client::accounts::tls_config() {
        Some(config) => Connector::Rustls(tokio_rustls::TlsConnector::from(config)),
        None => Connector::Plain,
    })
}

pub async fn connect(url: &str) -> Option<Socket> {
    let builder = ClientBuilder::new().uri(url).ok()?.connector(connector());
    match tokio::time::timeout(CONNECT_TIMEOUT, builder.connect()).await {
        Ok(Ok((socket, _))) => Some(socket),
        _ => None,
    }
}

/// Whether `message` went out.
pub async fn send(socket: &mut Socket, message: &impl Serialize) -> bool {
    let Ok(text) = serde_json::to_string(message) else {
        return false;
    };
    socket.send(Message::text(text)).await.is_ok()
}

pub enum Frame<T> {
    Server(T),
    Closed(Close),
    Other,
}

#[derive(Debug)]
pub enum Close {
    /// The server closed with this code and reason.
    Server(u16, String),
    /// Nothing came within the wait.
    Silent,
    /// The connection ended without a close from the server.
    Broken(String),
}

impl Close {
    pub fn code(&self) -> Option<u16> {
        match self {
            Close::Server(code, _) => Some(*code),
            _ => None,
        }
    }
}

impl std::fmt::Display for Close {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Close::Server(code, reason) => write!(f, "the server closed it ({code} {reason})"),
            Close::Silent => f.write_str("the server went silent"),
            Close::Broken(why) => write!(f, "the connection broke ({why})"),
        }
    }
}

/// The next frame, or closed when none came within `wait`.
pub async fn next_frame<T: DeserializeOwned>(socket: &mut Socket, wait: Duration) -> Frame<T> {
    let message = match tokio::time::timeout(wait, socket.next()).await {
        Ok(Some(Ok(message))) => message,
        Ok(Some(Err(error))) => return Frame::Closed(Close::Broken(error.to_string())),
        Ok(None) => return Frame::Closed(Close::Broken("it ended".into())),
        Err(_) => return Frame::Closed(Close::Silent),
    };
    if let Some((code, reason)) = message.as_close() {
        return Frame::Closed(Close::Server(code.into(), reason.to_owned()));
    }
    match message.as_text().map(serde_json::from_str) {
        Some(Ok(message)) => Frame::Server(message),
        _ => Frame::Other,
    }
}

/// The nonce of the challenge the server opens with, which `nonce` picks out
/// of its first message, or none when the connection closed first.
pub async fn challenge<T: DeserializeOwned>(
    socket: &mut Socket,
    nonce: impl Fn(T) -> Option<String>,
) -> Option<String> {
    loop {
        match next_frame(socket, CHALLENGE_WAIT).await {
            Frame::Server(message) => {
                if let Some(nonce) = nonce(message) {
                    return Some(nonce);
                }
            }
            Frame::Other => {}
            Frame::Closed(_) => return None,
        }
    }
}

/// How long the server may go silent, from the heartbeat its `ready` named.
pub fn silence(heartbeat_ms: i64) -> Duration {
    u64::try_from(heartbeat_ms).map_or(HEARTBEAT, Duration::from_millis) * 2
}

pub fn asked_wait(reconnect_after_ms: i64) -> Duration {
    Duration::from_millis(u64::try_from(reconnect_after_ms).unwrap_or_default())
}

/// A close the server chose that asks the host to wait before coming back.
pub enum Closed {
    /// A newer connection with the same key took over.
    Replaced,
    OverLimit,
}

/// How long to wait after the server closed as `closed`, beyond what it asked.
pub fn wait_after(closed: Closed, asked: Option<Duration>) -> Option<Duration> {
    match closed {
        Closed::Replaced => Some(REPLACED_WAIT),
        Closed::OverLimit => Some(asked.unwrap_or_default().max(OVER_LIMIT_WAIT)),
    }
}

/// Failed connections in a row, for how long to wait before the next.
#[derive(Default)]
pub struct Failures(u32);

impl Failures {
    /// The wait before connecting again after a connection ended: at least
    /// `asked`, and counted afresh when the connection had come up.
    pub fn wait(&mut self, asked: Option<Duration>, came_up: bool) -> Duration {
        if came_up {
            self.0 = 0;
        }
        self.0 = self.0.saturating_add(1);
        RECONNECT.after(self.0).max(asked.unwrap_or_default())
    }

    /// Something changed, such as the network coming back: start over.
    pub fn forget(&mut self) {
        self.0 = 0;
    }
}
