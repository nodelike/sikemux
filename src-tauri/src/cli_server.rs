use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::cli_protocol::{
    CliClientCommand, CliClientHello, CliCloseReason, CliEndpointDescriptor, CliFrontendRequest,
    CliOpenFailure, CliOpenRequest, CliOpenResult, CliServerResponse, CliTargetKind,
    CLI_PROTOCOL_VERSION, MAX_CLI_FRAME_BYTES, MAX_CLI_RESPONSE_BYTES, MAX_CLI_TARGETS,
};
use crate::error::{AppError, AppResult};

const CLI_EVENT: &str = "cli-open-available";
const FRONTEND_ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_READ_DEADLINE: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 128;

#[derive(Clone)]
pub struct CliBroker {
    inner: Arc<CliBrokerInner>,
}

/// Always-managed wrapper so an isolated CLI startup failure never prevents
/// the desktop workspace (or unrelated Tauri commands) from running.
#[derive(Default)]
pub struct CliBrokerState(pub Option<CliBroker>);

struct CliBrokerInner {
    app: AppHandle,
    endpoint_path: PathBuf,
    descriptor: CliEndpointDescriptor,
    requests: Mutex<HashMap<String, RequestEntry>>,
    harness: crate::harness::HarnessBroker,
    stopping: AtomicBool,
    connections: AtomicUsize,
}

struct RequestEntry {
    request: CliOpenRequest,
    dispatched: bool,
    pending_targets: HashSet<String>,
    opened_targets: Vec<String>,
    failed_targets: Vec<CliOpenFailure>,
    open_tabs: HashMap<String, (String, String)>,
    closed_before_ack: HashSet<(String, String)>,
    accepted: Option<mpsc::Sender<AcceptedResult>>,
    closed: Option<mpsc::Sender<CliCloseReason>>,
}

#[derive(Debug, Clone)]
struct AcceptedResult {
    opened: Vec<String>,
    failed: Vec<CliOpenFailure>,
}

impl CliBroker {
    pub fn start(app: AppHandle) -> AppResult<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let endpoint_path = cli_endpoint_path()
            .ok_or_else(|| AppError::State("no home directory for CLI endpoint".into()))?;
        let descriptor = CliEndpointDescriptor {
            protocol: CLI_PROTOCOL_VERSION,
            pid: std::process::id(),
            port,
            token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            version: env!("CARGO_PKG_VERSION").into(),
        };
        write_endpoint(&endpoint_path, &descriptor)?;

