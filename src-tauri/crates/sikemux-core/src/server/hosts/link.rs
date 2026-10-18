//! How two linked hosts ask each other things. Each ask is a stream of its
//! own on the connection between them: a control frame naming the method and
//! its params, then, for a git bundle, the bundle's bytes until the stream
//! ends. The answer comes back on the same stream as one control frame.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::{encode_control, read_frame_within, FrameKind};

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
}

#[derive(Serialize, Deserialize)]
struct Reply {
    #[serde(default)]
    ok: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

/// Why an ask came to nothing.
#[derive(Debug)]
pub(crate) enum Failed {
    /// The other host could not be reached, or the connection broke.
    Unreachable(String),
    /// The other host answered with this refusal.
    Refused(String),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Unreachable(why) | Failed::Refused(why) => f.write_str(why),
        }
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

async fn read_body(recv: &mut RecvStream) -> std::io::Result<Received> {
    let received = Received(
        std::env::temp_dir().join(format!("sikemux-link-{}", uuid::Uuid::new_v4().simple())),
    );
    let mut file = std::fs::File::create(received.path())?;
    let mut buffer = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let read = AsyncReadExt::read(recv, &mut buffer).await?;
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

/// Asks the host at the other end of `connection`, waiting at most `limit`.
pub(crate) async fn ask(
    connection: &Connection,
    method: &str,
    params: Value,
    body: Option<&Path>,
    limit: Duration,
) -> Result<Value, Failed> {
    let exchange = async {
        let (mut send, mut recv) = connection.open_bi().await.map_err(broken)?;
        let ask = Ask {
            method: method.to_owned(),
            params,
            body: body.is_some(),
        };
        write_message(&mut send, &ask).await.map_err(broken)?;
        if let Some(path) = body {
            write_body(&mut send, path).await.map_err(broken)?;
        }
        send.finish().map_err(broken)?;
        let reply: Reply = read_message(&mut recv).await.map_err(broken)?;
        match (reply.ok, reply.error) {
            (_, Some(error)) => Err(Failed::Refused(error)),
            (ok, None) => Ok(ok.unwrap_or(Value::Null)),
        }
    };
    tokio::time::timeout(limit, exchange)
        .await
        .unwrap_or_else(|_| {
            Err(Failed::Unreachable(
                "the other host did not answer in time".into(),
            ))
        })
}

/// What a host answers an ask with.
pub(crate) type Answer = Result<Value, String>;

/// One ask the other host made: its method, params and any bundle it sent.
pub(crate) struct Incoming {
    pub method: String,
    pub params: Value,
    pub body: Option<Received>,
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
                read_body(&mut recv)
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
            send,
        })
    };
    Some(read.await)
}

impl Incoming {
    pub(crate) async fn reply(mut self, answer: Answer) {
        let reply = match answer {
            Ok(ok) => Reply {
                ok: Some(ok),
                error: None,
            },
            Err(error) => Reply {
                ok: None,
                error: Some(error),
            },
        };
        if write_message(&mut self.send, &reply).await.is_ok() {
            let _ = self.send.finish();
        }
    }
}
