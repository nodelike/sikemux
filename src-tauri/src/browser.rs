use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::error::{AppError, AppResult};

const VIEWPORT_WIDTH: u32 = 1280;
const VIEWPORT_HEIGHT: u32 = 800;
const BROWSER_DRAIN_GRACE: Duration = Duration::from_millis(250);
const BROWSER_PID_FILE_ENV: &str = "SIKEMUX_BROWSER_PID_FILE";

#[derive(Default)]
pub struct BrowserManager {
    runtime: Mutex<BrowserRuntime>,
    child: std::sync::Mutex<Option<Child>>,
    broker: std::sync::Mutex<Option<BrowserBroker>>,
    generation: AtomicU64,
}

struct BrowserBroker {
    url: String,
    token: String,
    shutdown: oneshot::Sender<()>,
}

pub struct BrowserMcpLaunch {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Default)]
struct BrowserRuntime {
    cdp: Option<Arc<CdpClient>>,
    cdp_http_url: Option<String>,
    state_dir: Option<PathBuf>,
    active_targets: HashMap<String, String>,
    target_sessions: HashMap<String, String>,
}

struct CdpClient {
    sender: mpsc::UnboundedSender<Message>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    sequence: AtomicU64,
    closed: Arc<AtomicBool>,
}

fn browser_pid_file() -> Option<PathBuf> {
    std::env::var_os(BROWSER_PID_FILE_ENV).map(PathBuf::from)
}

fn record_browser_pid(pid: u32) -> AppResult<()> {
    let Some(path) = browser_pid_file() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{pid}.tmp"));
    std::fs::write(&temporary, format!("{pid}\n"))?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn clear_browser_pid(pid: u32) {
    let Some(path) = browser_pid_file() else {
        return;
    };
    let recorded = std::fs::read_to_string(&path)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    if recorded == Some(pid) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
fn configure_browser_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_browser_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_browser_process_tree(pid: u32, force: bool) {
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

#[cfg(windows)]
fn terminate_browser_process_tree(pid: u32, force: bool) {
    let mut command = Command::new("taskkill");
    let pid = pid.to_string();
    command.args(["/PID", &pid, "/T"]);
    if force {
        command.arg("/F");
    }
    let _ = command.status();
}

fn kill_and_reap_browser_child(child: &mut Child) {
    let pid = child.id();
    terminate_browser_process_tree(pid, true);
    let _ = child.kill();
    let _ = child.wait();
    clear_browser_pid(pid);
}

fn terminate_and_reap_browser_child(child: &mut Child) {
    let pid = child.id();
    terminate_browser_process_tree(pid, false);
    std::thread::sleep(BROWSER_DRAIN_GRACE);
    terminate_browser_process_tree(pid, true);
    let _ = child.kill();
    let _ = child.wait();
    clear_browser_pid(pid);
}

struct SpawnedBrowserChild(Option<Child>);

impl SpawnedBrowserChild {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn id(&self) -> u32 {
        self.0.as_ref().expect("browser child must exist").id()
    }

    fn into_inner(mut self) -> Child {
        self.0.take().expect("browser child must exist")
    }
}

impl Drop for SpawnedBrowserChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            kill_and_reap_browser_child(&mut child);
        }
    }
}

impl CdpClient {
    async fn connect(url: &str) -> AppResult<Arc<Self>> {
        let (socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|error| AppError::Other(format!("browser CDP connect failed: {error}")))?;
        let (mut writer, mut reader) = socket.split();
        let (sender, mut outbound) = mpsc::unbounded_channel::<Message>();
        let pending = Arc::new(Mutex::new(HashMap::<u64, oneshot::Sender<Value>>::new()));
        let pending_reader = pending.clone();
        let closed = Arc::new(AtomicBool::new(false));
        let closed_writer = closed.clone();
        let closed_reader = closed.clone();

        tokio::spawn(async move {
            while let Some(message) = outbound.recv().await {
                if writer.send(message).await.is_err() {
                    break;
                }
            }
            closed_writer.store(true, Ordering::Release);
        });
        tokio::spawn(async move {
            while let Some(Ok(message)) = reader.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let Some(id) = value.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                if let Some(reply) = pending_reader.lock().await.remove(&id) {
                    let _ = reply.send(value);
                }
            }
            closed_reader.store(true, Ordering::Release);
            let mut pending = pending_reader.lock().await;
            pending.clear();
        });