        let broker = Self {
            inner: Arc::new(CliBrokerInner {
                app,
                endpoint_path,
                descriptor,
                requests: Mutex::new(HashMap::new()),
                harness: crate::harness::HarnessBroker::default(),
                stopping: AtomicBool::new(false),
                connections: AtomicUsize::new(0),
            }),
        };
        let serving = broker.clone();
        thread::Builder::new()
            .name("sikemux-cli-listener".into())
            .spawn(move || serving.listen(listener))?;
        Ok(broker)
    }

    pub fn harness(&self) -> &crate::harness::HarnessBroker {
        &self.inner.harness
    }

    pub fn endpoint_path(&self) -> &Path {
        &self.inner.endpoint_path
    }

    pub fn cli_executable(&self) -> Option<PathBuf> {
        cli_executable_path()
    }

    /// Blocks until someone connects. Asking the socket 25 times a second
    /// whether anyone had arrived yet woke the process all day for nothing;
    /// `shutdown` connects to the port once to release this.
    fn listen(&self, listener: TcpListener) {
        while !self.inner.stopping.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if self.inner.stopping.load(Ordering::Acquire) {
                        return;
                    }
                    let Some(slot) = ConnectionSlot::claim(&self.inner) else {
                        continue;
                    };
                    let broker = self.clone();
                    let _ = thread::Builder::new()
                        .name("sikemux-cli-client".into())
                        .spawn(move || {
                            broker.serve(stream);
                            drop(slot);
                        });
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    fn serve(&self, mut stream: TcpStream) {
        if configure_client_stream(&stream).is_err() {
            return;
        }
        let cloned = match stream.try_clone() {
            Ok(value) => value,
            Err(_) => return,
        };
        let mut reader = BufReader::new(DeadlineReader {
            stream: cloned,
            deadline: Instant::now() + REQUEST_READ_DEADLINE,
        });
        let hello = match read_frame::<CliClientHello>(&mut reader) {
            Ok(CliClientHello::Hello { protocol, nonce }) => {
                if protocol != CLI_PROTOCOL_VERSION {
                    Err(format!(
                        "CLI protocol mismatch (client {protocol}, app {CLI_PROTOCOL_VERSION}); update or restart Sikemux"
                    ))
                } else if nonce.is_empty() || nonce.len() > 256 {
                    Err("invalid CLI hello".into())
                } else {
                    Ok(CliServerResponse::Hello {
                        proof: crate::cli_auth::server_proof(
                            &self.inner.descriptor.token,
                            self.inner.descriptor.port,
                            &nonce,
                        ),
                    })
                }
            }
            Err(message) => Err(message),
        };
        match hello {
            Ok(response) => {
                if write_response(&mut stream, &response).is_err() {
                    return;
                }
            }
            Err(message) => {
                let _ = write_response(&mut stream, &CliServerResponse::Error { message });
                return;
            }
        }
        let command = match read_frame::<CliClientCommand>(&mut reader) {
            Ok(value) => value,
            Err(message) => {
                let _ = write_response(&mut stream, &CliServerResponse::Error { message });
                return;
            }
        };

        match command {
            CliClientCommand::Harness {
                protocol,
                token,
                request,
            } => {
                let focus = request.method == "ui.open"
                    && request
                        .params
                        .get("focus")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true);
                let result = self.authenticate(protocol, &token).and_then(|_| {
                    crate::harness::execute(&self.inner.app, &self.inner.harness, request)
                });
                if focus && result.is_ok() {
                    if let Some(window) = self.inner.app.get_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
                let response = match result {
                    Ok(value) => CliServerResponse::Result { value },
                    Err(message) => CliServerResponse::Error { message },
                };
                let _ = write_response(&mut stream, &response);
            }
            CliClientCommand::Ping { protocol, token } => {
                if let Err(message) = self.authenticate(protocol, &token) {
                    let _ = write_response(&mut stream, &CliServerResponse::Error { message });
                    return;
                }
                let _ = write_response(
                    &mut stream,
                    &CliServerResponse::Pong {
                        protocol: CLI_PROTOCOL_VERSION,
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                );
            }
            CliClientCommand::Open {
                protocol,
                token,
                request,
            } => {
                if let Err(message) = self
                    .authenticate(protocol, &token)
                    .and_then(|_| validate_request(&request))
                {
                    let _ = write_response(&mut stream, &CliServerResponse::Error { message });
                    return;
                }
                let request_id = request.id.clone();
                let wait = request.wait;
                let (accepted_rx, closed_rx) = match self.enqueue(request) {
                    Ok(value) => value,
                    Err(message) => {
                        let _ = write_response(&mut stream, &CliServerResponse::Error { message });
                        return;
                    }
                };
                let accepted = match accepted_rx.recv_timeout(FRONTEND_ACCEPT_TIMEOUT) {
                    Ok(value) => value,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        self.cancel(&request_id);
                        let _ = write_response(
                            &mut stream,
                            &CliServerResponse::Error {
                                message:
                                    "Sikemux did not finish opening the request within 60 seconds"
                                        .into(),
                            },
                        );
                        return;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = write_response(
                            &mut stream,
                            &CliServerResponse::Error {
                                message: "Sikemux closed before the editor accepted the request"
                                    .into(),
                            },
                        );
                        return;
                    }
                };
                if write_response(
                    &mut stream,
                    &CliServerResponse::Accepted {
                        request_id: request_id.clone(),
                        opened: accepted.opened.clone(),
                        failed: accepted.failed,
                    },
                )
                .is_err()
                {
                    self.cancel(&request_id);
                    return;
                }
                if wait {
                    let reason = closed_rx
                        .and_then(|receiver| receiver.recv().ok())
                        .unwrap_or(CliCloseReason::AppExit);
                    let _ = write_response(
                        &mut stream,
                        &CliServerResponse::Closed { request_id, reason },
                    );
                }
            }
        }
    }

    fn authenticate(&self, protocol: u16, token: &str) -> Result<(), String> {
        if protocol != CLI_PROTOCOL_VERSION {
            return Err(format!(
                "CLI protocol mismatch (client {protocol}, app {CLI_PROTOCOL_VERSION}); update or restart Sikemux"
            ));
        }
        if !crate::cli_auth::same_secret(token, &self.inner.descriptor.token) {
            return Err("CLI authentication failed".into());
        }
        Ok(())
    }

    fn enqueue(
        &self,
        request: CliOpenRequest,
    ) -> Result<
        (
            mpsc::Receiver<AcceptedResult>,
            Option<mpsc::Receiver<CliCloseReason>>,
        ),
        String,
    > {
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (closed_tx, closed_rx) = mpsc::channel();
        let mut requests = self
            .inner
            .requests
            .lock()
            .map_err(|_| "CLI broker lock poisoned")?;
        if requests.contains_key(&request.id) {
            return Err("duplicate CLI request id".into());
        }
        let pending_targets = request
            .targets
            .iter()
            .map(|target| target.id.clone())
            .collect();
        let wait = request.wait;
        requests.insert(
            request.id.clone(),
            RequestEntry {
                request: request.clone(),
                dispatched: false,
                pending_targets,
                opened_targets: Vec::new(),
                failed_targets: Vec::new(),
                open_tabs: HashMap::new(),
                closed_before_ack: HashSet::new(),
                accepted: Some(accepted_tx),
                closed: wait.then_some(closed_tx),
            },
        );
        drop(requests);

        if let Some(window) = self.inner.app.get_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        let _ = self.inner.app.emit_to("main", CLI_EVENT, request.id);
        Ok((accepted_rx, wait.then_some(closed_rx)))
    }

    fn claim(&self, reset: bool) -> Vec<CliFrontendRequest> {
        let Ok(mut requests) = self.inner.requests.lock() else {
            return Vec::new();
        };
        if reset {
            for entry in requests.values_mut() {
                if !entry.pending_targets.is_empty() {
                    entry.dispatched = false;
                }
            }
        }
        requests
            .values_mut()
            .filter_map(|entry| {
                if entry.dispatched || entry.pending_targets.is_empty() {
                    return None;
                }
                entry.dispatched = true;
                let mut request = entry.request.clone();
                request
                    .targets
                    .retain(|target| entry.pending_targets.contains(&target.id));
                Some(CliFrontendRequest { request })
            })
            .collect()
    }

    fn mark_result(&self, result: CliOpenResult) {
        let Ok(mut requests) = self.inner.requests.lock() else {
            return;
        };
        let mut remove = false;
        let Some(entry) = requests.get_mut(&result.request_id) else {
            return;
        };
        let Some(target) = entry
            .request
            .targets
            .iter()
            .find(|target| target.id == result.target_id)
        else {
            return;
        };
        let target_kind = target.kind;
        let target_path = target.path.clone();
        if !entry.pending_targets.remove(&result.target_id) {
            return;
        }
        if result.path != target_path {
            entry.failed_targets.push(CliOpenFailure {
                target_id: result.target_id,
                message: "Sikemux reported the wrong path for this CLI target".into(),
            });
        } else if let Some(message) = result.error {
            entry.failed_targets.push(CliOpenFailure {
                target_id: result.target_id,
                message,
            });
        } else if target_kind == CliTargetKind::File && result.pane_id.is_none() {
            entry.failed_targets.push(CliOpenFailure {
                target_id: result.target_id,
                message: "Sikemux did not attach this file to an editor pane".into(),
            });
        } else {
            entry.opened_targets.push(result.target_id.clone());
            if let Some(pane_id) = result.pane_id {
                let closed_key = (pane_id.clone(), result.path.clone());
                if !entry.closed_before_ack.remove(&closed_key) {
                    entry
                        .open_tabs
                        .insert(result.target_id, (pane_id, result.path));
                }
            }
        }
        if entry.pending_targets.is_empty() {
            if let Some(sender) = entry.accepted.take() {
                let _ = sender.send(AcceptedResult {
                    opened: entry.opened_targets.clone(),
                    failed: entry.failed_targets.clone(),
                });
            }
            if entry.request.wait && entry.open_tabs.is_empty() {
                if let Some(sender) = entry.closed.take() {
                    let _ = sender.send(CliCloseReason::TabsClosed);
                }
                remove = true;
            } else {
                remove = !entry.request.wait;
            }
        }
        if remove {
            requests.remove(&result.request_id);
        }
    }

    fn tabs_closed(&self, pane_id: &str, paths: &[String]) {
        let closed: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let Ok(mut requests) = self.inner.requests.lock() else {
            return;
        };
        let mut completed = Vec::new();
        for (request_id, entry) in requests.iter_mut() {
            for target in &entry.request.targets {
                if entry.pending_targets.contains(&target.id)
                    && closed.contains(target.path.as_str())
                {
                    entry
                        .closed_before_ack
                        .insert((pane_id.to_string(), target.path.clone()));
                }
            }
            entry.open_tabs.retain(|_, (candidate_pane, path)| {
                candidate_pane != pane_id || !closed.contains(path.as_str())
            });
            if entry.request.wait && entry.pending_targets.is_empty() && entry.open_tabs.is_empty()
            {
                if let Some(sender) = entry.closed.take() {
                    let _ = sender.send(CliCloseReason::TabsClosed);
                }
                completed.push(request_id.clone());
            }
        }
        for request_id in completed {
            requests.remove(&request_id);
        }
    }

    fn cancel(&self, request_id: &str) {
        if let Ok(mut requests) = self.inner.requests.lock() {
            requests.remove(request_id);
        }
    }

    pub fn shutdown(&self) {
        if self.inner.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        // Wake the listener thread so it sees the flag and leaves.
        let _ = TcpStream::connect(SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            self.inner.descriptor.port,
        ));
        if let Ok(mut requests) = self.inner.requests.lock() {
            for entry in requests.values_mut() {
                if let Some(sender) = entry.accepted.take() {
                    let failed = entry
                        .pending_targets
                        .iter()
                        .map(|target_id| CliOpenFailure {
                            target_id: target_id.clone(),
                            message: "Sikemux exited before opening this target".into(),
                        })
                        .collect();
                    let _ = sender.send(AcceptedResult {
                        opened: entry.opened_targets.clone(),
                        failed,
                    });
                }
                if let Some(sender) = entry.closed.take() {
                    let _ = sender.send(CliCloseReason::AppExit);
                }
            }
            requests.clear();
        }
        self.inner.harness.shutdown();
        remove_owned_endpoint(&self.inner.endpoint_path, &self.inner.descriptor.token);
    }
}

