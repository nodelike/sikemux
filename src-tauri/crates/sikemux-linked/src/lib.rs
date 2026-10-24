//! The account's other hosts, reached directly over iroh as a device of the person's own
//! (ADR 0017), beside this host's own core. Each host this host is linked with is joined
//! once, with a ticket this host asks the account for, and kept connected: the connection
//! is taken up again whenever it drops, until the host is no longer linked.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iroh::EndpointAddr;
use serde_json::{json, Value};
use sikemux_client::client::{ClientError, ClientEvent, CoreClient, EventSink};
use sikemux_client::device::{host_addr, Device};
use sikemux_client::join::{JoinHello, JoinReply};
use sikemux_client::remote::{PublicKey, SecretKey};
use sikemux_wire::accounts::protocol::{JoinTicket, Relay};
use sikemux_wire::protocol::{CallId, Event, StreamId, WindowCall};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// How long a request waits for a host that is reconnecting.
const REACH: Duration = Duration::from_secs(20);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(30);

/// A device key as it is kept: its 32 bytes in hex.
pub fn key_text(key: &SecretKey) -> String {
    hex::encode(key.to_bytes())
}

/// A device key from how it is kept, when it reads as one.
pub fn key_from_text(text: &str) -> Option<SecretKey> {
    let bytes = hex::decode(text.trim()).ok()?;
    Some(SecretKey::from_bytes(&<[u8; 32]>::try_from(bytes).ok()?))
}

/// A new device key for an app that has none yet.
pub fn new_key() -> SecretKey {
    SecretKey::generate()
}

/// What happened on another host's connection, by the host's key.
pub enum Happened {
    Connected(String),
    /// The connection dropped; it is being taken up again.
    Lost(String),
    /// Something the host's core sent unasked.
    Event(String, Box<ClientEvent>),
}

/// The account's other hosts this app keeps connections to.
pub struct Linked {
    device: Device,
    relays: Vec<Relay>,
    /// What the account calls this app, before " on <host>".
    app: String,
    platform: String,
    own: Mutex<Option<Arc<CoreClient>>>,
    hosts: Mutex<HashMap<String, Host>>,
    happened: mpsc::UnboundedSender<Happened>,
}

struct Host {
    name: String,
    client: watch::Receiver<Option<Arc<CoreClient>>>,
    task: JoinHandle<()>,
}

impl Drop for Host {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Linked {
    /// Puts this app on the network under `key`, through `relays`.
    pub async fn bind(
        key: SecretKey,
        relays: Vec<Relay>,
        app: impl Into<String>,
        platform: impl Into<String>,
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<Happened>), ClientError> {
        let device = Device::bind(key, relays.clone()).await?;
        let (happened, heard) = mpsc::unbounded_channel();
        let linked = Arc::new(Self {
            device,
            relays,
            app: app.into(),
            platform: platform.into(),
            own: Mutex::new(None),
            hosts: Mutex::new(HashMap::new()),
            happened,
        });
        Ok((linked, heard))
    }

    /// This app's key on the network.
    pub fn id(&self) -> String {
        self.device.id()
    }

    /// Joins every host this host's core `own` is linked with, and lets go of those it no
    /// longer is. Runs again whenever the links change.
    pub async fn follow(self: &Arc<Self>, own: Arc<CoreClient>) -> Result<(), ClientError> {
        *lock(&self.own) = Some(own.clone());
        let pairing = own.call("pairing", json!({})).await?;
        let links: Vec<(String, String)> = pairing["links"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|link| {
                Some((
                    link["id"].as_str()?.to_owned(),
                    link["name"].as_str().unwrap_or_default().to_owned(),
                ))
            })
            .collect();
        let mut hosts = lock(&self.hosts);
        hosts.retain(|id, _| links.iter().any(|(linked, _)| linked == id));
        for (id, name) in links {
            if let Some(host) = hosts.get_mut(&id) {
                host.name = name;
                continue;
            }
            let Ok(key) = id.parse::<PublicKey>() else {
                continue;
            };
            let (send, client) = watch::channel(None);
            let task = tokio::spawn(stay_connected(self.clone(), id.clone(), key, send));
            hosts.insert(id, Host { name, client, task });
        }
        Ok(())
    }

    /// The hosts this app keeps connections to, by key, with what the account calls them.
    pub fn hosts(&self) -> Vec<(String, String)> {
        lock(&self.hosts)
            .iter()
            .map(|(id, host)| (id.clone(), host.name.clone()))
            .collect()
    }

    /// The connection to `host`, waiting a while for one that is reconnecting.
    pub async fn client(&self, host: &str) -> Result<Arc<CoreClient>, ClientError> {
        let mut client = lock(&self.hosts)
            .get(host)
            .map(|host| host.client.clone())
            .ok_or_else(|| ClientError::Core("that host is not linked with this one".into()))?;
        let reached = tokio::time::timeout(REACH, client.wait_for(Option::is_some)).await;
        match reached {
            Ok(Ok(client)) => client
                .clone()
                .ok_or_else(|| ClientError::Core("that host is not reachable now".into())),
            _ => Err(ClientError::Core("that host is not reachable now".into())),
        }
    }

