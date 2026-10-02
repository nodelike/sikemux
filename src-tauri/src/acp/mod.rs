//! Chat agents run in the background core, so a turn keeps going when the
//! window reloads or the app quits. The app prepares each launch — the ACP
//! adapter it installs, the agent binary, the browser tools and the
//! environment the agent gets — and forwards every command to the core. The
//! core's events come back on its connection and reach the page as
//! `acp_event`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use sikemux_core::acp::{bounded_text, native};
use sikemux_core::client::{ClientError, CoreClient, Reply};
use sikemux_core::protocol::{
    ChatAttachment, ChatContext, ChatEvent, ChatEventKind, ChatInfo, ChatLaunch, ChatLauncher,
    ChatStart, Request, Response,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::pty::PtyManager;

const CLAUDE_ADAPTER: &str = "@agentclientprotocol/claude-agent-acp@0.81.2";
const CODEX_ADAPTER: &str = "@agentclientprotocol/codex-acp@1.8.0";
const MAX_AGENT_ID: usize = 200;
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
const INSTALL_OUTPUT_LIMIT: usize = 1024 * 1024;
/// Streamed updates in one replayed event, so a long chat comes back in a
/// few script evals rather than one per update.
const REPLAY_BATCH: usize = 2_000;

#[derive(Clone, Copy)]
struct AdapterSpec {
    package: &'static str,
    package_dir: &'static str,
    executable: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpEvent<'a> {
    agent_id: &'a str,
    kind: &'a str,
    payload: Value,
}

/// Names the conversation on disk so the session watcher can leave it alone
/// while a turn writes to it.
#[derive(Clone)]
struct StreamMark {
    provider: String,
    cwd: String,
    config_path: Option<String>,
    session_id: String,
}

impl StreamMark {
    fn set(&self, streaming: bool) {
        crate::agents::note_streaming_session(
            &self.provider,
            &self.cwd,
            self.config_path.as_deref(),
            &self.session_id,
            streaming,
        );
    }
}

#[derive(Default)]
pub struct AcpManager {
    adapter_installs: dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    /// Launches being prepared, which a stop cancels. The id tells a launch
    /// from a later one for the same agent.
    installing: dashmap::DashMap<String, (uuid::Uuid, crate::bounded_process::ProcessCancellation)>,
    /// The chats this app shows, which hear the core's events for them.
    shown: Mutex<HashMap<String, StreamMark>>,
}

impl AcpManager {
    fn show(&self, agent_id: &str, mark: StreamMark, streaming: bool) {
        mark.set(streaming);
        if let Ok(mut shown) = self.shown.lock() {
            if let Some(previous) = shown.insert(agent_id.to_owned(), mark) {
                previous.set(false);
            }
        }
    }

    fn forget(&self, agent_id: &str) {
        let mark = self
            .shown
            .lock()
            .ok()
            .and_then(|mut shown| shown.remove(agent_id));
        if let Some(mark) = mark {
            mark.set(false);
        }
    }

    fn mark(&self, agent_id: &str, streaming: bool) {
        if let Some(mark) = self
            .shown
            .lock()
            .ok()
            .and_then(|shown| shown.get(agent_id).cloned())
        {
            mark.set(streaming);
        }
    }

    fn take_shown(&self) -> Vec<String> {
        let shown = self
            .shown
            .lock()
            .map(|mut shown| std::mem::take(&mut *shown))
            .unwrap_or_default();
        shown
            .into_iter()
            .map(|(agent_id, mark)| {
                mark.set(false);
                agent_id
            })
            .collect()
    }
}

fn emit(app: &AppHandle, agent_id: &str, kind: &str, payload: Value) {
    let _ = app.emit_to(
        "main",
        "acp_event",
        AcpEvent {
            agent_id,
            kind,
            payload,
        },
    );
}

fn kind_name(kind: ChatEventKind) -> &'static str {
    match kind {
        ChatEventKind::Status => "status",
        ChatEventKind::Ready => "ready",
        ChatEventKind::SessionUpdate => "session_update",
        ChatEventKind::Prompt => "prompt",
        ChatEventKind::TurnStarted => "turn_started",
        ChatEventKind::TurnCompleted => "turn_completed",
        ChatEventKind::PermissionRequest => "permission_request",
        ChatEventKind::Error => "error",
    }
}

