use crate::server::parts::Parts;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sikemux_process::sync::Locked;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Notify};

use sikemux_wire::protocol::{
    ask::Ask, decode_input, encode_control, fits, read_frame, ClientMessage, FrameKind, Request,
    RequestId, Response, ServerMessage, SessionId, StreamId, OLDEST_PROTOCOL_VERSION, PROTOCOL,
    PROTOCOL_VERSION,
};

use super::access::{self, Peer};
use super::session;
use super::{local_peer, requests, upgrade, Core, CoreError, CoreResult};

pub(crate) type ClientId = u64;
pub(crate) type FrameReader = BufReader<Box<dyn AsyncRead + Send + Unpin>>;
pub(crate) type FrameWriter = BufWriter<Box<dyn AsyncWrite + Send + Unpin>>;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// A client that stops reading entirely is cut off rather than buffered for.
const MAX_CLIENT_BACKLOG: usize = 64 * 1024 * 1024;
pub(super) const SHUTDOWN_FLUSH: Duration = Duration::from_secs(1);

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
    pub(super) watches_view: AtomicBool,
    /// A phone's app is in front, as far as it last said.
    pub(super) foreground: AtomicBool,
}

impl ClientConn {
    fn new(
        core: &Core,
        peer: Peer,
        pid: Option<u32>,
        frames: mpsc::UnboundedSender<Arc<[u8]>>,
    ) -> Arc<Self> {
        Arc::new(Self {
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
        })
    }

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
        (Self::new(core, peer, None, frames), queue)
    }

    /// The reader of a client [`inside`](Self::inside) the core took a frame.
    pub(crate) fn took(&self, bytes: usize) {
        self.queued.fetch_sub(bytes, Ordering::AcqRel);
    }

    /// A client with nobody on the other end, registered with the core.
    #[cfg(test)]
    pub(crate) fn stand_in(core: &Core, peer: Peer) -> Arc<Self> {
        let (frames, _) = mpsc::unbounded_channel();
        let client = Self::new(core, peer, None, frames);
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
                kind: None,
                detail: None,
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
                message: error.message,
                kind: error.kind,
                detail: error.detail,
            },
        });
    }

    pub(crate) fn answer<A: Ask>(&self, request_id: RequestId, result: CoreResult<A::Answer>) {
        self.respond(request_id, result.map(A::reply));
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
    if core.updating().is_frozen() {
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
                    build: core.config.build.clone(),
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
                    kind: None,
                    detail: None,
                },
            )
            .await;
            false
        }
    }
}

pub(crate) async fn serve_local(core: Arc<Core>, stream: UnixStream) {
    let peer = local_peer::identify(&stream).await;
    let pid = local_peer::pid(&stream);
    let (read_half, write_half) = stream.into_split();
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
    let client = ClientConn::new(&core, peer, pid, frames);
    core.register_client(client.clone());
    if !client.peer.is_local() {
        core.send_device_view(&client);
    }
    let (work, work_queue) = mpsc::unbounded_channel();
    tokio::select! {
        _ = read_requests(&core, &client, &mut reader, work) => {}
        _ = write_frames(&mut writer, queue, &client) => {}
        _ = requests::run(&core, &client, work_queue) => {}
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
                    kind: None,
                    detail: None,
                }),
                Err(error) => client.send_message(&ServerMessage::Error {
                    request_id: unreadable_request_id(&frame.payload),
                    message: format!("unreadable message: {error}"),
                    kind: None,
                    detail: None,
                }),
            },
            FrameKind::Input => {
                let Some((request_id, id, bytes)) = decode_input(&frame.payload) else {
                    return;
                };
                if let Err(refused) = requests::admit(core, client, &access::Rule::TERMINAL_INPUT) {
                    client.respond(request_id, Err(refused));
                    continue;
                }
                let Some(target) = core.session(id) else {
                    client.respond(request_id, Err("invalid argument: pty not found".into()));
                    continue;
                };
                requests::write_to_program(core, client, request_id, &target, bytes.to_vec());
            }
            FrameKind::Grid | FrameKind::Snapshot | FrameKind::Lines | FrameKind::Frozen => return,
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

/// The id of a request this core cannot read, such as one added in a later
/// release, so the client hears it was refused instead of waiting.
fn unreadable_request_id(payload: &[u8]) -> Option<RequestId> {
    let message: serde_json::Value = serde_json::from_slice(payload).ok()?;
    if message.get("type")?.as_str()? != "request" {
        return None;
    }
    message.get("requestId")?.as_u64()
}

/// The connection a call came on, for a stream to send its parts down.
pub(super) fn connection_of(core: &Core, client: &ClientConn) -> CoreResult<Arc<ClientConn>> {
    core.client_conn(client.id)
        .ok_or_else(|| CoreError::from("this connection is closing"))
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
        let hello: sikemux_wire::protocol::ClientMessage =
            serde_json::from_str(r#"{"type":"hello","protocol":"sikemux-core","version":9}"#)
                .expect("reads");
        assert!(matches!(
            hello,
            sikemux_wire::protocol::ClientMessage::Hello {
                version: 9,
                newest: None,
                ..
            }
        ));
    }
}