    /// Asks the account, through this host's core, for the ticket that lets this app join
    /// `host`, adding the app to the account first as a device of the person's.
    async fn ticket(&self, host: &str) -> Result<JoinTicket, ClientError> {
        let own = lock(&self.own)
            .clone()
            .ok_or_else(|| ClientError::Core("this host's core is not connected".into()))?;
        let challenge = own.call("deviceChallenge", json!({})).await?;
        let text = |key: &str| challenge[key].as_str().unwrap_or_default().to_owned();
        let signature = self
            .device
            .sign_registration(&text("nonce"), &text("userId"))
            .map_err(|reason| ClientError::Core(reason.into()))?;
        let params = json!({
            "key": self.device.id(),
            "name": format!("{} on {}", self.app, text("hostName")),
            "platform": self.platform,
            "nonce": text("nonce"),
            "signature": signature,
            "host": host,
        });
        let ticket: Value = own.call("deviceJoinTicket", params).await?;
        Ok(serde_json::from_value(ticket)?)
    }

    /// Where `host` is dialled: through the relays, and in a test kit also at the addresses
    /// the kit names for it.
    fn addr(&self, host: PublicKey) -> EndpointAddr {
        kit_addresses(&host.to_string())
            .into_iter()
            .fold(host_addr(host, &self.relays), EndpointAddr::with_ip_addr)
    }

    async fn join(&self, host: &str, key: PublicKey) -> Result<(), ClientError> {
        let ticket = self.ticket(host).await?;
        let hello = JoinHello {
            ticket,
            name: self.app.clone(),
            platform: self.platform.clone(),
        };
        match self.device.join_at(self.addr(key), &hello).await {
            Ok(JoinReply::Allowed { .. }) => Ok(()),
            Ok(JoinReply::Denied) => Err(ClientError::Core("the host turned this app away".into())),
            Ok(JoinReply::Refused { reason }) => Err(ClientError::Core(reason)),
            Err(error) => Err(ClientError::Core(error.to_string())),
        }
    }
}

/// Keeps one connection to `host`, joining it first when it does not know this app, and
/// takes it up again whenever it drops, waiting longer each time it fails.
async fn stay_connected(
    linked: Arc<Linked>,
    host: String,
    key: PublicKey,
    client: watch::Sender<Option<Arc<CoreClient>>>,
) {
    let mut wait = FIRST_RETRY;
    loop {
        let (events, mut heard) = mpsc::unbounded_channel();
        match linked
            .device
            .connect_to(linked.addr(key), Arc::new(HostSink(events)))
            .await
        {
            Ok((connected, connection)) => {
                wait = FIRST_RETRY;
                client.send_replace(Some(Arc::new(connected)));
                let _ = linked.happened.send(Happened::Connected(host.clone()));
                while let Some(Some(event)) = heard.recv().await {
                    let _ = linked
                        .happened
                        .send(Happened::Event(host.clone(), Box::new(event)));
                }
                connection.close(0u32.into(), b"");
                client.send_replace(None);
                let _ = linked.happened.send(Happened::Lost(host.clone()));
            }
            Err(ClientError::NotPaired) => {
                if linked.join(&host, key).await.is_ok() {
                    continue;
                }
            }
            Err(_) => {}
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(LAST_RETRY);
    }
}

/// A host's events as they come, then nothing once its connection has gone.
struct HostSink(mpsc::UnboundedSender<Option<ClientEvent>>);

impl EventSink for HostSink {
    fn grid(&self, stream: StreamId, frame: &[u8]) {
        let _ = self.0.send(Some(ClientEvent::Grid {
            stream,
            frame: frame.to_vec(),
        }));
    }

    fn event(&self, event: Event) {
        let _ = self.0.send(Some(ClientEvent::Event(event)));
    }

    fn window_call(&self, call_id: CallId, call: WindowCall) {
        let _ = self.0.send(Some(ClientEvent::WindowCall { call_id, call }));
    }

    fn closed(&self) {
        let _ = self.0.send(None);
    }
}

/// The addresses a test kit names for `host` in `dev/host-addresses.json` under this app's
/// config folder, as test cores on one machine have no relay to meet through.
#[cfg(feature = "host-addresses-file")]
fn kit_addresses(host: &str) -> Vec<std::net::SocketAddr> {
    let folder = std::env::var_os("SIKEMUX_CONFIG_DIR").map(std::path::PathBuf::from);
    let Some(text) = folder
        .and_then(|folder| std::fs::read_to_string(folder.join("dev/host-addresses.json")).ok())
    else {
        return Vec::new();
    };
    let Ok(named) = serde_json::from_str::<HashMap<String, Vec<std::net::SocketAddr>>>(&text)
    else {
        return Vec::new();
    };
    let on_this_network = |ip: std::net::IpAddr| match ip {
        std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        std::net::IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
    };
    if !named
        .values()
        .flatten()
        .all(|address| on_this_network(address.ip()))
    {
        return Vec::new();
    }
    named.get(host).cloned().unwrap_or_default()
}

#[cfg(not(feature = "host-addresses-file"))]
fn kit_addresses(_: &str) -> Vec<std::net::SocketAddr> {
    Vec::new()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
