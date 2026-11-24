//! A device on the network, known by its key: the phone, or the desktop app
//! reaching hosts other than its own. It joins hosts with tickets from the
//! account and opens sessions with the hosts that let it in.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iroh::endpoint::{presets, Connection};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, RelayUrl, SecretKey, TransportAddr};
use sikemux_wire::accounts::protocol::Relay;
use sikemux_wire::protocol::{CallId, Event, StreamId, WindowCall};

use crate::accounts::network;
use crate::client::{ClientError, CoreClient, EventSink};
use crate::join::{JoinError, JoinHello, JoinReply};
use crate::remote;

/// Long enough to find a host through a relay on a slow network; past it the
/// host shows as unreachable and the caller tries again.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// If a dial stalls past this with no live sessions or joins on the endpoint,
/// the device renews the endpoint and tries again from a fresh socket.
const REDIAL_TIMEOUT: Duration = Duration::from_secs(6);
/// Minimum interval between automatic endpoint renewals so multiple dead hosts
/// cannot thrash binds back-to-back.
const MIN_RENEW_GAP: Duration = Duration::from_secs(10);
/// Cap how long callers wait for an old endpoint to close; the close task itself
/// keeps running in the background so iroh still finishes tearing down its relay actor.
const CLOSE_TIMEOUT: Duration = Duration::from_millis(200);
/// Maximum direct addresses remembered per host across endpoint renewals and restarts.
const MAX_DIRECT_ADDRS: usize = 8;

/// Process-wide cache of last-seen direct IP addresses per host, so coming back
/// after `goOffline()` still races direct + relay on the very first dial.
fn direct_cache() -> &'static Mutex<HashMap<PublicKey, Vec<SocketAddr>>> {
    static CACHE: OnceLock<Mutex<HashMap<PublicKey, Vec<SocketAddr>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

struct Online {
    endpoint: Endpoint,
    relays: Vec<Relay>,
    generation: u64,
    reached: HashSet<PublicKey>,
    /// Live sessions currently open on `endpoint`, keyed by `Connection::stable_id` so
    /// closing can release synchronously without holding strong `Connection` handles.
    open_connections: HashSet<usize>,
    /// In-flight join handshakes waiting for host approval on `endpoint`.
    joins_in_flight: usize,
    /// Last time automatic renewal replaced the endpoint.
    last_renewed: Option<Instant>,
    /// Taken off the network; it comes back only as a new device.
    closed: bool,
}

impl Online {
    fn has_live_work(&self) -> bool {
        !self.open_connections.is_empty() || self.joins_in_flight > 0
    }
}

struct JoinGuard {
    online: Arc<Mutex<Online>>,
    generation: u64,
}

impl JoinGuard {
    fn enter(online: &Arc<Mutex<Online>>, generation: u64) -> Self {
        {
            let mut guard = lock_online(online);
            if guard.generation == generation {
                guard.joins_in_flight += 1;
            }
        }
        Self {
            online: online.clone(),
            generation,
        }
    }
}

impl Drop for JoinGuard {
    fn drop(&mut self) {
        let mut guard = lock_online(&self.online);
        if guard.generation == self.generation {
            guard.joins_in_flight = guard.joins_in_flight.saturating_sub(1);
        }
    }
}

struct TrackedSink {
    inner: Arc<dyn EventSink>,
    online: Arc<Mutex<Online>>,
    generation: u64,
    stable_id: Mutex<Option<usize>>,
    released: AtomicBool,
    closed_notify: Arc<tokio::sync::Notify>,
}

impl TrackedSink {
    fn track(&self, id: usize) {
        if let Ok(mut slot) = self.stable_id.lock() {
            *slot = Some(id);
        }
        self.released.store(false, Ordering::Release);
    }

    fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let id = self.stable_id.lock().ok().and_then(|mut slot| slot.take());
        if let Some(id) = id {
            let mut guard = lock_online(&self.online);
            if guard.generation == self.generation {
                guard.open_connections.remove(&id);
            }
        }
        self.closed_notify.notify_waiters();
    }
}

impl EventSink for TrackedSink {
    fn grid(&self, stream: StreamId, frame: &[u8]) {
        self.inner.grid(stream, frame);
    }

    fn event(&self, event: Event) {
        self.inner.event(event);
    }

    fn device_view(&self, view_json: &str) {
        self.inner.device_view(view_json);
    }

    fn window_call(&self, call_id: CallId, call: WindowCall) {
        self.inner.window_call(call_id, call);
    }

    fn closed(&self) {
        self.release();
        self.inner.closed();
    }
}

impl Drop for TrackedSink {
    fn drop(&mut self) {
        self.release();
    }
}

fn lock_online(online: &Mutex<Online>) -> std::sync::MutexGuard<'_, Online> {
    online
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Closes `endpoint` in a detached task that runs to completion, while only
/// waiting up to `CLOSE_TIMEOUT` before letting the caller move on.
async fn close_bounded(endpoint: Endpoint) {
    let task = tokio::spawn(async move {
        endpoint.close().await;
    });
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, task).await;
}

pub struct Device {
    key: SecretKey,
    online: Arc<Mutex<Online>>,
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
            online: Arc::new(Mutex::new(Online {
                endpoint,
                relays,
                generation: 0,
                reached: HashSet::new(),
                open_connections: HashSet::new(),
                joins_in_flight: 0,
                last_renewed: None,
                closed: false,
            })),
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

    /// Where `host` is found: every relay this device is on plus any direct IP
    /// addresses last seen for that host, so reconnects race direct and relay at once.
    pub fn addr(&self, host: PublicKey) -> EndpointAddr {
        let mut addr = {
            let online = self.lock();
            host_addr(host, &online.relays)
        };
        for ip in remembered_direct(host) {
            addr = addr.with_ip_addr(ip);
        }
        addr
    }

    /// Hands the host the ticket names the hello, and waits while the person
    /// there decides. An allowed device is paired with the host.
    pub async fn join(&self, host: PublicKey, hello: &JoinHello) -> Result<JoinReply, JoinError> {
        self.join_at(self.addr(host), hello).await
    }

    /// Like [`Self::join`], at an address known some other way.
    pub async fn join_at(
        &self,
        addr: EndpointAddr,
        hello: &JoinHello,
    ) -> Result<JoinReply, JoinError> {
        let (endpoint, generation) = self
            .endpoint()
            .map_err(|error| JoinError::Connection(error.to_string()))?;
        let _guard = JoinGuard::enter(&self.online, generation);
        crate::join::join(&endpoint, addr, hello).await
    }

    /// Opens a session with a host this device paired with. Closing the
    /// connection ends the session at once.
    ///
    /// When no other host has a live session or in-flight join on this endpoint,
    /// a stalled dial renews the endpoint and retries from a fresh socket so a
    /// stale or Doze-reclaimed bind recovers quickly without cutting live hosts.
    pub async fn connect(
        &self,
        host: PublicKey,
        sink: Arc<dyn EventSink>,
    ) -> Result<(CoreClient, Connection), ClientError> {
        self.connect_to(self.addr(host), sink).await
    }

