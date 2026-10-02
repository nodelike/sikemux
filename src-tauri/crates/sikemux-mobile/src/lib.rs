//! What the phone app calls to pair with a Mac and talk to its core. The
//! connection, pairing and wire format are the core's own (`sikemux-core`),
//! so the phone and the Mac cannot drift apart; this crate only exposes them
//! through UniFFI, as typed calls and records.
//!
//! A chat's events cross as JSON, since the phone reads them with the Mac
//! app's chat code. Terminal bytes cross as bytes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use base64::Engine;
use iroh::address_lookup::{DnsAddressLookup, PkarrResolver};
use iroh::endpoint::{default_relay_mode, presets};
use iroh::{Endpoint, EndpointAddr, SecretKey};
use sikemux_core::client::{ClientError, CoreClient, EventSink, Reply};
use sikemux_core::pairing::{self, PairError, PairingRequest};
use sikemux_core::protocol::{
    CallId, Event, Request, Response, SessionId, WindowCall, PROTOCOL_VERSION,
};
use sikemux_core::remote;
use tokio::sync::mpsc;

mod records;

pub use records::*;

#[cfg(target_os = "android")]
mod android;

uniffi::setup_scaffolding!();

/// Long enough to find a Mac through a relay on a slow network; past it the
/// app shows the Mac as unreachable and tries again.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Long enough for a long chat's replay over a relay. A Mac that has not
/// answered by then has most likely gone, though the connection has not
/// noticed yet.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// iroh and the core's client both need a Tokio runtime, which the phone's
/// JavaScript thread does not have. The phone talks to a few Macs at most, so
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
/// cancels pairing.
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
    #[error("the code does not match the one on the Mac")]
    WrongCode,
    #[error("{message}")]
    Connection { message: String },
    #[error("{message}")]
    Invalid { message: String },
    /// The Mac and this app speak different versions of the core's protocol.
    #[error("this Mac and this app need the same Sikemux release")]
    Outdated { mac_is_older: bool },
    /// The Mac forgot this phone, so it has to pair again.
    #[error("this Mac no longer knows this phone; pair with it again")]
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
                mac_is_older: version < PROTOCOL_VERSION,
            },
            ClientError::NotPaired => MobileError::Unpaired,
            other => MobileError::Connection {
                message: other.to_string(),
            },
        }
    }
}

impl From<PairError> for MobileError {
    fn from(error: PairError) -> Self {
        match error {
            PairError::Refused(message) => MobileError::Refused { message },
            PairError::WrongCode => MobileError::WrongCode,
            PairError::Connection(message) => MobileError::Connection { message },
        }
    }
}

/// A new device key. The app keeps it in the Keychain or Keystore; it is the
/// device's identity to every Mac it pairs with.
#[uniffi::export]
pub fn new_device_key() -> Vec<u8> {
    SecretKey::generate().to_bytes().to_vec()
}

#[derive(uniffi::Record)]
pub struct PairingLink {
    pub core: String,
    pub code: String,
}

/// Reads the link a Mac's pairing QR code holds.
#[uniffi::export]
pub fn parse_pairing_link(text: String) -> Option<PairingLink> {
    pairing::PairingLink::parse(&text).map(|link| PairingLink {
        core: link.core.to_string(),
        code: link.code,
    })
}

/// The endpoint and the Macs it has reached.
struct Online {
    endpoint: Endpoint,
    generation: u64,
    reached: HashSet<String>,
}

/// This phone on the network, known by its key.
#[derive(uniffi::Object)]
pub struct Device {
    key: SecretKey,
    online: Mutex<Online>,
    renewing: tokio::sync::Mutex<()>,
}

/// The phone looks Macs up but never publishes its own addresses: no Mac
/// dials a phone, and publishing would announce where the phone is.
async fn bind(key: SecretKey) -> Result<Endpoint, MobileError> {
    on_runtime(async move {
        Endpoint::builder(presets::Minimal)
            .address_lookup(PkarrResolver::n0_dns())
            .address_lookup(DnsAddressLookup::n0_dns())
            .relay_mode(default_relay_mode())
            .secret_key(key)
            .bind()
            .await
    })
    .await?
    .map_err(|error| MobileError::Connection {
        message: error.to_string(),
    })
}

fn core_addr(core: &str) -> Result<EndpointAddr, MobileError> {
    Ok(EndpointAddr::new(core.parse().map_err(invalid)?))
}