fn read_frame<T: serde::de::DeserializeOwned>(
    reader: &mut BufReader<DeadlineReader>,
) -> Result<T, String> {
    let mut frame = Vec::new();
    reader
        .by_ref()
        .take(MAX_CLI_FRAME_BYTES + 1)
        .read_until(b'\n', &mut frame)
        .map_err(|_| "could not read the CLI request")?;
    if frame.len() as u64 > MAX_CLI_FRAME_BYTES {
        return Err("CLI request is too large".into());
    }
    serde_json::from_slice(&frame).map_err(|_| "invalid CLI request".into())
}

struct ConnectionSlot(Arc<CliBrokerInner>);

impl ConnectionSlot {
    fn claim(inner: &Arc<CliBrokerInner>) -> Option<Self> {
        inner
            .connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |open| {
                (open < MAX_CONNECTIONS).then_some(open + 1)
            })
            .ok()
            .map(|_| Self(inner.clone()))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Reads until one deadline for the whole request, so a client sending a byte
/// every few seconds cannot keep its thread forever.
struct DeadlineReader {
    stream: TcpStream,
    deadline: Instant,
}

impl Read for DeadlineReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buffer)
    }
}

fn configure_client_stream(stream: &TcpStream) -> std::io::Result<()> {
    // The listening socket is nonblocking so the broker thread can observe
    // shutdown promptly. Accepted sockets can inherit that flag on supported
    // platforms; a larger JSON frame may then lose a race and make the first
    // read return WouldBlock before the client finishes writing. Each client
    // already has its own thread, so restore blocking reads with a hard timeout.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))
}