        Ok(Arc::new(Self {
            sender,
            pending,
            sequence: AtomicU64::new(1),
            closed,
        }))
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> AppResult<Value> {
        if self.is_closed() {
            return Err(AppError::Other("browser CDP connection closed".into()));
        }
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let (reply, receive) = oneshot::channel();
        self.pending.lock().await.insert(id, reply);
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session_id) = session_id {
            message["sessionId"] = Value::String(session_id.to_owned());
        }
        if self
            .sender
            .send(Message::Text(message.to_string().into()))
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return Err(AppError::Other("browser CDP connection closed".into()));
        }
        let response = match tokio::time::timeout(Duration::from_secs(15), receive).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                return Err(AppError::Other(
                    "browser CDP response channel closed".into(),
                ));
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(AppError::Other(format!("browser CDP {method} timed out")));
            }
        };
        if let Some(error) = response.get("error") {
            return Err(AppError::Other(format!(
                "browser CDP {method} failed: {error}"
            )));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTab {
    id: String,
    title: String,
    url: String,
    active: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    tabs: Vec<BrowserTab>,
    active_tab_id: Option<String>,
    frame: Option<String>,
    viewport_width: u32,
    viewport_height: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPointerInput {
    kind: String,
    x: f64,
    y: f64,
    #[serde(default)]
    button: Option<String>,
    #[serde(default)]
    delta_x: f64,
    #[serde(default)]
    delta_y: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserKeyInput {
    kind: String,
    key: String,
    code: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    modifiers: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserViewport {
    width: u32,
    height: u32,
}

impl BrowserManager {
    async fn is_started(&self) -> bool {
        self.runtime
            .lock()
            .await
            .cdp
            .as_ref()
            .is_some_and(|cdp| !cdp.is_closed())
    }

    pub fn drain(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut broker) = self.broker.lock() {
            if let Some(broker) = broker.take() {
                let _ = broker.shutdown.send(());
            }
        }
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut child) = child.take() {
                terminate_and_reap_browser_child(&mut child);
            }
        }
        if let Ok(mut runtime) = self.runtime.try_lock() {
            runtime.cdp = None;
        }
    }

    async fn ensure_broker(&self, app: &AppHandle) -> AppResult<(String, String)> {
        if let Ok(broker) = self.broker.lock() {
            if let Some(broker) = broker.as_ref() {
                return Ok((broker.url.clone(), broker.token.clone()));
            }
        }

        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|error| AppError::Other(format!("browser broker bind failed: {error}")))?;
        let address = listener.local_addr().map_err(|error| {
            AppError::Other(format!("browser broker address unavailable: {error}"))
        })?;
        let url = format!("http://{address}");
        let token = uuid::Uuid::new_v4().to_string();
        let (shutdown, mut stop) = oneshot::channel();

        {
            let mut broker = self
                .broker
                .lock()
                .map_err(|_| AppError::Other("browser broker lock poisoned".into()))?;
            if let Some(existing) = broker.as_ref() {
                return Ok((existing.url.clone(), existing.token.clone()));
            }
            *broker = Some(BrowserBroker {
                url: url.clone(),
                token: token.clone(),
                shutdown,
            });
        }

        let app = app.clone();
        let expected_token = token.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stop => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let app = app.clone();
                        let token = expected_token.clone();
                        tokio::spawn(async move {
                            let _ = serve_broker_connection(stream, app, &token).await;
                        });
                    }
                }
            }
        });
        Ok((url, token))
    }

    pub async fn ensure_started(&self, app: &AppHandle) -> AppResult<()> {
        let generation = self.generation.load(Ordering::Acquire);
        let mut runtime = self.runtime.lock().await;
        if let Some(cdp) = runtime.cdp.as_ref() {
            if !cdp.is_closed() {
                return Ok(());
            }
            runtime.cdp = None;
        }

        let state_dir = browser_state_dir(app)?;
        let profile_dir = state_dir.join("profile");
        std::fs::create_dir_all(&profile_dir)?;
        initialize_registry(&state_dir)?;
        let active_port = profile_dir.join("DevToolsActivePort");
        let _ = std::fs::remove_file(&active_port);

        let stopped_previous = if let Ok(mut current) = self.child.lock() {
            if let Some(mut child) = current.take() {
                kill_and_reap_browser_child(&mut child);
                true
            } else {
                false
            }
        } else {
            false
        };
        if stopped_previous {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let executable = browser_executable(app)?;
        let mut command = Command::new(&executable);
        command
            .args([
                "--headless=new",
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-default-apps",
                "--disable-sync",
                &format!("--window-size={VIEWPORT_WIDTH},{VIEWPORT_HEIGHT}"),
                &format!("--user-data-dir={}", profile_dir.display()),
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_browser_process_group(&mut command);
        let child = command.spawn().map_err(|error| {
            AppError::Other(format!("could not start bundled browser: {error}"))
        })?;
        let child = SpawnedBrowserChild::new(child);
        record_browser_pid(child.id())?;

        let (http_url, websocket_url) = match wait_for_debug_endpoint(&active_port).await {
            Ok(endpoint) => endpoint,
            Err(error) => return Err(error),
        };
        let cdp = match CdpClient::connect(&websocket_url).await {
            Ok(cdp) => cdp,
            Err(error) => return Err(error),
        };
        if self.generation.load(Ordering::Acquire) != generation {
            return Err(AppError::Other("browser startup was canceled".into()));
        }
        std::fs::write(
            state_dir.join("runtime.json"),
            serde_json::to_vec(
                &json!({ "cdpUrl": http_url, "webSocketDebuggerUrl": websocket_url }),
            )?,
        )?;
        let mut child_slot = self
            .child
            .lock()
            .map_err(|_| AppError::Other("browser process lock poisoned".into()))?;
        *child_slot = Some(child.into_inner());
        drop(child_slot);
        runtime.cdp = Some(cdp);
        runtime.cdp_http_url = Some(http_url);
        runtime.state_dir = Some(state_dir);
        runtime.active_targets.clear();
        runtime.target_sessions.clear();
        Ok(())
    }

    pub async fn environment(
        &self,
        app: &AppHandle,
        agent_id: &str,
    ) -> AppResult<Vec<(String, String)>> {
        validate_agent_id(agent_id)?;
        let state_dir = browser_state_dir(app)?;
        initialize_registry(&state_dir)?;
        let (broker_url, broker_token) = self.ensure_broker(app).await?;
        let launch = self.mcp_launch(app)?;
        Ok(vec![
            ("SIKEMUX_BROWSER_MCP_COMMAND".into(), launch.command),
            (
                "SIKEMUX_BROWSER_MCP_ARGS".into(),
                serde_json::to_string(&launch.args)?,
            ),
            (
                "SIKEMUX_BROWSER_STATE_DIR".into(),
                state_dir.to_string_lossy().into_owned(),
            ),
            ("SIKEMUX_BROWSER_BROKER_URL".into(), broker_url),
            ("SIKEMUX_BROWSER_BROKER_TOKEN".into(), broker_token),
            ("SIKEMUX_BROWSER_AGENT_ID".into(), agent_id.to_owned()),
        ])
    }

    pub fn mcp_launch(&self, app: &AppHandle) -> AppResult<BrowserMcpLaunch> {
        if let Some(command) = std::env::var_os("SIKEMUX_BROWSER_MCP_EXECUTABLE") {
            return Ok(BrowserMcpLaunch {
                command: PathBuf::from(command).to_string_lossy().into_owned(),
                args: Vec::new(),
            });
        }
        let executable_name = if cfg!(windows) {
            "sikemux-browser-mcp.exe"
        } else {
            "sikemux-browser-mcp"
        };
        if let Ok(current) = std::env::current_exe() {
            if let Some(parent) = current.parent() {
                let bundled = parent.join(executable_name);
                if bundled.is_file() {
                    return Ok(BrowserMcpLaunch {
                        command: bundled.to_string_lossy().into_owned(),
                        args: Vec::new(),
                    });
                }
            }
        }
        if let Ok(resource_dir) = app.path().resource_dir() {
            let bundled = resource_dir.join(executable_name);
            if bundled.is_file() {
                return Ok(BrowserMcpLaunch {
                    command: bundled.to_string_lossy().into_owned(),
                    args: Vec::new(),
                });
            }
        }
        #[cfg(debug_assertions)]
        {
            let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("Tauri manifest has a project parent")
                .join("browser");
            return Ok(BrowserMcpLaunch {
                command: "uv".into(),
                args: vec![
                    "run".into(),
                    "--project".into(),
                    project.to_string_lossy().into_owned(),
                    "python".into(),
                    project
                        .join("sikemux_browser_mcp.py")
                        .to_string_lossy()
                        .into_owned(),
                ],
            });
        }
        #[allow(unreachable_code)]
        Err(AppError::Other(
            "bundled browser MCP sidecar is missing".into(),
        ))
    }

    async fn cdp_and_state_dir(&self, app: &AppHandle) -> AppResult<(Arc<CdpClient>, PathBuf)> {
        self.ensure_started(app).await?;
        let runtime = self.runtime.lock().await;
        Ok((
            runtime
                .cdp
                .clone()
                .ok_or_else(|| AppError::Other("browser CDP unavailable".into()))?,
            runtime
                .state_dir
                .clone()
                .ok_or_else(|| AppError::Other("browser state directory unavailable".into()))?,
        ))
    }

    async fn target_session(
        &self,
        app: &AppHandle,
        target_id: &str,
    ) -> AppResult<(Arc<CdpClient>, String)> {
        let (cdp, _) = self.cdp_and_state_dir(app).await?;
        {
            let runtime = self.runtime.lock().await;
            if let Some(session_id) = runtime.target_sessions.get(target_id) {
                return Ok((cdp, session_id.clone()));
            }
        }
        let result = cdp
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Other("browser target attach returned no session".into()))?
            .to_owned();
        self.runtime
            .lock()
            .await
            .target_sessions
            .insert(target_id.to_owned(), session_id.clone());
        cdp.call("Page.enable", json!({}), Some(&session_id))
            .await?;
        Ok((cdp, session_id))
    }

    async fn owned_tabs(&self, app: &AppHandle, agent_id: &str) -> AppResult<Vec<BrowserTab>> {
        validate_agent_id(agent_id)?;
        let (cdp, state_dir) = self.cdp_and_state_dir(app).await?;
        let targets = cdp.call("Target.getTargets", json!({}), None).await?;
        let available = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|target| {
                target
                    .get("targetId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<std::collections::HashSet<_>>();
        prune_owned_targets(&state_dir, agent_id, &available)?;
        let owned = owned_target_ids(&state_dir, agent_id)?;
        let active_from_agent = read_active_target(&state_dir, agent_id)?;
        let active = active_from_agent.or_else(|| {
            self.runtime
                .try_lock()
                .ok()
                .and_then(|runtime| runtime.active_targets.get(agent_id).cloned())
        });
        let mut tabs = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|target| target.get("type").and_then(Value::as_str) == Some("page"))
            .filter_map(|target| {
                let id = target.get("targetId")?.as_str()?.to_owned();
                owned.contains(&id).then(|| BrowserTab {
                    active: active.as_deref() == Some(id.as_str()),
                    id,
                    title: target
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    url: target
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or("about:blank")
                        .to_owned(),
                })
            })
            .collect::<Vec<_>>();
        if !tabs.is_empty() && !tabs.iter().any(|tab| tab.active) {
            tabs[0].active = true;
            write_active_target(&state_dir, agent_id, Some(&tabs[0].id))?;
            self.runtime
                .lock()
                .await
                .active_targets
                .insert(agent_id.to_owned(), tabs[0].id.clone());
        }
        if let Some(active) = tabs.iter().find(|tab| tab.active).map(|tab| tab.id.clone()) {
            self.runtime
                .lock()
                .await
                .active_targets
                .insert(agent_id.to_owned(), active);
        }
        Ok(tabs)
    }

    async fn active_target(&self, app: &AppHandle, agent_id: &str) -> AppResult<Option<String>> {
        let tabs = self.owned_tabs(app, agent_id).await?;
        Ok(tabs.iter().find(|tab| tab.active).map(|tab| tab.id.clone()))
    }

    async fn cached_active_target(&self, agent_id: &str) -> Option<String> {
        self.runtime
            .lock()
            .await
            .active_targets
            .get(agent_id)
            .cloned()
    }

    async fn close_agent(&self, app: &AppHandle, agent_id: &str) -> AppResult<()> {
        validate_agent_id(agent_id)?;
        let state_dir = browser_state_dir(app)?;
        if !state_dir.join("tabs.sqlite3").is_file() {
            return Ok(());
        }
        initialize_registry(&state_dir)?;
        let target_ids = owned_target_ids(&state_dir, agent_id)?;
        let cdp = self.runtime.lock().await.cdp.clone();
        let mut close_errors = Vec::new();
        if let Some(cdp) = cdp.filter(|cdp| !cdp.is_closed()) {
            for target_id in &target_ids {
                if let Err(error) = cdp
                    .call("Target.closeTarget", json!({ "targetId": target_id }), None)
                    .await
                {
                    close_errors.push(error.to_string());
                }
            }
        }
        clear_agent_registry(&state_dir, agent_id)?;
        let mut runtime = self.runtime.lock().await;
        runtime.active_targets.remove(agent_id);
        for target_id in target_ids {
            runtime.target_sessions.remove(&target_id);
        }
        if close_errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Other(format!(
                "browser target cleanup failed: {}",
                close_errors.join("; ")
            )))
        }
    }
}

