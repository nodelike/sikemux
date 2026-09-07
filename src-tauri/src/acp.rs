use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, Implementation, InitializeRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ResourceLink, SelectedPermissionOutcome, SessionNotification,
    SetSessionConfigOptionRequest, SetSessionModeRequest,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, Responder};
use dashmap::mapref::entry::Entry;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot};
use url::Url;
use uuid::Uuid;

const CLAUDE_ADAPTER: &str = "@agentclientprotocol/claude-agent-acp@0.73.0";
const CODEX_ADAPTER: &str = "@agentclientprotocol/codex-acp@1.8.0";
const MAX_AGENT_ID: usize = 200;
const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 32;
const START_TIMEOUT: Duration = Duration::from_secs(150);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
const INSTALL_OUTPUT_LIMIT: usize = 1024 * 1024;
type ReadySender = Arc<Mutex<Option<oneshot::Sender<Result<AcpStartResponse, String>>>>>;

#[derive(Clone, Copy)]
struct AdapterSpec {
    package: &'static str,
    package_dir: &'static str,
    executable: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpEvent {
    agent_id: String,
    kind: &'static str,
    payload: Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpStartResponse {
    session_id: String,
    capabilities: Value,
    setup: Value,
}

enum AcpCommand {
    Prompt {
        text: String,
        paths: Vec<String>,
    },
    SetPermissionMode {
        mode: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Cancel,
    Stop,
}

struct PendingPermission {
    agent_id: String,
    option_ids: HashSet<String>,
    responder: Responder<RequestPermissionResponse>,
}

#[derive(Clone)]
struct AcpConnectionHandle {
    generation: Uuid,
    commands: mpsc::UnboundedSender<AcpCommand>,
    abort: tokio::task::AbortHandle,
    install_cancellation: crate::bounded_process::ProcessCancellation,
}

#[derive(Clone, Default)]
pub struct AcpManager {
    connections: Arc<dashmap::DashMap<String, AcpConnectionHandle>>,
    permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    adapter_installs: Arc<dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AcpManager {
    pub fn drain(&self) {
        for item in self.connections.iter() {
            let _ = item.commands.send(AcpCommand::Stop);
            item.install_cancellation.cancel();
            item.abort.abort();
        }
        self.connections.clear();
        self.cancel_permissions(None);
    }

    fn cancel_permissions(&self, agent_id: Option<&str>) {
        let pending = {
            let Ok(mut permissions) = self.permissions.lock() else {
                return;
            };
            let ids = permissions
                .iter()
                .filter_map(|(id, request)| {
                    if agent_id.is_none_or(|expected| request.agent_id == expected) {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| permissions.remove(&id))
                .collect::<Vec<_>>()
        };
        for request in pending {
            let _ = request.responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }
    }
}

fn bounded_text(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max || value.contains(['\0', '\r', '\n']) {
        return Err(format!("{name} must be bounded non-blank text"));
    }
    Ok(())
}

fn emit(app: &AppHandle, agent_id: &str, kind: &'static str, payload: Value) {
    let _ = app.emit(
        "acp_event",
        AcpEvent {
            agent_id: agent_id.to_owned(),
            kind,
            payload,
        },
    );
}

fn adapter_spec(provider: &str) -> Result<AdapterSpec, String> {
    match provider {
        "claude" => Ok(AdapterSpec {
            package: CLAUDE_ADAPTER,
            package_dir: "claude-0.73.0",
            executable: "@agentclientprotocol/claude-agent-acp/dist/index.js",
        }),
        "codex" => Ok(AdapterSpec {
            package: CODEX_ADAPTER,
            package_dir: "codex-1.8.0",
            executable: "@agentclientprotocol/codex-acp/dist/index.js",
        }),
        _ => Err(format!("{provider} does not have a Sikemux ACP adapter")),
    }
}

fn installed_adapter(root: &Path, spec: AdapterSpec) -> PathBuf {
    root.join("node_modules").join(spec.executable)
}

fn install_failure(stderr: &[u8]) -> String {
    let output = String::from_utf8_lossy(stderr);
    let detail = output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("npm exited without an error message");
    format!(
        "ACP adapter install failed: {}",
        detail.chars().take(512).collect::<String>()
    )
}

async fn ensure_adapter(
    app: &AppHandle,
    manager: &AcpManager,
    agent_id: &str,
    provider: &str,
    cancellation: crate::bounded_process::ProcessCancellation,
) -> Result<PathBuf, String> {
    let spec = adapter_spec(provider)?;
    let root = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("ACP adapter cache is unavailable: {error}"))?
        .join("acp-adapters")
        .join(spec.package_dir);
    let executable = installed_adapter(&root, spec);
    if executable.is_file() {
        return Ok(executable);
    }

    emit(app, agent_id, "status", json!({ "state": "installing" }));
    let install_lock = manager
        .adapter_installs
        .entry(provider.to_owned())
        .or_default()
        .clone();
    let _install = install_lock.lock().await;
    if executable.is_file() {
        return Ok(executable);
    }

    tokio::task::spawn_blocking(move || {
        let parent = root.parent().ok_or("ACP adapter cache has no parent")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(parent)
            .map_err(|error| error.to_string())?;
        let install_root = staging.path();
        let mut command = Command::new("npm");
        command.stdin(Stdio::null());
        command.args([
            "install",
            "--no-save",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--package-lock=false",
            "--loglevel=error",
            "--prefix",
        ]);
        command.arg(install_root).arg(spec.package);
        let output = crate::bounded_process::run(
            &mut command,
            None,
            INSTALL_TIMEOUT,
            INSTALL_OUTPUT_LIMIT,
            Some(&cancellation),
        )
        .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(install_failure(&output.stderr));
        }
        if !installed_adapter(install_root, spec).is_file() {
            return Err("ACP adapter installed without its executable".into());
        }
        if root.exists() {
            std::fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
        }
        std::fs::rename(install_root, &root).map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|error| format!("ACP adapter installer stopped: {error}"))??;

    Ok(executable)
}

fn adapter_config(
    provider: &str,
    executable: &Path,
    config_path: Option<&str>,
    executable_path: Option<&str>,
    environment_keys: &[String],
) -> Result<AcpAgentConfig, String> {
    let mut config = AcpAgentConfig::new("node").arg(executable.to_string_lossy());
    if let Some(path) = config_path {
        bounded_text("config path", path, 4_096)?;
        let path = expand_config_path(path);
        let key = if provider == "claude" {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        config = config.env(key, path.to_string_lossy());
    }
    if let Some(path) = executable_path {
        bounded_text("agent executable", path, 4_096)?;
        let key = if provider == "claude" {
            "CLAUDE_CODE_EXECUTABLE"
        } else {
            "CODEX_PATH"
        };
        config = config.env(key, path);
    }
    let mut seen = HashSet::new();
    for key in environment_keys.iter().take(64) {
        if !seen.insert(key) || !valid_environment_key(key) {
            continue;
        }
        if let Ok(value) = std::env::var(key) {
            config = config.env(key, value);
        }
    }
    Ok(config)
}

fn expand_config_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    let expanded = if trimmed == "~" {
        crate::system::user_home()
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        crate::system::user_home().join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if matches!(
        expanded.file_name().and_then(|value| value.to_str()),
        Some("config.toml" | "settings.json" | "settings.local.json")
    ) {
        expanded.parent().map(PathBuf::from).unwrap_or(expanded)
    } else {
        expanded
    }
}

fn valid_environment_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z' | 'a'..='z'))
        && chars.all(|ch| matches!(ch, '_' | 'A'..='Z' | 'a'..='z' | '0'..='9'))
        && key.len() <= 128
}

fn resource_link(path: &str) -> Result<ContentBlock, String> {
    bounded_text("attachment path", path, 4_096)?;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("attachment paths must be absolute".into());
    }
    let uri = Url::from_file_path(&path)
        .map_err(|_| "attachment path cannot be represented as a file URL")?
        .to_string();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment")
        .to_owned();
    Ok(ContentBlock::ResourceLink(ResourceLink::new(name, uri)))
}

fn prompt_blocks(text: String, paths: Vec<String>) -> Result<Vec<ContentBlock>, String> {
    if text.len() > MAX_PROMPT_BYTES {
        return Err("prompt is too large".into());
    }
    if paths.len() > MAX_ATTACHMENTS {
        return Err(format!(
            "a prompt can include at most {MAX_ATTACHMENTS} attachments"
        ));
    }
    let mut blocks = Vec::with_capacity(paths.len() + usize::from(!text.trim().is_empty()));
    if !text.trim().is_empty() {
        blocks.push(text.into());
    }
    for path in paths {
        blocks.push(resource_link(&path)?);
    }
    if blocks.is_empty() {
        return Err("prompt is empty".into());
    }
    Ok(blocks)
}

fn permission_mode_id(provider: &str, mode: &str, setup: &Value) -> Result<&'static str, String> {
    let expected = match (provider, mode) {
        ("codex", "bypass") => "agent-full-access",
        ("codex", "workspace-write") => "read-only",
        ("claude", "bypass") => "bypassPermissions",
        ("claude", "workspace-write") => "acceptEdits",
        _ => return Err(format!("Unsupported permission mode: {mode}")),
    };
    setup
        .pointer("/modes/availableModes")
        .and_then(Value::as_array)
        .and_then(|modes| {
            modes
                .iter()
                .filter_map(|mode| mode.get("id").and_then(Value::as_str))
                .find(|id| *id == expected)
        })
        .map(|_| expected)
        .ok_or_else(|| format!("The {provider} adapter does not offer permission mode {expected}"))
}

#[tauri::command]
pub async fn acp_set_permission_mode(
    manager: State<'_, AcpManager>,
    agent_id: String,
    permission_mode: String,
) -> Result<(), String> {
    let (reply, response) = oneshot::channel();
    {
        let connection = manager
            .connections
            .get(&agent_id)
            .ok_or("ACP session is not running")?;
        connection
            .commands
            .send(AcpCommand::SetPermissionMode {
                mode: permission_mode,
                reply,
            })
            .map_err(|_| "ACP session stopped")?;
    }
    tokio::time::timeout(Duration::from_secs(15), response)
        .await
        .map_err(|_| "Permission update timed out")?
        .map_err(|_| "ACP session stopped")?
}

#[allow(clippy::too_many_arguments)]
async fn run_connection(
    app: AppHandle,
    manager: AcpManager,
    agent_id: String,
    provider: String,
    cwd: PathBuf,
    resume_id: Option<String>,
    permission_mode: String,
    config_path: Option<String>,
    executable_path: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    environment_keys: Vec<String>,
    install_cancellation: crate::bounded_process::ProcessCancellation,
    mut commands: mpsc::UnboundedReceiver<AcpCommand>,
    ready: ReadySender,
) -> Result<(), String> {
    let agent_executable =
        crate::agents::resolve_agent_executable(&provider, executable_path.as_deref()).await?;
    let executable =
        ensure_adapter(&app, &manager, &agent_id, &provider, install_cancellation).await?;
    emit(&app, &agent_id, "status", json!({ "state": "starting" }));
    let config = adapter_config(
        &provider,
        &executable,
        config_path.as_deref(),
        Some(&agent_executable.to_string_lossy()),
        &environment_keys,
    )?;
    let agent = AcpAgent::new(config);
    let event_app = app.clone();
    let event_agent_id = agent_id.clone();
    let permission_app = app.clone();
    let permission_agent_id = agent_id.clone();
    let permission_manager = manager.clone();

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                match serde_json::to_value(notification) {
                    Ok(value) => emit(&event_app, &event_agent_id, "session_update", value),
                    Err(error) => emit(
                        &event_app,
                        &event_agent_id,
                        "error",
                        json!({ "message": format!("invalid ACP update: {error}") }),
                    ),
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let request_id = Uuid::new_v4().to_string();
                let option_ids = request
                    .options
                    .iter()
                    .map(|option| option.option_id.to_string())
                    .collect();
                let mut payload = serde_json::to_value(&request).unwrap_or_else(|_| json!({}));
                if let Some(object) = payload.as_object_mut() {
                    object.insert("requestId".into(), Value::String(request_id.clone()));
                }
                let Ok(mut permissions) = permission_manager.permissions.lock() else {
                    return responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ));
                };
                permissions.insert(
                    request_id,
                    PendingPermission {
                        agent_id: permission_agent_id.clone(),
                        option_ids,
                        responder,
                    },
                );
                emit(
                    &permission_app,
                    &permission_agent_id,
                    "permission_request",
                    payload,
                );
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| {
            let app = app.clone();
            let agent_id = agent_id.clone();
            let ready = ready.clone();
            async move {
                emit(
                    &app,
                    &agent_id,
                    "status",
                    json!({ "state": "initializing" }),
                );
                let initialize = connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_info(Implementation::new("sikemux", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await?;
                let capabilities = serde_json::to_value(&initialize.agent_capabilities)?;

                let (session_id, mut setup) = if let Some(existing) = resume_id {
                    if !initialize.agent_capabilities.load_session {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data("This agent cannot load existing sessions"));
                    }
                    let response = connection
                        .send_request(LoadSessionRequest::new(existing.clone(), &cwd))
                        .block_task()
                        .await?;
                    (existing, serde_json::to_value(response)?)
                } else {
                    let response = connection
                        .send_request(NewSessionRequest::new(&cwd))
                        .block_task()
                        .await?;
                    let session_id = response.session_id.to_string();
                    (session_id, serde_json::to_value(response)?)
                };

                let mode_id = permission_mode_id(&provider, &permission_mode, &setup)
                    .map_err(|error| agent_client_protocol::Error::invalid_params().data(error))?;
                connection
                    .send_request(SetSessionModeRequest::new(session_id.clone(), mode_id))
                    .block_task()
                    .await?;
                if let Some(modes) = setup.get_mut("modes").and_then(Value::as_object_mut) {
                    modes.insert("currentModeId".into(), json!(mode_id));
                }

                for (config_id, value) in [
                    ("model", model.as_deref()),
                    (
                        if provider == "claude" {
                            "effort"
                        } else {
                            "reasoning_effort"
                        },
                        effort.as_deref(),
                    ),
                ] {
                    if let Some(value) = value {
                        let response = connection
                            .send_request(SetSessionConfigOptionRequest::new(
                                session_id.clone(),
                                config_id,
                                value,
                            ))
                            .block_task()
                            .await?;
                        setup["configOptions"] = serde_json::to_value(response.config_options)?;
                    }
                }

                let start = AcpStartResponse {
                    session_id: session_id.clone(),
                    capabilities,
                    setup,
                };
                if let Ok(mut sender) = ready.lock() {
                    if let Some(sender) = sender.take() {
                        let _ = sender.send(Ok(start.clone()));
                    }
                }
                emit(
                    &app,
                    &agent_id,
                    "ready",
                    serde_json::to_value(&start).unwrap_or_else(|_| json!({})),
                );

                let running = Arc::new(AtomicBool::new(false));
                while let Some(command) = commands.recv().await {
                    match command {
                        AcpCommand::Prompt { text, paths } => {
                            if running.swap(true, Ordering::AcqRel) {
                                emit(
                                    &app,
                                    &agent_id,
                                    "error",
                                    json!({ "message": "wait for the current turn to finish" }),
                                );
                                continue;
                            }
                            let blocks = match prompt_blocks(text, paths) {
                                Ok(blocks) => blocks,
                                Err(error) => {
                                    running.store(false, Ordering::Release);
                                    emit(&app, &agent_id, "error", json!({ "message": error }));
                                    continue;
                                }
                            };
                            emit(&app, &agent_id, "turn_started", json!({}));
                            let response_app = app.clone();
                            let response_agent_id = agent_id.clone();
                            let response_running = running.clone();
                            let response_manager = manager.clone();
                            let sent = connection
                                .send_request(PromptRequest::new(session_id.clone(), blocks))
                                .on_receiving_result(async move |result| {
                                    response_running.store(false, Ordering::Release);
                                    response_manager.cancel_permissions(Some(&response_agent_id));
                                    match result {
                                        Ok(response) => emit(
                                            &response_app,
                                            &response_agent_id,
                                            "turn_completed",
                                            serde_json::to_value(response)
                                                .unwrap_or_else(|_| json!({})),
                                        ),
                                        Err(error) => emit(
                                            &response_app,
                                            &response_agent_id,
                                            "error",
                                            json!({ "message": error.to_string() }),
                                        ),
                                    }
                                    Ok(())
                                });
                            if let Err(error) = sent {
                                running.store(false, Ordering::Release);
                                emit(
                                    &app,
                                    &agent_id,
                                    "error",
                                    json!({ "message": error.to_string() }),
                                );
                            }
                        }
                        AcpCommand::SetPermissionMode { mode, reply } => {
                            let result = if running.load(Ordering::Acquire) {
                                Err("Stop the current turn before changing permissions".into())
                            } else {
                                match permission_mode_id(&provider, &mode, &start.setup) {
                                    Ok(mode_id) => connection
                                        .send_request(SetSessionModeRequest::new(
                                            session_id.clone(),
                                            mode_id,
                                        ))
                                        .block_task()
                                        .await
                                        .map(|_| ())
                                        .map_err(|error| error.to_string()),
                                    Err(error) => Err(error),
                                }
                            };
                            let _ = reply.send(result);
                        }
                        AcpCommand::Cancel => {
                            connection
                                .send_notification(CancelNotification::new(session_id.clone()))?;
                        }
                        AcpCommand::Stop => break,
                    }
                }
                Ok(())
            }
        })
        .await
        .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn acp_start(
    app: AppHandle,
    manager: State<'_, AcpManager>,
    agent_id: String,
    provider: String,
    cwd: String,
    resume_id: Option<String>,
    permission_mode: String,
    config_path: Option<String>,
    executable_path: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    environment_keys: Vec<String>,
) -> Result<AcpStartResponse, String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    bounded_text("provider", &provider, 64)?;
    bounded_text("working directory", &cwd, 4_096)?;
    if let Some(resume_id) = resume_id.as_deref() {
        bounded_text("session id", resume_id, 4_096)?;
    }
    let cwd = PathBuf::from(cwd);
    if !cwd.is_absolute() {
        return Err("ACP working directory must be absolute".into());
    }
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (launch_tx, launch_rx) = oneshot::channel();
    let ready = Arc::new(Mutex::new(Some(ready_tx)));
    let generation = Uuid::new_v4();
    let install_cancellation = crate::bounded_process::ProcessCancellation::new();

    let owned_manager = manager.inner().clone();
    let task_manager = owned_manager.clone();
    let task_agent_id = agent_id.clone();
    let task_app = app.clone();
    let task_ready = ready.clone();
    let task_generation = generation;
    let task_install_cancellation = install_cancellation.clone();
    let task = tauri::async_runtime::spawn(async move {
        if launch_rx.await.is_err() {
            return;
        }
        let result = run_connection(
            task_app.clone(),
            task_manager.clone(),
            task_agent_id.clone(),
            provider,
            cwd,
            resume_id,
            permission_mode,
            config_path,
            executable_path,
            model,
            effort,
            environment_keys,
            task_install_cancellation,
            commands_rx,
            task_ready.clone(),
        )
        .await;
        if let Err(error) = &result {
            emit(
                &task_app,
                &task_agent_id,
                "status",
                json!({ "state": "error" }),
            );
            if let Ok(mut sender) = task_ready.lock() {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(Err(error.clone()));
                }
            }
            emit(
                &task_app,
                &task_agent_id,
                "error",
                json!({ "message": error }),
            );
        } else {
            emit(
                &task_app,
                &task_agent_id,
                "status",
                json!({ "state": "stopped" }),
            );
        }
        task_manager
            .connections
            .remove_if(&task_agent_id, |_, handle| {
                handle.generation == task_generation
            });
        task_manager.cancel_permissions(Some(&task_agent_id));
    });
    let abort = task.inner().abort_handle();
    match manager.connections.entry(agent_id.clone()) {
        Entry::Vacant(entry) => {
            entry.insert(AcpConnectionHandle {
                generation,
                commands: commands_tx,
                abort,
                install_cancellation,
            });
        }
        Entry::Occupied(_) => {
            task.abort();
            return Err("ACP session is already running".into());
        }
    }
    let _ = launch_tx.send(());

    match tokio::time::timeout(START_TIMEOUT, ready_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            if let Some((_, connection)) = owned_manager
                .connections
                .remove_if(&agent_id, |_, handle| handle.generation == generation)
            {
                connection.install_cancellation.cancel();
                connection.abort.abort();
            }
            Err("ACP session stopped before initialization completed".into())
        }
        Err(_) => {
            if let Some((_, connection)) = owned_manager
                .connections
                .remove_if(&agent_id, |_, handle| handle.generation == generation)
            {
                let _ = connection.commands.send(AcpCommand::Stop);
                connection.install_cancellation.cancel();
                connection.abort.abort();
            }
            Err("ACP adapter did not become ready within 150 seconds".into())
        }
    }
}

