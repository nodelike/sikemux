//! The loopback TCP endpoint the `sikemux` CLI and agents' tool server call.
//! One request per connection, one JSON object per line. The endpoint file
//! names the port and a token; before a client sends the token it makes the
//! core prove it holds the same one.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::cli::auth::{same_secret, server_proof};
use crate::cli::endpoint::{remove_owned_endpoint, write_endpoint};
use crate::cli::protocol::{
    CliClientCommand, CliClientHello, CliCloseReason, CliEndpointDescriptor, CliOpenOutcome,
    CliServerResponse, CLI_PROTOCOL_VERSION, MAX_CLI_FRAME_BYTES, MAX_CLI_RESPONSE_BYTES,
};
use crate::protocol::WindowCall;

use super::{harness, Core};

const MAX_CONNECTIONS: usize = 128;
/// One deadline for the whole request, so a client sending a byte every few
/// seconds cannot hold its connection forever.
const REQUEST_READ_DEADLINE: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const OPEN_ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) struct ToolEndpoint {
    path: PathBuf,
    token: String,
    fd: RawFd,
    open: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn descriptor(core: &Core, port: u16, token: &str) -> CliEndpointDescriptor {
    CliEndpointDescriptor {
        protocol: CLI_PROTOCOL_VERSION,
        pid: std::process::id(),
        port,
        token: token.to_owned(),
        version: core.build.version.clone(),
    }
}

impl ToolEndpoint {
    /// Listens on a free loopback port and publishes it at `path`.
    pub(crate) async fn start(core: Arc<Core>, path: PathBuf) -> std::io::Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let token = new_token();
        let published = descriptor(&core, port, &token);
        let publish = path.clone();
        tokio::task::spawn_blocking(move || write_endpoint(&publish, &published))
            .await
            .map_err(std::io::Error::other)??;
        Ok(Self::serve(core, listener, path, port, token))
    }

    /// Keeps serving a listener an earlier core handed over, with its port and
    /// token, and publishes it again if the endpoint file no longer says so.
    pub(crate) async fn adopt(
        core: Arc<Core>,
        listener: std::net::TcpListener,
        path: PathBuf,
        token: String,
    ) -> std::io::Result<Self> {
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        let port = listener.local_addr()?.port();
        let published = descriptor(&core, port, &token);
        let publish = path.clone();
        tokio::task::spawn_blocking(move || {
            let current = std::fs::read(&publish)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<CliEndpointDescriptor>(&bytes).ok());
            if current.as_ref() == Some(&published) {
                return Ok(());
            }
            write_endpoint(&publish, &published)
        })
        .await
        .map_err(std::io::Error::other)??;
        Ok(Self::serve(core, listener, path, port, token))
    }

    fn serve(
        core: Arc<Core>,
        listener: TcpListener,
        path: PathBuf,
        port: u16,
        token: String,
    ) -> Self {
        let fd = listener.as_raw_fd();
        let secret: Arc<str> = token.clone().into();
        let open = Arc::new(AtomicUsize::new(0));
        let counted = open.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                if counted
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        (count < MAX_CONNECTIONS).then_some(count + 1)
                    })
                    .is_err()
                {
                    continue;
                }
                let slot = Slot(counted.clone());
                let core = core.clone();
                let secret = secret.clone();
                tokio::spawn(async move {
                    serve(core, stream, &secret, port).await;
                    drop(slot);
                });
            }
        });
        Self {
            path,
            token,
            fd,
            open,
            task,
        }
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.fd
    }

    /// How many calls are being answered right now.
    pub(crate) fn open_calls(&self) -> Arc<AtomicUsize> {
        self.open.clone()
    }

    /// Stops listening and removes the endpoint file, unless another process
    /// has published its own since.
    pub(crate) fn stop(self) {
        self.task.abort();
        remove_owned_endpoint(&self.path, &self.token);
    }
}

async fn read_line<T: DeserializeOwned>(
    reader: &mut BufReader<OwnedReadHalf>,
    deadline: Instant,
) -> Result<T, String> {
    let mut frame = Vec::new();
    let read = tokio::time::timeout_at(
        deadline,
        (&mut *reader)
            .take(MAX_CLI_FRAME_BYTES + 1)
            .read_until(b'\n', &mut frame),
    )
    .await;
    if !matches!(read, Ok(Ok(_))) {
        return Err("could not read the CLI request".into());
    }
    if frame.len() as u64 > MAX_CLI_FRAME_BYTES {
        return Err("CLI request is too large".into());
    }
    serde_json::from_slice(&frame).map_err(|_| "invalid CLI request".into())
}

async fn write(writer: &mut OwnedWriteHalf, response: &CliServerResponse) -> bool {
    let mut bytes = match serde_json::to_vec(response) {
        Ok(bytes) if (bytes.len() as u64) < MAX_CLI_RESPONSE_BYTES => bytes,
        Ok(_) => serde_json::to_vec(&CliServerResponse::Error {
            message: "Response exceeds 4 MiB; reduce the requested output".into(),
        })
        .unwrap_or_default(),
        Err(_) => return false,
    };
    bytes.push(b'\n');
    matches!(
        tokio::time::timeout(WRITE_TIMEOUT, async {
            writer.write_all(&bytes).await?;
            writer.flush().await
        })
        .await,
        Ok(Ok(()))
    )
}

