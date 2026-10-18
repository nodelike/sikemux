//! One call to Sikemux's tool endpoint, as an agent's tools server and the git
//! shim make it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

use super::protocol::MAX_CLI_FRAME_BYTES;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Why a call failed: whether Sikemux could have started on the request.
pub enum Failure {
    NotSent(String),
    Sent(String),
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
    let unreachable = |message: &str| Failure::NotSent(message.to_owned());
    let descriptor: Value = serde_json::from_slice(
        &std::fs::read(endpoint).map_err(|_| unreachable("Sikemux is not running"))?,
    )
    .map_err(|_| unreachable("Sikemux CLI endpoint is invalid"))?;
    let port = descriptor
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| unreachable("Sikemux CLI endpoint is invalid"))?;
    let (Some(protocol), Some(token)) = (
        descriptor.get("protocol"),
        descriptor.get("token").and_then(Value::as_str),
    ) else {
        return Err(unreachable("Sikemux CLI endpoint is invalid"));
    };

    let mut frame = serde_json::to_vec(&json!({
        "command": "harness",
        "protocol": protocol,
        "token": token,
        "request": {
            "id": Uuid::new_v4().to_string(),
            "project": project,
            "agentId": agent_id,
            "method": method,
            "params": params,
        },
    }))
    .map_err(|error| Failure::Sent(error.to_string()))?;
    frame.push(b'\n');
    if frame.len() as u64 > MAX_CLI_FRAME_BYTES {
        return Err(Failure::Sent(format!(
            "Harness request exceeds {} KiB",
            MAX_CLI_FRAME_BYTES / 1024
        )));
    }

    let stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        CONNECT_TIMEOUT,
    )
    .map_err(|_| unreachable("Sikemux is not running"))?;
    stream
        .set_write_timeout(Some(CONNECT_TIMEOUT))
        .and_then(|()| stream.set_read_timeout(Some(timeout)))
        .map_err(|error| {
            Failure::NotSent(format!("cannot configure the Sikemux connection: {error}"))
        })?;
    let mut reader = BufReader::new(&stream);
    let nonce = super::auth::new_nonce();
    let mut hello = json!({ "command": "hello", "protocol": protocol, "nonce": nonce }).to_string();
    hello.push('\n');
    send(&stream, hello.as_bytes()).map_err(Failure::NotSent)?;
    let expected = super::auth::server_proof(token, port, &nonce);
    let answer = receive(&mut reader).map_err(Failure::NotSent)?;
    let proven = answer.get("status").and_then(Value::as_str) == Some("hello")
        && answer
            .get("proof")
            .and_then(Value::as_str)
            .is_some_and(|proof| super::auth::same_secret(proof, &expected));
    if !proven {
        return Err(Failure::NotSent(
            match answer.get("message").and_then(Value::as_str) {
                Some(message) if answer.get("status").and_then(Value::as_str) == Some("error") => {
                    message.to_owned()
                }
                _ => "The program on Sikemux's CLI port is not Sikemux; restart Sikemux".into(),
            },
        ));
    }

    send(&stream, &frame).map_err(Failure::Sent)?;
    let response = receive(&mut reader).map_err(Failure::Sent)?;
    match response.get("status").and_then(Value::as_str) {
        Some("result") => response
            .get("value")
            .cloned()
            .ok_or_else(|| Failure::Sent("Unexpected harness response".into())),
        Some("error") => Err(Failure::Sent(
            response
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Harness request failed")
                .to_owned(),
        )),
        _ => Err(Failure::Sent("Unexpected harness response".into())),
    }
}

fn send(mut stream: &TcpStream, frame: &[u8]) -> Result<(), String> {
    stream
        .write_all(frame)
        .and_then(|()| stream.flush())
        .map_err(|error| format!("Sikemux did not accept the request: {error}"))
}

fn receive(reader: &mut BufReader<&TcpStream>) -> Result<Value, String> {
    let mut answer = Vec::new();
    reader
        .by_ref()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_until(b'\n', &mut answer)
        .map_err(|error| format!("Sikemux did not answer: {error}"))?;
    if answer.len() as u64 > MAX_RESPONSE_BYTES || !answer.ends_with(b"\n") {
        return Err("Invalid or oversized harness response".into());
    }
    serde_json::from_slice(&answer).map_err(|_| "Invalid or oversized harness response".into())
}
