//! The phone's side of joining a host: it hands the host a ticket from the
//! accounts server and waits while the person at the host decides.

use std::time::Duration;

use iroh::{Endpoint, EndpointAddr};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sikemux_wire::accounts::protocol::JoinTicket;
use sikemux_wire::protocol::{encode_control, read_frame_within, DeviceAccess, FrameKind};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

pub const JOIN_ALPN: &[u8] = b"sikemux/join/1";

/// How long the host waits for the person at it to answer.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const STEP_TIMEOUT: Duration = Duration::from_secs(15);
/// Every join message is a few hundred bytes. Either side reads them before
/// it knows who sent them, so nothing larger is accepted.
const MAX_MESSAGE_BYTES: usize = 4096;

/// What the phone sends: the ticket, with what the phone calls itself beside
/// it. The name and platform are not signed, so nothing vouches for them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinHello {
    #[serde(flatten)]
    pub ticket: JoinTicket,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub platform: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum JoinReply {
    /// Also the answer to a phone that is already paired, with the access it
    /// has.
    Allowed {
        access: DeviceAccess,
    },
    Denied,
    Refused {
        reason: String,
    },
}

pub async fn send(
    writer: &mut (impl AsyncWrite + Unpin),
    message: &impl Serialize,
) -> std::io::Result<()> {
    writer.write_all(&encode_control(message)?).await
}

pub async fn receive<T: DeserializeOwned>(
    reader: &mut (impl AsyncRead + Unpin),
    limit: Duration,
) -> std::io::Result<T> {
    let frame = tokio::time::timeout(limit, read_frame_within(reader, MAX_MESSAGE_BYTES))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "no answer in time"))??
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
    if frame.kind != FrameKind::Control {
        return Err(std::io::Error::other("unexpected frame"));
    }
    Ok(serde_json::from_slice(&frame.payload)?)
}

#[derive(Debug, thiserror::Error)]
pub enum JoinError {
    #[error("could not reach the host: {0}")]
    Connection(String),
}

fn connection_error(error: impl std::fmt::Display) -> JoinError {
    JoinError::Connection(error.to_string())
}

/// Hands `hello` to the host at `host` and waits while the person there
/// decides. Dropping the future withdraws the request.
pub async fn join(
    endpoint: &Endpoint,
    host: EndpointAddr,
    hello: &JoinHello,
) -> Result<JoinReply, JoinError> {
    let connection = endpoint
        .connect(host, JOIN_ALPN)
        .await
        .map_err(connection_error)?;
    let (mut writer, mut reader) = connection.open_bi().await.map_err(connection_error)?;
    send(&mut writer, hello).await.map_err(connection_error)?;
    let reply = receive(&mut reader, APPROVAL_TIMEOUT + STEP_TIMEOUT)
        .await
        .map_err(connection_error)?;
    connection.close(0u32.into(), b"answered");
    Ok(reply)
}