fn authenticate(protocol: u16, token: &str, secret: &str) -> Result<(), String> {
    if protocol != CLI_PROTOCOL_VERSION {
        return Err(format!(
            "CLI protocol mismatch (client {protocol}, app {CLI_PROTOCOL_VERSION}); update or restart Sikemux"
        ));
    }
    if !same_secret(token, secret) {
        return Err("CLI authentication failed".into());
    }
    Ok(())
}

async fn serve(core: Arc<Core>, stream: TcpStream, secret: &str, port: u16) {
    let (read_half, mut writer) = stream.into_split();
    if core.is_frozen() {
        write(
            &mut writer,
            &CliServerResponse::Error {
                message: super::upgrade::UPDATING.into(),
            },
        )
        .await;
        return;
    }
    let mut reader = BufReader::new(read_half);
    let deadline = Instant::now() + REQUEST_READ_DEADLINE;
    let hello = match read_line::<CliClientHello>(&mut reader, deadline).await {
        Ok(CliClientHello::Hello { protocol, .. }) if protocol != CLI_PROTOCOL_VERSION => Err(format!(
            "CLI protocol mismatch (client {protocol}, app {CLI_PROTOCOL_VERSION}); update or restart Sikemux"
        )),
        Ok(CliClientHello::Hello { nonce, .. }) if nonce.is_empty() || nonce.len() > 256 => {
            Err("invalid CLI hello".into())
        }
        Ok(CliClientHello::Hello { nonce, .. }) => Ok(CliServerResponse::Hello {
            proof: server_proof(secret, port, &nonce),
        }),
        Err(message) => Err(message),
    };
    match hello {
        Ok(response) => {
            if !write(&mut writer, &response).await {
                return;
            }
        }
        Err(message) => {
            write(&mut writer, &CliServerResponse::Error { message }).await;
            return;
        }
    }
    let command = match read_line::<CliClientCommand>(&mut reader, deadline).await {
        Ok(command) => command,
        Err(message) => {
            write(&mut writer, &CliServerResponse::Error { message }).await;
            return;
        }
    };
    let response = match command {
        CliClientCommand::Ping { protocol, token } => {
            authenticate(protocol, &token, secret).map(|()| CliServerResponse::Pong {
                protocol: CLI_PROTOCOL_VERSION,
                version: core.build.version.clone(),
                window: core.window.is_open(),
            })
        }
        CliClientCommand::Harness {
            protocol,
            token,
            request,
        } => match authenticate(protocol, &token, secret) {
            Ok(()) => {
                let answer = tokio::select! {
                    answer = harness::call(&core, request) => answer,
                    () = core.until_frozen() => Err(super::upgrade::UPDATING.into()),
                };
                Ok(match answer {
                    Ok(value) => CliServerResponse::Result { value },
                    Err(message) => CliServerResponse::Error { message },
                })
            }
            Err(message) => Err(message),
        },
        CliClientCommand::Open {
            protocol,
            token,
            request,
        } => {
            if let Err(message) =
                authenticate(protocol, &token, secret).and_then(|()| request.validate())
            {
                write(&mut writer, &CliServerResponse::Error { message }).await;
                return;
            }
            let closed = tokio::select! {
                () = open(&core, &mut writer, request) => false,
                () = core.until_frozen() => true,
            };
            if closed {
                write(
                    &mut writer,
                    &CliServerResponse::Error {
                        message: super::upgrade::UPDATING.into(),
                    },
                )
                .await;
            }
            return;
        }
    };
    let response = response.unwrap_or_else(|message| CliServerResponse::Error { message });
    write(&mut writer, &response).await;
}

/// Answers once the window has opened every target, and for `--wait` again
/// once their tabs have closed.
async fn open(
    core: &Core,
    writer: &mut OwnedWriteHalf,
    request: crate::cli::protocol::CliOpenRequest,
) {
    let request_id = request.id.clone();
    let wait = request.wait;
    let call = match core
        .window
        .call(WindowCall::Open { request }, "open files", wait)
    {
        Ok(call) => call,
        Err(message) => {
            write(writer, &CliServerResponse::Error { message }).await;
            return;
        }
    };
    let call_id = call.id;
    let accepted = match tokio::time::timeout(OPEN_ACCEPT_TIMEOUT, call.answer).await {
        Ok(Ok(Ok(value))) => serde_json::from_value::<CliOpenOutcome>(value)
            .map_err(|_| "Sikemux sent an unreadable open result".to_string()),
        Ok(Ok(Err(message))) => Err(message),
        Ok(Err(_)) => Err("Sikemux closed before the editor accepted the request".into()),
        Err(_) => Err("Sikemux did not finish opening the request within 60 seconds".into()),
    };
    let outcome = match accepted {
        Ok(outcome) => outcome,
        Err(message) => {
            core.window.forget(call_id);
            write(writer, &CliServerResponse::Error { message }).await;
            return;
        }
    };
    let sent = write(
        writer,
        &CliServerResponse::Accepted {
            request_id: request_id.clone(),
            opened: outcome.opened,
            failed: outcome.failed,
        },
    )
    .await;
    if !sent || !wait {
        core.window.forget(call_id);
        return;
    }
    let reason = match call.closed.await {
        Ok(true) => CliCloseReason::TabsClosed,
        _ => CliCloseReason::AppExit,
    };
    write(writer, &CliServerResponse::Closed { request_id, reason }).await;
}