fn validate_request(request: &CliOpenRequest) -> Result<(), String> {
    if request.id.trim().is_empty() || request.targets.is_empty() {
        return Err("CLI open request has no targets".into());
    }
    if !Path::new(&request.cwd).is_absolute() {
        return Err("CLI working directory must be an absolute path".into());
    }
    if request.targets.len() > MAX_CLI_TARGETS {
        return Err(format!(
            "CLI open request exceeds {MAX_CLI_TARGETS} targets"
        ));
    }
    let mut ids = HashSet::new();
    for target in &request.targets {
        if target.id.trim().is_empty() {
            return Err("CLI open request has an empty target id".into());
        }
        if !ids.insert(&target.id) {
            return Err("CLI open request has duplicate target ids".into());
        }
        if !Path::new(&target.path).is_absolute() || !Path::new(&target.project_root).is_absolute()
        {
            return Err("CLI targets and project roots must be absolute paths".into());
        }
        if target.kind == CliTargetKind::Directory
            && (target.line.is_some() || target.column.is_some())
        {
            return Err("directory targets cannot include a line or column".into());
        }
    }
    if request.wait
        && request
            .targets
            .iter()
            .all(|target| target.kind == CliTargetKind::Directory)
    {
        return Err("--wait requires at least one file target".into());
    }
    Ok(())
}

