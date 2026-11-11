//! How two linked hosts ask each other things. Each ask is a stream of its
//! own on the connection between them: a control frame naming the method and
//! its params, then, for a git bundle, the bundle's bytes until the stream
//! ends. The answer comes back on the same stream as one control frame.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use sikemux_wire::protocol::{encode_control, read_frame_within, FrameKind};

pub(crate) const LINK_ALPN: &[u8] = b"sikemux/link/1";
/// The code a host closes a connection with when it is not linked with the
/// host on the other end.
pub(super) const NOT_LINKED: u32 = 2;
const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const CHUNK: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
struct Ask {
    method: String,
    params: Value,
    #[serde(default)]
    body: bool,
    /// Names a call that does work, the same on every retry, so the other host does it once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    call: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    ok: Option<Value>,
    error: Option<String>,
    /// What kind of refusal `error` is; a host that does not say means it is final.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refusal: Option<String>,
    /// A file's bytes follow the answer, until the stream ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    body: bool,
}

/// Why a host turned an ask away, which tells the asker whether to ask again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Asking again changes nothing.
    Final,
    /// It may go through later, as when a checkout is busy.
    Retry,
    /// The host no longer has a link with the asker.
    NotLinked,
}

impl Kind {
    fn word(self) -> Option<&'static str> {
        match self {
            Kind::Final => None,
            Kind::Retry => Some("retry"),
            Kind::NotLinked => Some("notLinked"),
        }
    }

    fn of(word: Option<&str>) -> Self {
        match word {
            Some("retry") => Kind::Retry,
            Some("notLinked") => Kind::NotLinked,
            _ => Kind::Final,
        }
    }
}

/// A host's refusal of an ask, and why.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub why: String,
    pub kind: Kind,
}

impl Refusal {
    pub(crate) fn retry(why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            kind: Kind::Retry,
        }
    }

    pub(crate) fn not_linked(why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            kind: Kind::NotLinked,
        }
    }
}

impl From<String> for Refusal {
    fn from(why: String) -> Self {
        Self {
            why,
            kind: Kind::Final,
        }
    }
}

impl From<&str> for Refusal {
    fn from(why: &str) -> Self {
        why.to_owned().into()
    }
}

/// Why an ask came to nothing.
#[derive(Debug)]
pub(crate) enum Failed {
    /// The ask never reached the other host.
    Unreachable(String),
    /// The ask went out but no answer came back, so whether it was done is not known yet.
    Unknown(String),
    /// The other host answered with a refusal that asking again does not change.
    Refused(String),
    /// The other host answered with a refusal that may not hold later.
    Retry(String),
    /// The other host no longer has a link with this one.
    NotLinked(String),
}

impl Failed {
    /// Whether the ask is worth sending again later.
    pub(crate) fn passing(&self) -> bool {
        matches!(
            self,
            Failed::Unreachable(_) | Failed::Unknown(_) | Failed::Retry(_)
        )
    }
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Unreachable(why)
            | Failed::Unknown(why)
            | Failed::Refused(why)
            | Failed::Retry(why)
            | Failed::NotLinked(why) => f.write_str(why),
        }
    }
}

fn refused(why: String, kind: Option<&str>) -> Failed {
    match Kind::of(kind) {
        Kind::Final => Failed::Refused(why),
        Kind::Retry => Failed::Retry(why),
        Kind::NotLinked => Failed::NotLinked(why),
    }
}

fn broken(error: impl std::fmt::Display) -> Failed {
    Failed::Unreachable(error.to_string())
}

/// A file the link received, removed once it is dropped.
pub(crate) struct Received(PathBuf);

impl Received {
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Received {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn write_message(send: &mut SendStream, message: &impl Serialize) -> std::io::Result<()> {
    AsyncWriteExt::write_all(send, &encode_control(message)?).await
}

async fn read_message<T: for<'de> Deserialize<'de>>(recv: &mut RecvStream) -> std::io::Result<T> {
    let frame = read_frame_within(recv, MAX_MESSAGE_BYTES)
        .await?
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
    if frame.kind != FrameKind::Control {
        return Err(std::io::Error::other("unexpected frame"));
    }
    Ok(serde_json::from_slice(&frame.payload)?)
}

async fn write_body(send: &mut SendStream, path: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        AsyncWriteExt::write_all(send, &buffer[..read]).await?;
    }
}

/// Reads a file's bytes until the stream ends, giving up once none came for `stall`.
async fn read_body(recv: &mut RecvStream, stall: Option<Duration>) -> std::io::Result<Received> {
    let received = Received(
        std::env::temp_dir().join(format!("sikemux-link-{}", uuid::Uuid::new_v4().simple())),
    );
    let mut file = std::fs::File::create(received.path())?;
    let mut buffer = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let reading = AsyncReadExt::read(recv, &mut buffer);
        let read = match stall {
            Some(stall) => tokio::time::timeout(stall, reading)
                .await
                .map_err(|_| std::io::Error::other("the file stopped coming"))??,
            None => reading.await?,
        };
        if read == 0 {
            return Ok(received);
        }
        total += read as u64;
        if total > MAX_BODY_BYTES {
            return Err(std::io::Error::other(
                "the bundle is larger than a link carries",
            ));
        }
        file.write_all(&buffer[..read])?;
    }
}

