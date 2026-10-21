//! A device on the network, known by its key: the phone, or the desktop app
//! reaching hosts other than its own. It joins hosts with tickets from the
//! account and opens sessions with the hosts that let it in.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{presets, Connection};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, RelayUrl, SecretKey};
use sikemux_wire::accounts::protocol::Relay;

use crate::accounts::network;
use crate::client::{ClientError, CoreClient, EventSink};
use crate::join::{JoinError, JoinHello, JoinReply};
use crate::remote;

/// Long enough to find a host through a relay on a slow network; past it the
/// host shows as unreachable and the caller tries again.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A host this endpoint reached before answers again within this, unless the
/// endpoint has gone stale; past it the device dials again from a fresh one.
const REDIAL_TIMEOUT: Duration = Duration::from_secs(6);

/// The endpoint and the hosts it has reached.
struct Online {
    endpoint: Endpoint,
    generation: u64,
    reached: HashSet<PublicKey>,
    /// Taken off the network; it comes back only as a new device.
    closed: bool,
}

pub struct Device {
    key: SecretKey,
    relays: Vec<Relay>,
    online: Mutex<Online>,
    renewing: tokio::sync::Mutex<()>,
}

/// The relays to use, or the built-in one when none of `relays` is usable.
/// Never iroh's public relays: no host listens there.
pub fn usable(relays: Vec<Relay>) -> Vec<Relay> {
    let relays: Vec<Relay> = relays
        .into_iter()
        .filter(|relay| relay.url.parse::<RelayUrl>().is_ok())
        .collect();
    if relays.is_empty() {
        network::default_relays()
    } else {
        relays
    }
}

/// A host listens on one of the relays, so the device offers iroh all of them.
pub fn host_addr(host: PublicKey, relays: &[Relay]) -> EndpointAddr {
    relays
        .iter()
        .filter_map(|relay| relay.url.parse::<RelayUrl>().ok())
        .fold(EndpointAddr::new(host), EndpointAddr::with_relay_url)
}

/// A device finds a host through the relay the host listens on and never
/// publishes its own addresses: no host dials a device.
async fn bind(key: SecretKey, relays: &[Relay]) -> Result<Endpoint, ClientError> {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(network::relay_map(relays)))
        .secret_key(key)
        .bind()
        .await
        .map_err(|error| ClientError::Handshake(error.to_string()))
}

/// This device's signature, in hex, over the text that registers it with the
/// account `user_id`, for the accounts server's challenge `nonce`.
pub fn sign_registration(
    key: &SecretKey,
    nonce: &str,
    user_id: &str,
) -> Result<String, &'static str> {
    crate::accounts::check_registration(nonce, user_id)?;
    let message = crate::accounts::registration_message(nonce, user_id, &key.public().to_string());
    Ok(hex::encode(key.sign(message.as_bytes()).to_bytes()))
}

/// Why a dial failed: `Stalled` means a fresh endpoint may reach the host.
enum Dial {
    Final(ClientError),
    Stalled(ClientError),
}

impl Dial {
    fn error(self) -> ClientError {
        match self {
            Dial::Final(error) | Dial::Stalled(error) => error,
        }
    }
}

impl Device {
    /// Comes online as `key`, reaching hosts through `relays`, best first.
    pub async fn bind(key: SecretKey, relays: Vec<Relay>) -> Result<Self, ClientError> {
        let relays = usable(relays);
        let endpoint = bind(key.clone(), &relays).await?;
        Ok(Self::on_endpoint(key, endpoint, relays))
    }

    /// A device on an endpoint already bound, such as one on loopback only.
    pub fn on_endpoint(key: SecretKey, endpoint: Endpoint, relays: Vec<Relay>) -> Self {
        Self {
            key,
            relays,
            online: Mutex::new(Online {
                endpoint,
                generation: 0,
                reached: HashSet::new(),
                closed: false,
            }),
            renewing: tokio::sync::Mutex::new(()),
        }
    }

    /// The key hosts and the accounts server know this device by.
    pub fn id(&self) -> String {
        self.key.public().to_string()
    }