fn write_response(stream: &mut TcpStream, response: &CliServerResponse) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(response)?;
    if bytes.len() as u64 >= MAX_CLI_RESPONSE_BYTES {
        serde_json::to_writer(
            &mut *stream,
            &CliServerResponse::Error {
                message: "Response exceeds 4 MiB; reduce the requested output".into(),
            },
        )?;
    } else {
        stream.write_all(&bytes)?;
    }
    stream.write_all(b"\n")?;
    stream.flush()
}

fn write_endpoint(path: &Path, descriptor: &CliEndpointDescriptor) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::State("invalid CLI endpoint path".into()))?;
    let ours = !parent.exists()
        || default_cli_endpoint_path().is_some_and(|default| default.parent() == Some(parent));
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    if ours {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(&mut temp, descriptor)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| AppError::from(error.error))?;
    Ok(())
}

fn remove_owned_endpoint(path: &Path, token: &str) {
    let owned = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CliEndpointDescriptor>(&bytes).ok())
        .is_some_and(|descriptor| descriptor.token == token);
    if owned {
        let _ = fs::remove_file(path);
    }
}

pub fn cli_endpoint_path() -> Option<PathBuf> {
    // Deliberately not SIKEMUX_CLI_ENDPOINT: that one is handed to everything
    // this app spawns so it can find us. Reading it here would mean a second
    // Sikemux started from one of our own terminals publishes itself over our
    // endpoint file, and every agent we launched would then be talking to it.
    if let Some(path) = std::env::var_os("SIKEMUX_CLI_ENDPOINT_PUBLISH") {
        return Some(PathBuf::from(path));
    }
    default_cli_endpoint_path()
}

fn default_cli_endpoint_path() -> Option<PathBuf> {
    let parent = crate::state::state_path()?.parent()?.to_path_buf();
    Some(parent.join(if cfg!(debug_assertions) {
        "cli.dev.json"
    } else {
        "cli.json"
    }))
}

pub fn cli_executable_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SIKEMUX_BIN_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let current = std::env::current_exe().ok()?;
    let filename = if cfg!(windows) {
        "sikemux-editor.exe"
    } else {
        "sikemux-editor"
    };
    let sibling = current.parent()?.join(filename);
    sibling.is_file().then_some(sibling)
}