    /// Like [`Self::connect`], at an address known some other way.
    pub async fn connect_to(
        &self,
        addr: EndpointAddr,
        sink: Arc<dyn EventSink>,
    ) -> Result<(CoreClient, Connection), ClientError> {
        let host = addr.id;
        let (endpoint, generation, has_live) = {
            let online = self.lock();
            if online.closed {
                return Err(ClientError::Offline);
            }
            (
                online.endpoint.clone(),
                online.generation,
                online.has_live_work(),
            )
        };
        let first_wait = if has_live {
            CONNECT_TIMEOUT
        } else {
            REDIAL_TIMEOUT
        };
        let stalled = match self
            .dial(addr.clone(), endpoint, generation, sink.clone(), first_wait)
            .await
        {
            Ok(opened) => return Ok(opened),
            Err(Dial::Final(error)) => return Err(error),
            Err(Dial::Stalled(error)) => error,
        };
        let (mut endpoint, mut now) = self.endpoint()?;
        if now == generation {
            if has_live {
                return Err(stalled);
            }
            let _ = self.replace_endpoint(Some(generation), false, None).await;
            (endpoint, now) = self.endpoint()?;
        }
        if now == generation {
            return Err(stalled);
        }
        let mut retry_addr = addr;
        for ip in remembered_direct(host) {
            retry_addr = retry_addr.with_ip_addr(ip);
        }
        self.dial(retry_addr, endpoint, now, sink, CONNECT_TIMEOUT)
            .await
            .map_err(Dial::error)
    }

    /// Notifies the underlying endpoint that the OS network changed (required on
    /// Android where native code cannot observe netlink/sysfs route changes).
    pub async fn network_changed(&self) {
        let Ok((endpoint, _)) = self.endpoint() else {
            return;
        };
        endpoint.network_change().await;
    }

    /// Closes the current endpoint in the background (waiting up to 200ms) and
    /// binds a fresh one on the same key using `relays` when provided, keeping
    /// remembered direct host addresses for the next dial. Refuses to close if a
    /// live session or join is active on the endpoint.
    pub async fn refresh(&self, relays: Option<Vec<Relay>>) {
        self.replace_endpoint(None, false, relays).await;
    }

    /// Marks `connection` closed synchronously so an immediate `refresh()` does
    /// not race the async reader task's `closed()` callback.
    pub fn note_closed(&self, connection: &Connection) {
        self.lock().open_connections.remove(&connection.stable_id());
    }

    /// Takes the device off the network. Open connections end with it.
    pub async fn close(&self) {
        let endpoint = {
            let mut online = self.lock();
            online.closed = true;
            online.endpoint.clone()
        };
        close_bounded(endpoint).await;
    }

    async fn dial(
        &self,
        addr: EndpointAddr,
        endpoint: Endpoint,
        generation: u64,
        sink: Arc<dyn EventSink>,
        wait: Duration,
    ) -> Result<(CoreClient, Connection), Dial> {
        let host = addr.id;
        let closed_notify = Arc::new(tokio::sync::Notify::new());
        let tracked = Arc::new(TrackedSink {
            inner: sink,
            online: self.online.clone(),
            generation,
            stable_id: Mutex::new(None),
            released: AtomicBool::new(true),
            closed_notify: closed_notify.clone(),
        });
        let opened =
            match tokio::time::timeout(wait, remote::open_with(&endpoint, addr, tracked.clone()))
                .await
            {
                Ok(Ok(opened)) => opened,
                Ok(Err(
                    error @ (ClientError::Core(_)
                    | ClientError::Refused { .. }
                    | ClientError::VersionMismatch { .. }
                    | ClientError::NotPaired),
                )) => return Err(Dial::Final(error)),
                Ok(Err(error)) => return Err(Dial::Stalled(error)),
                Err(_) => return Err(Dial::Stalled(ClientError::TimedOut)),
            };
        {
            let mut online = self.lock();
            if online.generation == generation {
                let id = opened.1.stable_id();
                online.reached.insert(host);
                online.open_connections.insert(id);
                tracked.track(id);
            }
        }
        record_connection_paths(host, &opened.1);
        watch_direct_paths(host, opened.1.clone(), closed_notify);
        Ok(opened)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Online> {
        lock_online(&self.online)
    }

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

    /// Renews the endpoint if `host` had been reached before and `generation` is still current.
    pub async fn renew_after_failing(&self, host: PublicKey, generation: u64) {
        let should_renew = {
            let online = self.lock();
            !online.closed && online.generation == generation && online.reached.contains(&host)
        };
        if should_renew {
            self.replace_endpoint(Some(generation), true, None).await;
        }
    }

    /// Closes the existing endpoint in a detached task (waiting up to 200ms)
    /// before binding a fresh one on the same key.
    async fn replace_endpoint(
        &self,
        expected_generation: Option<u64>,
        allow_with_live: bool,
        new_relays: Option<Vec<Relay>>,
    ) -> bool {
        let _renewing = self.renewing.lock().await;
        let (retired, relays) = {
            let online = self.lock();
            if online.closed
                || expected_generation.is_some_and(|g| online.generation != g)
                || (!allow_with_live && online.has_live_work())
                || (expected_generation.is_some()
                    && !allow_with_live
                    && online
                        .last_renewed
                        .is_some_and(|at| at.elapsed() < MIN_RENEW_GAP))
            {
                return false;
            }
            let relays = new_relays
                .filter(|list| !list.is_empty())
                .map(usable)
                .unwrap_or_else(|| online.relays.clone());
            (online.endpoint.clone(), relays)
        };
        close_bounded(retired).await;
        let Ok(fresh) = bind(self.key.clone(), &relays).await else {
            return false;
        };
        let mut online = self.lock();
        if online.closed {
            drop(online);
            tokio::spawn(async move {
                fresh.close().await;
            });
            return false;
        }
        online.generation += 1;
        online.reached.clear();
        online.open_connections.clear();
        online.joins_in_flight = 0;
        online.last_renewed = Some(Instant::now());
        online.relays = relays;
        online.endpoint = fresh;
        true
    }
}

fn remembered_direct(host: PublicKey) -> Vec<SocketAddr> {
    direct_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&host)
        .cloned()
        .unwrap_or_default()
}

