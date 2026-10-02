use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sikemux_pty::validate_pty_dimensions;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Notify};

use crate::protocol::{
    decode_input, encode_control, fits, read_frame, ClientMessage, FrameKind, LaunchIdentity,
    Request, RequestId, Response, ServerMessage, SessionId, SpawnTarget, PROTOCOL,
    PROTOCOL_VERSION,
};

use super::access::{self, Needs, Peer};
use super::host;
use super::prepare::{prepare_task, prepare_terminal};
use super::session::{self, PendingStart};
use super::{agent, chat, harness, remote, upgrade, workspace, Core, CoreError, CoreResult};

pub(crate) type ClientId = u64;
pub(crate) type FrameReader = BufReader<Box<dyn AsyncRead + Send + Unpin>>;
pub(crate) type FrameWriter = BufWriter<Box<dyn AsyncWrite + Send + Unpin>>;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// A client that stops reading entirely is cut off rather than buffered for.
const MAX_CLIENT_BACKLOG: usize = 64 * 1024 * 1024;
const SHUTDOWN_FLUSH: Duration = Duration::from_secs(1);

pub(crate) struct ClientConn {
    pub id: ClientId,
    pub peer: Peer,
    frames: mpsc::UnboundedSender<Arc<[u8]>>,
    queued: AtomicUsize,
    closed: AtomicBool,
    kick: Notify,
    subscriptions: Mutex<HashSet<SessionId>>,
}

impl ClientConn {
    pub(crate) fn send(&self, frame: Arc<[u8]>) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let queued = self.queued.fetch_add(frame.len(), Ordering::AcqRel) + frame.len();
        if queued > MAX_CLIENT_BACKLOG {
            self.close();
            return false;
        }
        if self.frames.send(frame).is_err() {
            self.closed.store(true, Ordering::Release);
            return false;
        }
        true
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.kick.notify_one();
    }

    pub(crate) fn send_message(&self, message: &ServerMessage) {
        let Ok(frame) = encode_control(message) else {
            return;
        };
        if fits(&frame) {
            self.send(frame.into());
            return;
        }
        // The client would drop the connection over a frame it cannot read, and
        // ask again for the same thing once it reconnects.
        if let ServerMessage::Response { request_id, .. } = message {
            self.send_message(&ServerMessage::Error {
                request_id: Some(*request_id),
                message: "the answer is larger than a connection to the core carries".into(),
            });
        } else {
            eprintln!(
                "sikemux core: a message of {} bytes is too large to send",
                frame.len()
            );
        }
    }

    pub(crate) fn respond(&self, request_id: RequestId, result: CoreResult<Response>) {
        self.send_message(&match result {
            Ok(response) => ServerMessage::Response {
                request_id,
                response,
            },
            Err(error) => ServerMessage::Error {
                request_id: Some(request_id),
                message: error.to_string(),
            },
        });
    }

    pub(crate) fn note_subscription(&self, id: SessionId, subscribed: bool) {
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            if subscribed {
                subscriptions.insert(id);
            } else {
                subscriptions.remove(&id);
            }
        }
    }

    pub(crate) fn take_subscriptions(&self) -> HashSet<SessionId> {
        self.subscriptions
            .lock()
            .map(|mut subscriptions| std::mem::take(&mut *subscriptions))
            .unwrap_or_default()
    }

    pub(crate) async fn wait_flushed(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        while self.queued.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

async fn write_direct(writer: &mut FrameWriter, message: &ServerMessage) {
    if let Ok(frame) = encode_control(message) {
        let _ = writer.write_all(&frame).await;
        let _ = writer.flush().await;
    }
}

async fn handshake(
    core: &Arc<Core>,
    peer: &Peer,
    reader: &mut FrameReader,
    writer: &mut FrameWriter,
) -> bool {
    let frame = match tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(reader)).await {
        Ok(Ok(Some(frame))) if frame.kind == FrameKind::Control => frame,
        Ok(Ok(Some(frame))) if frame.kind == FrameKind::Frozen && peer.is_local() => {
            upgrade::answer(core, &frame.payload, writer).await;
            return false;
        }
        _ => return false,
    };
    // A client that connected while the core hands itself over talks to the
    // replacement instead.
    if core.is_frozen() {
        return false;
    }
    let pid = std::process::id();
    match serde_json::from_slice::<ClientMessage>(&frame.payload) {
        Ok(ClientMessage::Hello { protocol, version })
            if protocol == PROTOCOL && version == PROTOCOL_VERSION =>
        {
            write_direct(
                writer,
                &ServerMessage::HelloAck {
                    protocol: PROTOCOL.into(),
                    version: PROTOCOL_VERSION,
                    pid,
                    build: core.build.clone(),
                },
            )
            .await;
            true
        }
        Ok(ClientMessage::Hello { protocol, version }) => {
            write_direct(
                writer,
                &ServerMessage::HelloRejected {
                    protocol: PROTOCOL.into(),
                    version: PROTOCOL_VERSION,
                    pid,
                    message: format!(
                        "this core speaks {PROTOCOL} version {PROTOCOL_VERSION}, the client asked for {protocol} version {version}"
                    ),
                },
            )
            .await;
            false
        }
        _ => {
            write_direct(
                writer,
                &ServerMessage::Error {
                    request_id: None,
                    message: "expected a hello".into(),
                },
            )
            .await;
            false
        }
    }
}

