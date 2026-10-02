use std::time::Duration;

use sikemux_core::client::{Attached, ClientError, Reply};
use sikemux_core::protocol::{
    Continuation, LaunchIdentity, Request, Response as CoreResponse, SessionExit, SessionId,
    SessionInfo, SpawnTarget, TerminalSpawn,
};
use sikemux_pty::agent_detection::{DetectionExplain, ManifestReloadReport};
use sikemux_pty::launch::{PtyContext, PtyDirectCommand};
use sikemux_pty::shell_protocol::ShellMetadataSnapshot;
use sikemux_pty::task::{TaskProcessExit, TaskSpawnRequest};
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::observability::{global_observability, Metadata, SpanOutcome};

use super::streams::Release;
use super::{core_error, PtyManager};

fn launch_identity(app: &AppHandle) -> LaunchIdentity {
    LaunchIdentity {
        version: app.package_info().version.to_string(),
        cli_executable: crate::cli_paths::cli_executable_path(),
        cli_endpoint: crate::cli_paths::cli_endpoint_path(),
    }
}

fn spawned(reply: Result<Reply, ClientError>) -> AppResult<SessionId> {
    match reply.map_err(core_error)? {
        Reply::Response(CoreResponse::Spawned { id }) => Ok(id),
        _ => Err(core_error(ClientError::UnexpectedReply)),
    }
}

