//! What the phone app calls to join a host and talk to its core. The
//! connection, joining and wire format are the core's own (`sikemux-core`),
//! so the phone and the host cannot drift apart; this crate only exposes them
//! through UniFFI, as typed calls and records.
//!
//! A chat's events cross as JSON, since the phone reads them with the host
//! app's chat code. Terminal bytes cross as bytes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use base64::Engine;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl, SecretKey};
use sikemux_client::accounts::network;
use sikemux_client::client::{ClientError, CoreClient, EventSink, Reply};
use sikemux_client::join::{JoinHello, JoinReply};
use sikemux_client::remote;
use sikemux_wire::accounts::protocol::{JoinTicket, Relay};
use sikemux_wire::protocol::{
    CallId, Event, NotifyPrefs, Request, Response, SessionId, WindowCall, MAX_ATTACHMENT_BYTES,
    OLDEST_PROTOCOL_VERSION, WAKE_WAIT,
};
use tokio::sync::mpsc;

mod records;

pub use records::*;

#[cfg(target_os = "android")]
mod android;

uniffi::setup_scaffolding!();

/// Long enough to find a host through a relay on a slow network; past it the
/// app shows the host as unreachable and tries again.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A host this endpoint reached before answers again within this, unless the
/// endpoint has gone stale; past it the phone dials again from a fresh one.
const REDIAL_TIMEOUT: Duration = Duration::from_secs(6);
/// Long enough for a long chat's replay over a relay. A host that has not
/// answered by then has most likely gone, though the connection has not
/// noticed yet.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Room for the largest file over a slow relay.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Past the host's own wait, so its reason for giving up reaches the app.
const WAKE_TIMEOUT: Duration = Duration::from_secs(WAKE_WAIT.as_secs() + 15);

/// iroh and the core's client both need a Tokio runtime, which the phone's
/// JavaScript thread does not have. The phone talks to a few hosts at most, so
/// two threads are plenty.
static RUNTIME: LazyLock<Result<tokio::runtime::Runtime, String>> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .thread_name("sikemux-mobile")
        .build()
        .map_err(|error| error.to_string())
});

/// Stops the work when the app stops waiting for it, as when a person
/// gives up joining a host.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn on_runtime<T: Send + 'static>(
    work: impl std::future::Future<Output = T> + Send + 'static,
) -> Result<T, MobileError> {
    let runtime = RUNTIME
        .as_ref()
        .map_err(|message| MobileError::Connection {
            message: format!("the phone could not start its network runtime: {message}"),
        })?;
    let mut task = AbortOnDrop(runtime.spawn(work));
    (&mut task.0).await.map_err(|_| MobileError::Connection {
        message: "network work on the phone stopped unexpectedly".into(),
    })
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    #[error("{message}")]
    Refused { message: String },
    #[error("{message}")]
    Connection { message: String },
    #[error("{message}")]
    Invalid { message: String },
    /// The host and this app have no version of the core's protocol in common.
    #[error("{}", if *mac_is_older { "update Sikemux on this host to reach it from this app" } else { "update this app to reach this host" })]
    Outdated { mac_is_older: bool },
    /// The host forgot this phone, so it has to join again.
    #[error("this host no longer knows this phone; connect to it again")]
    Unpaired,
}

fn invalid(message: impl ToString) -> MobileError {
    MobileError::Invalid {
        message: message.to_string(),
    }
}

impl From<ClientError> for MobileError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Core(message) => MobileError::Refused { message },
            ClientError::VersionMismatch { version, .. } => MobileError::Outdated {
                mac_is_older: version < OLDEST_PROTOCOL_VERSION,
            },
            ClientError::NotPaired => MobileError::Unpaired,
            other => MobileError::Connection {
                message: other.to_string(),
            },
        }
    }
}

/// A new device key. The app keeps it in the Keychain or Keystore; it is the
/// device's identity to every host it pairs with.
#[uniffi::export]
pub fn new_device_key() -> Vec<u8> {
    SecretKey::generate().to_bytes().to_vec()
}

/// A relay from the accounts server's `GET /v1/network`, which hosts listen on
/// and this phone dials them through.
#[derive(Clone, uniffi::Record)]
pub struct RelaySetting {
    pub url: String,
    /// The relay's QUIC address discovery port, if it runs one.
    pub quic_port: Option<u16>,
}

/// The relays to use, or the built-in one when none of `settings` is usable.
/// Never iroh's public relays: no host listens there.
fn relays_from(settings: Vec<RelaySetting>) -> Vec<Relay> {
    let relays: Vec<Relay> = settings
        .into_iter()
        .filter(|setting| setting.url.parse::<RelayUrl>().is_ok())
        .map(|setting| Relay {
            url: setting.url,
            region: String::new(),
            quic_port: setting.quic_port.map(i64::from),
        })
        .collect();
    if relays.is_empty() {
        network::default_relays()
    } else {
        relays
    }
}

/// The endpoint and the hosts it has reached.
struct Online {
    endpoint: Endpoint,
    generation: u64,
    reached: HashSet<String>,
    /// The app took the phone off the network; it comes back as a new device.
    closed: bool,
}

