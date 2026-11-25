//! Sends panics, native crashes, release health and how long the app took to start to Sentry.
//! Only builds the release workflow publishes know where to send, and they send only while the
//! person shares usage data, has been shown that choice, and `DO_NOT_TRACK` is unset. Nothing a
//! person wrote or opened goes along, nor any memory, their home folder is written `~`, and they are named only
//! by the account hash the usage ping sends.

use std::borrow::Cow;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use sentry::integrations::contexts::ContextIntegration;
use sentry::integrations::minidump::MinidumpIntegration;
use sentry::protocol::{
    Breadcrumb, DebugImage, DebugMeta, EnvelopeItem, Event, Exception, Frame, IpAddress, Mechanism,
    SessionAttributes, SessionStatus, SessionUpdate, SpanId, Stacktrace, Transaction,
};
use sentry::transports::ReqwestHttpTransportOptions;
use sentry::types::Uuid;
use sentry::{Client, ClientOptions, Hub, Level, TransactionContext, Transport};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sikemux_corekit::usage::{self, Install, Sharing};
use sikemux_process::sync::Locked;

use crate::crash_stacks::{self, CrashStack};
use crate::panic_reports::{self, PanicReport};

const DSN: Option<&str> = if cfg!(debug_assertions) {
    None
} else {
    option_env!("SIKEMUX_SENTRY_DSN")
};
const REPORTER_VARIABLE: &str = "SIKEMUX_CRASH_REPORTER";
const PARENT_VARIABLE: &str = "SIKEMUX_CRASH_REPORTER_PARENT";
const SESSION_VARIABLE: &str = "SIKEMUX_CRASH_REPORTER_SESSION";
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
/// The crash reporter ends once this runs out, sent or not.
const CRASH_FLUSH_TIMEOUT: Duration = Duration::from_secs(10);
/// By when a crash reporter that is still running has read and deleted its dump.
const DUMP_LEFT_AFTER: Duration = Duration::from_secs(60);
/// How often the core looks for panics that other processes left behind.
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// How often a process that may not send looks whether it may now.
const ASK_AGAIN_EVERY: Duration = Duration::from_secs(5 * 60);
const STARTUP_SAMPLE_RATE: f32 = 0.1;
const MAX_BREADCRUMBS: usize = 50;
const BREADCRUMBS_PER_SECOND: u64 = 10;
const MAX_BREADCRUMB_TEXT: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Process {
    App,
    Core,
}

impl Process {
    fn name(self) -> &'static str {
        match self {
            Process::App => "app",
            Process::Core => "core",
        }
    }
}

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static SOCKET: OnceLock<PathBuf> = OnceLock::new();
static USER: OnceLock<String> = OnceLock::new();
static SENDING_CRASH: AtomicBool = AtomicBool::new(false);
static STARTUP: Mutex<Option<Startup>> = Mutex::new(None);
static SESSION: Mutex<Option<SessionUpdate<'static>>> = Mutex::new(None);

