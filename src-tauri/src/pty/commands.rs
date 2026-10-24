use std::sync::Arc;
use std::time::Duration;

use sikemux_core::client::{Attached, ClientError, CoreClient, Reply};
use sikemux_core::grid::{FindOptions, FindResult, GridPoint, TerminalInput};
use sikemux_core::protocol::{
    Continuation, LaunchIdentity, Request, SessionId, SessionInfo, SpawnTarget, TerminalSpawn,
};
use sikemux_pty::agent_detection::{DetectionExplain, ManifestReloadReport};
use sikemux_pty::launch::{PtyContext, PtyDirectCommand};
use sikemux_pty::shell_protocol::ShellMetadataSnapshot;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::hosts::Hosts;
use crate::observability::{global_observability, Metadata, SpanOutcome};

use super::streams::StreamTable;
use super::{core_error, PtyManager};

/// The core a session the page names runs in, and its id there.
async fn session_client(
    manager: &PtyManager,
    hosts: &Hosts,
    id: SessionId,
) -> AppResult<(Arc<CoreClient>, SessionId)> {
    match hosts.of_session(id)? {
        None => Ok((manager.client().await?, id)),
        Some((host, core_id)) => Ok((host.client().await?, core_id)),
    }
}

/// The panes showing a session the page names.
fn session_streams(
    manager: &PtyManager,
    hosts: &Hosts,
    id: SessionId,
) -> AppResult<Arc<StreamTable>> {
    Ok(match hosts.of_session(id)? {
        None => manager.streams.clone(),
        Some((host, _)) => host.streams.clone(),
    })
}

/// The live connection to a session's core, if there is one now.
fn session_current(
    manager: &PtyManager,
    hosts: &Hosts,
    id: SessionId,
) -> Option<(Arc<CoreClient>, Arc<StreamTable>, SessionId)> {
    match hosts.of_session(id).ok()? {
        None => Some((manager.current()?, manager.streams.clone(), id)),
        Some((host, core_id)) => Some((host.current()?, host.streams.clone(), core_id)),
    }
}

pub(crate) fn launch_identity(app: &AppHandle) -> LaunchIdentity {
    LaunchIdentity {
        version: app.package_info().version.to_string(),
        cli_executable: crate::cli_paths::cli_executable_path(),
        cli_endpoint: crate::cli_paths::cli_endpoint_path(),
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
    hosts: State<'_, Hosts>,
    host: Option<String>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    startup: Option<String>,
    direct_command: Option<PtyDirectCommand>,
    context: Option<PtyContext>,
    continues: Option<Continuation>,
) -> AppResult<SessionId> {
    let agent = direct_command.is_some()
        && context
            .as_ref()
            .is_some_and(|context| context.agent_type.is_some());
    let spawn = |env: Vec<(String, String)>| {
        SpawnTarget::Terminal(TerminalSpawn {
            cols,
            rows,
            cwd,
            startup,
            direct_command,
            context,
            env: env.into_iter().collect(),
            continues,
        })
    };
    if let Some(host) = host {
        let host = hosts.by_id(&host)?;
        let client = host.client().await?;
        let id = client
            .spawn(launch_identity(&app), spawn(Vec::new()))
            .await
            .map_err(core_error)?;
        return Ok(host.page_id(id));
    }
    let env: Vec<(String, String)> = if agent {
        Vec::new()
    } else {
        crate::system::child_path()
            .map(|path| ("PATH".to_owned(), path))
            .into_iter()
            .collect()
    };
    let client = manager.client().await?;
    let id = client
        .spawn(launch_identity(&app), spawn(env))
        .await
        .map_err(core_error)?;
    Ok(id)
}

/// Everything about an attach except its first frame, which follows the
/// header in the same raw response instead of crossing as JSON numbers.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachResult {
    pub sub_id: u32,
    /// The latest shell state, so a remounted pane recovers its metadata.
    pub shell: Option<ShellMetadataSnapshot>,
}

/// `[header length as 4 little-endian bytes][header JSON][frame bytes]`.
fn encode_attach_response(header: &AttachResult, frame: &[u8]) -> AppResult<Vec<u8>> {
    let json = serde_json::to_vec(header)?;
    let mut body = Vec::with_capacity(4 + json.len() + frame.len());
    body.extend_from_slice(&(json.len() as u32).to_le_bytes());
    body.extend_from_slice(&json);
    body.extend_from_slice(frame);
    Ok(body)
}

/// Starts a stream of the terminal's frames for one pane. The pane joins on
/// the connection's reader as the answer lands, so it receives every frame
/// after the first.
#[tauri::command]
pub async fn pty_attach(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
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
        let (client, core_id) = session_client(&manager, &hosts, id).await?;
        let table = session_streams(&manager, &hosts, id)?;
        let replied = client
            .submit(Request::Attach { id: core_id }, move |reply| match reply {
                Ok(Reply::Attached(Attached {
                    stream,
                    shell,
                    exited,
                    frame,
                })) => {
                    let sub_id = table.add(id, on_event.clone(), stream);
                    // The exit went out before this frame, so only this pane
                    // has yet to hear of it, and it hears after the frame.
                    if exited {
                        let _ = on_event.send(Response::new(Vec::new()));
                    }
                    encode_attach_response(&AttachResult { sub_id, shell }, &frame)
                }
                Ok(_) => Err(core_error(ClientError::UnexpectedReply)),
                Err(error) => Err(core_error(error)),
            })
            .map_err(core_error)?;
        replied.await.map_err(core_error)?
    }
    .await;
    finish(outcome, operation).map(Response::new)
}

