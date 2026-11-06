use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_wire::grid::{FindResult, GridPoint};
use sikemux_wire::pty::agent_detection::DetectionExplain;
use sikemux_wire::pty::shell_protocol::ShellMetadataSnapshot;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use sikemux_wire::protocol::{
    ask, ask::Ask, decode_grid, decode_lines, decode_snapshot, device_view_json, encode_control,
    encode_input, read_frame, AllowedDevice, BuildIdentity, CallId, ChatAttachment, ChatLaunch,
    ChatStart, ClientMessage, DeviceAccess, ErrorKind, Event, FrameKind, LaunchIdentity,
    RemoteStatus, Request, RequestId, Response, ServerMessage, SessionId, SpawnTarget, StreamId,
    WindowAnswer, WindowCall, MAX_FRAME_BYTES, OLDEST_PROTOCOL_VERSION, PROTOCOL, PROTOCOL_VERSION,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("could not reach the Sikemux core: {0}")]
    Io(#[from] io::Error),
    #[error("{message}")]
    VersionMismatch {
        version: u32,
        pid: u32,
        message: String,
    },
    #[error("the Sikemux core did not finish the handshake: {0}")]
    Handshake(String),
    /// A failure of a kind a client branches on.
    #[error("{message}")]
    Refused {
        kind: ErrorKind,
        message: String,
        detail: Option<serde_json::Value>,
    },
    #[error("{0}")]
    Core(String),
    #[error("the connection to the Sikemux core closed")]
    Disconnected,
    #[error("this device is no longer paired with this host")]
    NotPaired,
    #[error("the Sikemux core sent a reply of the wrong kind")]
    UnexpectedReply,
    #[error("could not encode a message for the Sikemux core: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("the Sikemux core did not start within {0:?}")]
    StartTimeout(Duration),
    #[error("this host did not answer in time")]
    TimedOut,
    #[error("this device went off the network")]
    Offline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreHello {
    pub pid: u32,
    pub version: u32,
    pub build: BuildIdentity,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ClientEvent {
    Grid {
        stream: StreamId,
        frame: Vec<u8>,
    },
    Event(Event),
    /// What this client shows of the host, as the core's JSON.
    DeviceView(String),
    WindowCall {
        call_id: CallId,
        call: WindowCall,
    },
}

/// Receives what the core sends unasked, on the connection's reader task and
/// in the order it was sent. Reply callbacks run on the same task, so a reply
/// lands between exactly the frames it was sent between.
pub trait EventSink: Send + Sync + 'static {
    /// A frame of a stream an attach started, as [`sikemux_wire::grid`]
    /// lays it out.
    fn grid(&self, stream: StreamId, frame: &[u8]);
    fn event(&self, event: Event);
    /// What this client shows of the host, as the core's JSON, so fields and values newer than
    /// this client still reach whoever reads it.
    fn device_view(&self, _view_json: &str) {}
    /// The core asks the registered window to do something. Answer with
    /// [`CoreClient::answer_window`].
    fn window_call(&self, _call_id: CallId, _call: WindowCall) {}
    /// The connection is gone. Every pending reply has already failed.
    fn closed(&self);
}

/// The answer to an attach: the stream's first frame, with the whole screen.
/// The stream's next frames follow on the event stream.
#[derive(Clone, Debug)]
pub struct Attached {
    pub stream: StreamId,
    pub shell: Option<ShellMetadataSnapshot>,
    /// The client already heard this session exit.
    pub exited: bool,
    pub frame: Vec<u8>,
}

#[derive(Debug)]
pub enum Reply {
    Response(Response),
    Attached(Attached),
    /// History lines, as [`sikemux_wire::grid`] lays them out.
    Lines(Vec<u8>),
}

type Waiter = Box<dyn FnOnce(Result<Reply, ClientError>) + Send>;
type Pending = Arc<Mutex<Option<HashMap<RequestId, Waiter>>>>;

/// Room for the request and session ids in front of the bytes of a write.
const MAX_INPUT_CHUNK: usize = MAX_FRAME_BYTES - 16;

/// One connection to the core. Dropping it disconnects; sessions keep running.
/// Reconnecting is the caller's job.
pub struct CoreClient {
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    next_request: AtomicU64,
    hello: CoreHello,
    reader: JoinHandle<()>,
}

impl Drop for CoreClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

pub fn hello_reply(message: ServerMessage) -> Result<CoreHello, ClientError> {
    match message {
        ServerMessage::HelloAck {
            protocol,
            version,
            pid,
            build,
        } if protocol == PROTOCOL => Ok(CoreHello {
            pid,
            version,
            build,
        }),
        ServerMessage::HelloRejected {
            version,
            pid,
            message,
            ..
        } => Err(ClientError::VersionMismatch {
            version,
            pid,
            message,
        }),
        ServerMessage::Error { message, .. } => Err(ClientError::Handshake(message)),
        _ => Err(ClientError::Handshake("unexpected reply to hello".into())),
    }
}

/// The app and its own core ship together, so they speak one version.
pub fn hello_frame() -> Result<Vec<u8>, ClientError> {
    Ok(encode_control(&ClientMessage::Hello {
        protocol: PROTOCOL.into(),
        version: PROTOCOL_VERSION,
        newest: None,
    })?)
}

/// A device and a core on another machine update apart, so a device offers
/// every version it speaks.
fn device_hello_frame() -> Result<Vec<u8>, ClientError> {
    Ok(encode_control(&ClientMessage::Hello {
        protocol: PROTOCOL.into(),
        version: OLDEST_PROTOCOL_VERSION,
        newest: Some(PROTOCOL_VERSION),
    })?)
}

pub(crate) struct ChannelSink(pub(crate) mpsc::UnboundedSender<ClientEvent>);

impl EventSink for ChannelSink {
    fn grid(&self, stream: StreamId, frame: &[u8]) {
        let _ = self.0.send(ClientEvent::Grid {
            stream,
            frame: frame.to_vec(),
        });
    }

    fn event(&self, event: Event) {
        let _ = self.0.send(ClientEvent::Event(event));
    }

    fn device_view(&self, view_json: &str) {
        let _ = self.0.send(ClientEvent::DeviceView(view_json.to_owned()));
    }

    fn window_call(&self, call_id: CallId, call: WindowCall) {
        let _ = self.0.send(ClientEvent::WindowCall { call_id, call });
    }

    fn closed(&self) {}
}

fn fail_pending(pending: &Pending) {
    let waiters = pending
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
        .unwrap_or_default();
    for (_, waiter) in waiters {
        waiter(Err(ClientError::Disconnected));
    }
}

impl CoreClient {
    pub async fn connect(
        socket: &Path,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ClientEvent>), ClientError> {
        let (events, event_queue) = mpsc::unbounded_channel();
        let client = Self::connect_with(socket, Arc::new(ChannelSink(events))).await?;
        Ok((client, event_queue))
    }

    pub async fn connect_with(
        socket: &Path,
        sink: Arc<dyn EventSink>,
    ) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket).await?;
        let (read_half, write_half) = stream.into_split();
        Self::connect_streams(read_half, write_half, sink).await
    }

    pub async fn connect_streams(
        read_half: impl AsyncRead + Send + Unpin + 'static,
        write_half: impl AsyncWrite + Send + Unpin + 'static,
        sink: Arc<dyn EventSink>,
    ) -> Result<Self, ClientError> {
        Self::open(read_half, write_half, sink, hello_frame()?).await
    }

    /// Like [`Self::connect_streams`], for a device reaching a core on
    /// another machine, which may run another release.
    pub async fn connect_device_streams(
        read_half: impl AsyncRead + Send + Unpin + 'static,
        write_half: impl AsyncWrite + Send + Unpin + 'static,
        sink: Arc<dyn EventSink>,
    ) -> Result<Self, ClientError> {
        Self::open(read_half, write_half, sink, device_hello_frame()?).await
    }

    async fn open(
        read_half: impl AsyncRead + Send + Unpin + 'static,
        mut write_half: impl AsyncWrite + Send + Unpin + 'static,
        sink: Arc<dyn EventSink>,
        hello: Vec<u8>,
    ) -> Result<Self, ClientError> {
        let mut reader = BufReader::with_capacity(256 * 1024, read_half);
        write_half.write_all(&hello).await?;
        let frame = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut reader))
            .await
            .map_err(|_| ClientError::Handshake("timed out".into()))??
            .ok_or_else(|| ClientError::Handshake("the core closed the connection".into()))?;
        if frame.kind != FrameKind::Control {
            return Err(ClientError::Handshake("unexpected frame".into()));
        }
        let hello = hello_reply(serde_json::from_slice(&frame.payload)?)?;

        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let (outgoing, mut outgoing_queue) = mpsc::unbounded_channel::<Vec<u8>>();
        // Outlives the client, so what it queued before it was dropped still
        // reaches the core.
        tokio::spawn(async move {
            while let Some(frame) = outgoing_queue.recv().await {
                if write_half.write_all(&frame).await.is_err() {
                    return;
                }
            }
            let _ = write_half.shutdown().await;
        });
        let reader_pending = pending.clone();
        let reader = tokio::spawn(async move {
            while let Ok(Some(frame)) = read_frame(&mut reader).await {
                dispatch(frame, &reader_pending, sink.as_ref());
            }
            fail_pending(&reader_pending);
            sink.closed();
        });
        Ok(Self {
            outgoing,
            pending,
            next_request: AtomicU64::new(1),
            hello,
            reader,
        })
    }

    pub fn core_pid(&self) -> u32 {
        self.hello.pid
    }

    pub fn hello(&self) -> &CoreHello {
        &self.hello
    }

    pub fn is_connected(&self) -> bool {
        !self.reader.is_finished()
    }

    fn queue(
        &self,
        request_id: RequestId,
        frame: Vec<u8>,
        waiter: Waiter,
    ) -> Result<(), ClientError> {
        {
            let mut pending = self.pending.lock().map_err(|_| ClientError::Disconnected)?;
            pending
                .as_mut()
                .ok_or(ClientError::Disconnected)?
                .insert(request_id, waiter);
        }
        if self.outgoing.send(frame).is_err() {
            if let Ok(mut pending) = self.pending.lock() {
                if let Some(pending) = pending.as_mut() {
                    pending.remove(&request_id);
                }
            }
            return Err(ClientError::Disconnected);
        }
        Ok(())
    }

    fn queue_with<T, F>(
        &self,
        request_id: RequestId,
        frame: Vec<u8>,
        on_reply: F,
    ) -> Result<impl Future<Output = Result<T, ClientError>> + Send + 'static, ClientError>
    where
        T: Send + 'static,
        F: FnOnce(Result<Reply, ClientError>) -> T + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.queue(
            request_id,
            frame,
            Box::new(move |reply| {
                let _ = sender.send(on_reply(reply));
            }),
        )?;
        Ok(async move { receiver.await.map_err(|_| ClientError::Disconnected) })
    }

    /// Sends a request now and runs `on_reply` on the reader task when the
    /// answer arrives, before any frame the core sent after it. The returned
    /// future yields what `on_reply` returned.
    pub fn submit<T, F>(
        &self,
        request: Request,
        on_reply: F,
    ) -> Result<impl Future<Output = Result<T, ClientError>> + Send + 'static, ClientError>
    where
        T: Send + 'static,
        F: FnOnce(Result<Reply, ClientError>) -> T + Send + 'static,
    {
        let request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let frame = encode_control(&ClientMessage::Request {
            request_id,
            request,
        })?;
        self.queue_with(request_id, frame, on_reply)
    }

    pub async fn request(&self, request: Request) -> Result<Response, ClientError> {
        match self.submit(request, |reply| reply)?.await?? {
            Reply::Response(response) => Ok(response),
            Reply::Attached(_) | Reply::Lines(_) => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn ask<A: Ask>(&self, request: A) -> Result<A::Answer, ClientError> {
        A::answer(self.request(request.into()).await?).ok_or(ClientError::UnexpectedReply)
    }

    pub async fn spawn(
        &self,
        launch: LaunchIdentity,
        target: SpawnTarget,
    ) -> Result<SessionId, ClientError> {
        let target = Box::new(target);
        self.ask(ask::Spawn { launch, target }).await
    }

    /// Writes larger than one frame go out as several, in order.
    pub async fn write(&self, id: SessionId, bytes: &[u8]) -> Result<(), ClientError> {
        let mut replies = Vec::new();
        for chunk in bytes.chunks(MAX_INPUT_CHUNK) {
            let request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
            let frame = encode_input(request_id, id, chunk);
            replies.push(self.queue_with(request_id, frame, |reply| reply)?);
        }
        for reply in replies {
            match reply.await?? {
                Reply::Response(Response::Done) => {}
                _ => return Err(ClientError::UnexpectedReply),
            }
        }
        Ok(())
    }

    pub async fn attach(&self, id: SessionId) -> Result<Attached, ClientError> {
        match self
            .submit(ask::Attach { id }.into(), |reply| reply)?
            .await??
        {
            Reply::Attached(attached) => Ok(attached),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    /// History lines `start..start + count`, as many as the core still has.
    pub async fn lines(
        &self,
        id: SessionId,
        start: u64,
        count: u32,
    ) -> Result<Vec<u8>, ClientError> {
        match self
            .submit(ask::TerminalLines { id, start, count }.into(), |reply| {
                reply
            })?
            .await??
        {
            Reply::Lines(lines) => Ok(lines),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn text(
        &self,
        id: SessionId,
        range: Option<(GridPoint, GridPoint)>,
    ) -> Result<String, ClientError> {
        self.ask(ask::TerminalText { id, range }).await
    }

    pub async fn find(&self, find: ask::TerminalFind) -> Result<FindResult, ClientError> {
        self.ask(find).await
    }

    /// Reports the last frame of a stream this client has drawn. Never
    /// answered.
    pub fn ack(&self, stream: StreamId, frame: u32) {
        self.send(&ClientMessage::Ack { stream, frame });
    }

    pub async fn explain_agent_detection(
        &self,
        agent_id: String,
    ) -> Result<DetectionExplain, ClientError> {
        let explain = self.ask(ask::ExplainAgentDetection { agent_id }).await?;
        Ok(*explain)
    }

    fn send(&self, message: &ClientMessage) {
        if let Ok(frame) = encode_control(message) {
            let _ = self.outgoing.send(frame);
        }
    }

    pub fn answer_window(&self, call_id: CallId, answer: WindowAnswer) {
        self.send(&ClientMessage::WindowReply { call_id, answer });
    }

    /// Every tab a waiting CLI `open` opened has closed.
    pub fn window_open_closed(&self, call_id: CallId) {
        self.send(&ClientMessage::WindowOpenClosed { call_id });
    }

    /// Starts a chat agent. Its events reach this connection from the first.
    pub async fn acp_start(&self, launch: ChatLaunch) -> Result<ChatStart, ClientError> {
        let launch = Box::new(launch);
        self.ask(ask::AcpStart { launch }).await
    }

    /// Takes up a running chat. Its live events follow the replay on the
    /// event stream.
    pub async fn acp_attach(&self, agent_id: String) -> Result<ChatAttachment, ClientError> {
        self.ask(ask::AcpAttach {
            agent_id,
            since: None,
        })
        .await
    }

    /// Sends a file for the chat's next message and answers with where the
    /// host keeps it, for the prompt's `paths`.
    pub async fn attach_file(
        &self,
        agent_id: String,
        name: String,
        mime: String,
        bytes: &[u8],
    ) -> Result<PathBuf, ClientError> {
        use base64::Engine;
        self.ask(ask::AttachFile {
            agent_id,
            name,
            mime,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
        .await
    }

    async fn remote<A: Ask<Answer = Box<RemoteStatus>>>(
        &self,
        request: A,
    ) -> Result<RemoteStatus, ClientError> {
        Ok(*self.ask(request).await?)
    }

    pub async fn set_owner(&self, owner: Option<String>) -> Result<RemoteStatus, ClientError> {
        self.remote(ask::SetOwner { owner }).await
    }

    pub async fn remote_status(&self) -> Result<RemoteStatus, ClientError> {
        self.remote(ask::RemoteStatus).await
    }

    pub async fn set_remote_access(&self, enabled: bool) -> Result<RemoteStatus, ClientError> {
        self.remote(ask::SetRemoteAccess { enabled }).await
    }

    pub async fn set_device_access(
        &self,
        id: String,
        access: DeviceAccess,
    ) -> Result<RemoteStatus, ClientError> {
        self.remote(ask::SetDeviceAccess { id, access }).await
    }

    pub async fn revoke_device(&self, id: String) -> Result<RemoteStatus, ClientError> {
        self.remote(ask::RevokeDevice { id }).await
    }

    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        let method = method.to_owned();
        self.ask(ask::Call { method, params }).await
    }

    /// Starts a chat agent in one of the app's projects. Answers with the
    /// agent's id once its session is ready; its events follow.
    pub async fn start_chat(
        &self,
        launcher: String,
        project: String,
        model: Option<String>,
    ) -> Result<(String, ChatStart), ClientError> {
        self.ask(ask::StartChat {
            launcher,
            project,
            permission_mode: None,
            model,
            effort: None,
        })
        .await
    }

    /// Takes up again a chat the app published as recent, by its id in the
    /// device view. Answers like [`Self::start_chat`].
    pub async fn resume_chat(&self, recent: String) -> Result<(String, ChatStart), ClientError> {
        self.ask(ask::ResumeChat {
            recent,
            permission_mode: None,
            model: None,
            effort: None,
        })
        .await
    }

    pub async fn answer_pairing(
        &self,
        id: String,
        allow: bool,
        access: DeviceAccess,
    ) -> Result<RemoteStatus, ClientError> {
        self.remote(ask::AnswerPairing { id, allow, access }).await
    }

    pub async fn allow_devices(
        &self,
        devices: Vec<AllowedDevice>,
    ) -> Result<RemoteStatus, ClientError> {
        self.remote(ask::AllowDevices { devices }).await
    }
}

fn unreadable_reply(payload: &[u8]) -> Option<RequestId> {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()?
        .get("requestId")?
        .as_u64()
}

fn dispatch(frame: sikemux_wire::protocol::Frame, pending: &Pending, sink: &dyn EventSink) {
    let resolve = |request_id: RequestId, reply: Result<Reply, ClientError>| {
        let waiter = pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.as_mut()?.remove(&request_id));
        if let Some(waiter) = waiter {
            waiter(reply);
        }
    };
    match frame.kind {
        FrameKind::Grid => {
            if let Some((stream, grid)) = decode_grid(&frame.payload) {
                sink.grid(stream, grid);
            }
        }
        FrameKind::Snapshot => {
            if let Some(snapshot) = decode_snapshot(&frame.payload) {
                resolve(
                    snapshot.request_id,
                    Ok(Reply::Attached(Attached {
                        stream: snapshot.stream,
                        shell: snapshot.header.shell,
                        exited: snapshot.header.exited,
                        frame: snapshot.frame.to_vec(),
                    })),
                );
            }
        }
        FrameKind::Lines => {
            if let Some((request_id, lines)) = decode_lines(&frame.payload) {
                resolve(request_id, Ok(Reply::Lines(lines.to_vec())));
            }
        }
        FrameKind::Control if let Some(view) = device_view_json(&frame.payload) => {
            sink.device_view(view)
        }
        FrameKind::Control => match serde_json::from_slice::<ServerMessage>(&frame.payload) {
            Ok(ServerMessage::Response {
                request_id,
                response,
            }) => resolve(request_id, Ok(Reply::Response(response))),
            Ok(ServerMessage::Error {
                request_id: Some(request_id),
                message,
                kind,
                detail,
            }) => {
                let failure = match kind {
                    Some(kind) => ClientError::Refused {
                        kind,
                        message,
                        detail,
                    },
                    None => ClientError::Core(message),
                };
                resolve(request_id, Err(failure))
            }
            Ok(ServerMessage::Event { event }) => sink.event(event),
            Ok(ServerMessage::WindowCall { call_id, call }) => sink.window_call(call_id, call),
            Ok(_) => {}
            // A reply this client cannot read still ends the wait for it.
            Err(_) => {
                if let Some(request_id) = unreadable_reply(&frame.payload) {
                    resolve(request_id, Err(ClientError::UnexpectedReply));
                }
            }
        },
        FrameKind::Input | FrameKind::Frozen => {}
    }
}
