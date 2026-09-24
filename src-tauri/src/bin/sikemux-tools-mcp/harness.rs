//! One tool call, one connection to the app's CLI broker.

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The app answers a browser call only once the page settles, which it gives
/// itself a minute to do.
const REPLY_TIMEOUT: Duration = Duration::from_secs(70);
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

pub fn call(agent_id: &str, method: &str, params: &Value) -> Result<Value, String> {
    let endpoint = env::var_os("SIKEMUX_CLI_ENDPOINT")
        .filter(|value| !value.is_empty())
        .ok_or("Missing SIKEMUX_CLI_ENDPOINT; launch this MCP from Sikemux")?;
    let project = env::var("SIKEMUX_PROJECT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .ok_or("the current directory is unavailable")?;
    relay(Path::new(&endpoint), &project, agent_id, method, params)
}

pub fn relay(
    endpoint: &Path,
    project: &str,
    agent_id: &str,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    let descriptor: Value = serde_json::from_slice(
        &std::fs::read(endpoint).map_err(|_| "Sikemux is not running".to_string())?,
    )
    .map_err(|_| "Sikemux CLI endpoint is invalid")?;
    let port = descriptor
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .ok_or("Sikemux CLI endpoint is invalid")?;
    let (Some(protocol), Some(token)) = (descriptor.get("protocol"), descriptor.get("token"))
    else {
        return Err("Sikemux CLI endpoint is invalid".into());
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
    .map_err(|error| error.to_string())?;
    frame.push(b'\n');
    if frame.len() > MAX_REQUEST_BYTES {
        return Err("Harness request exceeds 64 KiB".into());
    }

    let stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        CONNECT_TIMEOUT,
    )
    .map_err(|_| "Sikemux is not running".to_string())?;
    stream
        .set_write_timeout(Some(CONNECT_TIMEOUT))
        .and_then(|()| stream.set_read_timeout(Some(REPLY_TIMEOUT)))
        .map_err(|error| format!("cannot configure the Sikemux connection: {error}"))?;
    let mut writer = &stream;
    writer
        .write_all(&frame)
        .and_then(|()| writer.flush())
        .map_err(|error| format!("Sikemux did not accept the request: {error}"))?;

    let mut answer = Vec::new();
    BufReader::new(&stream)
        .take(MAX_RESPONSE_BYTES + 1)
        .read_until(b'\n', &mut answer)
        .map_err(|error| format!("Sikemux did not answer: {error}"))?;
    if answer.len() as u64 > MAX_RESPONSE_BYTES || !answer.ends_with(b"\n") {
        return Err("Invalid or oversized harness response".into());
    }
    let response: Value =
        serde_json::from_slice(&answer).map_err(|_| "Invalid or oversized harness response")?;
    match response.get("status").and_then(Value::as_str) {
        Some("result") => response
            .get("value")
            .cloned()
            .ok_or_else(|| "Unexpected harness response".into()),
        Some("error") => Err(response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Harness request failed")
            .to_owned()),
        _ => Err("Unexpected harness response".into()),
    }
}