impl Drop for BrowserManager {
    fn drop(&mut self) {
        self.drain();
    }
}

fn browser_state_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("browser data directory unavailable: {error}")))?
        .join("browser"))
}

async fn serve_broker_connection(
    mut stream: TcpStream,
    app: AppHandle,
    expected_token: &str,
) -> AppResult<()> {
    let mut request = Vec::with_capacity(1024);
    loop {
        let mut chunk = [0_u8; 1024];
        let count = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .map_err(|_| AppError::Other("browser broker request timed out".into()))?
            .map_err(|error| AppError::Other(format!("browser broker read failed: {error}")))?;
        if count == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > 8 * 1024 {
            write_broker_response(&mut stream, 413, json!({ "error": "request too large" }))
                .await?;
            return Ok(());
        }
    }

    let request = String::from_utf8_lossy(&request);
    if !broker_request_authorized(&request, expected_token) {
        write_broker_response(&mut stream, 401, json!({ "error": "unauthorized" })).await?;
        return Ok(());
    }

    let manager = app.state::<BrowserManager>();
    let broker_is_current = manager
        .broker
        .lock()
        .ok()
        .and_then(|broker| broker.as_ref().map(|broker| broker.token == expected_token))
        .unwrap_or(false);
    if !broker_is_current {
        write_broker_response(
            &mut stream,
            503,
            json!({ "error": "browser broker stopped" }),
        )
        .await?;
        return Ok(());
    }
    match manager.ensure_started(&app).await {
        Ok(()) => {
            let cdp_url = manager
                .runtime
                .lock()
                .await
                .cdp_http_url
                .clone()
                .ok_or_else(|| AppError::Other("browser CDP URL unavailable".into()))?;
            write_broker_response(&mut stream, 200, json!({ "cdpUrl": cdp_url })).await?;
        }
        Err(error) => {
            write_broker_response(&mut stream, 503, json!({ "error": error.to_string() })).await?;
        }
    }
    Ok(())
}