fn record_direct(host: PublicKey, addrs: impl IntoIterator<Item = SocketAddr>) {
    let mut guard = direct_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let list = guard.entry(host).or_default();
    for addr in addrs {
        if let Some(pos) = list.iter().position(|existing| *existing == addr) {
            list.remove(pos);
        }
        list.insert(0, addr);
        if list.len() > MAX_DIRECT_ADDRS {
            list.truncate(MAX_DIRECT_ADDRS);
        }
    }
}

fn record_connection_paths(host: PublicKey, connection: &Connection) {
    let ips: Vec<SocketAddr> = connection
        .paths()
        .iter()
        .filter_map(|path| match path.remote_addr() {
            TransportAddr::Ip(ip) => Some(*ip),
            _ => None,
        })
        .collect();
    if !ips.is_empty() {
        record_direct(host, ips);
    }
}

fn watch_direct_paths(
    host: PublicKey,
    connection: Connection,
    closed_notify: Arc<tokio::sync::Notify>,
) {
    tokio::spawn(async move {
        for wait in [Duration::from_secs(1), Duration::from_secs(4)] {
            tokio::select! {
                _ = connection.closed() => return,
                _ = closed_notify.notified() => return,
                _ = tokio::time::sleep(wait) => record_connection_paths(host, &connection),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(url: &str) -> Relay {
        Relay {
            url: url.into(),
            region: String::new(),
            quic_port: None,
        }
    }

    #[test]
    fn hosts_are_dialled_through_every_usable_relay_and_never_none() {
        let host = SecretKey::generate().public();
        let addr = host_addr(
            host,
            &usable(vec![
                relay("https://sg.relay.example/"),
                relay("not a url"),
                relay("https://eu.relay.example/"),
            ]),
        );
        let mut urls: Vec<_> = addr.relay_urls().map(ToString::to_string).collect();
        urls.sort();
        assert_eq!(
            urls,
            vec!["https://eu.relay.example/", "https://sg.relay.example/"]
        );

        let fallback = host_addr(host, &usable(vec![relay("not a url")]));
        assert_eq!(fallback.relay_urls().count(), 1);
    }
}