pub(crate) async fn serve_local(core: Arc<Core>, stream: UnixStream) {
    let (read_half, write_half) = stream.into_split();
    serve_client(core, Peer::Local, Box::new(read_half), Box::new(write_half)).await;
}

pub(crate) async fn serve_client(
    core: Arc<Core>,
    peer: Peer,
    read_half: Box<dyn AsyncRead + Send + Unpin>,
    write_half: Box<dyn AsyncWrite + Send + Unpin>,
) {
    let mut reader = BufReader::new(read_half);
    let mut writer = BufWriter::with_capacity(64 * 1024, write_half);
    if !handshake(&core, &peer, &mut reader, &mut writer).await {
        return;
    }
    let (frames, queue) = mpsc::unbounded_channel();
    let client = Arc::new(ClientConn {
        id: core.next_client_id.fetch_add(1, Ordering::Relaxed),
        peer,
        frames,
        queued: AtomicUsize::new(0),
        closed: AtomicBool::new(false),
        kick: Notify::new(),
        subscriptions: Mutex::new(HashSet::new()),
    });
    core.register_client(client.clone());
    if !client.peer.is_local() {
        core.send_device_view(&client);
    }
    let (work, work_queue) = mpsc::unbounded_channel();
    tokio::select! {
        _ = read_requests(&core, &client, &mut reader, work) => {}
        _ = write_frames(&mut writer, queue, &client) => {}
        _ = run_requests(&core, &client, work_queue) => {}
        _ = client.kick.notified() => {}
    }
    core.unregister_client(&client);
}

async fn write_frames(
    writer: &mut FrameWriter,
    mut queue: mpsc::UnboundedReceiver<Arc<[u8]>>,
    client: &ClientConn,
) -> std::io::Result<()> {
    while let Some(frame) = queue.recv().await {
        let mut written = frame.len();
        writer.write_all(&frame).await?;
        while let Ok(frame) = queue.try_recv() {
            written += frame.len();
            writer.write_all(&frame).await?;
        }
        writer.flush().await?;
        client.queued.fetch_sub(written, Ordering::AcqRel);
    }
    Ok(())
}