fn broker_request_authorized(request: &str, expected_token: &str) -> bool {
    let mut lines = request.lines();
    if lines.next() != Some("POST /cdp HTTP/1.1") {
        return false;
    }
    lines.any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.eq_ignore_ascii_case("authorization")
            && value
                .trim()
                .strip_prefix("Bearer ")
                .is_some_and(|token| token == expected_token)
    })
}

async fn write_broker_response(stream: &mut TcpStream, status: u16, body: Value) -> AppResult<()> {
    let body = body.to_string();
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        413 => "Payload Too Large",
        _ => "Service Unavailable",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|error| AppError::Other(format!("browser broker write failed: {error}")))
}

fn initialize_registry(state_dir: &Path) -> AppResult<()> {
    std::fs::create_dir_all(state_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let connection = Connection::open(state_dir.join("tabs.sqlite3"))
        .map_err(|error| AppError::State(error.to_string()))?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS browser_tabs (
                target_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE IF NOT EXISTS browser_active_tabs (
                agent_id TEXT PRIMARY KEY,
                target_id TEXT NOT NULL
            );",
        )
        .map_err(|error| AppError::State(error.to_string()))?;
    Ok(())
}

fn validate_agent_id(agent_id: &str) -> AppResult<()> {
    if agent_id.is_empty()
        || agent_id.len() > 128
        || !agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
    {
        return Err(AppError::BadArg("invalid browser agent id"));
    }
    Ok(())
}

fn validate_target_id(target_id: &str) -> AppResult<()> {
    if target_id.is_empty()
        || target_id.len() > 128
        || !target_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::BadArg("invalid browser target id"));
    }
    Ok(())
}