#[uniffi::export]
impl Device {
    /// Comes online with the key from [`new_device_key`].
    #[uniffi::constructor]
    pub async fn create(key: Vec<u8>) -> Result<Arc<Self>, MobileError> {
        let bytes: [u8; 32] = key
            .try_into()
            .map_err(|_| invalid("a device key is 32 bytes"))?;
        #[cfg(target_os = "android")]
        android::ensure_context().map_err(|message| MobileError::Connection { message })?;
        let key = SecretKey::from_bytes(&bytes);
        let endpoint = bind(key.clone()).await?;
        Ok(Arc::new(Self {
            key,
            online: Mutex::new(Online {
                endpoint,
                generation: 0,
                reached: HashSet::new(),
            }),
            renewing: tokio::sync::Mutex::new(()),
        }))
    }

    /// The key Macs know this phone by.
    pub fn id(&self) -> String {
        self.key.public().to_string()
    }

    /// Pairs with the Mac whose key is `core`, waiting while the person
    /// there decides. Answers with the access they gave: `full` or `watch`.
    pub async fn pair(
        &self,
        core: String,
        code: String,
        name: String,
        platform: String,
    ) -> Result<String, MobileError> {
        let (endpoint, _) = self.endpoint();
        let addr = core_addr(&core)?;
        let access = on_runtime(async move {
            let request = PairingRequest {
                code: &code,
                name: &name,
                platform: &platform,
            };
            pairing::pair(&endpoint, addr, request).await
        })
        .await??;
        serde_json::to_value(access)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| invalid("the Mac gave an access this app does not know"))
    }

    /// Opens a session with a Mac this phone paired with. Everything the core
    /// sends unasked arrives on `listener`, in order, off the network's threads.
    pub async fn connect(
        &self,
        core: String,
        listener: Arc<dyn CoreListener>,
    ) -> Result<Arc<Connection>, MobileError> {
        let addr = core_addr(&core)?;
        self.connect_to(core, addr, listener).await
    }

    /// Takes the phone off the network until the app makes a new device. Open
    /// connections end with it.
    pub async fn close(&self) {
        let (endpoint, _) = self.endpoint();
        let _ = on_runtime(async move { endpoint.close().await }).await;
    }
}