struct Startup {
    began: SystemTime,
    steps: Vec<(&'static str, SystemTime, SystemTime)>,
}

/// Starts sending from this process. A copy of it started by the crash reporter runs it here
/// instead and never returns, so this comes first in `main`. The client starts on a thread of
/// its own, so it costs the person's start nothing. `socket` is the core's, which knows the
/// account the person is signed in to.
pub fn start(process: Process, data_dir: Option<PathBuf>, socket: Option<PathBuf>) {
    if let Some(dsn) = DSN {
        start_sending_to(dsn, process, data_dir, socket);
    }
}

fn start_sending_to(
    dsn: &'static str,
    process: Process,
    data_dir: Option<PathBuf>,
    socket: Option<PathBuf>,
) {
    if let Some(data_dir) = data_dir {
        let _ = DATA_DIR.set(data_dir);
    }
    if let Some(socket) = socket {
        let _ = SOCKET.set(socket);
    }
    let mut minidumps = MinidumpIntegration::new()
        .server_env_var(REPORTER_VARIABLE)
        .process_name(format!("sikemux {} crash reporter", process.name()))
        .flush_timeout(CRASH_FLUSH_TIMEOUT)
        .before_capture(|scope, dump| {
            SENDING_CRASH.store(true, Ordering::SeqCst);
            send_crashed_session();
            scope.clear_attachments();
            let stack = take_stack(dump);
            scope.add_event_processor(move |event| Some(native_crash(event, stack.as_ref())));
        })
        .on_process(|command| {
            command.env(PARENT_VARIABLE, std::process::id().to_string());
            if let Some(session) = SESSION.locked().as_ref() {
                if let Ok(session) = serde_json::to_string(session) {
                    command.env(SESSION_VARIABLE, session);
                }
            }
        });
    let dumps = dumps_dir();
    if let Some(dumps) = &dumps {
        minidumps = minidumps.crashes_dir(dumps);
    }
    if minidumps.is_crash_reporter_process() {
        if let Some(user) = crashed_session().and_then(|session| session.distinct_id) {
            let _ = USER.set(user);
        }
        #[cfg(unix)]
        leave_with_parent();
        Client::with_options(options(dsn, process, minidumps));
        std::process::exit(0);
    }
    let spawned = std::thread::Builder::new()
        .name("crash-reports".into())
        .spawn(move || {
            if let Some(dumps) = dumps {
                delete_left_dumps(&dumps);
            }
            if !allowed() {
                wait_until_allowed();
            }
            if let Some(user) = socket_path().as_deref().and_then(usage::signed_in_user) {
                let _ = USER.set(user);
            }
            *SESSION.locked() = Some(new_session());
            let hub = Hub::main();
            hub.bind_client(Some(Arc::new(Client::with_options(options(
                dsn, process, minidumps,
            )))));
            if let (Some(client), Some(session)) = (hub.client(), SESSION.locked().clone()) {
                client.send_envelope(session.into());
            }
            if log::set_logger(&LOGGER).is_ok() {
                log::set_max_level(log::LevelFilter::Warn);
            }
            send_startup();
            loop {
                send_left_panics();
                if process != Process::Core {
                    break;
                }
                std::thread::sleep(SWEEP_EVERY);
            }
        });
    if let Err(error) = spawned {
        eprintln!("sikemux: crash reports could not start: {error}");
    }
}

fn dumps_dir() -> Option<PathBuf> {
    Some(crate::hang_reports::autopsy_dir()?.join("minidumps"))
}

/// Deletes the dumps of crashes whose reporter ended before reading them.
fn delete_left_dumps(dumps: &Path) {
    let Ok(files) = std::fs::read_dir(dumps) else {
        return;
    };
    for file in files.flatten() {
        let written = file.metadata().and_then(|found| found.modified());
        if written.is_ok_and(|at| at.elapsed().is_ok_and(|age| age > DUMP_LEFT_AFTER)) {
            let _ = std::fs::remove_file(file.path());
        }
    }
}

/// Reads the stack out of a dump and deletes it.
fn take_stack(dump: &Path) -> Option<CrashStack> {
    let memory = std::fs::read(dump);
    let _ = std::fs::remove_file(dump);
    crash_stacks::read(&memory.ok()?)
}

/// Returns once this process may send. What happened before then is never sent.
fn wait_until_allowed() {
    while !allowed() {
        let left = left_panics();
        for (file, report) in &left {
            panic_reports::save(file, &sent(report));
        }
        std::thread::sleep(ASK_AGAIN_EVERY);
    }
    STARTUP.locked().take();
}

fn new_session() -> SessionUpdate<'static> {
    SessionUpdate {
        session_id: Uuid::new_v4(),
        distinct_id: USER.get().cloned(),
        sequence: None,
        timestamp: None,
        started: SystemTime::now(),
        init: true,
        duration: None,
        status: SessionStatus::Ok,
        errors: 0,
        attributes: SessionAttributes {
            release: release(env!("CARGO_PKG_VERSION")).into(),
            environment: Some(environment().into()),
            ip_address: Some(Ipv4Addr::UNSPECIFIED.into()),
            user_agent: None,
        },
    }
}

/// The crash reporter only stops once the process it watches disconnects, so one whose process
/// ended before connecting would run on forever. It ends a couple of seconds after that process,
/// unless it is sending its crash.
#[cfg(unix)]
fn leave_with_parent() {
    let Some(parent) = std::env::var(PARENT_VARIABLE)
        .ok()
        .and_then(|pid| pid.parse::<libc::pid_t>().ok())
    else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("parent-watch".into())
        .spawn(move || {
            let mut gone = 0;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                // SAFETY: getppid has no preconditions.
                if unsafe { libc::getppid() } == parent || SENDING_CRASH.load(Ordering::SeqCst) {
                    gone = 0;
                } else if gone == 1 {
                    std::process::exit(0);
                } else {
                    gone += 1;
                }
            }
        });
}