fn validate_url_input(url: &str) -> AppResult<()> {
    if url.len() > 16 * 1024
        || url.contains('\0')
        || url.chars().any(|character| character.is_control())
    {
        return Err(AppError::BadArg("invalid browser URL"));
    }
    Ok(())
}

fn pointer_params(input: &BrowserPointerInput) -> AppResult<Value> {
    let button = input.button.as_deref();
    let valid_button = match input.kind.as_str() {
        "move" | "wheel" => button.is_none() || button == Some("none"),
        "down" | "up" => matches!(button, Some("left" | "middle" | "right")),
        _ => false,
    };
    if !valid_button
        || !input.x.is_finite()
        || !input.y.is_finite()
        || !input.delta_x.is_finite()
        || !input.delta_y.is_finite()
        || input.x.abs() > 16_384.0
        || input.y.abs() > 16_384.0
        || input.delta_x.abs() > 16_384.0
        || input.delta_y.abs() > 16_384.0
    {
        return Err(AppError::BadArg("invalid browser pointer input"));
    }
    Ok(match input.kind.as_str() {
        "move" => json!({
            "type": "mouseMoved",
            "x": input.x,
            "y": input.y,
            "button": "none",
            "buttons": 0,
        }),
        "down" => json!({
            "type": "mousePressed",
            "x": input.x,
            "y": input.y,
            "button": button,
            "buttons": 1,
            "clickCount": 1,
        }),
        "up" => json!({
            "type": "mouseReleased",
            "x": input.x,
            "y": input.y,
            "button": button,
            "buttons": 0,
            "clickCount": 1,
        }),
        "wheel" => json!({
            "type": "mouseWheel",
            "x": input.x,
            "y": input.y,
            "deltaX": input.delta_x,
            "deltaY": input.delta_y,
        }),
        _ => unreachable!(),
    })
}

fn owned_target_ids(
    state_dir: &Path,
    agent_id: &str,
) -> AppResult<std::collections::HashSet<String>> {
    let connection = Connection::open(state_dir.join("tabs.sqlite3"))
        .map_err(|error| AppError::State(error.to_string()))?;
    let mut statement = connection
        .prepare("SELECT target_id FROM browser_tabs WHERE agent_id = ?1 ORDER BY created_at")
        .map_err(|error| AppError::State(error.to_string()))?;
    let rows = statement
        .query_map([agent_id], |row| row.get::<_, String>(0))
        .map_err(|error| AppError::State(error.to_string()))?;
    rows.collect::<Result<_, _>>()
        .map_err(|error| AppError::State(error.to_string()))
}

fn prune_owned_targets(
    state_dir: &Path,
    agent_id: &str,
    available: &std::collections::HashSet<String>,
) -> AppResult<()> {
    let connection = Connection::open(state_dir.join("tabs.sqlite3"))
        .map_err(|error| AppError::State(error.to_string()))?;
    let owned = owned_target_ids(state_dir, agent_id)?;
    for target_id in owned.difference(available) {
        connection
            .execute(
                "DELETE FROM browser_tabs WHERE target_id = ?1 AND agent_id = ?2",
                params![target_id, agent_id],
            )
            .map_err(|error| AppError::State(error.to_string()))?;
    }
    Ok(())
}

fn register_target(state_dir: &Path, agent_id: &str, target_id: &str) -> AppResult<()> {
    let connection = Connection::open(state_dir.join("tabs.sqlite3"))
        .map_err(|error| AppError::State(error.to_string()))?;
    connection
        .execute(
            "INSERT OR REPLACE INTO browser_tabs (target_id, agent_id) VALUES (?1, ?2)",
            params![target_id, agent_id],
        )
        .map_err(|error| AppError::State(error.to_string()))?;
    Ok(())
}

fn unregister_target(state_dir: &Path, target_id: &str) -> AppResult<()> {
    let connection = Connection::open(state_dir.join("tabs.sqlite3"))
        .map_err(|error| AppError::State(error.to_string()))?;
    connection
        .execute("DELETE FROM browser_tabs WHERE target_id = ?1", [target_id])
        .map_err(|error| AppError::State(error.to_string()))?;
    Ok(())
}

fn write_active_target(state_dir: &Path, agent_id: &str, target_id: Option<&str>) -> AppResult<()> {
    let connection = Connection::open(state_dir.join("tabs.sqlite3"))
        .map_err(|error| AppError::State(error.to_string()))?;
    if let Some(target_id) = target_id {
        connection
            .execute(
                "INSERT INTO browser_active_tabs (agent_id, target_id) VALUES (?1, ?2)
                 ON CONFLICT(agent_id) DO UPDATE SET target_id = excluded.target_id",
                params![agent_id, target_id],
            )
            .map_err(|error| AppError::State(error.to_string()))?;
    } else {
        connection
            .execute(
                "DELETE FROM browser_active_tabs WHERE agent_id = ?1",
                [agent_id],
            )
            .map_err(|error| AppError::State(error.to_string()))?;
    }
    Ok(())
}

fn read_active_target(state_dir: &Path, agent_id: &str) -> AppResult<Option<String>> {
    let connection = Connection::open(state_dir.join("tabs.sqlite3"))
        .map_err(|error| AppError::State(error.to_string()))?;
    let mut statement = connection
        .prepare("SELECT target_id FROM browser_active_tabs WHERE agent_id = ?1")
        .map_err(|error| AppError::State(error.to_string()))?;
    let mut rows = statement
        .query([agent_id])
        .map_err(|error| AppError::State(error.to_string()))?;
    rows.next()
        .map_err(|error| AppError::State(error.to_string()))?
        .map(|row| row.get(0))
        .transpose()
        .map_err(|error| AppError::State(error.to_string()))
}