async fn read_requests(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    reader: &mut FrameReader,
    work: mpsc::UnboundedSender<(RequestId, Request)>,
) {
    while let Ok(Some(frame)) = read_frame(reader).await {
        match frame.kind {
            FrameKind::Control => match serde_json::from_slice::<ClientMessage>(&frame.payload) {
                Ok(ClientMessage::Ack { id, bytes }) => {
                    if let Some(session) = core.session(id) {
                        session::ack(&session, client.id, bytes);
                    }
                }
                Ok(ClientMessage::Request {
                    request_id,
                    request,
                }) => {
                    if work.send((request_id, request)).is_err() {
                        return;
                    }
                }
                Ok(ClientMessage::WindowReply { call_id, answer }) => {
                    core.window.answer(client.id, call_id, answer);
                }
                Ok(ClientMessage::WindowOpenClosed { call_id }) => {
                    core.window.open_closed(client.id, call_id);
                }
                Ok(ClientMessage::Hello { .. }) => client.send_message(&ServerMessage::Error {
                    request_id: None,
                    message: "the handshake is already done".into(),
                }),
                Err(error) => client.send_message(&ServerMessage::Error {
                    request_id: None,
                    message: format!("unreadable message: {error}"),
                }),
            },
            FrameKind::Input => {
                let Some((request_id, id, bytes)) = decode_input(&frame.payload) else {
                    return;
                };
                if core.is_frozen() {
                    client.respond(request_id, Err(upgrade::UPDATING.into()));
                    continue;
                }
                if let Err(refused) = core.permit(&client.peer, Needs::Full) {
                    client.respond(request_id, Err(refused));
                    continue;
                }
                let Some(target) = core.session(id) else {
                    client.respond(request_id, Err("invalid argument: pty not found".into()));
                    continue;
                };
                agent::note_input(core, &target, bytes);
                let reply_to = client.clone();
                session::queue_input(
                    &target,
                    bytes.to_vec(),
                    Box::new(move |result| {
                        reply_to.respond(
                            request_id,
                            result.map(|()| Response::Done).map_err(CoreError::from),
                        );
                    }),
                );
            }
            FrameKind::Output | FrameKind::Snapshot | FrameKind::Frozen => return,
        }
    }
}

pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> CoreResult<T> + Send + 'static,
) -> CoreResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| CoreError::from(format!("pty: worker failed: {error}")))?
}

fn session_or_missing(core: &Core, id: SessionId) -> CoreResult<Arc<session::Session>> {
    core.session(id)
        .ok_or_else(|| CoreError::from("invalid argument: pty not found"))
}

