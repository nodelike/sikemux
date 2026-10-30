//! One call to Sikemux's tool endpoint, as an agent's tools server and the git
//! shim make it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::auth;
use super::protocol::{
    CliClientCommand, CliClientHello, CliEndpointDescriptor, CliServerResponse, HarnessRequest,
    MAX_CLI_FRAME_BYTES, MAX_CLI_RESPONSE_BYTES,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// What a client says when the program on the endpoint's port cannot prove it is Sikemux.
pub const STALE_ENDPOINT: &str =
    "the program on Sikemux's CLI port is not Sikemux; the endpoint file may be stale";

/// Why a call failed: whether Sikemux could have started on the request.
pub enum Failure {
    NotSent(String),
    Sent(String),
}

/// The endpoint the file at `path` describes.
pub fn read_endpoint(path: &Path) -> Result<CliEndpointDescriptor, String> {
    let file = std::fs::File::open(path).map_err(|_| "Sikemux is not running".to_owned())?;
    serde_json::from_reader(file).map_err(|_| "Sikemux CLI endpoint is invalid".to_owned())
}

/// A connection to Sikemux's CLI endpoint whose server proved it holds the endpoint's
/// token, before anything is sent that a program squatting on a stale port should not see.
pub struct Session {
    reader: BufReader<TcpStream>,
}

impl Session {
    /// Connects within `connect` and says hello; each answer is waited for up to `read`.
    pub fn open(
        endpoint: &CliEndpointDescriptor,
        connect: Duration,
        read: Duration,
    ) -> Result<Self, String> {
        let stream = TcpStream::connect_timeout(
            &SocketAddrV4::new(Ipv4Addr::LOCALHOST, endpoint.port).into(),
            connect,
        )
        .map_err(|_| "Sikemux is not running".to_owned())?;
        stream
            .set_write_timeout(Some(connect))
            .and_then(|()| stream.set_read_timeout(Some(read)))
            .map_err(|error| format!("cannot configure the Sikemux connection: {error}"))?;
        let mut session = Session {
            reader: BufReader::new(stream),
        };
        let nonce = auth::new_nonce();
        session.send(&CliClientHello::Hello {
            protocol: endpoint.protocol,
            nonce: nonce.clone(),
        })?;
        let expected = auth::server_proof(&endpoint.token, endpoint.port, &nonce);
        match session.receive() {
            Ok(CliServerResponse::Hello { proof }) if auth::same_secret(&proof, &expected) => {
                Ok(session)
            }
            Ok(CliServerResponse::Error { message }) => Err(message),
            _ => Err(STALE_ENDPOINT.into()),
        }
    }

    /// Waits up to `read` for each answer from now on, or for as long as it takes.
    pub fn wait_for_answers(&self, read: Option<Duration>) -> Result<(), String> {
        self.reader
            .get_ref()
            .set_read_timeout(read)
            .map_err(|error| format!("cannot configure the Sikemux connection: {error}"))
    }

    pub fn send(&mut self, command: &impl Serialize) -> Result<(), String> {
        let mut frame = serde_json::to_vec(command).map_err(|error| error.to_string())?;
        frame.push(b'\n');
        if frame.len() as u64 > MAX_CLI_FRAME_BYTES {
            return Err(format!(
                "the request exceeds {} KiB",
                MAX_CLI_FRAME_BYTES / 1024
            ));
        }
        let stream = self.reader.get_mut();
        stream
            .write_all(&frame)
            .and_then(|()| stream.flush())
            .map_err(|error| format!("Sikemux did not accept the request: {error}"))
    }

    pub fn receive(&mut self) -> Result<CliServerResponse, String> {
        let mut frame = Vec::new();
        self.reader
            .by_ref()
            .take(MAX_CLI_RESPONSE_BYTES + 1)
            .read_until(b'\n', &mut frame)
            .map_err(|error| format!("Sikemux did not answer: {error}"))?;
        if frame.is_empty() {
            return Err("Sikemux closed the CLI connection".into());
        }
        if frame.len() as u64 > MAX_CLI_RESPONSE_BYTES {
            return Err("Sikemux sent an oversized CLI response".into());
        }
        serde_json::from_slice(&frame).map_err(|_| "Sikemux sent an invalid CLI response".into())
    }
}

/// Sends `method` once to the endpoint the file at `endpoint` describes, as
/// `agent_id`, when it says, working in `project`, and waits up to `timeout` for its answer.
pub fn attempt(
    endpoint: &Path,
    project: &str,
    agent_id: Option<&str>,
    method: &str,
    params: &Value,
    timeout: Duration,
) -> Result<Value, Failure> {
    let descriptor = read_endpoint(endpoint).map_err(Failure::NotSent)?;
    let mut session =
        Session::open(&descriptor, CONNECT_TIMEOUT, timeout).map_err(Failure::NotSent)?;
    session
        .send(&CliClientCommand::Harness {
            protocol: descriptor.protocol,
            token: descriptor.token.clone(),
            request: HarnessRequest {
                id: Uuid::new_v4().to_string(),
                project: project.to_owned(),
                agent_id: agent_id.map(str::to_owned),
                method: method.to_owned(),
                params: params.clone(),
            },
        })
        .map_err(Failure::Sent)?;
    match session.receive().map_err(Failure::Sent)? {
        CliServerResponse::Result { value } => Ok(value),
        CliServerResponse::Error { message } => Err(Failure::Sent(message)),
        _ => Err(Failure::Sent("Unexpected harness response".into())),
    }
}