#[tauri::command]
pub fn acp_prompt(
    manager: State<'_, AcpManager>,
    agent_id: String,
    text: String,
    paths: Vec<String>,
) -> Result<(), String> {
    let Some(connection) = manager.connections.get(&agent_id) else {
        return Err("ACP session is not running".into());
    };
    connection
        .commands
        .send(AcpCommand::Prompt { text, paths })
        .map_err(|_| "ACP session stopped".into())
}

#[tauri::command]
pub fn acp_cancel(manager: State<'_, AcpManager>, agent_id: String) -> Result<(), String> {
    let Some(connection) = manager.connections.get(&agent_id) else {
        return Err("ACP session is not running".into());
    };
    connection
        .commands
        .send(AcpCommand::Cancel)
        .map_err(|_| "ACP session stopped".into())
}

#[tauri::command]
pub fn acp_permission_reply(
    manager: State<'_, AcpManager>,
    agent_id: String,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), String> {
    let request = {
        let mut permissions = manager
            .permissions
            .lock()
            .map_err(|_| "ACP permission state is unavailable")?;
        let Some(request) = permissions.get(&request_id) else {
            return Err("ACP permission request is no longer pending".into());
        };
        if request.agent_id != agent_id {
            return Err("ACP permission request belongs to another agent".into());
        }
        if let Some(option_id) = &option_id {
            if !request.option_ids.contains(option_id) {
                return Err("ACP permission option is invalid".into());
            }
        }
        permissions
            .remove(&request_id)
            .ok_or("ACP permission request is no longer pending")?
    };
    let outcome = match option_id {
        Some(option_id) => {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
        }
        None => RequestPermissionOutcome::Cancelled,
    };
    request
        .responder
        .respond(RequestPermissionResponse::new(outcome))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn acp_stop(manager: State<'_, AcpManager>, agent_id: String) -> Result<(), String> {
    let Some((_, connection)) = manager.connections.remove(&agent_id) else {
        manager.cancel_permissions(Some(&agent_id));
        return Ok(());
    };
    manager.cancel_permissions(Some(&agent_id));
    connection.install_cancellation.cancel();
    let sent = connection
        .commands
        .send(AcpCommand::Stop)
        .map_err(|_| "ACP session already stopped".into());
    connection.abort.abort();
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_commands_are_version_pinned() {
        assert_eq!(adapter_spec("claude").unwrap().package, CLAUDE_ADAPTER);
        assert_eq!(adapter_spec("codex").unwrap().package, CODEX_ADAPTER);
    }

    #[test]
    fn adapter_transport_bypasses_package_manager_stdio() {
        let executable = Path::new("/tmp/claude-agent-acp/dist/index.js");
        let config = adapter_config("claude", executable, None, None, &[]).unwrap();
        assert_eq!(config.command(), Path::new("node"));
        assert_eq!(
            config.arguments(),
            &[executable.to_string_lossy().to_string()]
        );
    }

    #[test]
    fn permissions_use_advertised_provider_modes() {
        for (provider, normal, bypass) in [
            ("codex", "read-only", "agent-full-access"),
            ("claude", "acceptEdits", "bypassPermissions"),
        ] {
            let setup =
                json!({ "modes": { "availableModes": [{ "id": normal }, { "id": bypass }] } });
            assert_eq!(
                permission_mode_id(provider, "workspace-write", &setup).unwrap(),
                normal
            );
            assert_eq!(
                permission_mode_id(provider, "bypass", &setup).unwrap(),
                bypass
            );
            assert!(permission_mode_id(provider, "invalid", &setup).is_err());
            assert!(permission_mode_id(provider, "bypass", &json!({})).is_err());
        }
    }

    #[test]
    fn adapter_uses_selected_executable_and_config() {
        for (provider, executable_key, config_key) in [
            ("codex", "CODEX_PATH", "CODEX_HOME"),
            ("claude", "CLAUDE_CODE_EXECUTABLE", "CLAUDE_CONFIG_DIR"),
        ] {
            let config = adapter_config(
                provider,
                Path::new("/adapter/index.js"),
                Some("/profile"),
                Some("/custom/agent"),
                &[],
            )
            .unwrap();
            assert_eq!(
                config.environment().get(executable_key).map(String::as_str),
                Some("/custom/agent")
            );
            assert_eq!(
                config.environment().get(config_key).map(String::as_str),
                Some("/profile")
            );
        }
    }

    #[test]
    fn prompt_rejects_relative_attachment_paths() {
        let error = prompt_blocks(String::new(), vec!["relative.txt".into()]).unwrap_err();
        assert_eq!(error, "attachment paths must be absolute");
    }

    #[test]
    fn prompt_accepts_text_and_resource_links() {
        let blocks = prompt_blocks("inspect this".into(), vec!["/tmp/example.txt".into()]).unwrap();
        assert_eq!(blocks.len(), 2);
    }
}
