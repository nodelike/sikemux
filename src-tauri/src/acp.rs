use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, Implementation, InitializeRequest, LoadSessionRequest,
    NewSessionRequest, PermissionOptionKind, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResourceLink, SelectedPermissionOutcome,
    SessionNotification,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, Responder};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{mpsc, oneshot};
use url::Url;
use uuid::Uuid;

const CLAUDE_ADAPTER: &str = "@agentclientprotocol/claude-agent-acp@0.73.0";
const CODEX_ADAPTER: &str = "@agentclientprotocol/codex-acp@1.8.0";
const MAX_AGENT_ID: usize = 200;
const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 32;
const START_TIMEOUT: Duration = Duration::from_secs(45);
type ReadySender = Arc<Mutex<Option<oneshot::Sender<Result<AcpStartResponse, String>>>>>;

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
    Prompt { text: String, paths: Vec<String> },
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
    commands: mpsc::UnboundedSender<AcpCommand>,
}

#[derive(Clone, Default)]
pub struct AcpManager {
    connections: Arc<dashmap::DashMap<String, AcpConnectionHandle>>,
    permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
}

impl AcpManager {
    pub fn drain(&self) {
        for item in self.connections.iter() {
            let _ = item.commands.send(AcpCommand::Stop);
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

fn adapter_config(
    provider: &str,
    config_path: Option<&str>,
    environment_keys: &[String],
) -> Result<AcpAgentConfig, String> {
    let package = match provider {
        "claude" => CLAUDE_ADAPTER,
        "codex" => CODEX_ADAPTER,
        _ => return Err(format!("{provider} does not have a Sikemux ACP adapter")),
    };
    let mut config = AcpAgentConfig::new("npx").args(["-y", package]);
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
    environment_keys: Vec<String>,
    mut commands: mpsc::UnboundedReceiver<AcpCommand>,
    ready: ReadySender,
) -> Result<(), String> {
    let config = adapter_config(&provider, config_path.as_deref(), &environment_keys)?;
    let agent = AcpAgent::new(config);
    let event_app = app.clone();
    let event_agent_id = agent_id.clone();
    let permission_app = app.clone();
    let permission_agent_id = agent_id.clone();
    let permission_manager = manager.clone();
    let bypass = permission_mode == "bypass";

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
                if bypass {
                    let selected = request.options.iter().find(|option| {
                        matches!(
                            option.kind,
                            PermissionOptionKind::AllowAlways | PermissionOptionKind::AllowOnce
                        )
                    });
                    return match selected {
                        Some(option) => responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                option.option_id.clone(),
                            )),
                        )),
                        None => responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        )),
                    };
                }

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

                let (session_id, setup) = if let Some(existing) =
                    resume_id.filter(|_| initialize.agent_capabilities.load_session)
                {
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
                            let sent = connection
                                .send_request(PromptRequest::new(session_id.clone(), blocks))
                                .on_receiving_result(async move |result| {
                                    response_running.store(false, Ordering::Release);
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
    if manager.connections.contains_key(&agent_id) {
        return Err("ACP session is already running".into());
    }

    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let ready = Arc::new(Mutex::new(Some(ready_tx)));
    manager.connections.insert(
        agent_id.clone(),
        AcpConnectionHandle {
            commands: commands_tx,
        },
    );

    let owned_manager = manager.inner().clone();
    let task_manager = owned_manager.clone();
    let task_agent_id = agent_id.clone();
    let task_app = app.clone();
    let task_ready = ready.clone();
    tauri::async_runtime::spawn(async move {
        let result = run_connection(
            task_app.clone(),
            task_manager.clone(),
            task_agent_id.clone(),
            provider,
            cwd,
            resume_id,
            permission_mode,
            config_path,
            environment_keys,
            commands_rx,
            task_ready.clone(),
        )
        .await;
        if let Err(error) = &result {
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
        task_manager.connections.remove(&task_agent_id);
        task_manager.cancel_permissions(Some(&task_agent_id));
    });

    match tokio::time::timeout(START_TIMEOUT, ready_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            owned_manager.connections.remove(&agent_id);
            Err("ACP session stopped before initialization completed".into())
        }
        Err(_) => {
            if let Some((_, connection)) = owned_manager.connections.remove(&agent_id) {
                let _ = connection.commands.send(AcpCommand::Stop);
            }
            Err("ACP session did not initialize within 45 seconds".into())
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
    connection
        .commands
        .send(AcpCommand::Stop)
        .map_err(|_| "ACP session already stopped".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_commands_are_version_pinned() {
        let claude = adapter_config("claude", None, &[]).unwrap();
        let codex = adapter_config("codex", None, &[]).unwrap();
        assert_eq!(claude.arguments(), &["-y", CLAUDE_ADAPTER]);
        assert_eq!(codex.arguments(), &["-y", CODEX_ADAPTER]);
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