/// This phone on the network, known by its key.
#[derive(uniffi::Object)]
pub struct Device {
    key: SecretKey,
    relays: Vec<Relay>,
    online: Mutex<Online>,
    renewing: tokio::sync::Mutex<()>,
}

/// The phone finds a host through the relay the host listens on and never
/// publishes its own addresses: no host dials a phone.
async fn bind(key: SecretKey, relays: &[Relay]) -> Result<Endpoint, MobileError> {
    let relays = network::relay_map(relays);
    on_runtime(async move {
        Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Custom(relays))
            .secret_key(key)
            .bind()
            .await
    })
    .await?
    .map_err(|error| MobileError::Connection {
        message: error.to_string(),
    })
}

fn sign_registration(key: &SecretKey, nonce: &str, user_id: &str) -> Result<String, MobileError> {
    sikemux_client::accounts::check_registration(nonce, user_id).map_err(invalid)?;
    let message =
        sikemux_client::accounts::registration_message(nonce, user_id, &key.public().to_string());
    Ok(hex::encode(key.sign(message.as_bytes()).to_bytes()))
}

fn sign_live(key: &SecretKey, nonce: &str) -> Result<String, MobileError> {
    sikemux_client::accounts::check_live(nonce).map_err(invalid)?;
    let message = sikemux_client::accounts::live_message(nonce, &key.public().to_string());
    Ok(hex::encode(key.sign(message.as_bytes()).to_bytes()))
}

fn sign_push(key: &SecretKey, nonce: &str, token_sha256: &str) -> Result<String, MobileError> {
    sikemux_client::accounts::check_live(nonce).map_err(invalid)?;
    sikemux_client::accounts::check_live(token_sha256)
        .map_err(|_| invalid("the token's hash is 64 lowercase hex characters"))?;
    let message = format!("sikemux-push|{nonce}|{}|{token_sha256}", key.public());
    Ok(hex::encode(key.sign(message.as_bytes()).to_bytes()))
}

fn notify_prefs(json: &str) -> Result<NotifyPrefs, MobileError> {
    serde_json::from_str(json).map_err(|error| {
        invalid(format!(
            "the notification settings are not readable: {error}"
        ))
    })
}

/// A host listens on one of the relays, so the phone offers iroh all of them.
fn core_addr(core: &str, relays: &[Relay]) -> Result<EndpointAddr, MobileError> {
    let addr = EndpointAddr::new(core.parse().map_err(invalid)?);
    Ok(relays
        .iter()
        .filter_map(|relay| relay.url.parse::<RelayUrl>().ok())
        .fold(addr, EndpointAddr::with_relay_url))
}

/// This phone's key, which proves who it is to the accounts server without
/// the phone going on the network.
#[derive(uniffi::Object)]
pub struct DeviceIdentity {
    key: SecretKey,
}

#[uniffi::export]
impl DeviceIdentity {
    /// The key from [`new_device_key`].
    #[uniffi::constructor]
    pub fn new(key: Vec<u8>) -> Result<Arc<Self>, MobileError> {
        let bytes: [u8; 32] = key
            .try_into()
            .map_err(|_| invalid("a device key is 32 bytes"))?;
        Ok(Arc::new(Self {
            key: SecretKey::from_bytes(&bytes),
        }))
    }

    /// The key hosts and the accounts server know this phone by.
    pub fn id(&self) -> String {
        self.key.public().to_string()
    }

    /// This phone's signature, in hex, over the text that registers it with
    /// the account `user_id`, for the accounts server's challenge `nonce`.
    pub fn sign_registration(&self, nonce: String, user_id: String) -> Result<String, MobileError> {
        sign_registration(&self.key, &nonce, &user_id)
    }

    /// This phone's signature, in hex, that proves its key on the accounts
    /// server's live connection, for that connection's challenge `nonce`.
    pub fn sign_live(&self, nonce: String) -> Result<String, MobileError> {
        sign_live(&self.key, &nonce)
    }

    /// This phone's signature, in hex, that sends its notifications to the push
    /// token whose SHA-256 is `token_sha256`, for the accounts server's
    /// challenge `nonce`.
    pub fn sign_push(&self, nonce: String, token_sha256: String) -> Result<String, MobileError> {
        sign_push(&self.key, &nonce, &token_sha256)
    }
}

#[uniffi::export]
impl Device {
    /// Comes online as `identity`, reaching hosts through `relays`, best
    /// first.
    #[uniffi::constructor]
    pub async fn create(
        identity: Arc<DeviceIdentity>,
        relays: Vec<RelaySetting>,
    ) -> Result<Arc<Self>, MobileError> {
        #[cfg(target_os = "android")]
        android::ensure_context().map_err(|message| MobileError::Connection { message })?;
        let key = identity.key.clone();
        let relays = relays_from(relays);
        let endpoint = bind(key.clone(), &relays).await?;
        Ok(Arc::new(Self {
            key,
            relays,
            online: Mutex::new(Online {
                endpoint,
                generation: 0,
                reached: HashSet::new(),
                closed: false,
            }),
            renewing: tokio::sync::Mutex::new(()),
        }))
    }

