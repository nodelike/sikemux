use std::sync::Arc;
use std::time::Duration;

use sikemux_core::client::{Attached, ClientError, CoreClient, Reply};
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

use super::streams::{Release, StreamTable};
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
        let streams = table.clone();
        let replied = {
            let mut guard = table.lock();
            guard.begin_attach(id)?;
            let submitted = client.submit(Request::Attach { id: core_id }, move |reply| {
                let mut guard = streams.lock();
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

#[tauri::command]
pub async fn pty_unsubscribe(
    manager: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    id: SessionId,
    sub_id: u32,
) -> AppResult<()> {
    let Some((client, streams, core_id)) = session_current(&manager, &hosts, id) else {
        return Ok(());
    };
    let detached = {
        let mut guard = streams.lock();
        match guard.unsubscribe(id, sub_id) {
            Release::Nothing => None,
            Release::Ack(bytes) => {
                client.ack(core_id, bytes);
                None
            }
            // Queued while the table is locked, so an attach decided after
            // this reaches the core after it.
            Release::Detach => Some(client.submit(Request::Detach { id: core_id }, |_| ())),
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
    hosts: State<'_, Hosts>,
    id: SessionId,
    sub_id: u32,
    bytes: usize,
) -> AppResult<()> {
    if crate::hosts::slot_of(id) == 0 {
        if let Some(forward) = manager.streams.ack(id, sub_id, bytes) {
            if let Some(client) = manager.current() {
                client.ack(id, forward);
            }
        }
        return Ok(());
    }
    if let Some((client, streams, core_id)) = session_current(&manager, &hosts, id) {
        if let Some(forward) = streams.ack(id, sub_id, bytes) {
            client.ack(core_id, forward);
        }
    }
    Ok(())
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
    session_streams(&manager, &hosts, id)?
        .channels(id)
        .send(&[]);
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
}

/// Has the core update after the coming relaunch stop the chats it cannot
/// carry over, rather than wait on them.
#[tauri::command]
pub fn update_force_core(manager: State<'_, PtyManager>) -> AppResult<()> {
    manager
        .force_next_core_update()
        .map_err(crate::error::AppError::Other)
}

/// The chats at work that an update now would stop and start again rather
/// than carry over, which the app waits on before it installs one.
#[tauri::command]
pub async fn update_blockers(
    manager: State<'_, PtyManager>,
) -> AppResult<Vec<sikemux_core::frozen::UpdateBlocker>> {
    Ok(manager.update_blockers().await)
}