fn clear_agent_registry(state_dir: &Path, agent_id: &str) -> AppResult<()> {
    let mut connection = Connection::open(state_dir.join("tabs.sqlite3"))
        .map_err(|error| AppError::State(error.to_string()))?;
    let transaction = connection
        .transaction()
        .map_err(|error| AppError::State(error.to_string()))?;
    transaction
        .execute("DELETE FROM browser_tabs WHERE agent_id = ?1", [agent_id])
        .map_err(|error| AppError::State(error.to_string()))?;
    transaction
        .execute(
            "DELETE FROM browser_active_tabs WHERE agent_id = ?1",
            [agent_id],
        )
        .map_err(|error| AppError::State(error.to_string()))?;
    transaction
        .commit()
        .map_err(|error| AppError::State(error.to_string()))
}

async fn wait_for_debug_endpoint(path: &Path) -> AppResult<(String, String)> {
    for _ in 0..150 {
        if let Ok(contents) = tokio::fs::read_to_string(path).await {
            let mut lines = contents.lines();
            if let (Some(port), Some(websocket_path)) = (lines.next(), lines.next()) {
                let http = format!("http://127.0.0.1:{port}");
                return Ok((
                    http.clone(),
                    format!("ws://127.0.0.1:{port}{websocket_path}"),
                ));
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(AppError::Other(
        "bundled browser did not expose a CDP endpoint".into(),
    ))
}

fn browser_executable(app: &AppHandle) -> AppResult<PathBuf> {
    if let Some(path) = std::env::var_os("SIKEMUX_BROWSER_EXECUTABLE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Ok(resource_dir) = app.path().resource_dir() {
        if let Some(path) = find_bundled_browser(&resource_dir.join("browser-runtime")) {
            return Ok(path);
        }
    }
    #[cfg(target_os = "macos")]
    for path in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ] {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    #[cfg(target_os = "windows")]
    for root in [
        std::env::var_os("PROGRAMFILES"),
        std::env::var_os("PROGRAMFILES(X86)"),
    ]
    .into_iter()
    .flatten()
    {
        let path = PathBuf::from(root).join("Google/Chrome/Application/chrome.exe");
        if path.is_file() {
            return Ok(path);
        }
    }
    #[cfg(target_os = "linux")]
    for path in [
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome",
    ] {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(AppError::Other(
        "bundled Chromium runtime is missing".into(),
    ))
}

fn find_bundled_browser(root: &Path) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if matches!(
                name,
                "Chromium"
                    | "chrome"
                    | "chrome.exe"
                    | "chrome-headless-shell"
                    | "headless_shell.exe"
                    | "Google Chrome for Testing"
            ) {
                return Some(path);
            }
        }
    }
    None
}

fn normalize_url(input: &str) -> String {
    let value = input.trim();
    if value.is_empty() {
        return "about:blank".into();
    }
    if value == "about:blank" || value.contains("://") {
        return value.to_owned();
    }
    if value.starts_with("localhost") || value.starts_with("127.0.0.1") || value.contains('.') {
        return format!(
            "http{}://{value}",
            if value.starts_with("localhost") || value.starts_with("127.0.0.1") {
                ""
            } else {
                "s"
            }
        );
    }
    let query = url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
    format!("https://www.google.com/search?q={query}")
}

#[tauri::command]
pub async fn browser_snapshot(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    include_frame: bool,
    viewport: Option<BrowserViewport>,
) -> AppResult<BrowserSnapshot> {
    if !include_frame && !manager.is_started().await {
        return Ok(BrowserSnapshot {
            tabs: Vec::new(),
            active_tab_id: None,
            frame: None,
            viewport_width: VIEWPORT_WIDTH,
            viewport_height: VIEWPORT_HEIGHT,
        });
    }
    let mut tabs = manager.owned_tabs(&app, &agent_id).await?;
    let active = tabs.iter().find(|tab| tab.active).map(|tab| tab.id.clone());
    let viewport = viewport.unwrap_or(BrowserViewport {
        width: VIEWPORT_WIDTH,
        height: VIEWPORT_HEIGHT,
    });
    let frame = if include_frame {
        if let Some(target_id) = active.as_deref() {
            let (cdp, session_id) = manager.target_session(&app, target_id).await?;
            cdp.call(
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": viewport.width.clamp(320, 3840),
                    "height": viewport.height.clamp(240, 2160),
                    "deviceScaleFactor": 1,
                    "mobile": false
                }),
                Some(&session_id),
            )
            .await?;
            let capture = cdp
                .call(
                    "Page.captureScreenshot",
                    json!({ "format": "jpeg", "quality": 82, "fromSurface": true }),
                    Some(&session_id),
                )
                .await?;
            capture
                .get("data")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        }
    } else {
        None
    };
    tabs.sort_by_key(|tab| !tab.active);
    Ok(BrowserSnapshot {
        tabs,
        active_tab_id: active,
        frame,
        viewport_width: viewport.width,
        viewport_height: viewport.height,
    })
}