    /// The key hosts know this phone by.
    pub fn id(&self) -> String {
        self.key.public().to_string()
    }

    /// Hands the host whose key is `core` the `ticket` the accounts server
    /// signed for it and this phone, as its JSON, and waits while the person
    /// there decides. An allowed phone is paired with the host.
    pub async fn join(
        &self,
        core: String,
        ticket: String,
        name: String,
        platform: String,
    ) -> Result<JoinAnswer, MobileError> {
        let ticket = read_ticket(&ticket, &self.id(), &core)?;
        let addr = core_addr(&core, &self.relays)?;
        let (endpoint, _) = self.endpoint()?;
        let hello = JoinHello {
            ticket,
            name,
            platform,
        };
        join_with(endpoint, addr, hello).await
    }

    /// Opens a session with a host this phone paired with. Everything the core
    /// sends unasked arrives on `listener`, in order, off the network's threads.
    pub async fn connect(
        &self,
        core: String,
        listener: Arc<dyn CoreListener>,
    ) -> Result<Arc<Connection>, MobileError> {
        let addr = core_addr(&core, &self.relays)?;
        self.connect_to(core, addr, listener).await
    }

    /// Takes the phone off the network until the app makes a new device. Open
    /// connections end with it.
    pub async fn close(&self) {
        let endpoint = {
            let mut online = self.lock();
            online.closed = true;
            online.endpoint.clone()
        };
        let _ = on_runtime(async move { endpoint.close().await }).await;
    }
}

/// What the host said to a join ticket.
#[derive(Debug, PartialEq, Eq, uniffi::Enum)]
pub enum JoinAnswer {
    /// `full` or `watch`.
    Allowed {
        access: String,
    },
    Denied,
    /// The host would not take the ticket; `reason` is a short machine word.
    Refused {
        reason: String,
    },
}

fn access_name(access: sikemux_wire::protocol::DeviceAccess) -> Result<String, MobileError> {
    serde_json::to_value(access)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| invalid("the host gave an access this app does not know"))
}

async fn join_with(
    endpoint: Endpoint,
    addr: EndpointAddr,
    hello: JoinHello,
) -> Result<JoinAnswer, MobileError> {
    let reply =
        on_runtime(async move { sikemux_client::join::join(&endpoint, addr, &hello).await })
            .await?
            .map_err(|error| MobileError::Connection {
                message: error.to_string(),
            })?;
    Ok(match reply {
        JoinReply::Allowed { access } => JoinAnswer::Allowed {
            access: access_name(access)?,
        },
        JoinReply::Denied => JoinAnswer::Denied,
        JoinReply::Refused { reason } => JoinAnswer::Refused { reason },
    })
}

/// The ticket the accounts server gave, checked to be for this phone and the
/// host it is about to dial, so a mix-up shows here and not as the host's
/// refusal.
fn read_ticket(json: &str, phone: &str, host: &str) -> Result<JoinTicket, MobileError> {
    let ticket: JoinTicket = serde_json::from_str(json)
        .map_err(|error| invalid(format!("the join ticket is not readable: {error}")))?;
    if ticket.phone != phone {
        return Err(invalid("the join ticket is for another phone"));
    }
    if ticket.host != host {
        return Err(invalid("the join ticket is for another host"));
    }
    Ok(ticket)
}

/// Why a dial failed: `Stalled` means a fresh endpoint may reach the host.
enum Dial {
    Final(MobileError),
    Stalled(MobileError),
}

impl Dial {
    fn error(self) -> MobileError {
        match self {
            Dial::Final(error) | Dial::Stalled(error) => error,
        }
    }
}

impl Device {
    /// A host this endpoint reached before is dialled with a short wait, and
    /// again from a fresh endpoint if that one has gone stale, so a dropped
    /// connection comes back in seconds rather than after a full timeout.
    async fn connect_to(
        &self,
        core: String,
        addr: EndpointAddr,
        listener: Arc<dyn CoreListener>,
    ) -> Result<Arc<Connection>, MobileError> {
        let (endpoint, generation) = self.endpoint()?;
        let redial = self.lock().reached.contains(&core);
        let wait = if redial {
            REDIAL_TIMEOUT
        } else {
            CONNECT_TIMEOUT
        };
        let first = self
            .dial(&core, endpoint, addr.clone(), listener.clone(), wait)
            .await;
        let stalled = match first {
            Ok(connection) => return Ok(connection),
            Err(Dial::Final(error)) => return Err(error),
            Err(Dial::Stalled(error)) => error,
        };
        self.renew_after_failing(&core, generation).await;
        let (endpoint, now) = self.endpoint()?;
        if !redial || now == generation {
            return Err(stalled);
        }
        self.dial(&core, endpoint, addr, listener, CONNECT_TIMEOUT)
            .await
            .map_err(Dial::error)
    }