/// Hands one event from the core to the page, keeping the session watcher's
/// marks in step with the turns.
pub(crate) fn deliver(app: &AppHandle, agent_id: &str, event: ChatEvent) {
    if let Some(manager) = app.try_state::<AcpManager>() {
        match event.kind {
            ChatEventKind::TurnStarted => manager.mark(agent_id, true),
            ChatEventKind::TurnCompleted => manager.mark(agent_id, false),
            ChatEventKind::Status
                if matches!(event.payload["state"].as_str(), Some("stopped" | "error")) =>
            {
                manager.forget(agent_id)
            }
            _ => {}
        }
    }
    emit(app, agent_id, kind_name(event.kind), event.payload);
}

/// Replays a chat to the page as the events it would have heard, with
/// streamed updates sent in large batches.
fn replay(app: &AppHandle, agent_id: &str, events: Vec<ChatEvent>) {
    let mut updates = Vec::new();
    let flush = |updates: &mut Vec<Value>| {
        if !updates.is_empty() {
            emit(
                app,
                agent_id,
                "session_update",
                json!({ "updates": std::mem::take(updates) }),
            );
        }
    };
    for event in events {
        if event.kind == ChatEventKind::SessionUpdate {
            updates.push(event.payload);
            if updates.len() >= REPLAY_BATCH {
                flush(&mut updates);
            }
            continue;
        }
        flush(&mut updates);
        emit(app, agent_id, kind_name(event.kind), event.payload);
    }
    flush(&mut updates);
}

/// The core went away and came back: the same core still runs every chat
/// the page shows, which it attaches to again; a new core has none of them.
pub(crate) async fn reconnected(app: &AppHandle, client: Option<&Arc<CoreClient>>) {
    let Some(manager) = app.try_state::<AcpManager>() else {
        return;
    };
    let shown = manager.take_shown();
    if shown.is_empty() {
        return;
    }
    let live: HashSet<String> = match client {
        Some(client) => client
            .acp_list()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|chat| chat.agent_id)
            .collect(),
        None => HashSet::new(),
    };
    for agent_id in shown {
        if live.contains(&agent_id) {
            emit(app, &agent_id, "reattach", json!({}));
        } else {
            emit(
                app,
                &agent_id,
                "status",
                json!({ "state": "stopped", "reason": "exited" }),
            );
        }
    }
}

