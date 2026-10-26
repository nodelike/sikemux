use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sikemux_process::sync::{Locked, Shared};
use sikemux_pty::validate_pty_dimensions;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, Notify};

use crate::protocol::{
    decode_input, encode_control, encode_lines, fits, read_frame, ClientMessage, FrameKind,
    HostRegistration, LaunchIdentity, Request, RequestId, Response, ServerMessage, SessionId,
    SpawnTarget, StreamId, OLDEST_PROTOCOL_VERSION, PROTOCOL, PROTOCOL_VERSION,
};

use super::access::{self, Needs, Peer};
use super::calls;
use super::host;
use super::prepare::{prepare_task, prepare_terminal};
use super::session::{self, PendingStart};
use super::{
    agent, attachments, chat, local_peer, messages, remote, upgrade, window, workspace, Core,
    CoreError, CoreResult,
};

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
    /// The process on the other end, for a client on this host.
    pub pid: Option<u32>,
    frames: mpsc::UnboundedSender<Arc<[u8]>>,
    queued: AtomicUsize,
    closed: AtomicBool,
    kick: Notify,
    /// The session each of this client's terminal streams shows.
    streams: Mutex<HashMap<StreamId, SessionId>>,
    /// A local client that asked for the device view.
    watches_view: AtomicBool,
    /// A phone's app is in front, as far as it last said.
    foreground: AtomicBool,
}

impl ClientConn {
    pub(crate) fn watches_view(&self) -> bool {
        self.watches_view.load(Ordering::Acquire)
    }