#[tauri::command]
pub async fn browser_new_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    url: Option<String>,
) -> AppResult<String> {
    validate_agent_id(&agent_id)?;
    if let Some(url) = url.as_deref() {
        validate_url_input(url)?;
    }
    let (cdp, state_dir) = manager.cdp_and_state_dir(&app).await?;
    let result = cdp
        .call(
            "Target.createTarget",
            json!({ "url": normalize_url(url.as_deref().unwrap_or("about:blank")) }),
            None,
        )
        .await?;
    let target_id = result
        .get("targetId")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Other("browser did not create a target".into()))?
        .to_owned();
    register_target(&state_dir, &agent_id, &target_id)?;
    write_active_target(&state_dir, &agent_id, Some(&target_id))?;
    manager
        .runtime
        .lock()
        .await
        .active_targets
        .insert(agent_id, target_id.clone());
    Ok(target_id)
}

#[tauri::command]
pub async fn browser_close_agent(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<()> {
    manager.close_agent(&app, &agent_id).await
}

#[tauri::command]
pub async fn browser_switch_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    target_id: String,
) -> AppResult<()> {
    validate_target_id(&target_id)?;
    let tabs = manager.owned_tabs(&app, &agent_id).await?;
    if !tabs.iter().any(|tab| tab.id == target_id) {
        return Err(AppError::BadArg("browser tab does not belong to agent"));
    }
    let (cdp, state_dir) = manager.cdp_and_state_dir(&app).await?;
    cdp.call(
        "Target.activateTarget",
        json!({ "targetId": target_id }),
        None,
    )
    .await?;
    manager
        .runtime
        .lock()
        .await
        .active_targets
        .insert(agent_id.clone(), target_id.clone());
    write_active_target(&state_dir, &agent_id, Some(&target_id))?;
    Ok(())
}

#[tauri::command]
pub async fn browser_close_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    target_id: String,
) -> AppResult<()> {
    validate_target_id(&target_id)?;
    let tabs = manager.owned_tabs(&app, &agent_id).await?;
    if !tabs.iter().any(|tab| tab.id == target_id) {
        return Err(AppError::BadArg("browser tab does not belong to agent"));
    }
    let (cdp, state_dir) = manager.cdp_and_state_dir(&app).await?;
    cdp.call("Target.closeTarget", json!({ "targetId": target_id }), None)
        .await?;
    unregister_target(&state_dir, &target_id)?;
    let mut runtime = manager.runtime.lock().await;
    runtime.target_sessions.remove(&target_id);
    if runtime.active_targets.get(&agent_id) == Some(&target_id) {
        runtime.active_targets.remove(&agent_id);
    }
    drop(runtime);
    let next = manager
        .owned_tabs(&app, &agent_id)
        .await?
        .first()
        .map(|tab| tab.id.as_str().to_owned());
    write_active_target(&state_dir, &agent_id, next.as_deref())?;
    Ok(())
}

#[tauri::command]
pub async fn browser_navigate(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    url: String,
) -> AppResult<()> {
    validate_url_input(&url)?;
    let Some(target_id) = manager.active_target(&app, &agent_id).await? else {
        browser_new_tab(app, manager, agent_id, Some(url)).await?;
        return Ok(());
    };
    let (cdp, session_id) = manager.target_session(&app, &target_id).await?;
    cdp.call(
        "Page.navigate",
        json!({ "url": normalize_url(&url) }),
        Some(&session_id),
    )
    .await?;
    Ok(())
}