#[tauri::command]
pub async fn pty_unsubscribe(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    sub_id: u32,
) -> AppResult<()> {
    let Some((client, streams, _)) = session_current(&manager, &hosts, id) else {
        return Ok(());
    };
    let Some(stream) = streams.remove(sub_id) else {
        return Ok(());
    };
    if let Ok(detached) = client.submit(Request::Detach { stream }, |_| ()) {
        let _ = detached.await;
    }
    Ok(())
}

/// The pane drew every frame up to `frame`, so the core may send the next.
#[tauri::command]
pub async fn pty_ack(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    sub_id: u32,
    frame: u32,
) -> AppResult<()> {
    if let Some((client, streams, _)) = session_current(&manager, &hosts, id) {
        if let Some(stream) = streams.stream(sub_id) {
            client.ack(stream, frame);
        }
    }
    Ok(())
}

/// A key, text, paste, mouse or focus change, which the core encodes for
/// the program under the modes it turned on.
#[tauri::command]
pub async fn pty_input(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    input: TerminalInput,
) -> AppResult<()> {
    let (client, id) = session_client(&manager, &hosts, id).await?;
    client.input(id, input).await.map_err(core_error)
}

/// History lines, raw as the core lays them out.
#[tauri::command]
pub async fn pty_lines(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    start: u64,
    count: u32,
) -> AppResult<Response> {
    let (client, id) = session_client(&manager, &hosts, id).await?;
    let lines = client.lines(id, start, count).await.map_err(core_error)?;
    Ok(Response::new(lines))
}

#[tauri::command]
pub async fn pty_text(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    from: Option<GridPoint>,
    to: Option<GridPoint>,
) -> AppResult<String> {
    let (client, id) = session_client(&manager, &hosts, id).await?;
    client.text(id, from.zip(to)).await.map_err(core_error)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pty_find(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    query: String,
    options: FindOptions,
    from: Option<GridPoint>,
    backwards: bool,
    inclusive: bool,
) -> AppResult<FindResult> {
    let (client, id) = session_client(&manager, &hosts, id).await?;
    client
        .find(Request::TerminalFind {
            id,
            query,
            options,
            from,
            backwards,
            inclusive,
        })
        .await
        .map_err(core_error)
}

#[tauri::command]
pub async fn pty_clear(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
) -> AppResult<()> {
    let (client, id) = session_client(&manager, &hosts, id).await?;
    client.clear(id).await.map_err(core_error)
}

#[tauri::command]
pub async fn pty_write(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
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
        let (client, id) = session_client(&manager, &hosts, id).await?;
        client.write(id, data.as_bytes()).await.map_err(core_error)
    }
    .await;
    finish(outcome, operation)
}

/// One of the core's calls about a terminal the page names, given the id the
/// terminal has in the core that runs it.
#[tauri::command]
pub async fn pty_call(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    method: String,
    mut params: serde_json::Map<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let (client, id) = session_client(&manager, &hosts, id).await?;
    params.insert("id".into(), id.into());
    client
        .call(&method, params.into())
        .await
        .map_err(core_error)
}

#[tauri::command]
pub async fn pty_resize(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    sikemux_pty::validate_pty_dimensions(cols, rows)?;
    let (client, id) = session_client(&manager, &hosts, id).await?;
    client.resize(id, cols, rows).await.map_err(core_error)
}

#[tauri::command]
pub async fn pty_reset_modes(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
) -> AppResult<()> {
    let (client, id) = session_client(&manager, &hosts, id).await?;
    client.reset_modes(id).await.map_err(core_error)
}

#[tauri::command]
pub async fn pty_kill(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
) -> AppResult<()> {
    // Panes render "[process exited]" before the unmount tears them down.
    for channel in session_streams(&manager, &hosts, id)?.channels(id) {
        let _ = channel.send(Response::new(Vec::new()));
    }
    let (client, core_id) = session_client(&manager, &hosts, id).await?;
    client.kill(core_id).await.map_err(core_error)
}

/// Every session a core holds, running or kept after it ended: this host's,
/// or those of `host`, another the app joined.
#[tauri::command]
pub async fn pty_sessions(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    host: Option<String>,
) -> AppResult<Vec<SessionInfo>> {
    match host {
        None => manager.sessions().await,
        Some(host) => crate::hosts::sessions(&hosts, &host).await,
    }
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
    use super::{encode_attach_response, AttachResult};

    #[test]
    fn attach_response_frames_the_header_before_the_frame() {
        let header = AttachResult {
            sub_id: 9,
            shell: None,
        };
        let body = encode_attach_response(&header, b"hello").expect("encode attach");
        let header_len = u32::from_le_bytes(body[..4].try_into().expect("length prefix")) as usize;
        let parsed: serde_json::Value =
            serde_json::from_slice(&body[4..4 + header_len]).expect("header json");
        assert_eq!(parsed["subId"], 9);
        assert_eq!(&body[4 + header_len..], b"hello");
    }
}

/// Has the core update after the coming relaunch stop the chats it cannot
/// carry over, rather than wait on them.
#[tauri::command]
pub fn update_force_core(manager: State<'_, PtyManager>) -> AppResult<()> {
    manager
        .force_next_core_update()
        .map_err(crate::error::AppError::Other)
}