static CLI_ON_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Agents shell out to `sikemux`, but the packaged CLI is named
/// `sikemux-editor`, so a `sikemux` link to it goes first on PATH for every
/// process this app launches. Call once at startup, before threads spawn.
pub fn put_cli_on_path() {
    let Some(executable) = cli_executable_path() else {
        return;
    };
    let Some(directory) = crate::state::state_path().and_then(|path| {
        Some(path.parent()?.join(if cfg!(debug_assertions) {
            "bin-dev"
        } else {
            "bin"
        }))
    }) else {
        return;
    };
    let Ok(link) = link_cli(&executable, &directory) else {
        return;
    };
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(directory.clone())
        .chain(std::env::split_paths(&existing).filter(|path| *path != directory));
    if let Ok(joined) = std::env::join_paths(paths) {
        // SAFETY: called once at startup before any threads spawn.
        unsafe { std::env::set_var("PATH", joined) };
        let _ = CLI_ON_PATH.set(link);
    }
}

pub fn cli_command_path() -> Option<PathBuf> {
    CLI_ON_PATH.get().cloned().or_else(cli_executable_path)
}

fn link_cli(executable: &Path, directory: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let link = directory.join(if cfg!(windows) {
        "sikemux.exe"
    } else {
        "sikemux"
    });
    #[cfg(unix)]
    {
        if fs::read_link(&link).is_ok_and(|target| target == executable) {
            return Ok(link);
        }
        let staging = directory.join(format!(".sikemux-{}", std::process::id()));
        let _ = fs::remove_file(&staging);
        std::os::unix::fs::symlink(executable, &staging)?;
        fs::rename(&staging, &link)?;
    }
    #[cfg(windows)]
    fs::copy(executable, &link)?;
    Ok(link)
}

#[tauri::command]
pub fn cli_frontend_ready(state: tauri::State<'_, CliBrokerState>) -> Vec<CliFrontendRequest> {
    state
        .0
        .as_ref()
        .map(|broker| broker.claim(true))
        .unwrap_or_default()
}

#[tauri::command]
pub fn cli_claim_open_requests(state: tauri::State<'_, CliBrokerState>) -> Vec<CliFrontendRequest> {
    state
        .0
        .as_ref()
        .map(|broker| broker.claim(false))
        .unwrap_or_default()
}

#[tauri::command]
pub fn cli_open_result(state: tauri::State<'_, CliBrokerState>, result: CliOpenResult) {
    if let Some(broker) = &state.0 {
        broker.mark_result(result);
    }
}