async fn browser_history(
    app: &AppHandle,
    manager: &BrowserManager,
    agent_id: &str,
    delta: i64,
) -> AppResult<()> {
    let Some(target_id) = manager.active_target(app, agent_id).await? else {
        return Ok(());
    };
    let (cdp, session_id) = manager.target_session(app, &target_id).await?;
    let history = cdp
        .call("Page.getNavigationHistory", json!({}), Some(&session_id))
        .await?;
    let current = history
        .get("currentIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let wanted = current + delta;
    if let Some(entry_id) = history
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|entries| entries.get(wanted.max(0) as usize))
        .and_then(|entry| entry.get("id"))
        .and_then(Value::as_i64)
    {
        cdp.call(
            "Page.navigateToHistoryEntry",
            json!({ "entryId": entry_id }),
            Some(&session_id),
        )
        .await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn browser_back(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<()> {
    browser_history(&app, &manager, &agent_id, -1).await
}

#[tauri::command]
pub async fn browser_forward(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<()> {
    browser_history(&app, &manager, &agent_id, 1).await
}

#[tauri::command]
pub async fn browser_reload(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
) -> AppResult<()> {
    let Some(target_id) = manager.active_target(&app, &agent_id).await? else {
        return Ok(());
    };
    let (cdp, session_id) = manager.target_session(&app, &target_id).await?;
    cdp.call("Page.reload", json!({}), Some(&session_id))
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn browser_pointer(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    input: BrowserPointerInput,
) -> AppResult<()> {
    validate_agent_id(&agent_id)?;
    let params = pointer_params(&input)?;
    let target_id = match manager.cached_active_target(&agent_id).await {
        Some(target_id) => Some(target_id),
        None => manager.active_target(&app, &agent_id).await?,
    };
    let Some(target_id) = target_id else {
        return Ok(());
    };
    let (cdp, session_id) = manager.target_session(&app, &target_id).await?;
    cdp.call("Input.dispatchMouseEvent", params, Some(&session_id))
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn browser_key(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    agent_id: String,
    input: BrowserKeyInput,
) -> AppResult<()> {
    validate_agent_id(&agent_id)?;
    if !matches!(input.kind.as_str(), "down" | "up" | "text")
        || input.key.len() > 128
        || input.code.len() > 128
        || input.text.len() > 16 * 1024
        || input.key.contains('\0')
        || input.code.contains('\0')
        || input.text.contains('\0')
        || input.modifiers > 15
    {
        return Err(AppError::BadArg("invalid browser key input"));
    }
    let target_id = match manager.cached_active_target(&agent_id).await {
        Some(target_id) => Some(target_id),
        None => manager.active_target(&app, &agent_id).await?,
    };
    let Some(target_id) = target_id else {
        return Ok(());
    };
    let (cdp, session_id) = manager.target_session(&app, &target_id).await?;
    if input.kind == "text" {
        cdp.call(
            "Input.insertText",
            json!({ "text": input.text }),
            Some(&session_id),
        )
        .await?;
        return Ok(());
    }
    cdp.call(
        "Input.dispatchKeyEvent",
        json!({
            "type": if input.kind == "up" { "keyUp" } else { "keyDown" },
            "key": input.key,
            "code": input.code,
            "text": input.text,
            "modifiers": input.modifiers
        }),
        Some(&session_id),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        broker_request_authorized, clear_agent_registry, initialize_registry, normalize_url,
        owned_target_ids, pointer_params, read_active_target, register_target, validate_agent_id,
        validate_target_id, validate_url_input, write_active_target, BrowserPointerInput,
    };
    #[cfg(unix)]
    use super::{configure_browser_process_group, terminate_and_reap_browser_child};
    #[cfg(unix)]
    use std::process::{Command, Stdio};

    #[cfg(unix)]
    #[test]
    fn browser_cleanup_terminates_its_process_group() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "trap '' TERM; while :; do sleep 1; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_browser_process_group(&mut command);
        let mut child = command.spawn().unwrap();
        let pid = child.id();

        terminate_and_reap_browser_child(&mut child);

        assert!(child.try_wait().unwrap().is_some());
        assert_eq!(unsafe { libc::kill(-(pid as i32), 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn normalizes_addresses_and_searches() {
        assert_eq!(normalize_url("about:blank"), "about:blank");
        assert_eq!(normalize_url("localhost:1420"), "http://localhost:1420");
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(
            normalize_url("browser use"),
            "https://www.google.com/search?q=browser+use"
        );
    }

    #[test]
    fn browser_agent_ids_are_path_safe() {
        assert!(validate_agent_id("agent-codex_42").is_ok());
        assert!(validate_agent_id("../profile").is_err());
        assert!(validate_agent_id("").is_err());
    }

    #[test]
    fn browser_target_and_url_inputs_are_bounded() {
        assert!(validate_target_id("A07f42").is_ok());
        assert!(validate_target_id("../tab").is_err());
        assert!(validate_url_input("https://example.com").is_ok());
        assert!(validate_url_input("https://example.com\nheader: value").is_err());
        assert!(validate_url_input(&"x".repeat(16 * 1024 + 1)).is_err());
    }

    #[test]
    fn pointer_payloads_only_send_fields_valid_for_each_cdp_event() {
        let moved = pointer_params(&BrowserPointerInput {
            kind: "move".into(),
            x: 12.0,
            y: 18.0,
            button: Some("none".into()),
            delta_x: 0.0,
            delta_y: 0.0,
        })
        .unwrap();
        assert_eq!(moved["type"], "mouseMoved");
        assert!(moved.get("deltaX").is_none());
        assert!(moved.get("clickCount").is_none());

        let wheel = pointer_params(&BrowserPointerInput {
            kind: "wheel".into(),
            x: 12.0,
            y: 18.0,
            button: Some("none".into()),
            delta_x: 0.0,
            delta_y: 100.0,
        })
        .unwrap();
        assert_eq!(wheel["type"], "mouseWheel");
        assert!(wheel.get("button").is_none());
        assert!(wheel.get("clickCount").is_none());
    }

    #[test]
    fn active_tabs_and_agent_cleanup_are_transactional() {
        let directory = tempfile::tempdir().unwrap();
        initialize_registry(directory.path()).unwrap();
        register_target(directory.path(), "agent-one", "target-one").unwrap();
        register_target(directory.path(), "agent-one", "target-two").unwrap();
        write_active_target(directory.path(), "agent-one", Some("target-two")).unwrap();

        assert_eq!(
            read_active_target(directory.path(), "agent-one").unwrap(),
            Some("target-two".into())
        );
        clear_agent_registry(directory.path(), "agent-one").unwrap();
        assert!(owned_target_ids(directory.path(), "agent-one")
            .unwrap()
            .is_empty());
        assert_eq!(
            read_active_target(directory.path(), "agent-one").unwrap(),
            None
        );
    }

    #[test]
    fn broker_requires_the_exact_bearer_token() {
        assert!(broker_request_authorized(
            "POST /cdp HTTP/1.1\r\nauthorization: Bearer secret\r\n\r\n",
            "secret"
        ));
        assert!(!broker_request_authorized(
            "POST /cdp HTTP/1.1\r\nAuthorization: Bearer wrong\r\n\r\n",
            "secret"
        ));
        assert!(!broker_request_authorized(
            "GET /cdp HTTP/1.1\r\nAuthorization: Bearer secret\r\n\r\n",
            "secret"
        ));
    }
}
