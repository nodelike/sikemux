use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sikemux_pty::agent_detection::{DetectionExplain, ManifestReloadReport};
use sikemux_pty::output_log::{OutputPage, OutputQuery};
use sikemux_pty::shell_protocol::ShellMetadataSnapshot;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::protocol::frozen::{FrozenReply, FrozenRequest};
use crate::protocol::{
    decode_output, decode_snapshot, encode_control, encode_frozen, encode_input, read_frame,
    read_frame_sync, Attention, BackdropImage, BuildIdentity, CallId, ChatAttachment, ChatContext,
    ChatInfo, ChatLaunch, ChatLauncher, ChatMark, ChatStart, ClientMessage, DeviceAccess, Event,
    FrameKind, LaunchIdentity, ProjectInfo, PublishedChat, RemoteStatus, Request, RequestId,
    Response, RunSelector, ServerMessage, SessionId, SessionInfo, SpawnTarget, WindowAnswer,
    WindowCall, Workspace, MAX_FRAME_BYTES, PROTOCOL, PROTOCOL_VERSION,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const START_TIMEOUT: Duration = Duration::from_secs(5);
const START_POLL: Duration = Duration::from_millis(20);

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
    #[error("{0}")]
    Core(String),
    #[error("the connection to the Sikemux core closed")]
    Disconnected,
    #[error("this device is no longer paired with this Mac")]
    NotPaired,
    #[error("the Sikemux core sent a reply of the wrong kind")]
    UnexpectedReply,
    #[error("could not encode a message for the Sikemux core: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("the Sikemux core did not start within {0:?}")]
    StartTimeout(Duration),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreHello {
    pub pid: u32,
    pub version: u32,
    pub build: BuildIdentity,
}

#[derive(Debug)]
pub enum ClientEvent {
    Output { id: SessionId, bytes: Vec<u8> },
    Event(Event),
    WindowCall { call_id: CallId, call: WindowCall },
}

/// Receives what the core sends unasked, on the connection's reader task and
/// in the order it was sent. Reply callbacks run on the same task, so a reply
/// lands between exactly the output frames it was sent between.
pub trait EventSink: Send + Sync + 'static {
    fn output(&self, id: SessionId, bytes: &[u8]);
    fn event(&self, event: Event);
    /// The core asks the registered window to do something. Answer with
    /// [`CoreClient::answer_window`].
    fn window_call(&self, _call_id: CallId, _call: WindowCall) {}
    /// The connection is gone. Every pending reply has already failed.
    fn closed(&self);
}

/// The replay of an attach. Live output for the session follows it on the
/// event stream, starting with the first byte the replay does not contain.
#[derive(Clone, Debug)]
pub struct Attached {
    pub alternate_screen: bool,
    pub shell: Option<ShellMetadataSnapshot>,
    /// The client already heard this session exit.
    pub exited: bool,
    pub replay: Vec<u8>,
}

