//! Reaching a core from another machine. Each side is known by an iroh key:
//! the core accepts only the keys it paired with, and a device dials the
//! core's key, so neither can be impersonated.

use std::sync::Arc;

use iroh::endpoint::{Connection, ConnectionError, VarInt};
use iroh::{Endpoint, EndpointAddr};
use tokio::sync::mpsc;

use crate::client::{ClientError, ClientEvent, CoreClient, EventSink};

pub use iroh::{PublicKey, SecretKey};

pub const CORE_ALPN: &[u8] = b"sikemux/core/1";
/// The code a core closes a connection with when it does not know the device.
pub const NOT_PAIRED: u32 = 1;

/// Opens a session with the core at `core` from a device it has paired with.
pub async fn connect_with(
    endpoint: &Endpoint,
    core: impl Into<EndpointAddr>,
    sink: Arc<dyn EventSink>,
) -> Result<CoreClient, ClientError> {
    open_with(endpoint, core, sink)
        .await
        .map(|(client, _)| client)
}

/// Like [`connect_with`], with the connection under the session, which ends
/// it at once when closed. The session otherwise lasts as long as a request
/// still waits on it.
pub async fn open_with(
    endpoint: &Endpoint,
    core: impl Into<EndpointAddr>,
    sink: Arc<dyn EventSink>,
) -> Result<(CoreClient, Connection), ClientError> {
    let connection = endpoint
        .connect(core, CORE_ALPN)
        .await
        .map_err(|error| ClientError::Handshake(error.to_string()))?;
    let opened = async {
        let (send, recv) = connection
            .open_bi()
            .await
            .map_err(|error| ClientError::Handshake(error.to_string()))?;
        CoreClient::connect_device_streams(recv, send, sink).await
    }
    .await;
    match opened {
        Ok(client) => Ok((client, connection)),
        Err(error) => Err(match connection.close_reason() {
            Some(ConnectionError::ApplicationClosed(close))
                if close.error_code == VarInt::from_u32(NOT_PAIRED) =>
            {
                ClientError::NotPaired
            }
            _ => error,
        }),
    }
}

pub async fn connect(
    endpoint: &Endpoint,
    core: impl Into<EndpointAddr>,
) -> Result<(CoreClient, mpsc::UnboundedReceiver<ClientEvent>), ClientError> {
    let (events, event_queue) = mpsc::unbounded_channel();
    let client = connect_with(endpoint, core, Arc::new(crate::client::ChannelSink(events))).await?;
    Ok((client, event_queue))
}