    pub(crate) fn in_front(&self) -> bool {
        self.foreground.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn set_foreground(&self, foreground: bool) {
        self.foreground.store(foreground, Ordering::Release);
    }

    /// A client inside the core, such as a shared chat's link, that reads the
    /// frames sent to it from `frames` itself. The core does not list it, so
    /// it hears only what it attaches to.
    pub(crate) fn inside(
        core: &Core,
        peer: Peer,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<Arc<[u8]>>) {
        let (frames, queue) = mpsc::unbounded_channel();
        let client = Arc::new(Self {
            id: core.next_client_id.fetch_add(1, Ordering::Relaxed),
            peer,
            pid: None,
            frames,
            queued: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            kick: Notify::new(),
            streams: Mutex::new(HashMap::new()),
            watches_view: AtomicBool::new(false),
            foreground: AtomicBool::new(true),
        });
        (client, queue)
    }

    /// The reader of a client [`inside`](Self::inside) the core took a frame.
    pub(crate) fn took(&self, bytes: usize) {
        self.queued.fetch_sub(bytes, Ordering::AcqRel);
    }

    /// A client with nobody on the other end, registered with the core.
    #[cfg(test)]
    pub(crate) fn stand_in(core: &Core, peer: Peer) -> Arc<Self> {
        let (frames, _) = mpsc::unbounded_channel();
        let client = Arc::new(Self {
            id: core.next_client_id.fetch_add(1, Ordering::Relaxed),
            peer,
            pid: None,
            frames,
            queued: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            kick: Notify::new(),
            streams: Mutex::new(HashMap::new()),
            watches_view: AtomicBool::new(false),
            foreground: AtomicBool::new(true),
        });
        core.register_client(client.clone());
        client
    }

    pub(crate) fn is_subscribed(&self, id: SessionId) -> bool {
        self.streams.locked().values().any(|session| *session == id)
    }

    pub(crate) fn stream_session(&self, stream: StreamId) -> Option<SessionId> {
        self.streams.locked().get(&stream).copied()
    }

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

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The bytes sent to the client that it has not read yet.
    pub(crate) fn backlog(&self) -> usize {
        self.queued.load(Ordering::Acquire)
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

    pub(crate) fn note_stream(&self, stream: StreamId, id: SessionId) {
        self.streams.locked().insert(stream, id);
    }

    pub(crate) fn forget_stream(&self, stream: StreamId) {
        self.streams.locked().remove(&stream);
    }

    pub(crate) fn take_streams(&self) -> HashMap<StreamId, SessionId> {
        std::mem::take(&mut *self.streams.locked())
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

/// The newest version both sides speak. The app and its own core ship
/// together and must match, so a local client that differs is told to
/// replace the core.
fn agreed_version(peer: &Peer, oldest: u32, newest: u32) -> Option<u32> {
    let core_oldest = if peer.is_local() {
        PROTOCOL_VERSION
    } else {
        OLDEST_PROTOCOL_VERSION
    };
    let agreed = newest.min(PROTOCOL_VERSION);
    (agreed >= oldest.max(core_oldest)).then_some(agreed)
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
            upgrade::answer(core, peer, &frame.payload, writer).await;
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
        Ok(ClientMessage::Hello {
            protocol,
            version,
            newest,
        }) => {
            let newest = newest.unwrap_or(version);
            let agreed = (protocol == PROTOCOL)
                .then(|| agreed_version(peer, version, newest))
                .flatten();
            let reply = match agreed {
                Some(agreed) => ServerMessage::HelloAck {
                    protocol: PROTOCOL.into(),
                    version: agreed,
                    pid,
                    build: core.build.clone(),
                },
                None => ServerMessage::HelloRejected {
                    protocol: PROTOCOL.into(),
                    version: PROTOCOL_VERSION,
                    pid,
                    message: format!(
                        "this core speaks {PROTOCOL} version {PROTOCOL_VERSION}, the client asked for {protocol} versions {version} to {newest}"
                    ),
                },
            };
            write_direct(writer, &reply).await;
            agreed.is_some()
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

pub(crate) async fn serve_local(core: Arc<Core>, stream: super::LocalConn) {
    let peer = local_peer::identify(&stream).await;
    let pid = local_peer::pid(&stream);
    #[cfg(unix)]
    let (read_half, write_half) = stream.into_split();
    #[cfg(windows)]
    let (read_half, write_half) = tokio::io::split(stream);
    serve_client(core, peer, pid, Box::new(read_half), Box::new(write_half)).await;
}

pub(crate) async fn serve_client(
    core: Arc<Core>,
    peer: Peer,
    pid: Option<u32>,
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
        pid,
        frames,
        queued: AtomicUsize::new(0),
        closed: AtomicBool::new(false),
        kick: Notify::new(),
        streams: Mutex::new(HashMap::new()),
        watches_view: AtomicBool::new(false),
        foreground: AtomicBool::new(true),
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
                Ok(ClientMessage::Ack { stream, frame }) => {
                    if let Some(session) = client
                        .stream_session(stream)
                        .and_then(|id| core.session(id))
                    {
                        session::ack(&session, stream, frame);
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
                    request_id: unreadable_request_id(&frame.payload),
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
                if client.peer == Peer::Local {
                    client.respond(request_id, Err(access::APP_ONLY.into()));
                    continue;
                }
                let Some(target) = core.session(id) else {
                    client.respond(request_id, Err("invalid argument: pty not found".into()));
                    continue;
                };
                write_to_program(core, client, request_id, &target, bytes.to_vec());
            }
            FrameKind::Grid | FrameKind::Snapshot | FrameKind::Lines | FrameKind::Frozen => return,
        }
    }
}

/// Who `client` types as: the person's own app, or anyone else.
pub(crate) fn typist(client: &ClientConn) -> session::Typist {
    if client.peer == Peer::App {
        session::Typist::App
    } else {
        session::Typist::Other
    }
}

/// Writes to a session's program in the order writes arrived, and tells
/// its agent's status a line was submitted.
fn write_to_program(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    target: &session::Session,
    bytes: Vec<u8>,
) {
    if !target.may_type(typist(client)) {
        client.respond(request_id, Err(session::SETUP_LOCKED.into()));
        return;
    }
    agent::note_input(core, target, &bytes);
    if let Some(agent) = target
        .agent
        .as_ref()
        .filter(|_| agent::submits_line(&bytes))
    {
        core.harness.questions.typed(agent.agent_id());
        messages::worked(core, agent.agent_id());
    }
    if bytes.is_empty() {
        client.respond(request_id, Ok(Response::Done));
        return;
    }
    let reply_to = client.clone();
    session::queue_input(
        target,
        bytes,
        typist(client),
        Box::new(move |result| {
            reply_to.respond(
                request_id,
                result.map(|()| Response::Done).map_err(CoreError::from),
            );
        }),
    );
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
        if client.peer == Peer::Local && access::for_person(&request, core.remote.setup()) {
            client.respond(request_id, Err(access::APP_ONLY.into()));
            continue;
        }
        match request {
            Request::Spawn { launch, target } => {
                tokio::spawn(async move {
                    let started_by = client.peer.device_id();
                    let launch = match started_by {
                        Some(_) => core.launch_identity(),
                        None => launch,
                    };
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
                super::notch::follow(&core);
                core.usage_wake.notify_one();
                if let Some(listening) = core.listening.get() {
                    crate::waiting::wake(&listening.config.socket);
                }
                core.agents_changed();
                client.respond(request_id, Ok(Response::Done));
            }
            Request::PluginsChanged => {
                core.harness.plugins_changed();
                client.respond(request_id, Ok(Response::Done));
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
                let report = core.manifest_report();
                client.respond(request_id, Ok(Response::Manifests { report }));
            }
            Request::ExplainAgentDetection { agent_id } => {
                let result = match core.agent_session(&agent_id) {
                    Some(target) => {
                        let explaining = core.clone();
                        blocking(move || {
                            let registry = explaining.detection.read_locked();
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
                match result {
                    Ok(()) => core.view_changed(),
                    Err(error) => client.respond(request_id, Err(error)),
                }
            }
            Request::ResetModes { id } => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => {
                        let resetting = core.clone();
                        blocking(move || session::reset_modes(&resetting, &target)).await
                    }
                    Err(error) => Err(error),
                };
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::TerminalInput { id, input } => match session_or_missing(&core, id) {
                Ok(target) => {
                    let bytes = session::encode(&target, &input);
                    write_to_program(&core, &client, request_id, &target, bytes);
                }
                Err(error) => client.respond(request_id, Err(error)),
            },
            Request::TerminalLines { id, start, count } => match session_or_missing(&core, id) {
                Ok(target) => {
                    tokio::spawn(async move {
                        let lines =
                            blocking(move || Ok(session::lines(&target, start, count))).await;
                        match lines {
                            Ok(lines) => {
                                client.send(encode_lines(request_id, &lines).into());
                            }
                            Err(error) => client.respond(request_id, Err(error)),
                        }
                    });
                }
                Err(error) => client.respond(request_id, Err(error)),
            },
            Request::TerminalText { id, range } => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => blocking(move || Ok(session::text(&target, range))).await,
                    Err(error) => Err(error),
                };
                client.respond(
                    request_id,
                    result.map(|text| Response::TerminalText { text }),
                );
            }
            Request::TerminalFind {
                id,
                query,
                options,
                from,
                backwards,
                inclusive,
            } => match session_or_missing(&core, id) {
                Ok(target) => {
                    tokio::spawn(async move {
                        let result = blocking(move || {
                            session::find(&target, &query, &options, from, backwards, inclusive)
                        })
                        .await;
                        client.respond(request_id, result.map(|result| Response::Found { result }));
                    });
                }
                Err(error) => client.respond(request_id, Err(error)),
            },
            Request::TerminalWaitFor { id, containing } => match session_or_missing(&core, id) {
                Ok(target) => {
                    let shown = session::line_shown(&target, containing);
                    drop(target);
                    tokio::spawn(async move {
                        let line = shown.await.unwrap_or(None);
                        client.respond(request_id, Ok(Response::TerminalLine { line }));
                    });
                }
                Err(error) => client.respond(request_id, Err(error)),
            },
            Request::TerminalClear { id } => {
                let result =
                    session_or_missing(&core, id).map(|target| session::clear(&core, &target));
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::ShellAtPrompt { id } => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => blocking(move || Ok(target.shell_at_prompt())).await,
                    Err(error) => Err(error),
                };
                client.respond(
                    request_id,
                    result.map(|at_prompt| Response::ShellAtPrompt { at_prompt }),
                );
            }
            Request::Detach { stream } => {
                if let Some(target) = client
                    .stream_session(stream)
                    .and_then(|id| core.session(id))
                {
                    let _ = blocking(move || {
                        session::detach(&target, stream);
                        Ok(())
                    })
                    .await;
                }
                client.forget_stream(stream);
                core.view_changed();
                client.respond(request_id, Ok(Response::Done));
            }
            Request::Resize { id, cols, rows } => {
                core.spares.note_size(cols, rows);
                let result = match validate_pty_dimensions(cols, rows) {
                    Ok(()) => match session_or_missing(&core, id) {
                        Ok(target) => blocking(move || session::resize(&target, cols, rows)).await,
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error.into()),
                };
                if result.is_ok() {
                    core.view_changed();
                }
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::List => {
                client.respond(
                    request_id,
                    Ok(Response::Sessions {
                        sessions: core.session_infos(),
                    }),
                );
            }
            Request::AcpStart { launch } => {
                match chat::begin(&core, *launch, Some(&client), None, false) {
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
            Request::AcpHistory {
                agent_id,
                feed,
                before,
                turns,
            } => {
                tokio::spawn(async move {
                    let reading = core.clone();
                    let page =
                        blocking(move || chat::history(&reading, &agent_id, &feed, before, turns))
                            .await;
                    client.respond(request_id, page);
                });
            }
            Request::AcpDetach { agent_id } => {
                chat::detach(&core, client.id, &agent_id);
                client.respond(request_id, Ok(Response::Done));
            }
            Request::AcpWake { agent_id } => {
                tokio::spawn(async move {
                    let result = super::launching::wake_chat(&core, agent_id).await;
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
                message_id,
                text,
                paths,
                context,
            } => {
                if core.workspaces.in_terminal(&agent_id) {
                    let result = super::messages::from_person(&core, &agent_id, text);
                    client.respond(request_id, result.map(|()| Response::Done));
                } else if chat::stopped(&core, &agent_id) || chat::mentions::wakes(&context) {
                    tokio::spawn(async move {
                        let mut context = context;
                        let result =
                            match chat::wake_for_message(&core, &agent_id, &mut context).await {
                                Ok(()) => {
                                    chat::prompt(&core, &agent_id, message_id, text, paths, context)
                                }
                                Err(error) => Err(error),
                            };
                        client.respond(request_id, result.map(|()| Response::Done));
                    });
                } else {
                    let result = chat::prompt(&core, &agent_id, message_id, text, paths, context);
                    client.respond(request_id, result.map(|()| Response::Done));
                }
            }
            Request::AcpEdit {
                agent_id,
                message_id,
                text,
                paths,
                context,
            } => {
                tokio::spawn(async move {
                    let mut context = context;
                    let result = match chat::wake_for_message(&core, &agent_id, &mut context).await
                    {
                        Ok(()) => {
                            chat::edit(&core, &agent_id, message_id, text, paths, context).await
                        }
                        Err(error) => Err(error),
                    };
                    client.respond(request_id, result.map(|()| Response::Done));
                });
            }
            Request::AcpSteer {
                agent_id,
                text,
                paths,
                context,
            } if core.workspaces.in_terminal(&agent_id) => {
                let _ = (paths, context);
                let result = super::messages::from_person(&core, &agent_id, text);
                let outcome = "queued".to_owned();
                client.respond(request_id, result.map(|()| Response::Steered { outcome }));
            }
            Request::AcpSteer {
                agent_id,
                text,
                paths,
                context,
            } => {
                tokio::spawn(async move {
                    let mut context = context;
                    let result = match chat::wake_for_message(&core, &agent_id, &mut context).await
                    {
                        Ok(()) => chat::steer(&core, &agent_id, text, paths, context).await,
                        Err(error) => Err(error),
                    };
                    client.respond(
                        request_id,
                        result.map(|outcome| Response::Steered { outcome }),
                    );
                });
            }
            Request::AttachFile {
                agent_id,
                name,
                mime,
                data,
            } => {
                let dir = core
                    .listening
                    .get()
                    .and_then(|listening| listening.config.attachment_dir.clone())
                    .ok_or_else(|| CoreError::from("this host has nowhere to keep files"));
                match core.chats.running(&agent_id).and(dir) {
                    Ok(dir) => {
                        tokio::spawn(async move {
                            let result =
                                blocking(move || attachments::save(&dir, &name, &mime, &data))
                                    .await;
                            client.respond(
                                request_id,
                                result.map(|path| Response::Attached { path }),
                            );
                        });
                    }
                    Err(error) => client.respond(request_id, Err(error)),
                }
            }
            Request::Call { method, params } => {
                let device = client.peer.device_id();
                tokio::spawn(super::layout::asked_by(device, async move {
                    let result = calls::call(core, &client, &method, params).await;
                    client.respond(request_id, result.map(|result| Response::Called { result }));
                }));
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
                if let Some(chat) = core.chats.get(&agent_id) {
                    core.workspaces.note_stopping(&chat);
                }
                if core.chats.stop(&agent_id) {
                    core.note_agent_gone(&agent_id);
                }
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
                        status: Box::new(core.remote.status()),
                    }),
                );
            }
            Request::SetRemoteAccess { enabled } => {
                tokio::spawn(async move {
                    let result = remote::set_enabled(&core, enabled).await;
                    client.respond(
                        request_id,
                        result.map(|status| Response::Remote {
                            status: Box::new(status),
                        }),
                    );
                });
            }
            Request::SetDeviceAccess { id, access } => {
                let result = remote::set_access(&core, &id, access);
                client.respond(
                    request_id,
                    result.map(|status| Response::Remote {
                        status: Box::new(status),
                    }),
                );
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
            Request::PublishOnScreen { agent_ids, focused } => {
                super::launching::show_terminals(&core, &agent_ids);
                core.seen.show(client.id, agent_ids, focused);
                core.view_changed();
                client.respond(request_id, Ok(Response::Done));
            }
            Request::FocusAgent { agent_id } => {
                let shown = core.window.focus(agent_id);
                client.respond(
                    request_id,
                    if shown {
                        Ok(Response::Done)
                    } else {
                        Err(window::not_open("show that agent").into())
                    },
                );
            }
            Request::SetNotifications { key_id, key, prefs } => {
                let result = match &client.peer {
                    Peer::Device { id } => core.remote.set_notifications(id, key_id, &key, prefs),
                    Peer::App | Peer::Local | Peer::Share { .. } => {
                        Err("only a paired phone can ask for notifications".into())
                    }
                };
                if result.is_ok() {
                    remote::announce(&core);
                }
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::ClearNotifications => {
                let result = match &client.peer {
                    Peer::Device { id } => core.remote.clear_notifications(id),
                    Peer::App | Peer::Local | Peer::Share { .. } => {
                        Err("only a paired phone has notifications to stop".into())
                    }
                };
                if result.is_ok() {
                    remote::announce(&core);
                }
                client.respond(request_id, result.map(|()| Response::Done));
            }
            Request::SetForeground { foreground } => {
                client.foreground.store(foreground, Ordering::Release);
                client.respond(request_id, Ok(Response::Done));
            }
            Request::WatchView => {
                client.watches_view.store(true, Ordering::Release);
                core.view_watched.notify_one();
                client.respond(request_id, Ok(Response::Done));
                core.send_device_view(&client);
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
                        workspace: core.workspace_view(),
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
                    resume_id: None,
                    cwd: None,
                    title: None,
                };
                super::launching::start_chat(&core, &client, request_id, choice);
            }
            Request::ResumeChat {
                recent,
                permission_mode,
                model,
                effort,
            } => {
                let choice = workspace::ResumeChoice {
                    recent,
                    permission_mode,
                    model,
                    effort,
                };
                super::launching::resume_chat(&core, &client, request_id, choice);
            }
            Request::AnswerPairing { id, allow, access } => {
                let result = core
                    .remote
                    .answer(&id, allow.then_some(access))
                    .map(|()| remote::announce(&core));
                client.respond(
                    request_id,
                    result.map(|status| Response::Remote {
                        status: Box::new(status),
                    }),
                );
            }
            Request::AllowDevices { devices } => {
                let result = core.remote.allow(devices).map(|()| remote::announce(&core));
                client.respond(
                    request_id,
                    result.map(|status| Response::Remote {
                        status: Box::new(status),
                    }),
                );
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
            Request::SignRegistration { nonce, user_id } => {
                let signed = core
                    .remote
                    .may_join(&user_id, &client.peer)
                    .and_then(|()| core.remote.sign_registration(&nonce, &user_id));
                tokio::spawn(async move {
                    let result = match signed {
                        Ok((key, signature)) => {
                            blocking(|| Ok(host::info()))
                                .await
                                .map(|host| Response::Registration {
                                    registration: HostRegistration {
                                        key,
                                        name: host.name,
                                        channel: host.channel,
                                        signature,
                                    },
                                })
                        }
                        Err(error) => Err(error),
                    };
                    client.respond(request_id, result);
                });
            }
            Request::SetOwner { owner } => {
                tokio::spawn(async move {
                    let result = remote::set_owner(&core, owner, &client.peer).await;
                    client.respond(
                        request_id,
                        result.map(|status| Response::Remote {
                            status: Box::new(status),
                        }),
                    );
                });
            }
            Request::RevokeDevice { id } => {
                let result = remote::revoke(&core, &id);
                client.respond(
                    request_id,
                    result.map(|status| Response::Remote {
                        status: Box::new(status),
                    }),
                );
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
            Request::AcpSwitchAccount { agent_id, account } => {
                let result = chat::switch_account(&core, &agent_id, account);
                client.respond(request_id, result.map(|()| Response::Done));
            }
        }
    }
}

pub(crate) async fn spawn(
    core: Arc<Core>,
    launch: LaunchIdentity,
    mut target: Box<SpawnTarget>,
    started_by: Option<String>,
) -> CoreResult<PendingStart> {
    if let SpawnTarget::Terminal(spawn) = target.as_mut() {
        core.spares.note_size(spawn.cols, spawn.rows);
        super::launching::terminal_agent(&core, spawn).await;
    }
    blocking(move || {
        let mut prepared = match *target {
            SpawnTarget::Terminal(mut spawn) => {
                let folder = spawn.cwd.clone().or_else(|| {
                    spawn
                        .context
                        .as_ref()
                        .and_then(|context| context.project.clone())
                        .filter(|project| !project.is_empty())
                });
                if let Some(folder) = &folder {
                    give_agent_environment(&core, folder, &mut spawn.env);
                }
                let resumed = super::activity_log::resumed_by(&spawn);
                let prepared = prepare_terminal(&launch, spawn)?;
                let owner = &prepared.owner;
                if let Some(resumed) = resumed {
                    super::activity_log::terminal_launched(&core, owner, resumed);
                }
                if let (Some(agent_id), Some(folder)) = (&owner.agent_id, folder) {
                    core.name_agent(
                        agent_id,
                        std::path::Path::new(&folder),
                        owner.agent_type.as_deref(),
                        None,
                    );
                }
                prepared
            }
            SpawnTarget::Task { mut request } => {
                give_agent_environment(&core, &request.cwd, &mut request.env);
                prepare_task(&launch, request)?
            }
        };
        prepared.owner.started_by = started_by;
        session::spawn_session(&core, prepared)
    })
    .await
}

fn give_agent_environment(core: &Core, folder: &str, env: &mut HashMap<String, String>) {
    let given = super::plugins::agent_environment(core, std::path::Path::new(folder));
    for (key, value) in given {
        env.entry(key).or_insert(value);
    }
}

async fn shutdown(core: Arc<Core>, client: Arc<ClientConn>, request_id: RequestId, stop_all: bool) {
    if stop_all {
        core.stop_everything().await;
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

/// The id of a request this core cannot read, such as one added in a later
/// release, so the client hears it was refused instead of waiting.
fn unreadable_request_id(payload: &[u8]) -> Option<RequestId> {
    let message: serde_json::Value = serde_json::from_slice(payload).ok()?;
    if message.get("type")?.as_str()? != "request" {
        return None;
    }
    message.get("requestId")?.as_u64()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_request_from_a_later_release_is_refused_by_its_id() {
        let later = br#"{"type":"request","requestId":12,"request":{"op":"addedLater"}}"#;
        assert_eq!(super::unreadable_request_id(later), Some(12));
        assert_eq!(super::unreadable_request_id(br#"{"type":"ack"}"#), None);
        assert_eq!(super::unreadable_request_id(b"not json"), None);
    }

    #[test]
    fn a_device_is_served_in_the_newest_version_both_sides_speak() {
        use super::{agreed_version, Peer, OLDEST_PROTOCOL_VERSION, PROTOCOL_VERSION};
        let phone = Peer::Device { id: "phone".into() };
        let (oldest, newest) = (OLDEST_PROTOCOL_VERSION, PROTOCOL_VERSION);
        assert_eq!(agreed_version(&phone, oldest, oldest), Some(oldest));
        assert_eq!(agreed_version(&phone, oldest, newest + 3), Some(newest));
        assert_eq!(agreed_version(&phone, newest + 1, newest + 3), None);
        assert_eq!(agreed_version(&phone, oldest - 2, oldest - 1), None);
        assert_eq!(agreed_version(&Peer::Local, newest, newest), Some(newest));
        assert_eq!(agreed_version(&Peer::Local, newest - 1, newest - 1), None);
    }

    #[test]
    fn a_hello_from_before_versions_were_agreed_still_reads() {
        let hello: crate::protocol::ClientMessage =
            serde_json::from_str(r#"{"type":"hello","protocol":"sikemux-core","version":9}"#)
                .expect("reads");
        assert!(matches!(
            hello,
            crate::protocol::ClientMessage::Hello {
                version: 9,
                newest: None,
                ..
            }
        ));
    }
}