    /// This device's signature over the text that registers it with the
    /// account `user_id`, for the accounts server's challenge `nonce`.
    pub fn sign_registration(&self, nonce: &str, user_id: &str) -> Result<String, &'static str> {
        sign_registration(&self.key, nonce, user_id)
    }

    /// Hands the host the ticket names the hello, and waits while the person
    /// there decides. An allowed device is paired with the host.
    pub async fn join(&self, host: PublicKey, hello: &JoinHello) -> Result<JoinReply, JoinError> {
        self.join_at(host_addr(host, &self.relays), hello).await
    }

    /// Like [`Self::join`], at an address known some other way.
    pub async fn join_at(
        &self,
        addr: EndpointAddr,
        hello: &JoinHello,
    ) -> Result<JoinReply, JoinError> {
        let (endpoint, _) = self
            .endpoint()
            .map_err(|error| JoinError::Connection(error.to_string()))?;
        crate::join::join(&endpoint, addr, hello).await
    }

    /// Opens a session with a host this device paired with. Closing the
    /// connection ends the session at once.
    ///
    /// A host this endpoint reached before is dialled with a short wait, and
    /// again from a fresh endpoint if that one has gone stale, so a dropped
    /// connection comes back in seconds rather than after a full timeout.
    pub async fn connect(
        &self,
        host: PublicKey,
        sink: Arc<dyn EventSink>,
    ) -> Result<(CoreClient, Connection), ClientError> {
        self.connect_to(host_addr(host, &self.relays), sink).await
    }

    /// Like [`Self::connect`], at an address known some other way.
    pub async fn connect_to(
        &self,
        addr: EndpointAddr,
        sink: Arc<dyn EventSink>,
    ) -> Result<(CoreClient, Connection), ClientError> {
        let host = addr.id;
        let (endpoint, generation) = self.endpoint()?;
        let redial = self.lock().reached.contains(&host);
        let wait = if redial {
            REDIAL_TIMEOUT
        } else {
            CONNECT_TIMEOUT
        };
        let stalled = match self.dial(addr.clone(), endpoint, sink.clone(), wait).await {
            Ok(opened) => return Ok(opened),
            Err(Dial::Final(error)) => return Err(error),
            Err(Dial::Stalled(error)) => error,
        };
        self.renew_after_failing(host, generation).await;
        let (endpoint, now) = self.endpoint()?;
        if !redial || now == generation {
            return Err(stalled);
        }
        self.dial(addr, endpoint, sink, CONNECT_TIMEOUT)
            .await
            .map_err(Dial::error)
    }

    /// Takes the device off the network. Open connections end with it.
    pub async fn close(&self) {
        let endpoint = {
            let mut online = self.lock();
            online.closed = true;
            online.endpoint.clone()
        };
        endpoint.close().await;
    }

    async fn dial(
        &self,
        addr: EndpointAddr,
        endpoint: Endpoint,
        sink: Arc<dyn EventSink>,
        wait: Duration,
    ) -> Result<(CoreClient, Connection), Dial> {
        let host = addr.id;
        let opened =
            match tokio::time::timeout(wait, remote::open_with(&endpoint, addr, sink)).await {
                Ok(Ok(opened)) => opened,
                Ok(Err(
                    error @ (ClientError::Core(_)
                    | ClientError::VersionMismatch { .. }
                    | ClientError::NotPaired),
                )) => return Err(Dial::Final(error)),
                Ok(Err(error)) => return Err(Dial::Stalled(error)),
                Err(_) => return Err(Dial::Stalled(ClientError::TimedOut)),
            };
        self.lock().reached.insert(host);
        Ok(opened)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Online> {
        self.online
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether the device was taken off the network.
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// How many times the endpoint was renewed.
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// The endpoint in use, and how many times it was renewed.
    pub fn endpoint(&self) -> Result<(Endpoint, u64), ClientError> {
        let online = self.lock();
        if online.closed {
            return Err(ClientError::Offline);
        }
        Ok((online.endpoint.clone(), online.generation))
    }

    /// Once a connection to a host closes, iroh 1.3 can leave the endpoint
    /// unable to reach that host again, while a new endpoint with the same key
    /// reaches it at once. A host this endpoint never reached is most likely
    /// just away, so it keeps the endpoint.
    ///
    /// The relay sends this key's traffic to its newest endpoint only, so the
    /// old one closes along with every connection on it, and the caller
    /// connects to those hosts again.
    pub async fn renew_after_failing(&self, host: PublicKey, generation: u64) {
        let _renewing = self.renewing.lock().await;
        {
            let online = self.lock();
            if online.closed || online.generation != generation || !online.reached.contains(&host) {
                return;
            }
        }
        let Ok(fresh) = bind(self.key.clone(), &self.relays).await else {
            return;
        };
        let retired = {
            let mut online = self.lock();
            if online.closed {
                fresh
            } else {
                online.generation += 1;
                online.reached.clear();
                std::mem::replace(&mut online.endpoint, fresh)
            }
        };
        tokio::spawn(async move { retired.close().await });
    }
}

#[cfg(test)]
mod tests {
    use super::{host_addr, usable};
    use sikemux_wire::accounts::protocol::Relay;

    fn relay(url: &str, quic_port: Option<i64>) -> Relay {
        Relay {
            url: url.into(),
            region: String::new(),
            quic_port,
        }
    }

    #[test]
    fn hosts_are_dialled_through_every_usable_relay_and_never_none() {
        let relays = usable(vec![
            relay("not a relay", None),
            relay("https://relay.example/", Some(7842)),
            relay("https://second.relay.example/", None),
        ]);
        assert_eq!(relays.len(), 2);
        assert_eq!(relays[0].quic_port, Some(7842));
        let host = iroh::SecretKey::generate().public();
        let mut dialled: Vec<String> = host_addr(host, &relays)
            .relay_urls()
            .map(ToString::to_string)
            .collect();
        dialled.sort();
        assert_eq!(
            dialled,
            ["https://relay.example/", "https://second.relay.example/"]
        );
        let fallback = usable(Vec::new());
        assert_eq!(
            fallback[0].url,
            crate::accounts::network::DEFAULT_RELAY.to_string()
        );
    }
}