pub(crate) async fn ask(
    connection: &Connection,
    method: &str,
    params: Value,
    body: Option<&Path>,
    limit: Duration,
    call: Option<&str>,
) -> Result<Value, Failed> {
    let sent = AtomicBool::new(false);
    let exchange = async {
        let (mut send, mut recv) = connection.open_bi().await.map_err(broken)?;
        let ask = Ask {
            method: method.to_owned(),
            params,
            body: body.is_some(),
            call: call.map(str::to_owned),
        };
        write_message(&mut send, &ask).await.map_err(broken)?;
        if let Some(path) = body {
            write_body(&mut send, path).await.map_err(broken)?;
        }
        send.finish().map_err(broken)?;
        sent.store(true, Ordering::Relaxed);
        let reply: Reply = read_message(&mut recv)
            .await
            .map_err(|error| Failed::Unknown(error.to_string()))?;
        match (reply.ok, reply.error) {
            (_, Some(error)) => Err(refused(error, reply.refusal.as_deref())),
            (ok, None) => Ok(ok.unwrap_or(Value::Null)),
        }
    };
    tokio::time::timeout(limit, exchange)
        .await
        .unwrap_or_else(|_| {
            let why = "the other host did not answer in time".to_owned();
            Err(match sent.load(Ordering::Relaxed) {
                true => Failed::Unknown(why),
                false => Failed::Unreachable(why),
            })
        })
}

/// Asks for a file, which comes back on the ask's own stream for as long as its bytes keep
/// moving: it is given up only once none came for `stall`.
pub(crate) async fn pull(
    connection: &Connection,
    method: &str,
    params: Value,
    stall: Duration,
) -> Result<Received, Failed> {
    let asked = async {
        let (mut send, recv) = connection.open_bi().await.map_err(broken)?;
        let ask = Ask {
            method: method.to_owned(),
            params,
            body: false,
            call: None,
        };
        write_message(&mut send, &ask).await.map_err(broken)?;
        send.finish().map_err(broken)?;
        Ok(recv)
    };
    let mut recv = tokio::time::timeout(stall, asked)
        .await
        .unwrap_or_else(|_| Err(broken("the other host did not answer in time")))?;
    let unknown = |error: std::io::Error| Failed::Unknown(error.to_string());
    let reply: Reply = tokio::time::timeout(stall, read_message(&mut recv))
        .await
        .map_err(|_| Failed::Unknown("the other host did not answer in time".into()))?
        .map_err(unknown)?;
    if let Some(error) = reply.error {
        return Err(refused(error, reply.refusal.as_deref()));
    }
    if !reply.body {
        return Err(Failed::Refused("the other host sent no file".into()));
    }
    read_body(&mut recv, Some(stall)).await.map_err(unknown)
}

pub(crate) type Answer = Result<Value, String>;

/// One ask the other host made: its method, params and any bundle it sent.
pub(crate) struct Incoming {
    pub method: String,
    pub params: Value,
    pub body: Option<Received>,
    pub call: Option<String>,
    send: SendStream,
}

/// Reads the next ask on `connection`, or none once it closes.
pub(crate) async fn next(connection: &Connection) -> Option<Result<Incoming, String>> {
    let (send, mut recv) = connection.accept_bi().await.ok()?;
    let read = async {
        let ask: Ask = read_message(&mut recv)
            .await
            .map_err(|error| error.to_string())?;
        let body = if ask.body {
            Some(
                read_body(&mut recv, None)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        Ok(Incoming {
            method: ask.method,
            params: ask.params,
            body,
            call: ask.call,
            send,
        })
    };
    Some(read.await)
}

impl Incoming {
    pub(crate) async fn reply(mut self, answer: Result<Value, Refusal>) {
        let reply = match answer {
            Ok(ok) => Reply {
                ok: Some(ok),
                error: None,
                refusal: None,
                body: false,
            },
            Err(refusal) => Reply {
                ok: None,
                error: Some(refusal.why),
                refusal: refusal.kind.word().map(str::to_owned),
                body: false,
            },
        };
        if write_message(&mut self.send, &reply).await.is_ok() {
            let _ = self.send.finish();
        }
    }

    /// Answers with the file at `path`, sent for as long as the asker reads it.
    pub(crate) async fn reply_body(mut self, path: &Path) {
        let reply = Reply {
            ok: Some(Value::Null),
            error: None,
            refusal: None,
            body: true,
        };
        if write_message(&mut self.send, &reply).await.is_ok()
            && write_body(&mut self.send, path).await.is_ok()
        {
            let _ = self.send.finish();
        }
    }
}