#[tauri::command]
pub fn cli_editor_tabs_closed(
    state: tauri::State<'_, CliBrokerState>,
    pane_id: String,
    paths: Vec<String>,
) {
    if let Some(broker) = &state.0 {
        broker.tabs_closed(&pane_id, &paths);
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliRuntimeInfo {
    endpoint: String,
    executable: Option<String>,
}

#[tauri::command]
pub fn cli_runtime_info(state: tauri::State<'_, CliBrokerState>) -> CliRuntimeInfo {
    CliRuntimeInfo {
        endpoint: state
            .0
            .as_ref()
            .map(|broker| broker.endpoint_path().to_path_buf())
            .or_else(cli_endpoint_path)
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        executable: state
            .0
            .as_ref()
            .and_then(CliBroker::cli_executable)
            .or_else(cli_executable_path)
            .map(|path| path.to_string_lossy().into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Shutdown;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn validates_absolute_bounded_requests() {
        let directory = tempfile::tempdir().unwrap();
        let project_root = directory.path().to_string_lossy().into_owned();
        let file = directory
            .path()
            .join("file.rs")
            .to_string_lossy()
            .into_owned();
        let request = CliOpenRequest {
            id: "request".into(),
            cwd: project_root.clone(),
            wait: true,
            targets: vec![crate::cli_protocol::CliOpenTarget {
                id: "target".into(),
                kind: CliTargetKind::File,
                path: file,
                project_root,
                line: Some(0),
                column: Some(0),
            }],
        };
        assert!(validate_request(&request).is_ok());
        let mut relative = request.clone();
        relative.targets[0].path = "file.rs".into();
        assert!(validate_request(&relative).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn endpoint_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/cli.json");
        let descriptor = CliEndpointDescriptor {
            protocol: 1,
            pid: 1,
            port: 42,
            token: "token".into(),
            version: "test".into(),
        };
        write_endpoint(&path, &descriptor).unwrap();
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_endpoint_directory_keeps_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let descriptor = CliEndpointDescriptor {
            protocol: 1,
            pid: 1,
            port: 42,
            token: "token".into(),
            version: "test".into(),
        };
        write_endpoint(&dir.path().join("cli.json"), &descriptor).unwrap();
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn a_dripping_client_runs_out_of_time_for_the_whole_request() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let writer = std::thread::spawn(move || {
            let mut client = TcpStream::connect(address).unwrap();
            for _ in 0..20 {
                if client.write_all(b" ").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(DeadlineReader {
            stream,
            deadline: Instant::now() + Duration::from_millis(150),
        });
        let started = Instant::now();
        let mut frame = Vec::new();
        let error = reader.read_until(b'\n', &mut frame).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        assert!(started.elapsed() < Duration::from_millis(400));
        drop(reader);
        writer.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_cli_is_linked_as_sikemux_and_relinked_when_it_moves() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first/sikemux-editor");
        let second = root.path().join("second/sikemux-editor");
        let directory = root.path().join("bin");
        let link = link_cli(&first, &directory).unwrap();
        assert_eq!(link, directory.join("sikemux"));
        assert_eq!(fs::read_link(&link).unwrap(), first);
        assert_eq!(link_cli(&first, &directory).unwrap(), link);
        link_cli(&second, &directory).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), second);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    }

    #[test]
    fn accepted_client_waits_for_a_delayed_complete_frame() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let writer = std::thread::spawn(move || {
            let mut client = TcpStream::connect(address).unwrap();
            std::thread::sleep(Duration::from_millis(40));
            client.write_all(br#"{"command":"ping"}"#).unwrap();
            client.write_all(b"\n").unwrap();
            client.shutdown(Shutdown::Write).unwrap();
        });

        let started = std::time::Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(started.elapsed() < Duration::from_secs(1));
                    std::thread::yield_now();
                }
                Err(error) => panic!("accept test client: {error}"),
            }
        };
        configure_client_stream(&stream).unwrap();
        let mut frame = String::new();
        BufReader::new(stream).read_line(&mut frame).unwrap();
        assert_eq!(frame, "{\"command\":\"ping\"}\n");
        writer.join().unwrap();
    }
}

#[cfg(test)]
mod endpoint_publish_tests {
    use super::cli_endpoint_path;
    use std::sync::Mutex;

    static ENV_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn a_second_app_started_from_our_terminal_keeps_its_own_endpoint() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|error| error.into_inner());
        // What a terminal we spawned carries, so a Sikemux launched there
        // inherits it. Publishing to it would steal our agents' broker.
        std::env::set_var("SIKEMUX_CLI_ENDPOINT", "/tmp/sikemux-other-instance.json");
        std::env::remove_var("SIKEMUX_CLI_ENDPOINT_PUBLISH");

        let published = cli_endpoint_path().expect("an endpoint path");

        assert_ne!(
            published,
            std::path::Path::new("/tmp/sikemux-other-instance.json")
        );
        std::env::remove_var("SIKEMUX_CLI_ENDPOINT");
    }

    #[test]
    fn the_publish_override_is_honoured() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|error| error.into_inner());
        std::env::set_var(
            "SIKEMUX_CLI_ENDPOINT_PUBLISH",
            "/tmp/sikemux-published-here.json",
        );

        let published = cli_endpoint_path().expect("an endpoint path");

        assert_eq!(
            published,
            std::path::Path::new("/tmp/sikemux-published-here.json")
        );
        std::env::remove_var("SIKEMUX_CLI_ENDPOINT_PUBLISH");
    }
}