#[derive(Debug)]
pub enum Reply {
    Response(Response),
    Attached(Attached),
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

fn hello_reply(message: ServerMessage) -> Result<CoreHello, ClientError> {
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

fn hello_frame() -> Result<Vec<u8>, ClientError> {
    Ok(encode_control(&ClientMessage::Hello {
        protocol: PROTOCOL.into(),
        version: PROTOCOL_VERSION,
    })?)
}

pub(crate) struct ChannelSink(pub(crate) mpsc::UnboundedSender<ClientEvent>);

impl EventSink for ChannelSink {
    fn output(&self, id: SessionId, bytes: &[u8]) {
        let _ = self.0.send(ClientEvent::Output {
            id,
            bytes: bytes.to_vec(),
        });
    }

    fn event(&self, event: Event) {
        let _ = self.0.send(ClientEvent::Event(event));
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
    /// Connects with a sink that queues everything on a channel.
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
        mut write_half: impl AsyncWrite + Send + Unpin + 'static,
        sink: Arc<dyn EventSink>,
    ) -> Result<Self, ClientError> {
        let mut reader = BufReader::with_capacity(256 * 1024, read_half);
        write_half.write_all(&hello_frame()?).await?;
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

    async fn request(&self, request: Request) -> Result<Response, ClientError> {
        match self.submit(request, |reply| reply)?.await?? {
            Reply::Response(response) => Ok(response),
            Reply::Attached(_) => Err(ClientError::UnexpectedReply),
        }
    }

    async fn request_done(&self, request: Request) -> Result<(), ClientError> {
        match self.request(request).await? {
            Response::Done => Ok(()),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn spawn(
        &self,
        launch: LaunchIdentity,
        target: SpawnTarget,
    ) -> Result<SessionId, ClientError> {
        match self
            .request(Request::Spawn {
                launch,
                target: Box::new(target),
            })
            .await?
        {
            Response::Spawned { id } => Ok(id),
            _ => Err(ClientError::UnexpectedReply),
        }
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

    pub async fn resize(&self, id: SessionId, cols: u16, rows: u16) -> Result<(), ClientError> {
        self.request_done(Request::Resize { id, cols, rows }).await
    }

    pub async fn kill(&self, id: SessionId) -> Result<(), ClientError> {
        self.request_done(Request::Kill { id }).await
    }

    pub async fn list(&self) -> Result<Vec<SessionInfo>, ClientError> {
        match self.request(Request::List).await? {
            Response::Sessions { sessions } => Ok(sessions),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn attach(&self, id: SessionId) -> Result<Attached, ClientError> {
        match self
            .submit(Request::Attach { id }, |reply| reply)?
            .await??
        {
            Reply::Attached(attached) => Ok(attached),
            Reply::Response(_) => Err(ClientError::UnexpectedReply),
        }
    }

    /// Live output without a replay, for a client that already has the screen.
    pub async fn subscribe(&self, id: SessionId) -> Result<(), ClientError> {
        self.request_done(Request::Subscribe { id }).await
    }

    /// No output for the session arrives after this resolves.
    pub async fn detach(&self, id: SessionId) -> Result<(), ClientError> {
        self.request_done(Request::Detach { id }).await
    }

    pub async fn reset_modes(&self, id: SessionId) -> Result<(), ClientError> {
        self.request_done(Request::ResetModes { id }).await
    }

    /// Reports output bytes this client has finished with. Never answered.
    pub fn ack(&self, id: SessionId, bytes: usize) {
        self.send(&ClientMessage::Ack { id, bytes });
    }

    pub async fn task_output(
        &self,
        id: SessionId,
        query: OutputQuery,
    ) -> Result<OutputPage, ClientError> {
        match self.request(Request::TaskOutput { id, query }).await? {
            Response::TaskOutput { page } => Ok(page),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    async fn manifests(&self, request: Request) -> Result<ManifestReloadReport, ClientError> {
        match self.request(request).await? {
            Response::Manifests { report } => Ok(report),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn configure(
        &self,
        manifest_dir: Option<PathBuf>,
    ) -> Result<ManifestReloadReport, ClientError> {
        self.manifests(Request::Configure { manifest_dir }).await
    }

    pub async fn list_manifests(&self) -> Result<ManifestReloadReport, ClientError> {
        self.manifests(Request::ListManifests).await
    }

    pub async fn reload_manifests(&self) -> Result<ManifestReloadReport, ClientError> {
        self.manifests(Request::ReloadManifests).await
    }

    pub async fn explain_agent_detection(
        &self,
        agent_id: String,
    ) -> Result<DetectionExplain, ClientError> {
        match self
            .request(Request::ExplainAgentDetection { agent_id })
            .await?
        {
            Response::DetectionExplain { explain } => Ok(*explain),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    /// Tool calls that need the window come to this connection from now on.
    pub async fn register_window(&self) -> Result<(), ClientError> {
        self.request_done(Request::RegisterWindow).await
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

    pub async fn harness_awaiting_trust(&self, execution_id: String) -> Result<(), ClientError> {
        self.request_done(Request::HarnessAwaitingTrust { execution_id })
            .await
    }

    /// Resolves once every matching run has stopped.
    pub async fn harness_stop_runs(&self, selector: RunSelector) -> Result<(), ClientError> {
        self.request_done(Request::HarnessStopRuns { selector })
            .await
    }

    /// Kills every session. The core keeps running.
    pub async fn stop_all(&self) -> Result<(), ClientError> {
        self.request_done(Request::StopAll).await
    }

    /// `stop_all` kills every session first; without it the core refuses to
    /// exit while any session is running.
    pub async fn shutdown(&self, stop_all: bool) -> Result<(), ClientError> {
        self.request_done(Request::Shutdown { stop_all }).await
    }

    /// Starts a chat agent. Its events reach this connection from the first.
    pub async fn acp_start(&self, launch: ChatLaunch) -> Result<ChatStart, ClientError> {
        match self
            .request(Request::AcpStart {
                launch: Box::new(launch),
            })
            .await?
        {
            Response::ChatStarted { start } => Ok(start),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    /// Takes up a running chat. Its live events follow the replay on the
    /// event stream.
    pub async fn acp_attach(&self, agent_id: String) -> Result<ChatAttachment, ClientError> {
        self.acp_attach_since(agent_id, None).await
    }

    /// Takes the chat up again from `since`, hearing only what was missed
    /// when the core still has it.
    pub async fn acp_attach_since(
        &self,
        agent_id: String,
        since: Option<ChatMark>,
    ) -> Result<ChatAttachment, ClientError> {
        match self.request(Request::AcpAttach { agent_id, since }).await? {
            Response::ChatAttached { attachment } => Ok(attachment),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    /// No more of the chat's events reach this client.
    pub async fn acp_detach(&self, agent_id: String) -> Result<(), ClientError> {
        self.request_done(Request::AcpDetach { agent_id }).await
    }

    pub async fn acp_list(&self) -> Result<Vec<ChatInfo>, ClientError> {
        match self.request(Request::AcpList).await? {
            Response::Chats { chats } => Ok(chats),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn acp_prompt(
        &self,
        agent_id: String,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    ) -> Result<(), ClientError> {
        self.request_done(Request::AcpPrompt {
            agent_id,
            text,
            paths,
            context,
        })
        .await
    }

    pub async fn acp_steer(
        &self,
        agent_id: String,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    ) -> Result<String, ClientError> {
        match self
            .request(Request::AcpSteer {
                agent_id,
                text,
                paths,
                context,
            })
            .await?
        {
            Response::Steered { outcome } => Ok(outcome),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn acp_cancel(&self, agent_id: String) -> Result<(), ClientError> {
        self.request_done(Request::AcpCancel { agent_id }).await
    }

    pub async fn acp_stop_task(
        &self,
        agent_id: String,
        task_id: String,
    ) -> Result<(), ClientError> {
        self.request_done(Request::AcpStopTask { agent_id, task_id })
            .await
    }

    pub async fn acp_permission_reply(
        &self,
        agent_id: String,
        request_id: String,
        option_id: Option<String>,
    ) -> Result<(), ClientError> {
        self.request_done(Request::AcpPermissionReply {
            agent_id,
            request_id,
            option_id,
        })
        .await
    }

    pub async fn acp_stop(&self, agent_id: String) -> Result<(), ClientError> {
        self.request_done(Request::AcpStop { agent_id }).await
    }

    pub async fn acp_set_permission_mode(
        &self,
        agent_id: String,
        mode: String,
    ) -> Result<(), ClientError> {
        self.request_done(Request::AcpSetPermissionMode { agent_id, mode })
            .await
    }

    pub async fn acp_set_config(
        &self,
        agent_id: String,
        config_id: String,
        value: String,
    ) -> Result<serde_json::Value, ClientError> {
        match self
            .request(Request::AcpSetConfig {
                agent_id,
                config_id,
                value,
            })
            .await?
        {
            Response::ChatConfig { value } => Ok(value),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    async fn remote_request(&self, request: Request) -> Result<RemoteStatus, ClientError> {
        match self.request(request).await? {
            Response::Remote { status } => Ok(status),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn remote_status(&self) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::RemoteStatus).await
    }

    pub async fn set_remote_access(&self, enabled: bool) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::SetRemoteAccess { enabled })
            .await
    }

    pub async fn set_device_access(
        &self,
        id: String,
        access: DeviceAccess,
    ) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::SetDeviceAccess { id, access })
            .await
    }

    pub async fn revoke_device(&self, id: String) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::RevokeDevice { id }).await
    }

    pub async fn publish_workspace(
        &self,
        projects: Vec<ProjectInfo>,
        launchers: Vec<ChatLauncher>,
    ) -> Result<(), ClientError> {
        self.request_done(Request::PublishWorkspace {
            projects,
            launchers,
        })
        .await
    }

    /// Removes this device from the core's paired devices.
    pub async fn unpair(&self) -> Result<(), ClientError> {
        self.request_done(Request::Unpair).await
    }

    pub async fn publish_backdrop(
        &self,
        texture: bool,
        image: Option<BackdropImage>,
    ) -> Result<(), ClientError> {
        self.request_done(Request::PublishBackdrop { texture, image })
            .await
    }

    pub async fn backdrop_image(&self) -> Result<Option<String>, ClientError> {
        match self.request(Request::BackdropImage).await? {
            Response::BackdropImage { data_url } => Ok(data_url),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn publish_palette(
        &self,
        palette: BTreeMap<String, String>,
    ) -> Result<(), ClientError> {
        self.request_done(Request::PublishPalette { palette }).await
    }

    pub async fn publish_chats(&self, chats: Vec<PublishedChat>) -> Result<(), ClientError> {
        self.request_done(Request::PublishChats { chats }).await
    }

    pub async fn acp_wake(&self, agent_id: String) -> Result<(), ClientError> {
        self.request_done(Request::AcpWake { agent_id }).await
    }

    pub async fn attentions(&self) -> Result<Vec<Attention>, ClientError> {
        match self.request(Request::Attentions).await? {
            Response::Attentions { attentions } => Ok(attentions),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn workspace(&self) -> Result<Workspace, ClientError> {
        match self.request(Request::Workspace).await? {
            Response::Workspace { workspace } => Ok(workspace),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    /// Starts a chat agent in one of the app's projects. Answers with the
    /// agent's id once its session is ready; its events follow.
    pub async fn start_chat(
        &self,
        launcher: String,
        project: String,
        model: Option<String>,
    ) -> Result<(String, ChatStart), ClientError> {
        let request = Request::StartChat {
            launcher,
            project,
            permission_mode: None,
            model,
            effort: None,
        };
        match self.request(request).await? {
            Response::ChatBegun { agent_id, start } => Ok((agent_id, start)),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn open_pairing(&self) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::OpenPairing).await
    }

    pub async fn close_pairing(&self) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::ClosePairing).await
    }

    pub async fn answer_pairing(
        &self,
        id: String,
        allow: bool,
        access: DeviceAccess,
    ) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::AnswerPairing { id, allow, access })
            .await
    }
}

fn unreadable_reply(payload: &[u8]) -> Option<RequestId> {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()?
        .get("requestId")?
        .as_u64()
}

fn dispatch(frame: crate::protocol::Frame, pending: &Pending, sink: &dyn EventSink) {
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
        FrameKind::Output => {
            if let Some((id, bytes)) = decode_output(&frame.payload) {
                sink.output(id, bytes);
            }
        }
        FrameKind::Snapshot => {
            if let Some((request_id, _, header, replay)) = decode_snapshot(&frame.payload) {
                resolve(
                    request_id,
                    Ok(Reply::Attached(Attached {
                        alternate_screen: header.alternate_screen,
                        shell: header.shell,
                        exited: header.exited,
                        replay: replay.to_vec(),
                    })),
                );
            }
        }
        FrameKind::Control => match serde_json::from_slice::<ServerMessage>(&frame.payload) {
            Ok(ServerMessage::Response {
                request_id,
                response,
            }) => resolve(request_id, Ok(Reply::Response(response))),
            Ok(ServerMessage::Error {
                request_id: Some(request_id),
                message,
            }) => resolve(request_id, Err(ClientError::Core(message))),
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

#[derive(Debug)]
pub enum ProbeError {
    NotRunning(io::Error),
    Rejected { version: u32, pid: u32 },
    Unanswered(String),
}

pub fn probe(socket: &Path, timeout: Duration) -> Result<CoreHello, ProbeError> {
    let mut stream = StdUnixStream::connect(socket).map_err(ProbeError::NotRunning)?;
    let unanswered = |error: &dyn std::fmt::Display| ProbeError::Unanswered(error.to_string());
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|error| unanswered(&error))?;
    let hello = hello_frame().map_err(|error| unanswered(&error))?;
    stream
        .write_all(&hello)
        .map_err(|error| unanswered(&error))?;
    let frame = read_frame_sync(&mut stream)
        .map_err(|error| unanswered(&error))?
        .ok_or_else(|| ProbeError::Unanswered("closed without answering".into()))?;
    if frame.kind != FrameKind::Control {
        return Err(ProbeError::Unanswered("unexpected frame".into()));
    }
    let message = serde_json::from_slice::<ServerMessage>(&frame.payload)
        .map_err(|error| unanswered(&error))?;
    match hello_reply(message) {
        Ok(hello) => Ok(hello),
        Err(ClientError::VersionMismatch { version, pid, .. }) => {
            Err(ProbeError::Rejected { version, pid })
        }
        Err(error) => Err(unanswered(&error)),
    }
}

/// Sends one of the requests every core answers, whatever protocol it speaks.
/// An upgrade is answered before the core replaces itself.
pub fn frozen_request(
    socket: &Path,
    request: &FrozenRequest,
    timeout: Duration,
) -> Result<FrozenReply, ClientError> {
    let mut stream = StdUnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(&encode_frozen(request)?)?;
    let frame = read_frame_sync(&mut stream)?
        .ok_or_else(|| ClientError::Handshake("the core closed without answering".into()))?;
    if frame.kind != FrameKind::Frozen {
        return Err(ClientError::Handshake(
            "the core answered with another kind of frame".into(),
        ));
    }
    Ok(serde_json::from_slice(&frame.payload)?)
}

/// Waits for the core at `socket`, which accepted an upgrade while it was
/// `pid` running `old`, to answer again from the same process with another
/// build. The same build answering means the upgrade failed and the old core
/// carried on. Without `old`, the old core spoke another protocol, so any
/// answer from the same process is the new build.
pub fn await_upgrade(
    socket: &Path,
    pid: u32,
    old: Option<&BuildIdentity>,
    timeout: Duration,
) -> Result<CoreHello, ClientError> {
    wait_for_new_build(socket, pid, old, timeout, false)
}

/// Like [`await_upgrade`] for an upgrade the core deferred: it goes on
/// answering as the old build until its chat turns end.
pub fn await_deferred_upgrade(
    socket: &Path,
    pid: u32,
    old: Option<&BuildIdentity>,
    timeout: Duration,
) -> Result<CoreHello, ClientError> {
    wait_for_new_build(socket, pid, old, timeout, true)
}

fn wait_for_new_build(
    socket: &Path,
    pid: u32,
    old: Option<&BuildIdentity>,
    timeout: Duration,
    deferred: bool,
) -> Result<CoreHello, ClientError> {
    let deadline = Instant::now() + timeout;
    let is_old = |hello: &CoreHello| old.is_some_and(|old| hello.build.same_build(old));
    loop {
        let timed_out = Instant::now() >= deadline;
        match probe(socket, PROBE_TIMEOUT) {
            Ok(hello) if hello.pid != pid => {
                return Err(ClientError::Core(format!(
                    "a new core (pid {}) answered instead of the upgraded one (pid {pid})",
                    hello.pid
                )))
            }
            Ok(hello) if !is_old(&hello) => return Ok(hello),
            Ok(_) if !deferred || timed_out => {
                return Err(ClientError::Core(
                    "the core could not replace itself and carried on as it was".into(),
                ))
            }
            Err(ProbeError::Rejected {
                pid: answered,
                version,
            }) if answered == pid && version != PROTOCOL_VERSION && (!deferred || timed_out) => {
                return Err(ClientError::VersionMismatch {
                    version,
                    pid,
                    message: format!(
                        "the upgraded core speaks protocol version {version}, not {PROTOCOL_VERSION}"
                    ),
                })
            }
            Err(_) if timed_out => {
                return Err(ClientError::Core(format!(
                    "the core did not come back within {timeout:?} of accepting an upgrade"
                )))
            }
            _ => std::thread::sleep(START_POLL),
        }
    }
}

/// Starts `<binary> core --socket <socket> <args>` in its own session,
/// detached from the caller, with its output appended to `log`.
fn start_detached(socket: &Path, binary: &Path, log: &Path, args: &[OsString]) -> io::Result<()> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)?;
    let mut command = sikemux_process::user_environment::command(binary);
    command
        .arg("core")
        .arg("--socket")
        .arg(socket)
        .args(args)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: the hook runs in the forked child before exec and calls only
    // fork, setsid and _exit, which are async-signal-safe. The intermediate
    // child exits at once, so the core is never the caller's child and is
    // never left a zombie.
    unsafe {
        command.pre_exec(|| match libc::fork() {
            -1 => Err(io::Error::last_os_error()),
            0 => {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }
            _ => libc::_exit(0),
        });
    }
    command.spawn()?.wait()?;
    Ok(())
}

/// Finds the core at `socket`, or starts one with `args` after its socket.
pub fn ensure_running(
    socket: &Path,
    binary: &Path,
    log: &Path,
    args: &[OsString],
) -> Result<CoreHello, ClientError> {
    let reject = |version, pid| {
        ClientError::VersionMismatch {
        version,
        pid,
        message: format!(
            "the Sikemux core at {} (pid {pid}) speaks protocol version {version}, not {PROTOCOL_VERSION}",
            socket.display()
        ),
    }
    };
    match probe(socket, PROBE_TIMEOUT) {
        Ok(hello) => return Ok(hello),
        Err(ProbeError::Rejected { version, pid }) => return Err(reject(version, pid)),
        Err(ProbeError::NotRunning(_) | ProbeError::Unanswered(_)) => {}
    }
    start_detached(socket, binary, log, args)?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match probe(socket, PROBE_TIMEOUT) {
            Ok(hello) => return Ok(hello),
            Err(ProbeError::Rejected { version, pid }) => return Err(reject(version, pid)),
            Err(_) if Instant::now() >= deadline => {
                return Err(ClientError::StartTimeout(START_TIMEOUT))
            }
            Err(_) => std::thread::sleep(START_POLL),
        }
    }
}