fn adapter_spec(provider: &str) -> Result<AdapterSpec, String> {
    match provider {
        "claude" => Ok(AdapterSpec {
            package: CLAUDE_ADAPTER,
            package_dir: "claude-0.81.2",
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

fn adapter_root(app: &AppHandle, spec: AdapterSpec) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("ACP adapter cache is unavailable: {error}"))?
        .join("acp-adapters")
        .join(spec.package_dir))
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
    let root = adapter_root(app, spec)?;
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
        let mut command = sikemux_process::user_environment::command("npm");
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

/// What starts an agent: a program, its arguments and the environment it
/// gets on top of the core's.
struct Program {
    program: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

fn adapter_program(
    provider: &str,
    executable: &Path,
    config_path: Option<&str>,
    executable_path: Option<&str>,
    environment_keys: &[String],
) -> Result<Program, String> {
    let mut env = forwarded_environment(environment_keys);
    if let Some(path) = config_path {
        bounded_text("config path", path, 4_096)?;
        let path = expand_config_path(path);
        let key = if provider == "claude" {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        env.insert(key.into(), path.to_string_lossy().into_owned());
    }
    if let Some(path) = executable_path {
        bounded_text("agent executable", path, 4_096)?;
        let key = if provider == "claude" {
            "CLAUDE_CODE_EXECUTABLE"
        } else {
            "CODEX_PATH"
        };
        env.insert(key.into(), path.into());
    }
    Ok(Program {
        program: crate::system::find_executable("node").unwrap_or_else(|| PathBuf::from("node")),
        args: vec![executable.to_string_lossy().into_owned()],
        env,
    })
}

fn native_program(executable: &Path, arguments: &[&str], environment_keys: &[String]) -> Program {
    Program {
        program: executable.to_path_buf(),
        args: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        env: forwarded_environment(environment_keys),
    }
}

/// The variables the person's profile names, with this app's values, since
/// the core may have started before they were set.
fn forwarded_environment(environment_keys: &[String]) -> BTreeMap<String, String> {
    let mut seen = HashSet::new();
    let mut env = BTreeMap::new();
    for key in environment_keys.iter().take(64) {
        if !seen.insert(key) || !valid_environment_key(key) {
            continue;
        }
        if let Ok(value) = std::env::var(key) {
            env.insert(key.clone(), value);
        }
    }
    env
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

fn failure(error: ClientError) -> String {
    error.to_string()
}

async fn core(pty: &PtyManager) -> Result<Arc<CoreClient>, String> {
    pty.client().await.map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn prepare(
    app: &AppHandle,
    manager: &AcpManager,
    agent_id: &str,
    provider: &str,
    config_path: Option<&str>,
    executable_path: Option<&str>,
    environment_keys: &[String],
    cancellation: crate::bounded_process::ProcessCancellation,
) -> Result<Program, String> {
    let agent_executable =
        crate::agents::resolve_agent_executable(provider, executable_path).await?;
    let mut program = match native::arguments(provider) {
        Some(arguments) => native_program(&agent_executable, arguments, environment_keys),
        None => {
            let adapter = ensure_adapter(app, manager, agent_id, provider, cancellation).await?;
            adapter_program(
                provider,
                &adapter,
                config_path,
                Some(&agent_executable.to_string_lossy()),
                environment_keys,
            )?
        }
    };
    program
        .env
        .extend(crate::model_providers::environment(provider).await);
    program
        .env
        .insert(crate::ports::AGENT_ID_ENV.into(), agent_id.to_owned());
    Ok(program)
}

/// One way to start a chat agent that the app offers paired devices.
pub(crate) struct LauncherSpec {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub config_path: Option<String>,
    pub executable_path: Option<String>,
    pub environment_keys: Vec<String>,
    pub permission_mode: String,
}

/// What [`acp_start`] would run for `spec`, for the core to start without
/// the window. Never installs an adapter: one this Mac has not used yet is
/// left out until it has.
pub(crate) async fn launcher(app: &AppHandle, spec: LauncherSpec) -> Result<ChatLauncher, String> {
    let executable =
        crate::agents::resolve_agent_executable(&spec.provider, spec.executable_path.as_deref())
            .await?;
    let mut program = match native::arguments(&spec.provider) {
        Some(arguments) => native_program(&executable, arguments, &spec.environment_keys),
        None => {
            let adapter_spec = adapter_spec(&spec.provider)?;
            let adapter = installed_adapter(&adapter_root(app, adapter_spec)?, adapter_spec);
            if !adapter.is_file() {
                return Err(format!(
                    "{} has not been started on this Mac yet",
                    spec.label
                ));
            }
            adapter_program(
                &spec.provider,
                &adapter,
                spec.config_path.as_deref(),
                Some(&executable.to_string_lossy()),
                &spec.environment_keys,
            )?
        }
    };
    program
        .env
        .extend(crate::model_providers::environment(&spec.provider).await);
    Ok(ChatLauncher {
        id: spec.id,
        provider: spec.provider,
        label: spec.label,
        program: program.program,
        args: program.args,
        env: program.env,
        permission_mode: spec.permission_mode,
    })
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn acp_start(
    app: AppHandle,
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
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
) -> Result<ChatStart, String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    bounded_text("provider", &provider, 64)?;
    bounded_text("working directory", &cwd, 4_096)?;
    let launch_id = uuid::Uuid::new_v4();
    let cancellation = crate::bounded_process::ProcessCancellation::new();
    manager
        .installing
        .insert(agent_id.clone(), (launch_id, cancellation.clone()));
    let prepared = prepare(
        &app,
        &manager,
        &agent_id,
        &provider,
        config_path.as_deref(),
        executable_path.as_deref(),
        &environment_keys,
        cancellation,
    )
    .await;
    let program = match prepared {
        Ok(program) => program,
        Err(error) => {
            manager
                .installing
                .remove_if(&agent_id, |_, (id, _)| *id == launch_id);
            return Err(error);
        }
    };
    // The tools this agent can drive its own browser tabs with. A session
    // that cannot be told about them still runs.
    let mcp_servers = match crate::browser::agents::acp_browser_server(&app, &agent_id) {
        Ok(server) => serde_json::to_value(server).into_iter().collect(),
        Err(error) => {
            eprintln!("Sikemux browser tools are unavailable to this agent: {error}");
            Vec::new()
        }
    };
    let launch = ChatLaunch {
        agent_id: agent_id.clone(),
        provider: provider.clone(),
        cwd: PathBuf::from(&cwd),
        program: program.program,
        args: program.args,
        env: program.env,
        mcp_servers,
        resume_id: resume_id.clone(),
        permission_mode,
        model,
        effort,
    };
    let client = core(&pty).await?;
    let started = client
        .submit(
            Request::AcpStart {
                launch: Box::new(launch),
            },
            |reply| match reply {
                Ok(Reply::Response(Response::ChatStarted { start })) => Ok(start),
                Ok(_) => Err(ClientError::UnexpectedReply),
                Err(error) => Err(error),
            },
        )
        .map_err(failure)?;
    // A stop that came while the launch was prepared reaches the core after
    // the start, whichever of the two it raced.
    let stopped = manager
        .installing
        .remove_if(&agent_id, |_, (id, _)| *id == launch_id)
        .is_none();
    if stopped {
        let _ = client.acp_stop(agent_id).await;
        return Err("ACP session stopped before initialization completed".into());
    }
    let start = started.await.map_err(failure)?.map_err(failure)?;
    manager.show(
        &agent_id,
        StreamMark {
            provider: provider.clone(),
            cwd: cwd.clone(),
            config_path: config_path.clone(),
            session_id: start.session_id.clone(),
        },
        false,
    );
    let session_id = start.session_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::activity::record_launch(
            &provider,
            &cwd,
            "chat",
            resume_id.is_some().then_some(session_id.as_str()),
            config_path.as_deref(),
        )
    });
    Ok(start)
}

/// What the page hears back from an attach. The replay itself arrives as
/// `acp_event`s before any live event.
#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Attachment {
    Live {
        start: ChatStart,
        permission_mode: String,
        running: bool,
        turned: bool,
    },
    Missing,
    Restart,
}

/// Takes up a chat the core already runs, if it runs one for this agent.
#[tauri::command]
pub async fn acp_attach(
    app: AppHandle,
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
    agent_id: String,
    provider: String,
    cwd: String,
    config_path: Option<String>,
) -> Result<Attachment, String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    let client = core(&pty).await?;
    let replaying = app.clone();
    let replay_agent = agent_id.clone();
    // The replay goes to the page on the connection's reader, before any
    // event the core sent after it.
    let replied = client
        .submit(
            Request::AcpAttach {
                agent_id: agent_id.clone(),
                since: None,
            },
            move |reply| match reply {
                Ok(Reply::Response(Response::ChatAttached { attachment })) => {
                    Ok(match attachment {
                        ChatAttachment::Live {
                            start,
                            permission_mode,
                            running,
                            turned,
                            replay: events,
                            ..
                        } => {
                            replay(&replaying, &replay_agent, events);
                            Attachment::Live {
                                start: *start,
                                permission_mode,
                                running,
                                turned,
                            }
                        }
                        ChatAttachment::Missing => Attachment::Missing,
                        // Only an attach that names a mark is resumed, and this one names none.
                        ChatAttachment::Restart | ChatAttachment::Resumed { .. } => {
                            Attachment::Restart
                        }
                    })
                }
                Ok(_) => Err(ClientError::UnexpectedReply),
                Err(error) => Err(error),
            },
        )
        .map_err(failure)?;
    let attachment = replied.await.map_err(failure)?.map_err(failure)?;
    if let Attachment::Live { start, running, .. } = &attachment {
        manager.show(
            &agent_id,
            StreamMark {
                provider,
                cwd,
                config_path,
                session_id: start.session_id.clone(),
            },
            *running,
        );
    }
    Ok(attachment)
}

/// The chats the core runs, for the page to stop the ones it no longer shows.
#[tauri::command]
pub async fn acp_list(pty: State<'_, PtyManager>) -> Result<Vec<ChatInfo>, String> {
    core(&pty).await?.acp_list().await.map_err(failure)
}

#[tauri::command]
pub async fn acp_set_permission_mode(
    pty: State<'_, PtyManager>,
    agent_id: String,
    permission_mode: String,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_set_permission_mode(agent_id, permission_mode)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_set_config(
    pty: State<'_, PtyManager>,
    agent_id: String,
    config_id: String,
    value: String,
) -> Result<Value, String> {
    core(&pty)
        .await?
        .acp_set_config(agent_id, config_id, value)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_prompt(
    pty: State<'_, PtyManager>,
    agent_id: String,
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_prompt(agent_id, text, paths, context)
        .await
        .map_err(failure)
}

/// Puts a message into the running turn, answering `promptRequired` when the
/// turn ended first and the caller should send it as a prompt of its own.
#[tauri::command]
pub async fn acp_steer(
    pty: State<'_, PtyManager>,
    agent_id: String,
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
) -> Result<String, String> {
    core(&pty)
        .await?
        .acp_steer(agent_id, text, paths, context)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_cancel(pty: State<'_, PtyManager>, agent_id: String) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_cancel(agent_id)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_stop_task(
    pty: State<'_, PtyManager>,
    agent_id: String,
    task_id: String,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_stop_task(agent_id, task_id)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_permission_reply(
    pty: State<'_, PtyManager>,
    agent_id: String,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_permission_reply(agent_id, request_id, option_id)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_stop(
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
    agent_id: String,
) -> Result<(), String> {
    if let Some((_, (_, installing))) = manager.installing.remove(&agent_id) {
        installing.cancel();
    }
    manager.forget(&agent_id);
    core(&pty).await?.acp_stop(agent_id).await.map_err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_transport_bypasses_package_manager_stdio() {
        let executable = Path::new("/tmp/claude-agent-acp/dist/index.js");
        let program = adapter_program("claude", executable, None, None, &[]).unwrap();
        assert_eq!(
            program.program.file_name().and_then(|name| name.to_str()),
            Some("node")
        );
        assert_eq!(program.args, [executable.to_string_lossy().to_string()]);
    }

    #[test]
    fn adapter_uses_selected_executable_and_config() {
        for (provider, executable_key, config_key) in [
            ("codex", "CODEX_PATH", "CODEX_HOME"),
            ("claude", "CLAUDE_CODE_EXECUTABLE", "CLAUDE_CONFIG_DIR"),
        ] {
            let program = adapter_program(
                provider,
                Path::new("/adapter/index.js"),
                Some("/profile"),
                Some("/custom/agent"),
                &[],
            )
            .unwrap();
            assert_eq!(
                program.env.get(executable_key).map(String::as_str),
                Some("/custom/agent")
            );
            assert_eq!(
                program.env.get(config_key).map(String::as_str),
                Some("/profile")
            );
        }
    }

    #[test]
    fn native_agents_run_their_own_binary_in_acp_mode() {
        let program = native_program(
            Path::new("/bin/grok"),
            native::arguments("grok").unwrap(),
            &[],
        );
        assert_eq!(program.program, Path::new("/bin/grok"));
        assert_eq!(program.args, ["agent", "--no-leader", "stdio"]);
    }

    #[test]
    fn only_named_valid_variables_are_forwarded() {
        let env = forwarded_environment(&["PATH".into(), "PATH".into(), "BAD-KEY".into()]);
        assert_eq!(env.keys().collect::<Vec<_>>(), ["PATH"]);
    }

    #[test]
    fn config_files_name_their_directory() {
        assert_eq!(
            expand_config_path("/home/me/.codex/config.toml"),
            Path::new("/home/me/.codex")
        );
        assert_eq!(
            expand_config_path("/home/me/.claude"),
            Path::new("/home/me/.claude")
        );
    }

    #[test]
    fn every_event_kind_keeps_the_name_the_page_reads() {
        for (kind, name) in [
            (ChatEventKind::Status, "status"),
            (ChatEventKind::Ready, "ready"),
            (ChatEventKind::SessionUpdate, "session_update"),
            (ChatEventKind::TurnStarted, "turn_started"),
            (ChatEventKind::TurnCompleted, "turn_completed"),
            (ChatEventKind::PermissionRequest, "permission_request"),
            (ChatEventKind::Error, "error"),
        ] {
            assert_eq!(kind_name(kind), name);
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
    }

    #[test]
    fn an_attachment_reads_as_the_page_expects() {
        let live = Attachment::Live {
            start: ChatStart {
                session_id: "s".into(),
                capabilities: json!({}),
                setup: json!({}),
            },
            permission_mode: "bypass".into(),
            running: true,
            turned: false,
        };
        assert_eq!(
            serde_json::to_value(live).unwrap(),
            json!({
                "status": "live",
                "start": { "sessionId": "s", "capabilities": {}, "setup": {} },
                "permissionMode": "bypass",
                "running": true,
                "turned": false,
            })
        );
        assert_eq!(
            serde_json::to_value(Attachment::Restart).unwrap(),
            json!({ "status": "restart" })
        );
    }
}