/// Whether this process is the crash reporter another one started to watch it.
pub fn is_reporter() -> bool {
    std::env::var_os(REPORTER_VARIABLE).is_some()
}

/// Ends this run's session as one that exited well, and sends what is waiting.
pub fn stop() {
    if let Some(client) = Hub::main().client() {
        end_session(&client, SessionStatus::Exited);
        client.flush(Some(FLUSH_TIMEOUT));
    }
}

fn end_session(client: &Client, status: SessionStatus) {
    if let Some(session) = SESSION.locked().take() {
        client.send_envelope(ended(session, status).into());
    }
}

/// In the crash reporter: ends the session of the process that crashed as crashed.
fn send_crashed_session() {
    let Some(session) = crashed_session() else {
        return;
    };
    if let Some(client) = Hub::main().client() {
        client.send_envelope(ended(session, SessionStatus::Crashed).into());
    }
}

/// In the crash reporter: the session of the process it watches.
fn crashed_session() -> Option<SessionUpdate<'static>> {
    serde_json::from_str(&std::env::var(SESSION_VARIABLE).ok()?).ok()
}

fn ended(mut session: SessionUpdate<'static>, status: SessionStatus) -> SessionUpdate<'static> {
    let now = SystemTime::now();
    session.init = false;
    session.timestamp = Some(now);
    session.duration = now
        .duration_since(session.started)
        .ok()
        .map(|duration| duration.as_secs_f64());
    if status == SessionStatus::Crashed {
        session.errors += 1;
    }
    session.status = status;
    session
}

/// Whether a panic written now may go to Sentry, now or once a core next sweeps.
pub(crate) fn wanted() -> bool {
    DSN.is_some() && allowed()
}

/// Sends the panic this process is dying of, and ends its session as crashed.
pub(crate) fn panicked(file: &Path, report: &PanicReport) {
    static ONCE: AtomicBool = AtomicBool::new(false);
    let hub = Hub::main();
    let Some(client) = hub.client() else {
        return;
    };
    if ONCE.swap(true, Ordering::SeqCst) {
        return;
    }
    hub.capture_event(event(report));
    end_session(&client, SessionStatus::Crashed);
    if client.flush(Some(FLUSH_TIMEOUT)) {
        panic_reports::save(file, &sent(report));
    }
    // The abort that follows would send the same panic again as a native crash.
    #[cfg(unix)]
    if cfg!(panic = "abort") {
        // SAFETY: _exit ends the process at once; nothing runs after it.
        unsafe { libc::_exit(101) };
    }
}

/// Keeps an error the process printed, to go along with a crash's report.
pub fn printed_error(line: &str) {
    breadcrumb("stderr", Level::Error, line);
}

/// Notes what happened before a crash, which goes along with its report.
fn breadcrumb(category: &str, level: Level, text: &str) {
    static SECOND: AtomicU64 = AtomicU64::new(0);
    static IN_SECOND: AtomicU64 = AtomicU64::new(0);
    let hub = Hub::main();
    if hub.client().is_none() {
        return;
    }
    let now = sikemux_process::clock::epoch_ms() / 1000;
    if SECOND.swap(now, Ordering::Relaxed) != now {
        IN_SECOND.store(0, Ordering::Relaxed);
    }
    if IN_SECOND.fetch_add(1, Ordering::Relaxed) >= BREADCRUMBS_PER_SECOND || !allowed() {
        return;
    }
    let message = without_paths(text)
        .chars()
        .take(MAX_BREADCRUMB_TEXT)
        .collect();
    let breadcrumb = Breadcrumb {
        category: Some(category.to_owned()),
        level,
        message: Some(message),
        ..Default::default()
    };
    hub.with_integration(|minidumps: &MinidumpIntegration| {
        minidumps.add_breadcrumb(breadcrumb.clone())
    });
    hub.add_breadcrumb(breadcrumb);
}

/// How long the app took from `began` to its window, step by step. One start in ten is sent.
pub fn started(began: SystemTime, steps: Vec<(&'static str, SystemTime, SystemTime)>) {
    if DSN.is_none() {
        return;
    }
    *STARTUP.locked() = Some(Startup { began, steps });
    send_startup();
}

fn send_startup() {
    if Hub::main().client().is_none() {
        return;
    }
    let Some(startup) = STARTUP.locked().take() else {
        return;
    };
    let context = TransactionContext::new("app start", "app.start");
    let transaction = sentry::start_transaction_with_timestamp(context, startup.began);
    let mut finished = startup.began;
    for (step, from, to) in startup.steps {
        transaction
            .start_child_with_details("app.start.step", step, SpanId::default(), from)
            .finish_with_timestamp(to);
        finished = finished.max(to);
    }
    transaction.finish_with_timestamp(finished);
}

fn options(dsn: &str, process: Process, minidumps: MinidumpIntegration) -> ClientOptions {
    crate::install_tls_crypto();
    let mut options = ClientOptions::new();
    options.dsn = dsn.parse().ok();
    options.release = Some(release(env!("CARGO_PKG_VERSION")).into());
    options.environment = Some(environment().into());
    options.send_default_pii = false;
    options.max_breadcrumbs = MAX_BREADCRUMBS;
    options.traces_sampling_strategy =
        sentry::TracesSamplingStrategy::FixedRate(STARTUP_SAMPLE_RATE);
    options.default_integrations = false;
    options.integrations = vec![Arc::new(ContextIntegration::new()), Arc::new(minidumps)];
    options.before_send = Some(Arc::new(move |event| before_send(event, process)));
    options.transport = Some(Arc::new(
        move |options: &ClientOptions| -> Arc<dyn Transport> {
            let Some(transport) = sentry::TransportOptions::try_from_client_options(options)
                .map(ReqwestHttpTransportOptions::from)
            else {
                return Arc::new(Discard);
            };
            let transport = match sikemux_base::http::builder().build() {
                Ok(client) => transport.with_client(client),
                Err(_) => transport,
            };
            Arc::new(WhileAllowed {
                inner: transport.build(),
                process,
            })
        },
    ));
    options
}

/// Drops whatever is ready to leave once the person stops sharing, wherever it came from, and
/// any attachment, which is where a crash's memory would be.
struct WhileAllowed<T> {
    inner: T,
    process: Process,
}

impl<T: Transport> Transport for WhileAllowed<T> {
    fn send_envelope(&self, envelope: sentry::Envelope) {
        if !allowed() {
            return;
        }
        let mut rebuilt = sentry::Envelope::new().with_headers(envelope.headers().clone());
        let mut kept = false;
        for item in envelope.into_items() {
            match item {
                EnvelopeItem::Attachment(_) => continue,
                EnvelopeItem::Transaction(transaction) => {
                    rebuilt.add_item(scrub_transaction(*transaction, self.process));
                    kept = true;
                }
                EnvelopeItem::SessionUpdate(mut session) => {
                    session.attributes.ip_address = Some(Ipv4Addr::UNSPECIFIED.into());
                    rebuilt.add_item(session);
                    kept = true;
                }
                EnvelopeItem::SessionAggregates(mut aggregates) => {
                    aggregates.attributes.ip_address = Some(Ipv4Addr::UNSPECIFIED.into());
                    rebuilt.add_item(aggregates);
                    kept = true;
                }
                other => {
                    rebuilt.add_item(other);
                    kept = true;
                }
            }
        }
        if kept {
            self.inner.send_envelope(rebuilt);
        }
    }

    fn flush(&self, timeout: Duration) -> bool {
        self.inner.flush(timeout)
    }

    fn shutdown(&self, timeout: Duration) -> bool {
        self.inner.shutdown(timeout)
    }
}

struct Discard;

impl Transport for Discard {
    fn send_envelope(&self, _: sentry::Envelope) {}
}

fn before_send(mut event: Event<'static>, process: Process) -> Option<Event<'static>> {
    event.server_name = None;
    let user = event.user.get_or_insert_with(Default::default);
    // An address of its own stops Sentry placing the person by the one the report came from.
    user.ip_address = Some(IpAddress::Exact(Ipv4Addr::UNSPECIFIED.into()));
    if user.id.is_none() {
        user.id = USER.get().cloned();
    }
    let channel = channel(event.release.as_deref().unwrap_or_default());
    for (tag, value) in [
        ("process", process.name()),
        ("channel", channel),
        ("install", install().name()),
    ] {
        event
            .tags
            .entry(tag.to_owned())
            .or_insert_with(|| value.to_owned());
    }
    Some(scrub_json(event))
}

fn scrub_transaction(
    mut transaction: Transaction<'static>,
    process: Process,
) -> Transaction<'static> {
    transaction.server_name = None;
    let user = transaction.user.get_or_insert_with(Default::default);
    user.ip_address = Some(IpAddress::Exact(Ipv4Addr::UNSPECIFIED.into()));
    if user.id.is_none() {
        user.id = USER.get().cloned();
    }
    let channel = channel(transaction.release.as_deref().unwrap_or_default());
    for (tag, value) in [
        ("process", process.name()),
        ("channel", channel),
        ("install", install().name()),
    ] {
        transaction
            .tags
            .entry(tag.to_owned())
            .or_insert_with(|| value.to_owned());
    }
    scrub_json(transaction)
}

fn scrub_json<T: Serialize + DeserializeOwned>(item: T) -> T {
    let home = sikemux_process::paths::home()
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| path.len() >= 2);
    let Ok(mut value) = serde_json::to_value(&item) else {
        return item;
    };
    scrub_value(&mut value, home.as_deref());
    serde_json::from_value(value).unwrap_or(item)
}

fn scrub_value(value: &mut Value, home: Option<&str>) {
    match value {
        Value::String(text) => {
            let cleaned = scrub_pii(text, home);
            if cleaned != *text {
                *text = cleaned;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| scrub_value(item, home)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|field| scrub_value(field, home)),
        _ => {}
    }
}

struct Scrubbers {
    user_home: Regex,
    url_userinfo: Regex,
    url_query: Regex,
    url_origin: Regex,
    email: Regex,
    auth_header: Regex,
    known_token: Regex,
    kv_secret: Regex,
    abs_path: Regex,
}

fn scrubbers() -> &'static Scrubbers {
    static SCRUBBERS: OnceLock<Scrubbers> = OnceLock::new();
    SCRUBBERS.get_or_init(|| {
        #[expect(clippy::expect_used, reason = "compile-time constant regexes")]
        Scrubbers {
            user_home: Regex::new(r#"(?i)(?:/Users/|/home/|C:\\Users\\)[^/\\\s"'<>`]+"#)
                .expect("valid user_home regex"),
            url_userinfo: Regex::new(r#"([a-zA-Z][a-zA-Z0-9+.-]*://)[^\s"'<>`/@]+@"#)
                .expect("valid url_userinfo regex"),
            url_query: Regex::new(r#"([a-zA-Z][a-zA-Z0-9+.-]*://[^\s"'<>`?#]+)[?#][^\s"'<>`]*"#)
                .expect("valid url_query regex"),
            url_origin: Regex::new(
                r#"([a-zA-Z][a-zA-Z0-9+.-]*://)(?:[^\s"'<>`/@]+@)?([^\s"'<>`/?#]+)[^\s"'<>`]*"#,
            )
            .expect("valid url_origin regex"),
            email: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b")
                .expect("valid email regex"),
            auth_header: Regex::new(r"(?i)\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]{8,}")
                .expect("valid auth_header regex"),
            known_token: Regex::new(
                r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{16,}|xox[baprs]-[A-Za-z0-9-]{10,}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}|AIza[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16})\b",
            )
            .expect("valid known_token regex"),
            kv_secret: Regex::new(
                r#"(?i)\b(token|api[_-]?key|secret|password|passwd|access_token|refresh_token|authorization)\s*[:=]\s*[^\s&"',;}\])]+"#,
            )
            .expect("valid kv_secret regex"),
            abs_path: Regex::new(
                r#"(?:~|[A-Za-z]:\\|/(?:Users|home|var|tmp|opt|Library|Volumes|Applications|System|private|etc|mnt|srv|root))(?:[/\\][A-Za-z0-9._+-]+(?: [A-Za-z0-9._+-]+){0,3})*(?:[/\\][^\s"'<>`:,;)\]]+)"#,
            )
            .expect("valid abs_path regex"),
        }
    })
}

fn scrub_pii(text: &str, home: Option<&str>) -> String {
    let mut out = match home {
        Some(home) if text.contains(home) => text.replace(home, "~"),
        _ => text.to_owned(),
    };
    let s = scrubbers();
    out = s.user_home.replace_all(&out, "~").into_owned();
    out = s.url_userinfo.replace_all(&out, "$1").into_owned();
    out = s.url_query.replace_all(&out, "$1").into_owned();
    out = s.email.replace_all(&out, "<email>").into_owned();
    out = s
        .auth_header
        .replace_all(&out, "$1 <redacted>")
        .into_owned();
    out = s.known_token.replace_all(&out, "<token>").into_owned();
    out = s.kv_secret.replace_all(&out, "$1=<redacted>").into_owned();
    out
}

fn is_crate_relative_path(word: &str) -> bool {
    let trimmed =
        word.trim_matches(|ch: char| matches!(ch, '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';'));
    ["crates/", "app/", "helpers/", "mobile/", "server/"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

fn without_paths(text: &str) -> String {
    let home = sikemux_process::paths::home()
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| path.len() >= 2);
    let s = scrubbers();
    let scrubbed = scrub_pii(text, home.as_deref());
    let scrubbed = s.url_origin.replace_all(&scrubbed, "$1$2");
    let scrubbed = s.abs_path.replace_all(&scrubbed, "<path>");
    scrubbed
        .split(' ')
        .map(|word| {
            if word.contains("://") || is_crate_relative_path(word) {
                word
            } else if word.contains(['/', '\\']) {
                "<path>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn allowed() -> bool {
    (DATA_DIR.get().cloned())
        .or_else(crate::login_item::data_dir)
        .is_some_and(|data_dir| {
            Sharing::saved(install(), &data_dir, socket_path().as_deref()) == Sharing::On
        })
}

fn socket_path() -> Option<PathBuf> {
    (SOCKET.get().cloned()).or_else(sikemux_corekit::default_socket_path)
}

fn environment() -> &'static str {
    if [Install::App, Install::Host]
        .into_iter()
        .any(Install::published)
    {
        "production"
    } else {
        "dev"
    }
}

fn install() -> Install {
    if Install::Host.published() {
        Install::Host
    } else {
        Install::App
    }
}

fn release(version: &str) -> String {
    format!("sikemux@{version}")
}

fn channel(release: &str) -> &'static str {
    if release.contains("-nightly.") {
        "nightly"
    } else {
        "stable"
    }
}

/// The panics no process has sent yet: those the core and the CLI died of, and any whose
/// sending did not finish.
fn left_panics() -> Vec<(PathBuf, PanicReport)> {
    let Some(directory) = crate::hang_reports::autopsy_dir() else {
        return Vec::new();
    };
    panic_reports::list_reports(&directory)
        .into_iter()
        .filter(|(_, report)| report.pending)
        .collect()
}

fn send_left_panics() {
    let Some(client) = Hub::main().client() else {
        return;
    };
    let left = left_panics();
    if left.is_empty() {
        return;
    }
    for (_, report) in &left {
        client.capture_event(event(report), None);
    }
    if client.flush(Some(FLUSH_TIMEOUT)) {
        for (file, report) in &left {
            panic_reports::save(file, &sent(report));
        }
    }
}

fn sent(report: &PanicReport) -> PanicReport {
    PanicReport {
        pending: false,
        ..report.clone()
    }
}

/// The report as Sentry reads a native crash, so it names the frames from the release's debug
/// files. The same report always makes the same event, which Sentry keeps once.
fn event(report: &PanicReport) -> Event<'static> {
    let frames = (report.frames.iter())
        .filter_map(|frame| u64::from_str_radix(frame.trim_start_matches("0x"), 16).ok());
    let identity = Sha256::digest(format!(
        "{}|{}|{}|{}",
        report.process, report.captured_at_ms, report.thread, report.message
    ));
    let release = release(&report.version);
    let mut event = Event {
        event_id: identity
            .first_chunk()
            .map_or_else(Uuid::new_v4, |bytes| Uuid::from_bytes(*bytes)),
        level: Level::Fatal,
        platform: "native".into(),
        timestamp: UNIX_EPOCH + Duration::from_millis(report.captured_at_ms),
        exception: vec![Exception {
            ty: "panic".into(),
            value: Some(without_paths(&report.message)),
            mechanism: Some(Mechanism {
                ty: "panic".into(),
                handled: Some(false),
                ..Default::default()
            }),
            stacktrace: Some(stacktrace(frames)),
            ..Default::default()
        }]
        .into(),
        debug_meta: debug_meta(&report.images),
        ..Default::default()
    };
    event.tags.insert("process".into(), report.process.clone());
    event
        .tags
        .insert("channel".into(), channel(&release).to_owned());
    event.tags.insert("commit".into(), report.commit.clone());
    event
        .extra
        .insert("location".into(), report.location.clone().into());
    event
        .extra
        .insert("thread".into(), report.thread.clone().into());
    event.release = Some(release.into());
    event.transaction = Some(report.location.clone());
    // A panic inside a library has no frame of ours but the panic hook's, so without its place
    // Sentry files every such panic under one issue.
    event.fingerprint = vec!["{{ default }}".into(), panic_site(&report.location).into()].into();
    event
}

/// A native crash as the addresses its thread was at, which Sentry names from the release's debug
/// files.
fn native_crash(mut event: Event<'static>, stack: Option<&CrashStack>) -> Event<'static> {
    event.platform = "native".into();
    event.exception = vec![Exception {
        ty: stack
            .map_or("native crash", |stack| &stack.reason)
            .to_owned(),
        mechanism: Some(Mechanism {
            // Sentry leaves an event of type "minidump" unread when no dump comes with it.
            ty: "signalhandler".into(),
            handled: Some(false),
            ..Default::default()
        }),
        stacktrace: stack.map(|stack| stacktrace(stack.addresses.iter().copied())),
        ..Default::default()
    }]
    .into();
    if let Some(stack) = stack {
        event.debug_meta = debug_meta(&stack.images);
    }
    event
}

fn stacktrace(innermost_first: impl DoubleEndedIterator<Item = u64>) -> Stacktrace {
    Stacktrace {
        frames: innermost_first
            .rev()
            .map(|address| Frame {
                instruction_addr: Some(address.into()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

fn debug_meta(images: &[DebugImage]) -> Cow<'static, DebugMeta> {
    Cow::Owned(DebugMeta {
        images: images.to_vec(),
        ..Default::default()
    })
}

/// Where a panic was raised, from its crate on, so the same line groups alike across machines and
/// toolchains.
fn panic_site(location: &str) -> String {
    let Some((folders, file)) = location.rsplit_once("/src/") else {
        return location.to_owned();
    };
    let krate = folders.rsplit('/').next().unwrap_or(folders);
    format!("{krate}/src/{file}")
}

static LOGGER: Breadcrumbs = Breadcrumbs;

/// Warnings and errors that libraries log through `log` or `tracing`, kept as breadcrumbs.
struct Breadcrumbs;

impl log::Log for Breadcrumbs {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => Level::Error,
            _ => Level::Warning,
        };
        breadcrumb(record.target(), level, &record.args().to_string());
    }

    fn flush(&self) {}
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use sikemux_process::command::{Child, Command, Stdio};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::time::Instant;

    use super::*;

    const TEST: &str = "crash_reports::tests::crashes_under_the_crash_handler_end";
    const ROLE: &str = "SIKEMUX_TEST_CRASH_ROLE";
    const PORT: &str = "SIKEMUX_TEST_CRASH_PORT";
    const IN_MEMORY: &[u8] = b"what the memory held when it crashed";

    /// A core with crash reports on, and the programs it starts, which inherit its handler. Each
    /// crash must end its process: the core's own after sending its stack and none of its memory. Reports go to a
    /// listener in this test, never to Sentry.
    #[test]
    fn crashes_under_the_crash_handler_end() {
        match std::env::var(ROLE).as_deref() {
            Ok("core") => core(),
            Ok("overflow") => overflow_a_tokio_worker(),
            Ok("segfault") => segfault(),
            _ => {}
        }
        let home = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let envelopes = Arc::new(Mutex::new(Vec::new()));
        let received = envelopes.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(body) = answer(stream) {
                    received.locked().push(body);
                }
            }
        });

        let mut core = test_process("core")
            .env("HOME", home.path())
            .env(PORT, port.to_string())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let ended = wait_for(&mut core, Duration::from_secs(60));
        let mut printed = String::new();
        core.stdout
            .take()
            .unwrap()
            .read_to_string(&mut printed)
            .unwrap();
        for role in ["overflow", "segfault"] {
            assert!(
                printed.contains(&format!("{role} ended by a signal")),
                "a program the core started did not end of its {role}:\n{printed}"
            );
        }
        assert!(
            ended.is_some_and(|status| !status.success()),
            "the core did not end of its own stack overflow: {ended:?}\n{printed}"
        );

        let crash_sent = || {
            let crash = |body: &&Vec<u8>| {
                contains(body, b"instruction_addr") && contains(body, b"\"process\":\"core\"")
            };
            envelopes.locked().iter().any(|body| crash(&body))
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while !crash_sent() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(crash_sent(), "the core's crash sent no stack");
        for memory in [IN_MEMORY, b"MDMP", b"event.minidump"] {
            assert!(
                !envelopes.locked().iter().any(|body| contains(body, memory)),
                "the core's crash sent its memory"
            );
        }
    }

    fn core() -> ! {
        clear_inherited_handlers();
        let home = sikemux_process::paths::home().unwrap();
        std::fs::create_dir_all(home.join("data")).unwrap();
        std::fs::write(
            home.join("data/preferences.json"),
            r#"{"onboardingComplete":true}"#,
        )
        .unwrap();
        let dsn = format!("http://public@127.0.0.1:{}/1", std::env::var(PORT).unwrap());
        start_sending_to(
            dsn.leak(),
            Process::Core,
            Some(home.join("data")),
            Some(home.join("core.sock")),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while Hub::main().client().is_none() {
            assert!(Instant::now() < deadline, "crash reports never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        for role in ["overflow", "segfault"] {
            let mut child = test_process(role)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            match wait_for(&mut child, Duration::from_secs(10)) {
                Some(status) if status.signal().is_some() => println!("{role} ended by a signal"),
                other => println!("{role} did not end by a signal: {other:?}"),
            }
            std::io::stdout().flush().unwrap();
        }
        overflow_a_tokio_worker()
    }

    /// Whatever started this test, such as a Sikemux already running, may have a crash handler
    /// these processes would inherit. A core started at login inherits none.
    fn clear_inherited_handlers() {
        unsafe extern "C" {
            static mach_task_self_: u32;
            fn task_set_exception_ports(
                task: u32,
                mask: u32,
                port: u32,
                behavior: i32,
                flavor: i32,
            ) -> i32;
        }
        const ALL_EXCEPTIONS: u32 = 0x1ffe;
        const DEFAULT_BEHAVIOR: i32 = 1;
        // SAFETY: sets this task's exception ports to none, which takes no memory of ours.
        let result = unsafe {
            task_set_exception_ports(mach_task_self_, ALL_EXCEPTIONS, 0, DEFAULT_BEHAVIOR, 0)
        };
        assert_eq!(result, 0);
    }

    fn overflow_a_tokio_worker() -> ! {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .unwrap()
            .block_on(async {
                let _ = tokio::spawn(async { deep(u64::MAX) }).await;
            });
        panic!("the stack did not overflow")
    }

    #[inline(never)]
    fn deep(depth: u64) -> u64 {
        let mut frame = [depth as u8; 8192];
        frame[8..][..IN_MEMORY.len()].copy_from_slice(IN_MEMORY);
        let frame = std::hint::black_box(frame);
        if depth == 0 {
            0
        } else {
            deep(depth - 1) + u64::from(frame[7])
        }
    }

    fn segfault() -> ! {
        // SAFETY: this process is meant to crash here, and nothing runs after the write.
        unsafe { std::ptr::null_mut::<u8>().write_volatile(1) };
        panic!("writing to null did not crash")
    }

    fn test_process(role: &str) -> Command {
        let mut command =
            sikemux_process::user_environment::command(std::env::current_exe().unwrap());
        command
            .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(ROLE, role);
        for name in ["HOME", PORT] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
    }

    fn wait_for(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        None
    }

    /// Reads one request and answers it as Sentry would, returning its body.
    fn answer(mut stream: TcpStream) -> Option<Vec<u8>> {
        let mut request = Vec::new();
        let mut chunk = [0; 8192];
        let head_end = loop {
            let read = stream.read(&mut chunk).ok().filter(|read| *read > 0)?;
            request.extend_from_slice(&chunk[..read]);
            if let Some(at) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&request[..head_end]).to_lowercase();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse().ok())?;
        while request.len() < head_end + length {
            let read = stream.read(&mut chunk).ok().filter(|read| *read > 0)?;
            request.extend_from_slice(&chunk[..read]);
        }
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 2\r\n\r\n{}");
        Some(request.split_off(head_end))
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
}
