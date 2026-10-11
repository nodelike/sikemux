use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_wire::pty::agent_detection::{DetectionExplain, ManifestReloadReport};
use sikemux_wire::pty::output_log::{OutputPage, OutputQuery};
use sikemux_wire::pty::shell_protocol::ShellMetadataSnapshot;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use sikemux_wire::protocol::{
    decode_output, decode_snapshot, encode_control, encode_input, read_frame, AllowedDevice,
    Attention, BackdropImage, BuildIdentity, CallId, ChatAccount, ChatAttachment, ChatContext,
    ChatEvent, ChatInfo, ChatLaunch, ChatLauncher, ChatMark, ChatStart, ClientMessage,
    DeviceAccess, Event, FrameKind, HostRegistration, LaunchIdentity, ProjectInfo, PublishedChat,
    PublishedRecent, RemoteStatus, Request, RequestId, Response, RunSelector, ServerMessage,
    SessionId, SessionInfo, SpawnTarget, WindowAnswer, WindowCall, Workspace, MAX_FRAME_BYTES,
    OLDEST_PROTOCOL_VERSION, PROTOCOL, PROTOCOL_VERSION,
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

    pub async fn shell_at_prompt(&self, id: SessionId) -> Result<bool, ClientError> {
        match self.request(Request::ShellAtPrompt { id }).await? {
            Response::ShellAtPrompt { at_prompt } => Ok(at_prompt),
            _ => Err(ClientError::UnexpectedReply),
        }
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

    pub async fn plugins_changed(&self) -> Result<(), ClientError> {
        self.request_done(Request::PluginsChanged).await
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

    /// The chat's turns before event `before` of its run `feed`, and where
    /// the page before them starts, for a client shown only the chat's end.
    pub async fn acp_history(
        &self,
        agent_id: String,
        feed: String,
        before: u64,
        turns: u32,
    ) -> Result<(Vec<ChatEvent>, Option<u64>), ClientError> {
        let request = Request::AcpHistory {
            agent_id,
            feed,
            before,
            turns,
        };
        match self.request(request).await? {
            Response::ChatHistory {
                events,
                older_before,
            } => Ok((events, older_before)),
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
        message_id: Option<String>,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    ) -> Result<(), ClientError> {
        self.request_done(Request::AcpPrompt {
            agent_id,
            message_id,
            text,
            paths,
            context,
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn acp_edit(
        &self,
        agent_id: String,
        message_id: String,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
        restore_files: bool,
    ) -> Result<(), ClientError> {
        self.request_done(Request::AcpEdit {
            agent_id,
            message_id,
            text,
            paths,
            context,
            restore_files,
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
        let request = Request::AttachFile {
            agent_id,
            name,
            mime,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        match self.request(request).await? {
            Response::Attached { path } => Ok(path),
            _ => Err(ClientError::UnexpectedReply),
        }
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

    pub async fn acp_switch_account(
        &self,
        agent_id: String,
        account: ChatAccount,
    ) -> Result<(), ClientError> {
        self.request_done(Request::AcpSwitchAccount { agent_id, account })
            .await
    }

    async fn remote_request(&self, request: Request) -> Result<RemoteStatus, ClientError> {
        match self.request(request).await? {
            Response::Remote { status } => Ok(*status),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    /// Signs the registration text for the accounts server with the core's key.
    pub async fn sign_registration(
        &self,
        nonce: String,
        user_id: String,
    ) -> Result<HostRegistration, ClientError> {
        match self
            .request(Request::SignRegistration { nonce, user_id })
            .await?
        {
            Response::Registration { registration } => Ok(registration),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    pub async fn set_owner(&self, owner: Option<String>) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::SetOwner { owner }).await
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

    pub async fn publish_agents(
        &self,
        chats: Vec<PublishedChat>,
        titles: BTreeMap<String, String>,
    ) -> Result<(), ClientError> {
        self.request_done(Request::PublishAgents { chats, titles })
            .await
    }

    pub async fn publish_recent(&self, chats: Vec<PublishedRecent>) -> Result<(), ClientError> {
        self.request_done(Request::PublishRecent { chats }).await
    }

    pub async fn publish_on_screen(&self, agent_ids: Vec<String>) -> Result<(), ClientError> {
        self.request_done(Request::PublishOnScreen { agent_ids })
            .await
    }

    pub async fn focus_agent(&self, agent_id: String) -> Result<(), ClientError> {
        self.request_done(Request::FocusAgent { agent_id }).await
    }

    /// The device view arrives as events from now on, starting with the whole.
    pub async fn watch_view(&self) -> Result<(), ClientError> {
        self.request_done(Request::WatchView).await
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

    /// Takes up again a chat the app published as recent, by its id in the
    /// device view. Answers like [`Self::start_chat`].
    pub async fn resume_chat(&self, recent: String) -> Result<(String, ChatStart), ClientError> {
        let request = Request::ResumeChat {
            recent,
            permission_mode: None,
            model: None,
            effort: None,
        };
        match self.request(request).await? {
            Response::ChatBegun { agent_id, start } => Ok((agent_id, start)),
            _ => Err(ClientError::UnexpectedReply),
        }
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

    pub async fn allow_devices(
        &self,
        devices: Vec<AllowedDevice>,
    ) -> Result<RemoteStatus, ClientError> {
        self.remote_request(Request::AllowDevices { devices }).await
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
