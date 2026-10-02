use std::io::{BufRead, Read, Write};
use std::process::ChildStdin;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

use super::limits::{insert_pending_request, take_pending_request, PENDING_REQUEST_COUNT};
use super::lsp;
use super::server::{lsp_tick, ServerHandle};
use crate::error::{AppError, AppResult};
use crate::observability::{global_observability, Metadata, ScalarValue, SpanOutcome};

const MAX_LSP_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_LSP_HEADER_BYTES: usize = 8 * 1024;

fn write_frame(stdin: &mut ChildStdin, msg: &Value) -> AppResult<()> {
    let body = serde_json::to_string(msg)?;
    if body.len() > MAX_LSP_FRAME_BYTES {
        return Err(AppError::Lsp(format!(
            "outbound LSP frame exceeds {} bytes",
            MAX_LSP_FRAME_BYTES
        )));
    }
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin.write_all(header.as_bytes())?;
    stdin.write_all(body.as_bytes())?;
    stdin.flush()?;
    Ok(())
}

pub(super) fn send(server: &ServerHandle, msg: &Value) -> AppResult<()> {
    if server.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(AppError::Lsp("server shut down".into()));
    }
    server
        .last_used
        .store(lsp_tick(), std::sync::atomic::Ordering::Relaxed);
    let mut guard = server.stdin.lock().map_err(lsp)?;
    let stdin = guard
        .as_mut()
        .ok_or_else(|| AppError::Lsp("stdin gone".into()))?;
    write_frame(stdin, msg)
}

pub(super) fn read_message<R: BufRead>(reader: &mut R) -> AppResult<Option<Value>> {
    let mut content_length: usize = 0;
    let mut header_bytes = 0usize;
    loop {
        let mut bytes = Vec::new();
        let remaining = MAX_LSP_HEADER_BYTES.saturating_sub(header_bytes);
        if remaining == 0 {
            return Err(AppError::Lsp("LSP headers exceed size limit".into()));
        }
        let n = reader
            .take((remaining + 1) as u64)
            .read_until(b'\n', &mut bytes)?;
        if n == 0 {
            return Ok(None);
        }
        header_bytes = header_bytes.saturating_add(n);
        if header_bytes > MAX_LSP_HEADER_BYTES || !bytes.ends_with(b"\n") {
            return Err(AppError::Lsp("LSP headers exceed size limit".into()));
        }
        let line = std::str::from_utf8(&bytes).map_err(lsp)?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().map_err(lsp)?;
            if content_length > MAX_LSP_FRAME_BYTES {
                return Err(AppError::Lsp(format!(
                    "LSP frame exceeds {} bytes",
                    MAX_LSP_FRAME_BYTES
                )));
            }
        }
    }
    if content_length == 0 {
        return Err(AppError::Lsp("missing or empty Content-Length".into()));
    }
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

pub(super) fn next_id(server: &ServerHandle) -> AppResult<i64> {
    // If the mutex is poisoned a request thread crashed mid-allocate.
    // Recover the inner counter instead of crashing the whole LSP layer —
    // request IDs remain unique even after recovery.
    let mut id = server.next_id.lock().unwrap_or_else(|p| p.into_inner());
    let v = *id;
    *id = id
        .checked_add(1)
        .ok_or_else(|| AppError::Lsp("language-server request id exhausted".into()))?;
    Ok(v)
}

pub(super) fn request_with_timeout(
    server: &ServerHandle,
    method: &str,
    params: Value,
    timeout: Duration,
) -> AppResult<Value> {
    let mut metadata = Metadata::new();
    metadata.insert("method".to_owned(), ScalarValue::from(method));
    let span = global_observability().begin_span("lsp.request", None, metadata);
    let result = request_with_timeout_inner(server, method, params, timeout);
    span.finish(if result.is_ok() {
        SpanOutcome::Success
    } else {
        SpanOutcome::Error
    });
    result
}

fn request_with_timeout_inner(
    server: &ServerHandle,
    method: &str,
    params: Value,
    timeout: Duration,
) -> AppResult<Value> {
    let id = next_id(server)?;
    let (tx, rx) = mpsc::channel();
    insert_pending_request(&server.pending, &PENDING_REQUEST_COUNT, id, tx)?;
    let req = json!({
        "jsonrpc": "2.0", "id": id,
        "method": method, "params": params
    });
    if let Err(e) = send(server, &req) {
        take_pending_request(&server.pending, &PENDING_REQUEST_COUNT, id);
        return Err(e);
    }
    match rx.recv_timeout(timeout) {
        Ok(v) => Ok(v),
        Err(e) => {
            take_pending_request(&server.pending, &PENDING_REQUEST_COUNT, id);
            Err(AppError::Lsp(match e {
                mpsc::RecvTimeoutError::Timeout => format!("{method} timed out"),
                mpsc::RecvTimeoutError::Disconnected => {
                    format!("{method} failed: the language server exited")
                }
            }))
        }
    }
}

pub(super) fn request(server: &ServerHandle, method: &str, params: Value) -> AppResult<Value> {
    request_with_timeout(server, method, params, Duration::from_secs(4))
}

pub(super) fn notify(server: &ServerHandle, method: &str, params: Value) -> AppResult<()> {
    let n = json!({"jsonrpc": "2.0", "method": method, "params": params});
    send(server, &n)
}

pub(super) fn response_for_server_request(method: &str, params: &Value) -> Value {
    match method {
        // gopls / rust-analyzer / pyright may ask for workspace config during
        // initialization. Returning null here can make them treat the client as
        // broken; an empty object per requested item is the safe fast default.
        "workspace/configuration" => {
            let n = params
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(1);
            Value::Array((0..n).map(|_| json!({})).collect())
        }
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability" => Value::Null,
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, Cursor};

    #[test]
    fn reads_bounded_lsp_frame() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend_from_slice(body);
        let mut reader = BufReader::new(Cursor::new(frame));

        assert_eq!(
            read_message(&mut reader).expect("frame"),
            Some(json!({
                "jsonrpc": "2.0", "id": 1, "result": null
            }))
        );
    }

    #[test]
    fn rejects_oversized_lsp_frame_before_allocating_body() {
        let frame = format!("Content-Length: {}\r\n\r\n", MAX_LSP_FRAME_BYTES + 1);
        let mut reader = BufReader::new(Cursor::new(frame.into_bytes()));
        let error = read_message(&mut reader).expect_err("oversized frame");
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn rejects_oversized_lsp_headers() {
        let frame = format!("X-Long: {}\r\n\r\n", "x".repeat(MAX_LSP_HEADER_BYTES));
        let mut reader = BufReader::new(Cursor::new(frame.into_bytes()));
        let error = read_message(&mut reader).expect_err("oversized headers");
        assert!(error.to_string().contains("headers exceed"));
    }
}