impl Device {
    async fn connect_to(
        &self,
        core: String,
        addr: EndpointAddr,
        listener: Arc<dyn CoreListener>,
    ) -> Result<Arc<Connection>, MobileError> {
        let (endpoint, generation) = self.endpoint();
        let (deliveries, queue) = mpsc::unbounded_channel();
        deliver(listener, queue);
        let sink = Arc::new(ListenerSink(deliveries));
        let attempt = on_runtime(async move {
            tokio::time::timeout(CONNECT_TIMEOUT, remote::connect_with(&endpoint, addr, sink)).await
        })
        .await?;
        let client = match attempt {
            Ok(Ok(client)) => client,
            Ok(Err(
                error @ (ClientError::Core(_)
                | ClientError::VersionMismatch { .. }
                | ClientError::NotPaired),
            )) => return Err(error.into()),
            Ok(Err(error)) => {
                self.renew_after_failing(&core, generation).await;
                return Err(error.into());
            }
            Err(_) => {
                self.renew_after_failing(&core, generation).await;
                return Err(MobileError::Connection {
                    message: "this Mac did not answer in time".into(),
                });
            }
        };
        self.lock().reached.insert(core);
        Ok(Arc::new(Connection {
            client: Mutex::new(Some(Arc::new(client))),
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Online> {
        self.online
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn endpoint(&self) -> (Endpoint, u64) {
        let online = self.lock();
        (online.endpoint.clone(), online.generation)
    }

    /// Once a connection to a Mac closes, iroh 1.3 can leave the endpoint
    /// unable to reach that Mac again, while a new endpoint with the same key
    /// reaches it at once. A Mac this endpoint never reached is most likely
    /// just away, so it keeps the endpoint. Connections still open on the old
    /// endpoint keep it alive until they end.
    async fn renew_after_failing(&self, core: &str, generation: u64) {
        let _renewing = self.renewing.lock().await;
        {
            let online = self.lock();
            if online.generation != generation || !online.reached.contains(core) {
                return;
            }
        }
        if let Ok(fresh) = bind(self.key.clone()).await {
            let mut online = self.lock();
            online.endpoint = fresh;
            online.generation += 1;
            online.reached.clear();
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
    /// Terminal bytes for a session the phone attached to. Pass their length
    /// back to [`Connection::ack`] once shown.
    fn output(&self, session: u64, bytes: Vec<u8>) -> Result<(), ListenerError>;
    /// The core's events in the order it sent them, several at once when
    /// they arrived faster than the app took them.
    fn events(&self, events: Vec<CoreEvent>) -> Result<(), ListenerError>;
    fn closed(&self) -> Result<(), ListenerError>;
}

enum Delivery {
    Output(SessionId, Vec<u8>),
    Event(CoreEvent),
    Closed,
}

/// Hands what the core sends to a queue, so the connection keeps reading
/// while the app is busy.
struct ListenerSink(mpsc::UnboundedSender<Delivery>);

impl EventSink for ListenerSink {
    fn output(&self, id: SessionId, bytes: &[u8]) {
        let _ = self.0.send(Delivery::Output(id, bytes.to_vec()));
    }

    fn event(&self, event: Event) {
        if let Some(event) = CoreEvent::from_core(event) {
            let _ = self.0.send(Delivery::Event(event));
        }
    }

    fn window_call(&self, _call_id: CallId, _call: WindowCall) {}

    fn closed(&self) {
        let _ = self.0.send(Delivery::Closed);
    }
}

/// Calls into the app wait for its JavaScript thread, so they run on a thread
/// of their own. Events that queued up meanwhile go over in one call.
fn deliver(listener: Arc<dyn CoreListener>, mut queue: mpsc::UnboundedReceiver<Delivery>) {
    let spawned = std::thread::Builder::new()
        .name("sikemux-listener".into())
        .spawn(move || {
            let mut events = Vec::new();
            while let Some(first) = queue.blocking_recv() {
                let mut next = Some(first);
                while let Some(delivery) = next.take() {
                    match delivery {
                        Delivery::Event(event) => events.push(event),
                        Delivery::Output(session, bytes) => {
                            flush(listener.as_ref(), &mut events);
                            let _ = listener.output(session, bytes);
                        }
                        Delivery::Closed => {
                            flush(listener.as_ref(), &mut events);
                            let _ = listener.closed();
                            return;
                        }
                    }
                    next = queue.try_recv().ok();
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

#[derive(uniffi::Record)]
pub struct AttachedScreen {
    /// Bytes that redraw the terminal as it is now; live output follows.
    pub replay: Vec<u8>,
    pub alternate_screen: bool,
    pub exited: bool,
}

/// An open session with one Mac's core.
#[derive(uniffi::Object)]
pub struct Connection {
    client: Mutex<Option<Arc<CoreClient>>>,
}

fn not_answered() -> MobileError {
    MobileError::Connection {
        message: "the Mac stopped answering".into(),
    }
}

fn unexpected() -> MobileError {
    invalid("the Mac answered with something this app did not ask for")
}

impl Connection {
    fn client(&self) -> Result<Arc<CoreClient>, MobileError> {
        self.client
            .lock()
            .ok()
            .and_then(|client| client.clone())
            .ok_or(MobileError::Connection {
                message: "the connection to the Mac is closed".into(),
            })
    }

    async fn reply(&self, request: Request) -> Result<Reply, MobileError> {
        let client = self.client()?;
        on_runtime(async move {
            let answer = client.submit(request, |reply| reply)?;
            match tokio::time::timeout(REQUEST_TIMEOUT, answer).await {
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

    /// Writes the Mac's backdrop picture into `dir` and answers with its path,
    /// or nothing when the Mac shows none.
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
        .map_err(|error| invalid(format!("could not save the Mac's backdrop: {error}")))?;
        Ok(Some(path.display().to_string()))
    }

    /// Starts a chat the Mac's app put to sleep. Answers once it runs.
    pub async fn wake_chat(&self, agent_id: String) -> Result<(), MobileError> {
        self.done(Request::AcpWake { agent_id }).await
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

    /// No more of the chat's events reach this phone.
    pub async fn detach_chat(&self, agent_id: String) -> Result<(), MobileError> {
        self.done(Request::AcpDetach { agent_id }).await
    }

    pub async fn prompt(&self, agent_id: String, text: String) -> Result<(), MobileError> {
        self.done(Request::AcpPrompt {
            agent_id,
            text,
            paths: Vec::new(),
            context: Vec::new(),
        })
        .await
    }

    pub async fn cancel(&self, agent_id: String) -> Result<(), MobileError> {
        self.done(Request::AcpCancel { agent_id }).await
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

    /// Starts a chat the way the Mac's app would, in one of its projects, and
    /// answers with the chat's agent id.
    pub async fn start_chat(
        &self,
        launcher: String,
        project: String,
    ) -> Result<String, MobileError> {
        let request = Request::StartChat {
            launcher,
            project,
            permission_mode: None,
            model: None,
            effort: None,
        };
        match self.ask(request).await? {
            Response::ChatBegun { agent_id, .. } => Ok(agent_id),
            _ => Err(unexpected()),
        }
    }

    /// Asks the Mac to forget this phone. The Mac closes the connection after.
    pub async fn unpair(&self) -> Result<(), MobileError> {
        self.done(Request::Unpair).await
    }

    pub async fn attach(&self, session: u64) -> Result<AttachedScreen, MobileError> {
        match self.reply(Request::Attach { id: session }).await? {
            Reply::Attached(attached) => Ok(AttachedScreen {
                replay: attached.replay,
                alternate_screen: attached.alternate_screen,
                exited: attached.exited,
            }),
            Reply::Response(_) => Err(unexpected()),
        }
    }

    pub async fn write(&self, session: u64, bytes: Vec<u8>) -> Result<(), MobileError> {
        let client = self.client()?;
        Ok(on_runtime(async move { client.write(session, &bytes).await }).await??)
    }

    pub async fn resize(&self, session: u64, cols: u16, rows: u16) -> Result<(), MobileError> {
        self.done(Request::Resize {
            id: session,
            cols,
            rows,
        })
        .await
    }

    /// Says the phone has shown this many of a session's output bytes, so the
    /// core sends more.
    pub fn ack(&self, session: u64, bytes: u64) -> Result<(), MobileError> {
        self.client()?.ack(session, bytes as usize);
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.client().is_ok_and(|client| client.is_connected())
    }

    /// What the phone already sent still reaches the Mac.
    pub fn close(&self) {
        if let Ok(mut client) = self.client.lock() {
            client.take();
        }
    }
}

/// The picture inside a `data:image/...;base64,` URL, and the file extension
/// for its kind.
fn decode_data_url(url: &str) -> Result<(&'static str, Vec<u8>), MobileError> {
    let unreadable = || invalid("the Mac's backdrop is not a picture this app can read");
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

/// The Mac names the picture, so the name keeps only characters that cannot
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

    #[test]
    fn a_device_key_is_32_bytes_and_new_each_time() {
        let key = new_device_key();
        assert_eq!(key.len(), 32);
        assert_ne!(key, new_device_key());
    }

    #[test]
    fn a_pairing_link_from_the_mac_reads_back() {
        let core = SecretKey::generate().public();
        let text = pairing::PairingLink {
            core,
            code: "482913".into(),
        }
        .to_url();
        let link = parse_pairing_link(text).expect("a link");
        assert_eq!(link.core, core.to_string());
        assert_eq!(link.code, "482913");
        assert!(parse_pairing_link("https://example.com".into()).is_none());
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
    fn an_unpaired_phone_is_told_so_rather_than_shown_a_network_error() {
        assert!(matches!(
            MobileError::from(ClientError::NotPaired),
            MobileError::Unpaired
        ));
        assert!(matches!(
            MobileError::from(ClientError::Disconnected),
            MobileError::Connection { .. }
        ));
    }

    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<String>>,
        fail: bool,
    }

    impl CoreListener for Recorder {
        fn output(&self, session: u64, bytes: Vec<u8>) -> Result<(), ListenerError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("output {session} {}", bytes.len()));
            Ok(())
        }

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
            event: sikemux_core::protocol::ChatEvent {
                kind: sikemux_core::protocol::ChatEventKind::TurnStarted,
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
    fn events_that_queue_up_reach_the_app_together_and_in_order_with_output() {
        let recorder = Arc::new(Recorder::default());
        let (deliveries, queue) = mpsc::unbounded_channel();
        let sink = ListenerSink(deliveries);
        sink.event(chat_event(1));
        sink.event(chat_event(2));
        sink.output(7, b"ab");
        sink.event(chat_event(3));
        sink.closed();
        deliver(recorder.clone(), queue);
        assert_eq!(
            delivered(&recorder),
            ["events 2", "output 7 2", "events 1", "closed"]
        );
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
        sink.output(1, b"x");
        sink.event(chat_event(2));
        sink.closed();
        deliver(recorder.clone(), queue);
        assert_eq!(
            delivered(&recorder),
            ["events 1", "output 1 1", "events 1", "closed"]
        );
    }
}
