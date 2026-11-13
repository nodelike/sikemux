//! Sends panics, native crashes, release health and how long the app took to start to Sentry.
//! Only builds the release workflow publishes know where to send, and they send only while the
//! person shares usage data and `DO_NOT_TRACK` is unset. Nothing a person wrote or opened goes
//! along, their home folder is written `~`, and they are named only by the account hash the
//! usage ping sends.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sentry::integrations::contexts::ContextIntegration;
use sentry::integrations::minidump::MinidumpIntegration;
use sentry::protocol::{
    Breadcrumb, DebugMeta, Event, Exception, Frame, IpAddress, Mechanism, SessionAttributes,
    SessionStatus, SessionUpdate, SpanId, Stacktrace,
};
use sentry::transports::ReqwestHttpTransportOptions;
use sentry::types::Uuid;
use sentry::{Client, ClientOptions, Hub, Level, TransactionContext, Transport};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sikemux_core::server::usage::{self, Install};
use sikemux_process::sync::Locked;

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
/// How often the core looks for panics that other processes left behind.
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
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
    let mut minidumps = MinidumpIntegration::new()
        .server_env_var(REPORTER_VARIABLE)
        .process_name(format!("sikemux {} crash reporter", process.name()))
        .flush_timeout(FLUSH_TIMEOUT)
        .before_capture(|_, _| {
            SENDING_CRASH.store(true, Ordering::SeqCst);
            send_crashed_session();
        })
        .on_process(|command| {
            command.env(PARENT_VARIABLE, std::process::id().to_string());
            if let Some(session) = SESSION.locked().as_ref() {
                if let Ok(session) = serde_json::to_string(session) {
                    command.env(SESSION_VARIABLE, session);
                }
            }
        });
    if let Some(autopsies) = crate::hang_reports::autopsy_dir() {
        minidumps = minidumps.crashes_dir(autopsies.join("minidumps"));
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
    if !allowed() {
        return;
    }
    *SESSION.locked() = Some(SessionUpdate {
        session_id: Uuid::new_v4(),
        distinct_id: None,
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
            ip_address: None,
            user_agent: None,
        },
    });
    let spawned = std::thread::Builder::new()
        .name("crash-reports".into())
        .spawn(move || {
            if let Some(user) = socket
                .or_else(sikemux_core::default_socket_path)
                .as_deref()
                .and_then(usage::signed_in_user)
            {
                if let Some(session) = SESSION.locked().as_mut() {
                    session.distinct_id = Some(user.clone());
                }
                let _ = USER.set(user);
            }
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
    if IN_SECOND.fetch_add(1, Ordering::Relaxed) >= BREADCRUMBS_PER_SECOND {
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
    options.transport = Some(Arc::new(|options: &ClientOptions| -> Arc<dyn Transport> {
        let Some(transport) = sentry::TransportOptions::try_from_client_options(options)
            .map(ReqwestHttpTransportOptions::from)
        else {
            return Arc::new(Discard);
        };
        let transport = match sikemux_core::http::builder().build() {
            Ok(client) => transport.with_client(client),
            Err(_) => transport,
        };
        Arc::new(transport.build())
    }));
    options
}

struct Discard;

impl Transport for Discard {
    fn send_envelope(&self, _: sentry::Envelope) {}
}

fn before_send(mut event: Event<'static>, process: Process) -> Option<Event<'static>> {
    if !allowed() {
        return None;
    }
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
        ("install", install()),
    ] {
        event
            .tags
            .entry(tag.to_owned())
            .or_insert_with(|| value.to_owned());
    }
    Some(without_home(event))
}

/// Writes the person's home folder as `~` everywhere in the event, so no path names them.
fn without_home(event: Event<'static>) -> Event<'static> {
    let Some(home) = sikemux_process::paths::home() else {
        return event;
    };
    let home = home.to_string_lossy();
    if home.len() < 2 {
        return event;
    }
    let Ok(mut value) = serde_json::to_value(&event) else {
        return event;
    };
    replace_home(&mut value, &home);
    serde_json::from_value(value).unwrap_or(event)
}

fn replace_home(value: &mut Value, home: &str) {
    match value {
        Value::String(text) if text.contains(home) => *text = text.replace(home, "~"),
        Value::Array(items) => items.iter_mut().for_each(|item| replace_home(item, home)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|field| replace_home(field, home)),
        _ => {}
    }
}

/// Writes `<path>` for each word holding a path or an address, which could name a project.
fn without_paths(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            if word.contains(['/', '\\']) {
                "<path>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn allowed() -> bool {
    !usage::do_not_track() && shares_usage()
}

fn shares_usage() -> bool {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    struct Shared {
        share_usage_data: bool,
    }
    impl Default for Shared {
        fn default() -> Self {
            Self {
                share_usage_data: true,
            }
        }
    }
    let Some(data_dir) = DATA_DIR.get().cloned().or_else(crate::login_item::data_dir) else {
        return true;
    };
    match std::fs::read(data_dir.join("preferences.json")) {
        Ok(bytes) => serde_json::from_slice::<Shared>(&bytes)
            .is_ok_and(|preferences| preferences.share_usage_data),
        Err(_) => true,
    }
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

fn install() -> &'static str {
    if Install::Host.published() {
        "host"
    } else {
        "app"
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

/// Sends the panics no process has sent yet: those the core and the CLI died of, and any whose
/// sending did not finish.
fn send_left_panics() {
    let Some(client) = Hub::main().client() else {
        return;
    };
    let Some(directory) = crate::hang_reports::autopsy_dir() else {
        return;
    };
    let left: Vec<(PathBuf, PanicReport)> = panic_reports::list_reports(&directory)
        .into_iter()
        .filter(|(_, report)| report.pending)
        .collect();
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
    let frames = report
        .frames
        .iter()
        .rev()
        .filter_map(|frame| u64::from_str_radix(frame.trim_start_matches("0x"), 16).ok())
        .map(|address| Frame {
            instruction_addr: Some(address.into()),
            ..Default::default()
        })
        .collect();
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
            stacktrace: Some(Stacktrace {
                frames,
                ..Default::default()
            }),
            ..Default::default()
        }]
        .into(),
        debug_meta: std::borrow::Cow::Owned(DebugMeta {
            images: report.images.clone(),
            ..Default::default()
        }),
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
    event
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

#[cfg(all(test, unix))]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::time::Instant;

    use super::*;

    const TEST: &str = "crash_reports::tests::crashes_under_the_crash_handler_end";
    const ROLE: &str = "SIKEMUX_TEST_CRASH_ROLE";
    const PORT: &str = "SIKEMUX_TEST_CRASH_PORT";

    /// A core with crash reports on, and the programs it starts, which inherit its handler. Each
    /// crash must end its process: the core's own after writing its minidump. Reports go to a
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

        let minidump_sent = || {
            envelopes.locked().iter().any(|body| {
                contains(body, b"event.minidump") && contains(body, b"\"process\":\"core\"")
            })
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while !minidump_sent() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(minidump_sent(), "the core's crash sent no minidump");
    }

    fn core() -> ! {
        clear_inherited_handlers();
        let home = sikemux_process::paths::home().unwrap();
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
            match wait_for(&mut child, Duration::from_secs(5)) {
                Some(status) if status.signal().is_some() => println!("{role} ended by a signal"),
                other => println!("{role} did not end by a signal: {other:?}"),
            }
            std::io::stdout().flush().unwrap();
        }
        overflow_a_tokio_worker()
    }

    /// Whatever started this test, such as a Sikemux already running, may have a crash handler
    /// these processes would inherit. A core started at login inherits none.
    #[cfg(target_os = "macos")]
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

    #[cfg(not(target_os = "macos"))]
    fn clear_inherited_handlers() {}

    fn overflow_a_tokio_worker() -> ! {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .unwrap()
            .block_on(async {
                let _ = tokio::spawn(async { deep(u64::MAX) }).await;
            });
        unreachable!("the stack did not overflow")
    }

    #[inline(never)]
    fn deep(depth: u64) -> u64 {
        let frame = std::hint::black_box([depth as u8; 8192]);
        if depth == 0 {
            0
        } else {
            deep(depth - 1) + u64::from(frame[7])
        }
    }

    fn segfault() -> ! {
        // SAFETY: this process is meant to crash here, and nothing runs after the write.
        unsafe { std::ptr::null_mut::<u8>().write_volatile(1) };
        unreachable!("writing to null did not crash")
    }

    fn test_process(role: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
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
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}");
        Some(request.split_off(head_end))
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
}