    async fn dial(
        &self,
        core: &str,
        endpoint: Endpoint,
        addr: EndpointAddr,
        listener: Arc<dyn CoreListener>,
        wait: Duration,
    ) -> Result<Arc<Connection>, Dial> {
        let open = Arc::new(AtomicBool::new(true));
        let (deliveries, queue) = mpsc::unbounded_channel();
        deliver(listener, queue, open.clone());
        let sink = Arc::new(ListenerSink(deliveries));
        let attempt = on_runtime(async move {
            tokio::time::timeout(wait, remote::open_with(&endpoint, addr, sink)).await
        })
        .await
        .map_err(Dial::Final)?;
        let (client, link) = match attempt {
            Ok(Ok(opened)) => opened,
            Ok(Err(
                error @ (ClientError::Core(_)
                | ClientError::VersionMismatch { .. }
                | ClientError::NotPaired),
            )) => return Err(Dial::Final(error.into())),
            Ok(Err(error)) => return Err(Dial::Stalled(error.into())),
            Err(_) => {
                return Err(Dial::Stalled(MobileError::Connection {
                    message: "this host did not answer in time".into(),
                }))
            }
        };
        self.lock().reached.insert(core.to_owned());
        Ok(Arc::new(Connection {
            client: Mutex::new(Some(Arc::new(client))),
            link,
            open,
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Online> {
        self.online
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn endpoint(&self) -> Result<(Endpoint, u64), MobileError> {
        let online = self.lock();
        if online.closed {
            return Err(MobileError::Connection {
                message: "this phone went off the network".into(),
            });
        }
        Ok((online.endpoint.clone(), online.generation))
    }

    /// Once a connection to a host closes, iroh 1.3 can leave the endpoint
    /// unable to reach that host again, while a new endpoint with the same key
    /// reaches it at once. A host this endpoint never reached is most likely
    /// just away, so it keeps the endpoint.
    ///
    /// The relay sends this key's traffic to its newest endpoint only, so the
    /// old one closes along with every connection on it, and the app connects
    /// to those hosts again.
    async fn renew_after_failing(&self, core: &str, generation: u64) {
        let _renewing = self.renewing.lock().await;
        {
            let online = self.lock();
            if online.closed || online.generation != generation || !online.reached.contains(core) {
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
        if let Ok(runtime) = RUNTIME.as_ref() {
            runtime.spawn(async move { retired.close().await });
        }
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ListenerError {
    #[error("{message}")]
    Failed { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for ListenerError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failed {
            message: error.reason,
        }
    }
}

/// What the core sends a phone without being asked. An error the app returns
/// or throws is dropped: the connection carries on.
#[uniffi::export(with_foreign)]
pub trait CoreListener: Send + Sync {
    /// The core's events in the order it sent them, several at once when
    /// they arrived faster than the app took them.
    fn events(&self, events: Vec<CoreEvent>) -> Result<(), ListenerError>;
    fn closed(&self) -> Result<(), ListenerError>;
}

enum Delivery {
    Event(Box<CoreEvent>),
    Closed,
}

/// Hands what the core sends to a queue, so the connection keeps reading
/// while the app is busy.
struct ListenerSink(mpsc::UnboundedSender<Delivery>);

impl EventSink for ListenerSink {
    fn output(&self, _id: SessionId, _bytes: &[u8]) {}

    fn event(&self, event: Event) {
        if let Some(event) = CoreEvent::from_core(event) {
            let _ = self.0.send(Delivery::Event(Box::new(event)));
        }
    }

    fn window_call(&self, _call_id: CallId, _call: WindowCall) {}

    fn closed(&self) {
        let _ = self.0.send(Delivery::Closed);
    }
}

/// Calls into the app wait for its JavaScript thread, so they run on a thread
/// of their own. Events that queued up meanwhile go over in one call.
/// Nothing reaches the app once it closed the connection itself.
fn deliver(
    listener: Arc<dyn CoreListener>,
    mut queue: mpsc::UnboundedReceiver<Delivery>,
    open: Arc<AtomicBool>,
) {
    let spawned = std::thread::Builder::new()
        .name("sikemux-listener".into())
        .spawn(move || {
            let mut events = Vec::new();
            while let Some(first) = queue.blocking_recv() {
                let mut next = Some(first);
                while let Some(delivery) = next.take() {
                    if !open.load(Ordering::Acquire) {
                        return;
                    }
                    match delivery {
                        Delivery::Event(event) => events.push(*event),
                        Delivery::Closed => {
                            flush(listener.as_ref(), &mut events);
                            let _ = listener.closed();
                            return;
                        }
                    }
                    next = queue.try_recv().ok();
                }
                if !open.load(Ordering::Acquire) {
                    return;
                }
                flush(listener.as_ref(), &mut events);
            }
        });
    if let Err(error) = spawned {
        eprintln!("sikemux: could not start the listener thread: {error}");
    }
}

fn flush(listener: &dyn CoreListener, events: &mut Vec<CoreEvent>) {
    if !events.is_empty() {
        let _ = listener.events(std::mem::take(events));
    }
}

/// An open session with one host's core.
#[derive(uniffi::Object)]
pub struct Connection {
    client: Mutex<Option<Arc<CoreClient>>>,
    link: iroh::endpoint::Connection,
    open: Arc<AtomicBool>,
}

fn not_answered() -> MobileError {
    MobileError::Connection {
        message: "the host stopped answering".into(),
    }
}

fn unexpected() -> MobileError {
    invalid("the host answered with something this app did not ask for")
}

impl Connection {
    fn client(&self) -> Result<Arc<CoreClient>, MobileError> {
        self.client
            .lock()
            .ok()
            .and_then(|client| client.clone())
            .ok_or(MobileError::Connection {
                message: "the connection to the host is closed".into(),
            })
    }

    async fn reply(&self, request: Request) -> Result<Reply, MobileError> {
        self.reply_within(request, REQUEST_TIMEOUT).await
    }

    async fn reply_within(&self, request: Request, wait: Duration) -> Result<Reply, MobileError> {
        let client = self.client()?;
        on_runtime(async move {
            let answer = client.submit(request, |reply| reply)?;
            match tokio::time::timeout(wait, answer).await {
                Ok(reply) => Ok(reply??),
                Err(_) => Err(not_answered()),
            }
        })
        .await?
    }

    async fn ask(&self, request: Request) -> Result<Response, MobileError> {
        match self.reply(request).await? {
            Reply::Response(response) => Ok(response),
            Reply::Attached(_) => Err(unexpected()),
        }
    }

    async fn done(&self, request: Request) -> Result<(), MobileError> {
        match self.ask(request).await? {
            Response::Done => Ok(()),
            _ => Err(unexpected()),
        }
    }
}

#[uniffi::export]
impl Connection {
    /// The computer the core runs on.
    pub async fn host(&self) -> Result<HostInfo, MobileError> {
        match self.ask(Request::Host).await? {
            Response::Host { host } => Ok(host.into()),
            _ => Err(unexpected()),
        }
    }

    /// Writes the host's backdrop picture into `dir` and answers with its path,
    /// or nothing when the host shows none.
    pub async fn save_backdrop(
        &self,
        dir: String,
        id: String,
    ) -> Result<Option<String>, MobileError> {
        let Response::BackdropImage { data_url } = self.ask(Request::BackdropImage).await? else {
            return Err(unexpected());
        };
        let Some(data_url) = data_url else {
            return Ok(None);
        };
        let (extension, bytes) = decode_data_url(&data_url)?;
        let path = backdrop_path(Path::new(&dir), &id, extension);
        let written = path.clone();
        on_runtime(async move {
            if let Some(parent) = written.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&written, bytes).await
        })
        .await?
        .map_err(|error| invalid(format!("could not save the host's backdrop: {error}")))?;
        Ok(Some(path.display().to_string()))
    }

    /// Starts a chat the host's app put to sleep. Answers once it is ready to
    /// take up.
    pub async fn wake_chat(&self, agent_id: String) -> Result<(), MobileError> {
        match self
            .reply_within(Request::AcpWake { agent_id }, WAKE_TIMEOUT)
            .await?
        {
            Reply::Response(Response::Done) => Ok(()),
            _ => Err(unexpected()),
        }
    }

    /// Takes up a chat. Its events follow on the listener; drop those
    /// numbered at or below the answer's mark.
    pub async fn attach_chat(
        &self,
        agent_id: String,
        since: Option<ChatMark>,
    ) -> Result<ChatAttachment, MobileError> {
        let request = Request::AcpAttach {
            agent_id,
            since: since.map(Into::into),
        };
        match self.ask(request).await? {
            Response::ChatAttached { attachment } => Ok(attachment.into()),
            _ => Err(unexpected()),
        }
    }

    /// The turns before event `before` of the chat's run `feed`, a page at a
    /// time, for a chat the phone was sent only the end of.
    pub async fn chat_history(
        &self,
        agent_id: String,
        feed: String,
        before: u64,
        turns: u32,
    ) -> Result<ChatHistory, MobileError> {
        let request = Request::AcpHistory {
            agent_id,
            feed,
            before,
            turns,
        };
        match self.ask(request).await? {
            Response::ChatHistory {
                events,
                older_before,
            } => Ok(ChatHistory {
                events_json: records::json(&events),
                older_before,
            }),
            _ => Err(unexpected()),
        }
    }

    /// No more of the chat's events reach this phone.
    pub async fn detach_chat(&self, agent_id: String) -> Result<(), MobileError> {
        self.done(Request::AcpDetach { agent_id }).await
    }

    /// `paths` are files on the host, such as those [`Self::attach_file`]
    /// answers with.
    pub async fn prompt(
        &self,
        agent_id: String,
        text: String,
        paths: Vec<String>,
    ) -> Result<(), MobileError> {
        self.done(Request::AcpPrompt {
            agent_id,
            message_id: None,
            text,
            paths,
            context: Vec::new(),
        })
        .await
    }

    /// Sends a file for the chat's next message and answers with where the
    /// host keeps it. A host too old to know the request refuses it.
    pub async fn attach_file(
        &self,
        agent_id: String,
        name: String,
        mime: String,
        bytes: Vec<u8>,
    ) -> Result<String, MobileError> {
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(invalid("a file sent to a chat can be at most 10 MB"));
        }
        let request = Request::AttachFile {
            agent_id,
            name,
            mime,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        match self.reply_within(request, UPLOAD_TIMEOUT).await? {
            Reply::Response(Response::Attached { path }) => Ok(path.display().to_string()),
            _ => Err(unexpected()),
        }
    }

    pub async fn cancel(&self, agent_id: String) -> Result<(), MobileError> {
        self.done(Request::AcpCancel { agent_id }).await
    }

    /// Puts a message into the running turn. Answers whether the agent took
    /// it; when the turn ended first, send it with [`Self::prompt`] instead.
    pub async fn steer(
        &self,
        agent_id: String,
        text: String,
        paths: Vec<String>,
    ) -> Result<bool, MobileError> {
        let request = Request::AcpSteer {
            agent_id,
            text,
            paths,
            context: Vec::new(),
        };
        match self.ask(request).await? {
            Response::Steered { outcome } => Ok(outcome != "promptRequired"),
            _ => Err(unexpected()),
        }
    }

    /// Stops one of the chat's background tasks, such as a shell it left
    /// running.
    pub async fn stop_task(&self, agent_id: String, task_id: String) -> Result<(), MobileError> {
        self.done(Request::AcpStopTask { agent_id, task_id }).await
    }

    /// `mode` is `bypass` to run without asking or `workspace-write` to ask
    /// first. The host refuses while a turn runs.
    pub async fn set_permission_mode(
        &self,
        agent_id: String,
        mode: String,
    ) -> Result<(), MobileError> {
        self.done(Request::AcpSetPermissionMode { agent_id, mode })
            .await
    }

    /// Ends the chat's agent on the host.
    pub async fn stop_chat(&self, agent_id: String) -> Result<(), MobileError> {
        self.done(Request::AcpStop { agent_id }).await
    }

    /// `option_id` absent turns the request down.
    pub async fn answer_permission(
        &self,
        agent_id: String,
        request_id: String,
        option_id: Option<String>,
    ) -> Result<(), MobileError> {
        self.done(Request::AcpPermissionReply {
            agent_id,
            request_id,
            option_id,
        })
        .await
    }

    /// Answers with the chat's new settings, as JSON.
    pub async fn set_chat_config(
        &self,
        agent_id: String,
        config_id: String,
        value: String,
    ) -> Result<String, MobileError> {
        let request = Request::AcpSetConfig {
            agent_id,
            config_id,
            value,
        };
        match self.ask(request).await? {
            Response::ChatConfig { value } => Ok(value.to_string()),
            _ => Err(unexpected()),
        }
    }

    /// Starts a chat the way the host's app would, in one of its projects, and
    /// answers with the chat's agent id. `permission_mode` overrides the
    /// launcher's, as in `set_permission_mode`.
    pub async fn start_chat(
        &self,
        launcher: String,
        project: String,
        permission_mode: Option<String>,
    ) -> Result<String, MobileError> {
        let request = Request::StartChat {
            launcher,
            project,
            permission_mode,
            model: None,
            effort: None,
        };
        match self.ask(request).await? {
            Response::ChatBegun { agent_id, .. } => Ok(agent_id),
            _ => Err(unexpected()),
        }
    }

    /// Takes up again one of the host's recent chats, by its id in the view,
    /// and answers with the chat's agent id. A host too old to know the
    /// request refuses it.
    pub async fn resume_chat(&self, recent: String) -> Result<String, MobileError> {
        let request = Request::ResumeChat {
            recent,
            permission_mode: None,
            model: None,
            effort: None,
        };
        match self.ask(request).await? {
            Response::ChatBegun { agent_id, .. } => Ok(agent_id),
            _ => Err(unexpected()),
        }
    }

    /// Gives the host the 32-byte `key` it seals this phone's notifications
    /// with, under `key_id`, and what the phone wants to hear about, as JSON:
    /// `{needsYou, finished, problems, when, muted}`. A host older than
    /// notifications never answers, so the app gives up waiting on its own.
    pub async fn set_notifications(
        &self,
        key_id: u32,
        key: Vec<u8>,
        prefs_json: String,
    ) -> Result<(), MobileError> {
        if key.len() != 32 {
            return Err(invalid("a notification key is 32 bytes"));
        }
        let prefs = notify_prefs(&prefs_json)?;
        self.done(Request::SetNotifications {
            key_id,
            key: hex::encode(key),
            prefs,
        })
        .await
    }

    /// The host sends this phone no more notifications.
    pub async fn clear_notifications(&self) -> Result<(), MobileError> {
        self.done(Request::ClearNotifications).await
    }

    /// Whether the app is in front, where a chat it shows needs no notification.
    pub async fn set_foreground(&self, foreground: bool) -> Result<(), MobileError> {
        self.done(Request::SetForeground { foreground }).await
    }

    /// Asks the host to forget this phone. The host closes the connection after.
    pub async fn unpair(&self) -> Result<(), MobileError> {
        self.done(Request::Unpair).await
    }

    pub fn is_open(&self) -> bool {
        self.client().is_ok_and(|client| client.is_connected())
    }

    /// Ends the session now: requests still waiting fail, and the listener
    /// hears nothing more, not even that it closed.
    pub fn close(&self) {
        self.open.store(false, Ordering::Release);
        if let Ok(mut client) = self.client.lock() {
            client.take();
        }
        self.link.close(0u32.into(), b"closed by the phone");
    }
}

/// The picture inside a `data:image/...;base64,` URL, and the file extension
/// for its kind.
fn decode_data_url(url: &str) -> Result<(&'static str, Vec<u8>), MobileError> {
    let unreadable = || invalid("the host's backdrop is not a picture this app can read");
    let rest = url.strip_prefix("data:image/").ok_or_else(unreadable)?;
    let (kind, data) = rest.split_once(";base64,").ok_or_else(unreadable)?;
    let extension = match kind {
        "png" => "png",
        "jpeg" | "jpg" => "jpg",
        "webp" => "webp",
        "gif" => "gif",
        "heic" => "heic",
        _ => return Err(unreadable()),
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| unreadable())?;
    Ok((extension, bytes))
}

/// The host names the picture, so the name keeps only characters that cannot
/// lead out of `dir`.
fn backdrop_path(dir: &Path, id: &str, extension: &str) -> PathBuf {
    let name: String = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    dir.join(format!("{name}.{extension}"))
}

#[cfg(test)]
mod loopback_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The vector the server's and the core's tests check too.
    #[test]
    fn registrations_sign_the_text_the_server_checks() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../server/protocol/vectors/registration.json"
        ))
        .expect("the vector is JSON");
        let text = |name: &str| vector[name].as_str().expect("a string").to_owned();
        let bytes: [u8; 32] = hex::decode(text("secretKey"))
            .expect("hex")
            .try_into()
            .expect("32 bytes");
        let key = SecretKey::from_bytes(&bytes);
        assert_eq!(key.public().to_string(), text("key"));
        assert_eq!(
            sign_registration(&key, &text("nonce"), &text("userId")).expect("signs"),
            text("signature")
        );
        assert!(sign_registration(&key, "not a challenge", &text("userId")).is_err());
    }

    #[test]
    fn live_hellos_sign_the_text_the_server_checks() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../server/protocol/vectors/live.json"
        ))
        .expect("the vector is JSON");
        let text = |name: &str| vector[name].as_str().expect("a string").to_owned();
        let bytes: [u8; 32] = hex::decode(text("secretKey"))
            .expect("hex")
            .try_into()
            .expect("32 bytes");
        let key = SecretKey::from_bytes(&bytes);
        assert_eq!(key.public().to_string(), text("key"));
        assert_eq!(
            sign_live(&key, &text("nonce")).expect("signs"),
            text("signature")
        );
        assert!(sign_live(&key, "not a challenge").is_err());
        assert!(sign_live(&key, &text("nonce").to_uppercase()).is_err());
    }

    #[test]
    fn push_tokens_sign_the_text_the_server_checks() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../server/protocol/vectors/push-token.json"
        ))
        .expect("the vector is JSON");
        let text = |name: &str| vector[name].as_str().expect("a string").to_owned();
        let bytes: [u8; 32] = hex::decode(text("secretKey"))
            .expect("hex")
            .try_into()
            .expect("32 bytes");
        let key = SecretKey::from_bytes(&bytes);
        assert_eq!(key.public().to_string(), text("key"));
        assert_eq!(
            sign_push(&key, &text("nonce"), &text("tokenSha256")).expect("signs"),
            text("signature")
        );
        assert!(sign_push(&key, "not a challenge", &text("tokenSha256")).is_err());
        assert!(sign_push(&key, &text("nonce"), &text("token")).is_err());
    }

    #[test]
    fn notification_settings_read_as_the_app_writes_them() {
        let prefs = notify_prefs(
            r#"{"needsYou":true,"finished":false,"problems":true,"when":"away","muted":[{"agentId":"chat-7f3a","until":null}]}"#,
        )
        .expect("reads");
        assert!(prefs.needs_you && !prefs.finished && prefs.problems);
        assert_eq!(prefs.when, sikemux_wire::protocol::NotifyWhen::Away);
        assert_eq!(prefs.muted[0].agent_id, "chat-7f3a");
        assert!(notify_prefs(r#"{"when":"sometimes"}"#).is_err());
    }

    #[test]
    fn hosts_are_dialled_through_every_usable_relay_and_never_none() {
        let setting = |url: &str, quic_port| RelaySetting {
            url: url.into(),
            quic_port,
        };
        let relays = relays_from(vec![
            setting("not a relay", None),
            setting("https://relay.example/", Some(7842)),
            setting("https://second.relay.example/", None),
        ]);
        assert_eq!(relays.len(), 2);
        assert_eq!(relays[0].quic_port, Some(7842));
        let core = SecretKey::generate().public().to_string();
        let addr = core_addr(&core, &relays).expect("an address");
        let mut dialled: Vec<String> = addr.relay_urls().map(ToString::to_string).collect();
        dialled.sort();
        assert_eq!(
            dialled,
            ["https://relay.example/", "https://second.relay.example/"]
        );

        let fallback = relays_from(Vec::new());
        assert_eq!(fallback[0].url, network::DEFAULT_RELAY);
        assert_eq!(
            fallback[0].quic_port,
            Some(i64::from(network::DEFAULT_QUIC_PORT))
        );
    }

    #[test]
    fn a_join_ticket_is_sent_only_to_the_host_it_names_from_the_phone_it_names() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../server/protocol/vectors/join.json"
        ))
        .expect("the vector is JSON");
        let json = vector["ticket"].to_string();
        let text = |name: &str| {
            vector["ticket"][name]
                .as_str()
                .expect("a string")
                .to_owned()
        };
        let (phone, host) = (text("phone"), text("host"));
        let ticket = read_ticket(&json, &phone, &host).expect("reads");
        assert_eq!(ticket.account, "user_2vectorTest");
        assert!(matches!(
            read_ticket(&json, &host, &host),
            Err(MobileError::Invalid { message }) if message.contains("another phone")
        ));
        assert!(matches!(
            read_ticket(&json, &phone, &phone),
            Err(MobileError::Invalid { message }) if message.contains("another host")
        ));
        assert!(read_ticket("{}", &phone, &host).is_err());
    }

    #[test]
    fn a_backdrop_is_decoded_and_named_inside_its_folder() {
        let (extension, bytes) = decode_data_url("data:image/png;base64,aGk=").unwrap();
        assert_eq!((extension, bytes.as_slice()), ("png", &b"hi"[..]));
        assert!(decode_data_url("data:text/html;base64,aGk=").is_err());
        assert!(decode_data_url("data:image/png;base64,%%%").is_err());
        assert_eq!(
            backdrop_path(Path::new("/b"), "../../etc/passwd", "png"),
            PathBuf::from("/b/______etc_passwd.png")
        );
    }

    #[test]
    fn a_host_that_turns_this_phone_away_is_named_the_older_by_the_newest_it_speaks() {
        let mismatch = |version| {
            MobileError::from(ClientError::VersionMismatch {
                version,
                pid: 1,
                message: String::new(),
            })
        };
        assert!(matches!(
            mismatch(OLDEST_PROTOCOL_VERSION - 1),
            MobileError::Outdated { mac_is_older: true }
        ));
        assert!(matches!(
            mismatch(OLDEST_PROTOCOL_VERSION),
            MobileError::Outdated {
                mac_is_older: false
            }
        ));
    }

    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<String>>,
        fail: bool,
    }

    impl CoreListener for Recorder {
        fn events(&self, events: Vec<CoreEvent>) -> Result<(), ListenerError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("events {}", events.len()));
            if self.fail {
                return Err(ListenerError::Failed {
                    message: "the app threw".into(),
                });
            }
            Ok(())
        }

        fn closed(&self) -> Result<(), ListenerError> {
            self.calls.lock().unwrap().push("closed".into());
            Ok(())
        }
    }

    fn chat_event(seq: u64) -> Event {
        Event::Chat {
            agent_id: "agent".into(),
            seq,
            event: sikemux_wire::protocol::ChatEvent {
                kind: sikemux_wire::protocol::ChatEventKind::TurnStarted,
                payload: serde_json::json!({}),
            },
        }
    }

    fn delivered(recorder: &Recorder) -> Vec<String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let calls = recorder.calls.lock().unwrap().clone();
            if calls.last().is_some_and(|call| call == "closed") {
                return calls;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the listener never closed"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn events_that_queue_up_reach_the_app_together_and_in_order() {
        let recorder = Arc::new(Recorder::default());
        let (deliveries, queue) = mpsc::unbounded_channel();
        let sink = ListenerSink(deliveries);
        sink.event(chat_event(1));
        sink.event(chat_event(2));
        sink.output(7, b"ab");
        sink.event(chat_event(3));
        sink.closed();
        deliver(recorder.clone(), queue, Arc::new(AtomicBool::new(true)));
        assert_eq!(delivered(&recorder), ["events 3", "closed"]);
    }

    #[test]
    fn an_app_that_fails_an_event_still_hears_the_rest() {
        let recorder = Arc::new(Recorder {
            fail: true,
            ..Recorder::default()
        });
        let (deliveries, queue) = mpsc::unbounded_channel();
        let sink = ListenerSink(deliveries);
        sink.event(chat_event(1));
        sink.event(chat_event(2));
        sink.closed();
        deliver(recorder.clone(), queue, Arc::new(AtomicBool::new(true)));
        assert_eq!(delivered(&recorder), ["events 2", "closed"]);
    }
}