/// Requests from one client run in order, except the slow ones (spawn, kill,
/// task output, shutdown), which run alongside so they hold nothing up.
async fn run_requests(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    mut queue: mpsc::UnboundedReceiver<(RequestId, Request)>,
) {
    while let Some((request_id, request)) = queue.recv().await {
        let core = core.clone();
        let client = client.clone();
        if core.is_frozen() {
            client.respond(request_id, Err(upgrade::UPDATING.into()));
            continue;
        }
        if let Err(refused) = core.permit(&client.peer, access::needs(&request)) {
            client.respond(request_id, Err(refused));
            continue;
        }
        match request {
            Request::Spawn { launch, target } => {
                tokio::spawn(async move {
                    let started_by = client.peer.device_id();
                    match spawn(core.clone(), launch, target, started_by).await {
                        Ok(pending) => {
                            let id = pending.id();
                            client.respond(request_id, Ok(Response::Spawned { id }));
                            let stopped = pending
                                .task()
                                .is_some_and(|task| core.harness.session_started(id, task));
                            pending.start(&core);
                            if stopped {
                                if let Some(target) = core.take_session_for_kill(id) {
                                    let killing = core.clone();
                                    let _ = blocking(move || {
                                        session::kill(&killing, &target);
                                        Ok(())
                                    })
                                    .await;
                                }
                            }
                        }
                        Err(error) => client.respond(request_id, Err(error)),
                    }
                });
            }
            Request::Kill { id } => {
                tokio::spawn(async move {
                    let result = match core.take_session_for_kill(id) {
                        Some(target) => {
                            let core = core.clone();
                            blocking(move || {
                                session::kill(&core, &target);
                                Ok(())
                            })
                            .await
                        }
                        None => Ok(()),
                    };
                    client.respond(request_id, result.map(|()| Response::Done));
                });
            }
            Request::TaskOutput { id, query } => {
                tokio::spawn(async move {
                    let result = match session_or_missing(&core, id) {
                        Ok(target) => blocking(move || session::task_output(&target, &query)).await,
                        Err(_) => Err("Task output expired or task no longer exists".into()),
                    };
                    client.respond(request_id, result.map(|page| Response::TaskOutput { page }));
                });
            }
            Request::Shutdown { stop_all } => {
                tokio::spawn(shutdown(core, client, request_id, stop_all));
            }
            Request::StopAll => {
                tokio::spawn(async move {
                    let draining = core.clone();
                    let result = blocking(move || {
                        draining.drain();
                        Ok(())
                    })
                    .await;
                    client.respond(request_id, result.map(|()| Response::Done));
                });
            }
            Request::RegisterWindow => {
                core.window.register(client.clone());
                client.respond(request_id, Ok(Response::Done));
            }
            Request::HarnessAwaitingTrust { execution_id } => {
                core.harness.awaiting_trust(&execution_id);
                client.respond(request_id, Ok(Response::Done));
            }
            Request::HarnessStopRuns { selector } => {
                tokio::spawn(async move {
                    harness::stop_runs(&core, selector).await;
                    client.respond(request_id, Ok(Response::Done));
                });
            }
            Request::Configure { manifest_dir } => {
                tokio::spawn(async move {
                    let result = blocking(move || core.configure_manifests(manifest_dir)).await;
                    client.respond(
                        request_id,
                        result.map(|report| Response::Manifests { report }),
                    );
                });
            }
            Request::ReloadManifests => {
                tokio::spawn(async move {
                    let result = blocking(move || core.reload_manifests()).await;
                    client.respond(
                        request_id,
                        result.map(|report| Response::Manifests { report }),
                    );
                });
            }
            Request::ListManifests => {
                let result = core.manifest_report();
                client.respond(
                    request_id,
                    result.map(|report| Response::Manifests { report }),
                );
            }
            Request::ExplainAgentDetection { agent_id } => {
                let result = match core.agent_session(&agent_id) {
                    Some(target) => {
                        let explaining = core.clone();
                        blocking(move || {
                            let registry = explaining.detection.read().map_err(|_| {
                                CoreError::from("agent detection registry lock poisoned")
                            })?;
                            agent::explain(&registry, &target)
                        })
                        .await
                    }
                    None => Err("invalid argument: agent has no live terminal".into()),
                };
                client.respond(
                    request_id,
                    result.map(|explain| Response::DetectionExplain {
                        explain: Box::new(explain),
                    }),
                );
            }
            Request::Attach { id } => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => {
                        let subscriber = client.clone();
                        blocking(move || session::attach(&target, &subscriber, request_id)).await
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    client.respond(request_id, Err(error));
                }
            }
            Request::Subscribe { id } => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => {
                        let subscriber = client.clone();
                        blocking(move || session::subscribe(&target, &subscriber, request_id)).await
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    client.respond(request_id, Err(error));
                }
            }
            Request::ResetModes { id } => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => blocking(move || session::reset_modes(&target)).await,
                    Err(error) => Err(error),
                };
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::Detach { id } => {
                if let Some(target) = core.session(id) {
                    let client_id = client.id;
                    let _ = blocking(move || {
                        session::detach(&target, client_id);
                        Ok(())
                    })
                    .await;
                }
                client.note_subscription(id, false);
                client.respond(request_id, Ok(Response::Done));
            }
            Request::Resize { id, cols, rows } => {
                let result = match validate_pty_dimensions(cols, rows) {
                    Ok(()) => match session_or_missing(&core, id) {
                        Ok(target) => blocking(move || session::resize(&target, cols, rows)).await,
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error.into()),
                };
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::List => {
                let mut sessions: Vec<_> = core.all_sessions().iter().map(|s| s.info()).collect();
                sessions.sort_by_key(|info| info.id);
                client.respond(request_id, Ok(Response::Sessions { sessions }));
            }
            Request::AcpStart { launch } => {
                match chat::begin(&core, *launch, Some(&client), None) {
                    Ok(started) => {
                        tokio::spawn(async move {
                            let result = chat::until_started(&core, started).await;
                            client.respond(
                                request_id,
                                result.map(|start| Response::ChatStarted { start }),
                            );
                        });
                    }
                    Err(error) => client.respond(request_id, Err(error)),
                }
            }
            Request::AcpAttach { agent_id, since } => {
                tokio::spawn(async move {
                    chat::attach(&core, &client, request_id, &agent_id, since).await;
                });
            }
            Request::AcpDetach { agent_id } => {
                chat::detach(&core, client.id, &agent_id);
                client.respond(request_id, Ok(Response::Done));
            }
            Request::AcpWake { agent_id } => {
                tokio::spawn(async move {
                    let result = workspace::wake_chat(&core, agent_id).await;
                    client.respond(request_id, result);
                });
            }
            Request::AcpList => {
                client.respond(
                    request_id,
                    Ok(Response::Chats {
                        chats: if client.peer.is_local() {
                            core.chats.list()
                        } else {
                            core.workspaces.listed(core.chats.list())
                        },
                    }),
                );
            }
            Request::AcpPrompt {
                agent_id,
                text,
                paths,
                context,
            } => {
                let result = chat::prompt(&core, client.id, &agent_id, text, paths, context);
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::AcpSteer {
                agent_id,
                text,
                paths,
                context,
            } => {
                tokio::spawn(async move {
                    let result =
                        chat::steer(&core, client.id, &agent_id, text, paths, context).await;
                    client.respond(
                        request_id,
                        result.map(|outcome| Response::Steered { outcome }),
                    );
                });
            }
            Request::AcpCancel { agent_id } => {
                let result = chat::cancel(&core, &agent_id);
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::AcpStopTask { agent_id, task_id } => {
                let result = chat::stop_task(&core, &agent_id, task_id);
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::AcpPermissionReply {
                agent_id,
                request_id: permission,
                option_id,
            } => {
                let result = chat::reply_permission(&core, &agent_id, &permission, option_id);
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::AcpStop { agent_id } => {
                core.chats.stop(&agent_id);
                client.respond(request_id, Ok(Response::Done));
            }
            Request::AcpSetPermissionMode { agent_id, mode } => {
                tokio::spawn(async move {
                    let result = chat::set_permission_mode(&core, &agent_id, mode).await;
                    client.respond(request_id, result.map(|()| Response::Done));
                });
            }
            Request::RemoteStatus => {
                client.respond(
                    request_id,
                    Ok(Response::Remote {
                        status: core.remote.status(),
                    }),
                );
            }
            Request::SetRemoteAccess { enabled } => {
                tokio::spawn(async move {
                    let result = remote::set_enabled(&core, enabled).await;
                    client.respond(request_id, result.map(|status| Response::Remote { status }));
                });
            }
            Request::SetDeviceAccess { id, access } => {
                let result = remote::set_access(&core, &id, access);
                client.respond(request_id, result.map(|status| Response::Remote { status }));
            }
            Request::PublishWorkspace {
                projects,
                launchers,
            } => {
                let result = core.workspaces.publish(projects, launchers);
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::PublishBackdrop { texture, image } => {
                let result = core.workspaces.publish_backdrop(texture, image);
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::BackdropImage => {
                client.respond(
                    request_id,
                    Ok(Response::BackdropImage {
                        data_url: core.workspaces.backdrop_image(),
                    }),
                );
            }
            Request::PublishPalette { palette } => {
                let result = core.workspaces.publish_palette(palette);
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::PublishChats { chats } => {
                let result = core.workspaces.publish_chats(chats);
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::Attentions => {
                client.respond(
                    request_id,
                    Ok(Response::Attentions {
                        attentions: core.chats.attentions(),
                    }),
                );
            }
            Request::Host => {
                tokio::spawn(async move {
                    let host = blocking(|| Ok(host::info())).await;
                    client.respond(request_id, host.map(|host| Response::Host { host }));
                });
            }
            Request::Workspace => {
                client.respond(
                    request_id,
                    Ok(Response::Workspace {
                        workspace: core.workspaces.view(),
                    }),
                );
            }
            Request::StartChat {
                launcher,
                project,
                permission_mode,
                model,
                effort,
            } => {
                let choice = workspace::ChatChoice {
                    launcher,
                    project,
                    permission_mode,
                    model,
                    effort,
                };
                workspace::start_chat(&core, &client, request_id, choice);
            }
            Request::OpenPairing => {
                let result = core.remote.open_offer().map(|()| remote::announce(&core));
                client.respond(request_id, result.map(|status| Response::Remote { status }));
            }
            Request::ClosePairing => {
                core.remote.close_offer();
                let status = remote::announce(&core);
                client.respond(request_id, Ok(Response::Remote { status }));
            }
            Request::AnswerPairing { id, allow, access } => {
                let result = core
                    .remote
                    .answer(&id, allow.then_some(access))
                    .map(|()| remote::announce(&core));
                client.respond(request_id, result.map(|status| Response::Remote { status }));
            }
            Request::Unpair => {
                let Peer::Device { id } = &client.peer else {
                    client.respond(
                        request_id,
                        Err("only a paired device can unpair itself".into()),
                    );
                    continue;
                };
                let id = id.clone();
                tokio::spawn(async move {
                    client.respond(request_id, Ok(Response::Done));
                    client.wait_flushed(SHUTDOWN_FLUSH).await;
                    if let Err(error) = remote::revoke(&core, &id) {
                        eprintln!("sikemux core: could not unpair {id}: {error}");
                    }
                });
            }
            Request::RevokeDevice { id } => {
                let result = remote::revoke(&core, &id);
                client.respond(request_id, result.map(|status| Response::Remote { status }));
            }
            Request::AcpSetConfig {
                agent_id,
                config_id,
                value,
            } => {
                tokio::spawn(async move {
                    let result = chat::set_config(&core, &agent_id, config_id, value).await;
                    client.respond(
                        request_id,
                        result.map(|value| Response::ChatConfig { value }),
                    );
                });
            }
        }
    }
}

async fn spawn(
    core: Arc<Core>,
    launch: LaunchIdentity,
    target: Box<SpawnTarget>,
    started_by: Option<String>,
) -> CoreResult<PendingStart> {
    blocking(move || {
        let mut prepared = match *target {
            SpawnTarget::Terminal(spawn) => prepare_terminal(&launch, spawn)?,
            SpawnTarget::Task { request } => prepare_task(&launch, request)?,
        };
        prepared.owner.started_by = started_by;
        session::spawn_session(&core, prepared)
    })
    .await
}

async fn shutdown(core: Arc<Core>, client: Arc<ClientConn>, request_id: RequestId, stop_all: bool) {
    if stop_all {
        let draining = core.clone();
        let _ = blocking(move || {
            draining.drain();
            Ok(())
        })
        .await;
    } else {
        let running = core.running_sessions() + core.chats.count();
        if running > 0 {
            client.respond(
                request_id,
                Err(format!("the core still has {running} running sessions").into()),
            );
            return;
        }
    }
    client.respond(request_id, Ok(Response::Done));
    client.wait_flushed(SHUTDOWN_FLUSH).await;
    core.begin_shutdown();
}