fn finish<T>(
    outcome: AppResult<T>,
    operation: crate::observability::SlowOperationGuard,
) -> AppResult<T> {
    operation.finish(if outcome.is_ok() {
        SpanOutcome::Success
    } else {
        SpanOutcome::Error
    });
    outcome
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pty_spawn(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    browser: State<'_, crate::browser::BrowserManager>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    startup: Option<String>,
    direct_command: Option<PtyDirectCommand>,
    context: Option<PtyContext>,
    continues: Option<Continuation>,
) -> AppResult<SessionId> {
    let mut direct_command = direct_command;
    let agent_launch = match (direct_command.as_ref(), context.as_ref()) {
        (Some(command), Some(context)) => {
            match (context.agent_type.clone(), context.project.clone()) {
                (Some(agent), Some(project)) => {
                    let resumed = crate::activity::resumed_session_in_args(&agent, &command.args)
                        .map(str::to_string);
                    let config_path = command
                        .profile
                        .as_ref()
                        .and_then(|profile| profile.config_path.clone());
                    Some((agent, project, resumed, config_path))
                }
                _ => None,
            }
        }
        _ => None,
    };
    // An agent can only reach the browser tools if its own host is told they
    // exist, and every host is told differently (see browser::agents). A host
    // that cannot be told still launches, without them.
    let mut env: Vec<(String, String)> = match (direct_command.as_mut(), context.as_ref()) {
        (Some(command), Some(context)) => {
            match (context.agent_id.as_deref(), context.agent_type.as_deref()) {
                (Some(agent_id), Some(agent_type))
                    if crate::browser::agents::is_supported(agent_type) =>
                {
                    match browser
                        .agent_integration(&app, agent_id, agent_type, &command.program)
                        .await
                    {
                        Ok(integration) => integration.apply(&mut command.args),
                        Err(error) => {
                            eprintln!("Sikemux browser integration is unavailable: {error}");
                            Default::default()
                        }
                    }
                }
                _ => Default::default(),
            }
        }
        _ => Default::default(),
    };
    if let (Some(_), Some(agent_type)) = (
        direct_command.as_ref(),
        context
            .as_ref()
            .and_then(|context| context.agent_type.as_deref()),
    ) {
        env.extend(crate::model_providers::environment(agent_type).await);
    }
    let client = manager.client().await?;
    let id = client
        .spawn(
            launch_identity(&app),
            SpawnTarget::Terminal(TerminalSpawn {
                cols,
                rows,
                cwd,
                startup,
                direct_command,
                context,
                env: env.into_iter().collect(),
                continues,
            }),
        )
        .await
        .map_err(core_error)?;
    if let Some((agent, project, resumed, config_path)) = agent_launch {
        tauri::async_runtime::spawn_blocking(move || {
            crate::activity::record_launch(
                &agent,
                &project,
                "terminal",
                resumed.as_deref(),
                config_path.as_deref(),
            )
        });
    }
    Ok(id)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSpawnResult {
    pty_id: SessionId,
}

#[tauri::command]
pub async fn task_spawn(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    request: TaskSpawnRequest,
    on_exit: Channel<TaskProcessExit>,
) -> AppResult<TaskSpawnResult> {
    let operation = global_observability().slow_operation(
        "pty.task_spawn",
        Duration::from_millis(50),
        None,
        Metadata::new(),
    );
    let outcome = async {
        let client = manager.client().await?;
        let streams = manager.streams.clone();
        // Registered as the reply lands, so an exit the core sends right
        // after it finds the channel.
        let replied = client
            .submit(
                Request::Spawn {
                    launch: launch_identity(&app),
                    target: Box::new(SpawnTarget::Task { request }),
                },
                move |reply| {
                    let id = spawned(reply)?;
                    streams.lock()?.register_task(id, on_exit);
                    Ok(id)
                },
            )
            .map_err(core_error)?;
        replied.await.map_err(core_error)?
    }
    .await;
    finish(outcome, operation).map(|pty_id| TaskSpawnResult { pty_id })
}

/// Everything about an attach except the replay bytes, which follow the
/// header in the same raw response instead of crossing as JSON numbers.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachResult {
    pub sub_id: u32,
    pub alternate_screen: bool,
    /// The latest shell state, so a remounted pane recovers its metadata
    /// although the OSC bytes that set it are not replayed.
    pub shell: Option<ShellMetadataSnapshot>,
}

/// `[header length as 4 little-endian bytes][header JSON][replay bytes]`.
fn encode_attach_response(header: &AttachResult, replay: &[u8]) -> AppResult<Vec<u8>> {
    let json = serde_json::to_vec(header)?;
    let mut body = Vec::with_capacity(4 + json.len() + replay.len());
    body.extend_from_slice(&(json.len() as u32).to_le_bytes());
    body.extend_from_slice(&json);
    body.extend_from_slice(replay);
    Ok(body)
}

/// Snapshot and subscribe in one step: the channel joins on the connection's
/// reader as the replay lands, so it receives exactly the bytes the replay
/// does not hold.
#[tauri::command]
pub async fn pty_attach(
    manager: State<'_, PtyManager>,
    id: SessionId,
    on_event: Channel<Response>,
) -> AppResult<Response> {
    let operation = global_observability().slow_operation(
        "pty.attach",
        Duration::from_millis(16),
        None,
        Metadata::new(),
    );
    let outcome = async {
        let client = manager.client().await?;
        let streams = manager.streams.clone();
        let replied = {
            let mut guard = manager.streams.lock()?;
            guard.begin_attach(id)?;
            let submitted = client.submit(Request::Attach { id }, move |reply| {
                let mut guard = streams.lock()?;
                match reply {
                    Ok(Reply::Attached(Attached {
                        alternate_screen,
                        shell,
                        exited,
                        replay,
                    })) => {
                        let sub_id = guard.finish_attach(id, on_event.clone())?;
                        // The exit went out before this replay, so only this
                        // channel has yet to hear of it, and it hears after
                        // the replay is shown.
                        if exited {
                            let _ = on_event.send(Response::new(Vec::new()));
                        }
                        encode_attach_response(
                            &AttachResult {
                                sub_id,
                                alternate_screen,
                                shell,
                            },
                            &replay,
                        )
                    }
                    Ok(Reply::Response(_)) => {
                        guard.cancel_attach(id);
                        Err(core_error(ClientError::UnexpectedReply))
                    }
                    Err(error) => {
                        guard.cancel_attach(id);
                        Err(core_error(error))
                    }
                }
            });
            if submitted.is_err() {
                guard.cancel_attach(id);
            }
            submitted.map_err(core_error)?
        };
        replied.await.map_err(core_error)?
    }
    .await;
    finish(outcome, operation).map(Response::new)
}

/// Live output only, for a pane that already holds the screen.
#[tauri::command]
pub async fn pty_subscribe(
    manager: State<'_, PtyManager>,
    id: SessionId,
    on_event: Channel<Response>,
) -> AppResult<u32> {
    let client = manager.client().await?;
    let streams = manager.streams.clone();
    let replied = {
        let mut guard = manager.streams.lock()?;
        if guard.is_core_subscribed(id) {
            return guard.add_channel(id, on_event);
        }
        guard.begin_attach(id)?;
        let submitted = client.submit(Request::Subscribe { id }, move |reply| {
            let mut guard = streams.lock()?;
            match reply {
                Ok(Reply::Response(CoreResponse::Done)) => {
                    guard.cancel_attach(id);
                    guard.add_channel(id, on_event)
                }
                Ok(_) => {
                    guard.cancel_attach(id);
                    Err(core_error(ClientError::UnexpectedReply))
                }
                Err(error) => {
                    guard.cancel_attach(id);
                    Err(core_error(error))
                }
            }
        });
        if submitted.is_err() {
            guard.cancel_attach(id);
        }
        submitted.map_err(core_error)?
    };
    replied.await.map_err(core_error)?
}

#[tauri::command]
pub async fn pty_unsubscribe(
    manager: State<'_, PtyManager>,
    id: SessionId,
    sub_id: u32,
) -> AppResult<()> {
    let Some(client) = manager.current() else {
        return Ok(());
    };
    let detached = {
        let mut guard = manager.streams.lock()?;
        match guard.unsubscribe(id, sub_id) {
            Release::Nothing => None,
            Release::Ack(bytes) => {
                client.ack(id, bytes);
                None
            }
            // Queued while the table is locked, so an attach decided after
            // this reaches the core after it.
            Release::Detach => Some(client.submit(Request::Detach { id }, |_| ())),
        }
    };
    if let Some(Ok(detached)) = detached {
        let _ = detached.await;
    }
    Ok(())
}

/// Reports how many delivered bytes a renderer has finished writing, which
/// lets the core read more from the program.
#[tauri::command]
pub async fn pty_ack(
    manager: State<'_, PtyManager>,
    id: SessionId,
    sub_id: u32,
    bytes: usize,
) -> AppResult<()> {
    if let Some(forward) = manager.streams.ack(id, sub_id, bytes) {
        if let Some(client) = manager.current() {
            client.ack(id, forward);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn pty_write(
    manager: State<'_, PtyManager>,
    id: SessionId,
    data: String,
) -> AppResult<()> {
    let operation = global_observability().slow_operation(
        "pty.write",
        Duration::from_millis(8),
        None,
        Metadata::new(),
    );
    let outcome = async {
        let client = manager.client().await?;
        client.write(id, data.as_bytes()).await.map_err(core_error)
    }
    .await;
    finish(outcome, operation)
}

#[tauri::command]
pub async fn pty_resize(
    manager: State<'_, PtyManager>,
    id: SessionId,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    sikemux_pty::validate_pty_dimensions(cols, rows)?;
    let client = manager.client().await?;
    client.resize(id, cols, rows).await.map_err(core_error)
}

#[tauri::command]
pub async fn pty_reset_modes(manager: State<'_, PtyManager>, id: SessionId) -> AppResult<()> {
    let client = manager.client().await?;
    client.reset_modes(id).await.map_err(core_error)
}

#[tauri::command]
pub async fn pty_kill(manager: State<'_, PtyManager>, id: SessionId) -> AppResult<()> {
    // Panes render "[process exited]" before the unmount tears them down.
    manager.streams.channels(id).send(&[]);
    let client = manager.client().await?;
    client.kill(id).await.map_err(core_error)
}

/// Every session the core holds, running or kept after it ended.
#[tauri::command]
pub async fn pty_sessions(manager: State<'_, PtyManager>) -> AppResult<Vec<SessionInfo>> {
    manager.sessions().await
}

/// Takes over a task this page did not start: `on_exit` hears how it ends,
/// at once if it already has.
#[tauri::command]
pub async fn task_watch(
    manager: State<'_, PtyManager>,
    id: SessionId,
    on_exit: Channel<TaskProcessExit>,
) -> AppResult<()> {
    manager.streams.lock()?.register_task(id, on_exit);
    let session = manager
        .sessions()
        .await?
        .into_iter()
        .find(|session| session.id == id);
    let ended = match session {
        Some(session) if session.running => return Ok(()),
        Some(session) => session.exit.unwrap_or(SessionExit {
            code: None,
            signal: None,
        }),
        None => SessionExit {
            code: None,
            signal: None,
        },
    };
    if let Some(channel) = manager.streams.lock()?.take_task_exit(id) {
        let _ = channel.send(super::sink::task_exit(ended.code, ended.signal));
    }
    Ok(())
}

#[tauri::command]
pub fn app_quit_and_stop_everything(app: AppHandle) {
    super::quit_and_stop_everything(&app);
}

#[tauri::command]
pub async fn agent_detection_manifests(
    manager: State<'_, PtyManager>,
) -> AppResult<ManifestReloadReport> {
    let client = manager.client().await?;
    client.list_manifests().await.map_err(core_error)
}

#[tauri::command]
pub async fn agent_detection_reload(
    manager: State<'_, PtyManager>,
) -> AppResult<ManifestReloadReport> {
    let client = manager.client().await?;
    client.reload_manifests().await.map_err(core_error)
}

#[tauri::command]
pub async fn agent_detection_explain(
    manager: State<'_, PtyManager>,
    agent_id: String,
) -> AppResult<DetectionExplain> {
    let client = manager.client().await?;
    client
        .explain_agent_detection(agent_id)
        .await
        .map_err(core_error)
}

#[cfg(test)]
mod tests {
    use super::{encode_attach_response, AttachResult, TaskSpawnResult};

    #[test]
    fn attach_response_frames_the_header_before_the_replay_bytes() {
        let header = AttachResult {
            sub_id: 9,
            alternate_screen: false,
            shell: None,
        };
        let body = encode_attach_response(&header, b"hello").expect("encode attach");
        let header_len = u32::from_le_bytes(body[..4].try_into().expect("length prefix")) as usize;
        let parsed: serde_json::Value =
            serde_json::from_slice(&body[4..4 + header_len]).expect("header json");
        assert_eq!(parsed["subId"], 9);
        assert_eq!(parsed["alternateScreen"], false);
        assert_eq!(&body[4 + header_len..], b"hello");
    }

    #[test]
    fn task_spawn_result_carries_the_whole_session_id() {
        assert_eq!(
            serde_json::to_value(TaskSpawnResult {
                pty_id: 1_759_300_000_123
            })
            .expect("serialize spawn"),
            serde_json::json!({ "ptyId": 1_759_300_000_123u64 })
        );
    }
}
