use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::AppHandle;

use super::diagnostics::handle_server_notification;
use super::discovery::server_command;
use super::limits::{clear_pending_requests, take_pending_request, PENDING_REQUEST_COUNT};
use super::protocol::path_to_uri;
use super::server::{lsp_tick, shutdown_server, Server, ServerHandle};
use super::transport::{
    notify, read_message, request_with_timeout, response_for_server_request, send,
};
use crate::error::{AppError, AppResult};

struct SpawnedProcessGuard(Option<Child>);

impl SpawnedProcessGuard {
    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("spawned process guard empty")
    }

    fn into_inner(mut self) -> Child {
        self.0.take().expect("spawned process guard empty")
    }
}

impl Drop for SpawnedProcessGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(super) fn spawn_server(
    project: &str,
    language: &str,
    app: AppHandle,
) -> AppResult<ServerHandle> {
    let (bin, args) = server_command(language)
        .ok_or_else(|| AppError::Lsp(format!("no language server configured for `{language}`")))?;
    let child = sikemux_process::user_environment::command(&bin)
        .args(&args)
        .current_dir(project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::LspServerMissing {
                    language: language.to_string(),
                    bin: bin.clone(),
                }
            } else {
                AppError::Lsp(format!("spawn {bin}: {e}"))
            }
        })?;
    let mut child = SpawnedProcessGuard(Some(child));
    let stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or(AppError::Lsp("no stdin".into()))?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or(AppError::Lsp("no stdout".into()))?;
    let stderr = child.child_mut().stderr.take();
    let child = child.into_inner();

    let server = Arc::new(Server {
        app,
        project: project.to_owned(),
        language: language.to_owned(),
        child: Mutex::new(Some(child)),
        stdin: Mutex::new(Some(stdin)),
        next_id: Mutex::new(1),
        pending: Mutex::new(HashMap::new()),
        last_change: Mutex::new(HashMap::new()),
        open_docs: Mutex::new(HashMap::new()),
        diagnostic_paths: Mutex::new(HashMap::new()),
        shutdown: std::sync::atomic::AtomicBool::new(false),
        last_used: std::sync::atomic::AtomicU64::new(lsp_tick()),
        idle_generation: std::sync::atomic::AtomicU64::new(0),
    });

    let reader_server = server.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Ok(Some(msg)) = read_message(&mut reader) {
            if reader_server
                .shutdown
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                break;
            }
            if let Some(id_value) = msg.get("id").cloned() {
                if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                    // Server-initiated request — reply with a shape the common
                    // language servers accept so they don't stall or downgrade
                    // features while waiting on client config/progress support.
                    let result = response_for_server_request(
                        method,
                        msg.get("params").unwrap_or(&Value::Null),
                    );
                    let reply = json!({"jsonrpc": "2.0", "id": id_value, "result": result});
                    let _ = send(&reader_server, &reply);
                } else if let Some(id) = id_value.as_i64() {
                    if let Some(tx) =
                        take_pending_request(&reader_server.pending, &PENDING_REQUEST_COUNT, id)
                    {
                        let val = msg.get("result").cloned().unwrap_or(Value::Null);
                        let _ = tx.send(val);
                    }
                }
            } else if let Some(method) = msg.get("method").and_then(Value::as_str) {
                handle_server_notification(
                    &reader_server,
                    method,
                    msg.get("params").unwrap_or(&Value::Null),
                );
            }
        }
        // Reader exit unblocks any pending RPC waiters so they fail fast
        // instead of timing out. It also owns teardown for malformed frames or
        // natural server exit; otherwise the registry would retain an unusable
        // server forever and later lsp_start calls would falsely succeed.
        clear_pending_requests(&reader_server.pending, &PENDING_REQUEST_COUNT);
        shutdown_server(reader_server);
    });

    let stderr_tail = stderr.map(drain_stderr);

    let init = json!({
        "processId": std::process::id(),
        "rootUri": path_to_uri(project),
        "capabilities": {
            "textDocument": {
                "synchronization": {
                    "dynamicRegistration": false,
                    "willSave": false,
                    "willSaveWaitUntil": false,
                    "didSave": true
                },
                "definition": { "linkSupport": false },
                "implementation": { "linkSupport": false },
                "references": {},
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                "completion": { "completionItem": { "snippetSupport": false } },
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "publishDiagnostics": {
                    "relatedInformation": false,
                    "versionSupport": true
                }
            },
            "workspace": {
                "workspaceFolders": true,
                "configuration": true
            }
        },
        "workspaceFolders": [{ "uri": path_to_uri(project), "name": project }]
    });
    if let Err(error) = request_with_timeout(&server, "initialize", init, Duration::from_secs(20))
        .and_then(|_| notify(&server, "initialized", json!({})))
    {
        shutdown_server(server.clone());
        let tail = stderr_tail
            .and_then(|tail| tail.recv_timeout(Duration::from_secs(1)).ok())
            .unwrap_or_default();
        return Err(with_stderr_tail(error, &tail));
    }
    Ok(server)
}

const STDERR_TAIL_LINES: usize = 8;
const STDERR_TAIL_LINE_CHARS: usize = 300;

// rust-analyzer and pyright log heavily to stderr; an undrained pipe fills up
// and blocks the server. The last few lines come back once the pipe closes, so
// a server that dies on startup can say why.
fn drain_stderr(stderr: impl Read + Send + 'static) -> mpsc::Receiver<Vec<String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        let mut tail = VecDeque::with_capacity(STDERR_TAIL_LINES);
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if tail.len() == STDERR_TAIL_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(trimmed.chars().take(STDERR_TAIL_LINE_CHARS).collect());
                }
            }
        }
        let _ = tx.send(tail.into());
    });
    rx
}

fn with_stderr_tail(error: AppError, tail: &[String]) -> AppError {
    if tail.is_empty() {
        return error;
    }
    AppError::Lsp(format!("{}\n{}", lsp_error_message(error), tail.join("\n")))
}

fn lsp_error_message(error: AppError) -> String {
    match error {
        AppError::Lsp(message) => message,
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn stderr_tail_keeps_the_last_lines_and_explains_a_startup_failure() {
        let mut log = String::new();
        for n in 0..20 {
            log.push_str(&format!("noise {n}\n"));
        }
        log.push_str("\nerror: Unknown binary 'rust-analyzer'\n");
        let tail = drain_stderr(Cursor::new(log.into_bytes()))
            .recv_timeout(Duration::from_secs(1))
            .expect("tail arrives once stderr closes");

        assert_eq!(tail.len(), STDERR_TAIL_LINES);
        assert_eq!(
            tail.last().unwrap(),
            "error: Unknown binary 'rust-analyzer'"
        );

        let error = with_stderr_tail(
            AppError::Lsp("initialize failed: the language server exited".into()),
            &tail[tail.len() - 1..],
        );
        assert_eq!(
            error.to_string(),
            "lsp: initialize failed: the language server exited\nerror: Unknown binary 'rust-analyzer'"
        );
    }
}
