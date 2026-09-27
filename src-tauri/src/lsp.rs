// Minimal LSP client foundation: spawn a language server per (project, lang),
// frame JSON-RPC over stdio (Content-Length headers), correlate request/
// response pairs.

use std::collections::{HashMap, VecDeque};
#[cfg(unix)]
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::task;
use url::Url;

use crate::error::{AppError, AppResult};
use crate::observability::{global_observability, Metadata, ScalarValue, SpanOutcome};

const MAX_LSP_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_LSP_HEADER_BYTES: usize = 8 * 1024;
const MAX_DIAGNOSTIC_FILES_PER_SERVER: usize = 512;
const MAX_DIAGNOSTICS_PER_PUBLISH: usize = 500;
const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 2_048;
const MAX_DIAGNOSTIC_SOURCE_BYTES: usize = 128;
const MAX_DIAGNOSTIC_CODE_BYTES: usize = 128;
const MAX_LSP_PATH_BYTES: usize = 4_096;
const MAX_LSP_LANGUAGE_BYTES: usize = 128;
const MAX_DOCUMENT_SYMBOLS: usize = 2_000;
const MAX_DOCUMENT_SYMBOL_DEPTH: usize = 16;
const MAX_SYMBOL_NAME_BYTES: usize = 256;
const MAX_SYMBOL_DETAIL_BYTES: usize = 1_024;
const MAX_LSP_SERVERS: usize = 6;
const MAX_OPEN_DOCUMENTS_PER_SERVER: usize = 512;
const MAX_OPEN_DOCUMENTS_GLOBAL: usize = 2_048;
const MAX_PENDING_REQUESTS_PER_SERVER: usize = 64;
const MAX_PENDING_REQUESTS_GLOBAL: usize = 256;

static OPEN_DOCUMENT_COUNT: AtomicUsize = AtomicUsize::new(0);
static PENDING_REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);

pub const LSP_DIAGNOSTICS_EVENT: &str = "lsp_diagnostics";

fn lsp<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Lsp(e.to_string())
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LspPos {
    pub line: u32,
    pub character: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LspRange {
    pub start: LspPos,
    pub end: LspPos,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LspTextChange {
    pub range: Option<LspRange>,
    #[serde(rename = "rangeLength")]
    pub range_length: Option<u32>,
    pub text: String,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LspDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnostic {
    pub range: LspRange,
    pub severity: Option<LspDiagnosticSeverity>,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsPayload {
    pub project: String,
    pub language: String,
    pub path: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: u32,
    pub range: LspRange,
    pub selection_range: LspRange,
    pub children: Vec<LspDocumentSymbol>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OpenDoc {
    refs: usize,
    version: u32,
}

// Stdin and the pending map are independent — splitting them lets the reader
// thread deliver responses while a writer is mid-flight.
//
// `child` is held so `lsp_stop` can SIGKILL the server process; without it
// the language server would outlive every session that ever opened it.
// `shutdown` flips true once a stop has been issued — readers and writers
// check it so they bail without spamming errors as the child dies.
struct Server {
    app: AppHandle,
    project: String,
    language: String,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    next_id: Mutex<i64>,
    pending: Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    // Per-(path) hash of last full didChange/didOpen payload — drops no-op
    // resends. Incremental edits clear the hash because the backend no longer
    // has the full text to compare.
    last_change: Mutex<HashMap<String, u64>>,
    // Backend-owned open-document refcounts + monotonically increasing LSP
    // versions. The frontend can have several editor panes pointed at the same
    // URI; the server must still see exactly one didOpen and one didClose.
    open_docs: Mutex<HashMap<String, OpenDoc>>,
    // Paths with a currently published non-empty diagnostic set. The UI owns
    // the diagnostic values; native retains only bounded keys/versions so a
    // server shutdown can emit deterministic clear events.
    diagnostic_paths: Mutex<HashMap<String, Option<i64>>>,
    shutdown: std::sync::atomic::AtomicBool,
    /// Logical LRU stamp, bumped on every message we send. Admission may evict
    /// the least-recently-used idle server before spawning a replacement.
    last_used: std::sync::atomic::AtomicU64,
    /// Invalidates a pending idle shutdown whenever a document reopens.
    idle_generation: std::sync::atomic::AtomicU64,
}

type ServerHandle = Arc<Server>;

/// Internal registry identity. Keeping the two user-controlled fields typed
/// avoids delimiter collisions and makes project-scoped teardown exact. This
/// type is deliberately not serialized; public commands and event payloads
/// retain their existing string fields.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ServerKey {
    project: String,
    language: String,
}

impl ServerKey {
    fn new(project: &str, language: &str) -> Self {
        Self {
            project: project.to_owned(),
            language: language.to_owned(),
        }
    }
}

/// Monotonic logical clock for LRU ordering — cheaper and jump-proof vs
/// wall-clock time; we only need relative ordering, not real timestamps.
fn lsp_tick() -> u64 {
    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    CLOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn registry() -> &'static Mutex<HashMap<ServerKey, ServerHandle>> {
    static R: OnceLock<Mutex<HashMap<ServerKey, ServerHandle>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn start_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn server_for(project: &str, language: &str) -> Option<ServerHandle> {
    registry()
        .lock()
        .ok()?
        .get(&ServerKey::new(project, language))
        .filter(|server| !server.shutdown.load(Ordering::Acquire))
        .cloned()
}

pub fn server_count() -> usize {
    registry()
        .lock()
        .map(|registry| {
            registry
                .values()
                .filter(|server| !server.shutdown.load(Ordering::Acquire))
                .count()
        })
        .unwrap_or(0)
}

pub fn document_counts() -> (usize, usize) {
    let idle_servers = registry()
        .lock()
        .map(|registry| {
            let mut idle_servers = 0usize;
            for server in registry.values() {
                if server.shutdown.load(Ordering::Acquire) {
                    continue;
                }
                match server.open_docs.lock() {
                    Ok(docs) if docs.is_empty() => idle_servers += 1,
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
            idle_servers
        })
        .unwrap_or_default();
    (OPEN_DOCUMENT_COUNT.load(Ordering::Acquire), idle_servers)
}

fn validate_server_identity(project: &str, language: &str) -> AppResult<()> {
    if project.len() > MAX_LSP_PATH_BYTES {
        return Err(AppError::Lsp(format!(
            "project path exceeds {MAX_LSP_PATH_BYTES} bytes"
        )));
    }
    if language.len() > MAX_LSP_LANGUAGE_BYTES {
        return Err(AppError::Lsp(format!(
            "language identifier exceeds {MAX_LSP_LANGUAGE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_document_path(path: &str) -> AppResult<()> {
    if path.len() > MAX_LSP_PATH_BYTES {
        return Err(AppError::Lsp(format!(
            "document path exceeds {MAX_LSP_PATH_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_document_language_id(language_id: &str) -> AppResult<()> {
    if language_id.len() > MAX_LSP_LANGUAGE_BYTES {
        return Err(AppError::Lsp(format!(
            "document language identifier exceeds {MAX_LSP_LANGUAGE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn server_limit_error() -> AppError {
    AppError::Lsp(format!(
        "language server limit reached ({MAX_LSP_SERVERS}); close a project before starting another"
    ))
}

fn reserve_counter_slot(
    counter: &AtomicUsize,
    limit: usize,
    error: impl FnOnce() -> AppError,
) -> AppResult<()> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < limit).then_some(current + 1)
        })
        .map(|_| ())
        .map_err(|_| error())
}

fn release_counter_slots(counter: &AtomicUsize, count: usize) {
    if count == 0 {
        return;
    }
    let released = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        current.checked_sub(count)
    });
    debug_assert!(released.is_ok(), "LSP resource counter underflow");
}

fn reserve_open_document_slot(
    per_server_count: usize,
    global_counter: &AtomicUsize,
) -> AppResult<()> {
    if per_server_count >= MAX_OPEN_DOCUMENTS_PER_SERVER {
        return Err(AppError::Lsp(format!(
            "open document limit reached for language server ({MAX_OPEN_DOCUMENTS_PER_SERVER})"
        )));
    }
    reserve_counter_slot(global_counter, MAX_OPEN_DOCUMENTS_GLOBAL, || {
        AppError::Lsp(format!(
            "global open document limit reached ({MAX_OPEN_DOCUMENTS_GLOBAL})"
        ))
    })
}

fn insert_pending_request(
    pending: &Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    global_counter: &AtomicUsize,
    id: i64,
    sender: mpsc::Sender<Value>,
) -> AppResult<()> {
    let mut pending = pending.lock().map_err(lsp)?;
    if pending.len() >= MAX_PENDING_REQUESTS_PER_SERVER {
        return Err(AppError::Lsp(format!(
            "pending request limit reached for language server ({MAX_PENDING_REQUESTS_PER_SERVER})"
        )));
    }
    if pending.contains_key(&id) {
        return Err(AppError::Lsp("duplicate language-server request id".into()));
    }
    reserve_counter_slot(global_counter, MAX_PENDING_REQUESTS_GLOBAL, || {
        AppError::Lsp(format!(
            "global pending request limit reached ({MAX_PENDING_REQUESTS_GLOBAL})"
        ))
    })?;
    pending.insert(id, sender);
    Ok(())
}

fn take_pending_request(
    pending: &Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    global_counter: &AtomicUsize,
    id: i64,
) -> Option<mpsc::Sender<Value>> {
    let sender = pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id);
    if sender.is_some() {
        release_counter_slots(global_counter, 1);
    }
    sender
}

fn clear_pending_requests(
    pending: &Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    global_counter: &AtomicUsize,
) {
    let count = {
        let mut pending = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = pending.len();
        pending.clear();
        count
    };
    release_counter_slots(global_counter, count);
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn executable_in_path(bin: &str) -> Option<PathBuf> {
    crate::system::find_executable(bin)
}

fn go_env(name: &str) -> Option<String> {
    let output = Command::new("go").args(["env", name]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn go_path_list(name: &str) -> Vec<PathBuf> {
    go_env(name)
        .map(|value| std::env::split_paths(&std::ffi::OsString::from(value)).collect())
        .unwrap_or_default()
}

fn go_lsp_path() -> Option<PathBuf> {
    executable_in_path("gopls")
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .and_then(|home| path_if_executable(home.join("go").join("bin").join("gopls")))
        })
        .or_else(|| {
            go_path_list("GOBIN")
                .into_iter()
                .find_map(|p| path_if_executable(p.join("gopls")))
        })
        .or_else(|| {
            go_path_list("GOPATH")
                .into_iter()
                .find_map(|p| path_if_executable(p.join("bin").join("gopls")))
        })
}

fn path_if_executable(path: PathBuf) -> Option<PathBuf> {
    is_executable(&path).then_some(path)
}

fn path_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

// (bin, args) tuple for a language. Order matters only for display.
//
// Built-in table covers the common cases; users override or extend per-
// language via an env var `SIKEMUX_LSP_<UPPER_LANG>="<bin> <arg>..."`.
// Example: `SIKEMUX_LSP_RUST="rust-analyzer --log /tmp/ra.log"`. The
// override applies to any matching language, including ones we didn't
// ship with — e.g. `SIKEMUX_LSP_LUA="lua-language-server"`.
const BUILTIN_LSP: &[(&str, &str, &[&str])] = &[
    ("typescript", "typescript-language-server", &["--stdio"]),
    ("javascript", "typescript-language-server", &["--stdio"]),
    ("go", "gopls", &[]),
    ("rust", "rust-analyzer", &[]),
    ("python", "pyright-langserver", &["--stdio"]),
];

fn server_command(language: &str) -> Option<(String, Vec<String>)> {
    let env_key = format!("SIKEMUX_LSP_{}", language.to_uppercase());
    if let Ok(spec) = std::env::var(&env_key) {
        // Naive split — sufficient for "bin arg1 arg2". Quoted args aren't
        // supported; users that need them can wrap in a shell script.
        let mut parts = spec.split_whitespace();
        let bin = parts.next()?.to_string();
        let args = parts.map(|s| s.to_string()).collect();
        return Some((bin, args));
    }
    for (lang, bin, args) in BUILTIN_LSP {
        if *lang == language {
            let resolved_bin = if language == "go" && *bin == "gopls" {
                go_lsp_path()
                    .map(path_string)
                    .unwrap_or_else(|| (*bin).to_string())
            } else {
                (*bin).to_string()
            };
            return Some((
                resolved_bin,
                args.iter().map(|s| (*s).to_string()).collect(),
            ));
        }
    }
    None
}

fn path_to_uri(path: &str) -> String {
    Url::from_file_path(path)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("file://{}", path))
}

fn bounded_string(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

fn uri_to_bounded_path(uri: &str) -> Option<String> {
    let url = Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?.to_string_lossy().into_owned();
    (path.len() <= MAX_LSP_PATH_BYTES).then_some(path)
}

fn parse_lsp_range(value: &Value) -> Option<LspRange> {
    serde_json::from_value(value.clone()).ok()
}

fn parse_diagnostic_code(value: Option<&Value>) -> Option<String> {
    let code = match value? {
        Value::String(code) => code.clone(),
        Value::Number(code) => code.to_string(),
        _ => return None,
    };
    Some(bounded_string(&code, MAX_DIAGNOSTIC_CODE_BYTES))
}

fn parse_diagnostic(value: &Value) -> Option<LspDiagnostic> {
    let range = parse_lsp_range(value.get("range")?)?;
    let message = bounded_string(
        value.get("message")?.as_str()?,
        MAX_DIAGNOSTIC_MESSAGE_BYTES,
    );
    let severity = match value.get("severity").and_then(Value::as_u64) {
        Some(1) => Some(LspDiagnosticSeverity::Error),
        Some(2) => Some(LspDiagnosticSeverity::Warning),
        Some(3) => Some(LspDiagnosticSeverity::Information),
        Some(4) => Some(LspDiagnosticSeverity::Hint),
        _ => None,
    };
    let source = value
        .get("source")
        .and_then(Value::as_str)
        .map(|source| bounded_string(source, MAX_DIAGNOSTIC_SOURCE_BYTES));
    Some(LspDiagnostic {
        range,
        severity,
        code: parse_diagnostic_code(value.get("code")),
        source,
        message,
    })
}

fn parse_diagnostics_payload(
    project: &str,
    language: &str,
    params: &Value,
) -> Option<LspDiagnosticsPayload> {
    if project.len() > MAX_LSP_PATH_BYTES || language.len() > MAX_LSP_LANGUAGE_BYTES {
        return None;
    }
    let path = uri_to_bounded_path(params.get("uri")?.as_str()?)?;
    let values = params.get("diagnostics")?.as_array()?;
    let diagnostics = values
        .iter()
        .take(MAX_DIAGNOSTICS_PER_PUBLISH)
        .filter_map(parse_diagnostic)
        .collect();
    Some(LspDiagnosticsPayload {
        project: project.to_owned(),
        language: language.to_owned(),
        path,
        version: params.get("version").and_then(Value::as_i64),
        diagnostics,
    })
}

fn track_diagnostic_publish(
    tracked: &mut HashMap<String, Option<i64>>,
    path: &str,
    version: Option<i64>,
    has_diagnostics: bool,
) -> bool {
    if !has_diagnostics {
        tracked.remove(path);
        return true;
    }
    if !tracked.contains_key(path) && tracked.len() >= MAX_DIAGNOSTIC_FILES_PER_SERVER {
        return false;
    }
    tracked.insert(path.to_owned(), version);
    true
}

fn publish_diagnostics(server: &Server, params: &Value) {
    let Some(payload) = parse_diagnostics_payload(&server.project, &server.language, params) else {
        return;
    };
    let mut tracked = match server.diagnostic_paths.lock() {
        Ok(tracked) => tracked,
        Err(poisoned) => poisoned.into_inner(),
    };
    if server.shutdown.load(std::sync::atomic::Ordering::Acquire)
        || !track_diagnostic_publish(
            &mut tracked,
            &payload.path,
            payload.version,
            !payload.diagnostics.is_empty(),
        )
    {
        return;
    }

    // Keep the tracking lock through emission. Shutdown flips its atomic flag
    // before taking this lock, guaranteeing that a non-empty publish racing
    // teardown is always followed by the corresponding clear event.
    let _ = server.app.emit_to("main", LSP_DIAGNOSTICS_EVENT, payload);
}

fn clear_server_diagnostics(server: &Server) {
    let tracked = {
        let mut tracked = match server.diagnostic_paths.lock() {
            Ok(tracked) => tracked,
            Err(poisoned) => poisoned.into_inner(),
        };
        tracked.drain().collect::<Vec<_>>()
    };
    for (path, version) in tracked {
        let _ = server.app.emit_to(
            "main",
            LSP_DIAGNOSTICS_EVENT,
            LspDiagnosticsPayload {
                project: server.project.clone(),
                language: server.language.clone(),
                path,
                version,
                diagnostics: Vec::new(),
            },
        );
    }
}

fn handle_server_notification(server: &Server, method: &str, params: &Value) {
    if method == "textDocument/publishDiagnostics" {
        publish_diagnostics(server, params);
    }
}

fn content_hash(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn restore_last_change(last_change: &mut HashMap<String, u64>, path: &str, value: Option<u64>) {
    if let Some(value) = value {
        last_change.insert(path.to_owned(), value);
    } else {
        last_change.remove(path);
    }
}

/// Takes the document text by value: the buffer is handed straight to the
/// outbound frame instead of being copied into it, and an editor document can
/// be megabytes.
fn open_document(
    server: &ServerHandle,
    path: &str,
    content: String,
    language_id: &str,
) -> AppResult<()> {
    if server.shutdown.load(Ordering::Acquire) {
        return Err(AppError::Lsp("server shut down".into()));
    }
    let hash = content_hash(&content);
    let mut documents = server.open_docs.lock().map_err(lsp)?;

    if documents.contains_key(path) {
        let mut last_change = server.last_change.lock().map_err(lsp)?;
        let previous = *documents
            .get(path)
            .expect("document disappeared while its map is locked");
        let previous_hash = last_change.get(path).copied();
        if previous.refs == usize::MAX {
            return Err(AppError::Lsp(
                "open document reference count exhausted".into(),
            ));
        }
        if previous_hash == Some(hash) {
            documents
                .get_mut(path)
                .expect("document disappeared while its map is locked")
                .refs += 1;
            return Ok(());
        }

        let next_version = previous.version.saturating_add(1);
        *documents
            .get_mut(path)
            .expect("document disappeared while its map is locked") = OpenDoc {
            refs: previous.refs + 1,
            version: next_version,
        };
        last_change.insert(path.to_owned(), hash);
        let result = notify(
            server,
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": next_version },
                "contentChanges": [{ "text": Value::String(content) }]
            }),
        );
        if result.is_err() {
            documents.insert(path.to_owned(), previous);
            restore_last_change(&mut last_change, path, previous_hash);
        }
        return result;
    }

    reserve_open_document_slot(documents.len(), &OPEN_DOCUMENT_COUNT)?;
    let mut last_change = match server.last_change.lock() {
        Ok(last_change) => last_change,
        Err(error) => {
            release_counter_slots(&OPEN_DOCUMENT_COUNT, 1);
            return Err(lsp(error));
        }
    };
    let previous_hash = last_change.insert(path.to_owned(), hash);
    documents.insert(
        path.to_owned(),
        OpenDoc {
            refs: 1,
            version: 1,
        },
    );
    let result = notify(
        server,
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": path_to_uri(path),
                "languageId": language_id,
                "version": 1,
                "text": Value::String(content)
            }
        }),
    );
    if result.is_err() {
        documents.remove(path);
        restore_last_change(&mut last_change, path, previous_hash);
        release_counter_slots(&OPEN_DOCUMENT_COUNT, 1);
    }
    result
}

fn change_document(
    server: &ServerHandle,
    path: &str,
    content: String,
    requested_version: u32,
) -> AppResult<()> {
    if server.shutdown.load(Ordering::Acquire) {
        return Err(AppError::Lsp("server shut down".into()));
    }
    let hash = content_hash(&content);
    let mut documents = server.open_docs.lock().map_err(lsp)?;
    let previous = *documents
        .get(path)
        .ok_or_else(|| AppError::Lsp("document not open".into()))?;
    let mut last_change = server.last_change.lock().map_err(lsp)?;
    let previous_hash = last_change.get(path).copied();
    if previous_hash == Some(hash) {
        return Ok(());
    }
    let version = requested_version.max(previous.version.saturating_add(1));
    documents
        .get_mut(path)
        .expect("document disappeared while its map is locked")
        .version = version;
    last_change.insert(path.to_owned(), hash);
    let result = notify(
        server,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": path_to_uri(path), "version": version },
            "contentChanges": [{ "text": Value::String(content) }]
        }),
    );
    if result.is_err() {
        documents.insert(path.to_owned(), previous);
        restore_last_change(&mut last_change, path, previous_hash);
    }
    result
}

fn change_document_incremental(
    server: &ServerHandle,
    path: &str,
    changes: Vec<LspTextChange>,
    requested_version: u32,
) -> AppResult<()> {
    if server.shutdown.load(Ordering::Acquire) {
        return Err(AppError::Lsp("server shut down".into()));
    }
    let mut documents = server.open_docs.lock().map_err(lsp)?;
    let previous = *documents
        .get(path)
        .ok_or_else(|| AppError::Lsp("document not open".into()))?;
    let mut last_change = server.last_change.lock().map_err(lsp)?;
    let previous_hash = last_change.remove(path);
    let version = requested_version.max(previous.version.saturating_add(1));
    documents
        .get_mut(path)
        .expect("document disappeared while its map is locked")
        .version = version;
    let changes = changes
        .into_iter()
        .map(|change| {
            json!({
                "range": change.range,
                "rangeLength": change.range_length,
                // Moved, not copied: a paste arrives here as one edit.
                "text": Value::String(change.text),
            })
        })
        .collect::<Vec<_>>();
    let result = notify(
        server,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": path_to_uri(path), "version": version },
            "contentChanges": changes
        }),
    );
    if result.is_err() {
        documents.insert(path.to_owned(), previous);
        restore_last_change(&mut last_change, path, previous_hash);
    }
    result
}

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

fn send(server: &ServerHandle, msg: &Value) -> AppResult<()> {
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

fn read_message<R: BufRead>(reader: &mut R) -> AppResult<Option<Value>> {
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

fn next_id(server: &ServerHandle) -> AppResult<i64> {
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

fn request_with_timeout(
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

fn request(server: &ServerHandle, method: &str, params: Value) -> AppResult<Value> {
    request_with_timeout(server, method, params, Duration::from_secs(4))
}

fn notify(server: &ServerHandle, method: &str, params: Value) -> AppResult<()> {
    let n = json!({"jsonrpc": "2.0", "method": method, "params": params});
    send(server, &n)
}

fn response_for_server_request(method: &str, params: &Value) -> Value {
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

fn spawn_server(project: &str, language: &str, app: AppHandle) -> AppResult<ServerHandle> {
    let (bin, args) = server_command(language)
        .ok_or_else(|| AppError::Lsp(format!("no language server configured for `{language}`")))?;
    let child = Command::new(&bin)
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

const LSP_IDLE_GRACE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
struct AdmissionEntry {
    key: ServerKey,
    live: bool,
    idle: bool,
    last_used: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ServerStartAction {
    Existing,
    Admit { victims: Vec<ServerKey> },
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServerAdmissionPlan {
    stale: Vec<ServerKey>,
    action: ServerStartAction,
}

/// Plan admission without mutating the registry so the all-busy rejection is
/// atomic: we never kill a partial set of servers and then discover that too
/// few idle victims existed. Ties use the typed key for deterministic tests
/// and repeatable eviction behavior.
fn plan_server_admission(
    requested: &ServerKey,
    entries: impl IntoIterator<Item = AdmissionEntry>,
) -> ServerAdmissionPlan {
    let entries = entries.into_iter().collect::<Vec<_>>();
    let mut stale = entries
        .iter()
        .filter(|entry| !entry.live)
        .map(|entry| entry.key.clone())
        .collect::<Vec<_>>();
    stale.sort();

    if entries
        .iter()
        .any(|entry| entry.live && entry.key == *requested)
    {
        return ServerAdmissionPlan {
            stale,
            action: ServerStartAction::Existing,
        };
    }

    let live_count = entries.iter().filter(|entry| entry.live).count();
    if live_count < MAX_LSP_SERVERS {
        return ServerAdmissionPlan {
            stale,
            action: ServerStartAction::Admit {
                victims: Vec::new(),
            },
        };
    }

    let victims_needed = live_count - MAX_LSP_SERVERS + 1;
    let mut idle = entries
        .iter()
        .filter(|entry| entry.live && entry.idle && entry.key != *requested)
        .map(|entry| (entry.last_used, entry.key.clone()))
        .collect::<Vec<_>>();
    idle.sort();
    if idle.len() < victims_needed {
        return ServerAdmissionPlan {
            stale,
            action: ServerStartAction::Reject,
        };
    }

    ServerAdmissionPlan {
        stale,
        action: ServerStartAction::Admit {
            victims: idle
                .into_iter()
                .take(victims_needed)
                .map(|(_, key)| key)
                .collect(),
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreparedServerStart {
    Existing,
    Admit,
}

/// Remove stale entries and any complete idle-victim set while the registry
/// is locked. `start_lock` serializes callers across this preparation, process
/// spawn, and insertion, so registry cardinality can never transiently exceed
/// `MAX_LSP_SERVERS`.
fn prepare_server_start(requested: &ServerKey) -> AppResult<PreparedServerStart> {
    let (action, to_shutdown) = {
        let mut registry = registry().lock().map_err(lsp)?;
        let entries = registry
            .iter()
            .map(|(key, server)| AdmissionEntry {
                key: key.clone(),
                live: !server.shutdown.load(Ordering::Acquire),
                idle: server
                    .open_docs
                    .lock()
                    .map(|documents| documents.is_empty())
                    .unwrap_or(false),
                last_used: server.last_used.load(Ordering::Relaxed),
            })
            .collect::<Vec<_>>();
        let plan = plan_server_admission(requested, entries);
        let mut to_shutdown = Vec::new();
        for key in &plan.stale {
            if let Some(server) = registry.remove(key) {
                to_shutdown.push(server);
            }
        }
        if let ServerStartAction::Admit { victims } = &plan.action {
            for key in victims {
                if let Some(server) = registry.remove(key) {
                    to_shutdown.push(server);
                }
            }
        }
        if matches!(plan.action, ServerStartAction::Existing) {
            if let Some(server) = registry.get(requested) {
                // A fresh start cancels any idle-shutdown timer that was
                // scheduled before the frontend decided to reuse the server.
                server.idle_generation.fetch_add(1, Ordering::AcqRel);
            }
        }
        (plan.action, to_shutdown)
    };

    for server in to_shutdown {
        shutdown_server(server);
    }

    match action {
        ServerStartAction::Existing => Ok(PreparedServerStart::Existing),
        ServerStartAction::Admit { .. } => Ok(PreparedServerStart::Admit),
        ServerStartAction::Reject => {
            let _ = global_observability().increment_counter("lsp.server_limit_rejections", 1);
            Err(server_limit_error())
        }
    }
}

fn schedule_idle_shutdown(server_key: ServerKey, server: ServerHandle) {
    let generation = server
        .idle_generation
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
        .saturating_add(1);
    task::spawn(async move {
        tokio::time::sleep(LSP_IDLE_GRACE).await;
        if server.shutdown.load(std::sync::atomic::Ordering::Acquire)
            || server
                .idle_generation
                .load(std::sync::atomic::Ordering::Acquire)
                != generation
            || server
                .open_docs
                .lock()
                .map(|docs| !docs.is_empty())
                .unwrap_or(true)
        {
            return;
        }
        let _ = task::spawn_blocking(move || {
            let _start_guard = start_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if server.shutdown.load(Ordering::Acquire)
                || server.idle_generation.load(Ordering::Acquire) != generation
                || server
                    .open_docs
                    .lock()
                    .map(|docs| !docs.is_empty())
                    .unwrap_or(true)
            {
                return;
            }
            let victim = registry().lock().ok().and_then(|mut reg| {
                let current = reg.get(&server_key)?;
                if !Arc::ptr_eq(current, &server) {
                    return None;
                }
                reg.remove(&server_key)
            });
            if let Some(victim) = victim {
                shutdown_server(victim);
            }
        })
        .await;
    });
}

/// Kill and reap before teardown waits on stdin or document-state locks. An
/// LSP server that stops reading can leave a writer blocked in `write_all`
/// while it owns both document mutexes; terminating the reader side of the
/// pipe is what lets that writer unwind and release them.
fn kill_and_reap_child(child: &Mutex<Option<Child>>) {
    let child = child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// SIGKILL + reap a server. Shared by `lsp_stop` and admission eviction.
fn shutdown_server(server: ServerHandle) {
    server
        .shutdown
        .store(true, std::sync::atomic::Ordering::Release);

    // This must remain ahead of stdin/open_docs/last_change acquisition. See
    // `shutdown_kills_before_waiting_for_a_blocked_document_writer`.
    kill_and_reap_child(&server.child);
    server
        .stdin
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();

    clear_pending_requests(&server.pending, &PENDING_REQUEST_COUNT);
    let released_documents = {
        let mut documents = server
            .open_docs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = documents.len();
        documents.clear();
        count
    };
    release_counter_slots(&OPEN_DOCUMENT_COUNT, released_documents);
    server
        .last_change
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    clear_server_diagnostics(&server);
}

/// Return true only for a usable registry entry. Reader-owned teardown marks
/// failed/exited servers shut down; remove those entries so a subsequent start
/// can actually spawn a replacement.
fn live_server_exists(key: &ServerKey) -> AppResult<bool> {
    let stale = {
        let mut registry = registry().lock().map_err(lsp)?;
        match registry.get(key) {
            Some(server) if !server.shutdown.load(std::sync::atomic::Ordering::Relaxed) => {
                return Ok(true)
            }
            Some(_) => registry.remove(key),
            None => None,
        }
    };
    if let Some(server) = stale {
        shutdown_server(server);
    }
    Ok(false)
}

/// Application-teardown backstop for servers whose project stop never arrived.
pub fn drain_all() {
    let _start_guard = start_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let servers = registry()
        .lock()
        .map(|mut registry| {
            registry
                .drain()
                .map(|(_, server)| server)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for server in servers {
        shutdown_server(server);
    }
}

fn server_keys_for_project<T>(registry: &HashMap<ServerKey, T>, project: &str) -> Vec<ServerKey> {
    registry
        .keys()
        .filter(|key| key.project == project)
        .cloned()
        .collect()
}

fn install_output_message(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    if stdout.is_empty() {
        "unknown failure".into()
    } else {
        stdout
    }
}

fn install_gopls() -> AppResult<String> {
    let output = Command::new("go")
        .args(["install", "golang.org/x/tools/gopls@latest"])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::Lsp(
                    "Go toolchain not found on PATH. Install Go first: https://go.dev/doc/install"
                        .into(),
                )
            } else {
                AppError::Lsp(format!("run go install gopls: {e}"))
            }
        })?;
    if !output.status.success() {
        return Err(AppError::Lsp(format!(
            "go install golang.org/x/tools/gopls@latest failed: {}",
            install_output_message(&output.stdout, &output.stderr)
        )));
    }
    go_lsp_path().map(path_string).ok_or_else(|| {
        AppError::Lsp(
            "gopls installed, but Sikemux could not locate it in PATH, GOBIN, or GOPATH/bin".into(),
        )
    })
}

// ---- Tauri commands -----------------------------------------------------

#[tauri::command]
pub async fn lsp_install_server(language: String) -> AppResult<String> {
    match language.as_str() {
        "go" => task::spawn_blocking(install_gopls)
            .await
            .map_err(|e| AppError::Lsp(format!("join: {e}")))?,
        _ => Err(AppError::Lsp(format!(
            "no language-server installer configured for `{language}`"
        ))),
    }
}

#[tauri::command]
pub async fn lsp_start(app: AppHandle, project: String, language: String) -> AppResult<()> {
    let span = global_observability().begin_span("lsp.start", None, Metadata::new());
    let result = lsp_start_inner(app, project, language).await;
    span.finish(if result.is_ok() {
        SpanOutcome::Success
    } else {
        SpanOutcome::Error
    });
    result
}

async fn lsp_start_inner(app: AppHandle, project: String, language: String) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    let server_key = ServerKey::new(&project, &language);
    // The initialize handshake blocks up to 20 s on slow servers; off the
    // Tauri worker pool so unrelated IPC isn't starved. The start lock spans
    // admission, process initialization, and registry insertion, making the
    // cap a hard pre-spawn admission limit rather than an eventual backstop.
    task::spawn_blocking(move || -> AppResult<()> {
        let _guard = start_lock().lock().map_err(lsp)?;
        if live_server_exists(&server_key)? {
            let existing = registry().lock().ok().and_then(|registry| {
                registry
                    .get(&server_key)
                    .filter(|server| !server.shutdown.load(Ordering::Acquire))
                    .cloned()
            });
            if let Some(server) = existing {
                server.idle_generation.fetch_add(1, Ordering::AcqRel);
                return Ok(());
            }
        }
        if prepare_server_start(&server_key)? == PreparedServerStart::Existing {
            return Ok(());
        }

        let server = spawn_server(&project, &language, app)?;
        let insertion = {
            let mut registry = registry().lock().map_err(lsp)?;
            if registry.len() >= MAX_LSP_SERVERS {
                Err(server_limit_error())
            } else {
                registry.insert(server_key, server.clone());
                Ok(())
            }
        };
        if let Err(error) = insertion {
            shutdown_server(server);
            return Err(error);
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

/// Shut every server owned by this project down. Called from the close-
/// session path so a long-running rust-analyzer doesn't hang around with
/// 500 MB resident after the user moves on. Idempotent.
#[tauri::command]
pub async fn lsp_stop(project: String) -> AppResult<()> {
    let span = global_observability().begin_span("lsp.stop", None, Metadata::new());
    let result = lsp_stop_inner(project).await;
    span.finish(if result.is_ok() {
        SpanOutcome::Success
    } else {
        SpanOutcome::Error
    });
    result
}

async fn lsp_stop_inner(project: String) -> AppResult<()> {
    if project.len() > MAX_LSP_PATH_BYTES {
        return Err(AppError::Lsp(format!(
            "project path exceeds {MAX_LSP_PATH_BYTES} bytes"
        )));
    }
    // Serialize against start so a stop racing a 20-second initialize cannot
    // miss the process and let it appear in the registry after stop returns.
    task::spawn_blocking(move || -> AppResult<()> {
        let _guard = start_lock().lock().map_err(lsp)?;
        let to_kill = {
            let mut registry = registry().lock().map_err(lsp)?;
            server_keys_for_project(&registry, &project)
                .into_iter()
                .filter_map(|key| registry.remove(&key))
                .collect::<Vec<_>>()
        };
        for server in to_kill {
            shutdown_server(server);
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_open(
    project: String,
    language: String,
    path: String,
    content: String,
    language_id: Option<String>,
) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    if let Some(language_id) = language_id.as_deref() {
        validate_document_language_id(language_id)?;
    }
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    server
        .idle_generation
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    task::spawn_blocking(move || {
        let language_id = language_id.unwrap_or(language);
        open_document(&server, &path, content, &language_id)
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_change(
    project: String,
    language: String,
    path: String,
    content: String,
    version: u32,
) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    task::spawn_blocking(move || change_document(&server, &path, content, version))
        .await
        .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_change_incremental(
    project: String,
    language: String,
    path: String,
    changes: Vec<LspTextChange>,
    version: u32,
) -> AppResult<()> {
    if changes.is_empty() {
        return Ok(());
    }
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    task::spawn_blocking(move || change_document_incremental(&server, &path, changes, version))
        .await
        .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_save(
    project: String,
    language: String,
    path: String,
    content: Option<String>,
) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let Some(server) = server_for(&project, &language) else {
        return Ok(());
    };
    task::spawn_blocking(move || {
        let mut params = json!({ "textDocument": { "uri": path_to_uri(&path) } });
        if let Some(text) = content {
            params["text"] = json!(text);
        }
        notify(&server, "textDocument/didSave", params)
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_close(project: String, language: String, path: String) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let Some(server) = server_for(&project, &language) else {
        return Ok(());
    };
    let server_key = ServerKey::new(&project, &language);
    let idle_server = server.clone();
    let became_idle = task::spawn_blocking(move || -> AppResult<bool> {
        let mut documents = server.open_docs.lock().map_err(lsp)?;
        let Some(document) = documents.get(&path).copied() else {
            return Ok(false);
        };
        if document.refs > 1 {
            documents
                .get_mut(&path)
                .expect("document disappeared while its map is locked")
                .refs -= 1;
            return Ok(false);
        }
        let mut last_change = server.last_change.lock().map_err(lsp)?;
        let previous_hash = last_change.remove(&path);
        documents.remove(&path);
        let result = notify(
            &server,
            "textDocument/didClose",
            json!({ "textDocument": { "uri": path_to_uri(&path) } }),
        );
        if let Err(error) = result {
            documents.insert(path.clone(), document);
            restore_last_change(&mut last_change, &path, previous_hash);
            return Err(error);
        }
        release_counter_slots(&OPEN_DOCUMENT_COUNT, 1);
        Ok(documents.is_empty())
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))??;
    if became_idle {
        schedule_idle_shutdown(server_key, idle_server);
    }
    Ok(())
}

fn parse_location_like(v: &Value) -> Option<LspLocation> {
    if let Ok(loc) = serde_json::from_value::<LspLocation>(v.clone()) {
        return Some(loc);
    }
    let uri = v
        .get("targetUri")
        .or_else(|| v.get("uri"))?
        .as_str()?
        .to_string();
    let range_value = v
        .get("targetSelectionRange")
        .or_else(|| v.get("targetRange"))
        .or_else(|| v.get("range"))?;
    let range = serde_json::from_value::<LspRange>(range_value.clone()).ok()?;
    Some(LspLocation { uri, range })
}

fn parse_locations(result: &Value) -> Vec<LspLocation> {
    if result.is_null() {
        return vec![];
    }
    if let Some(arr) = result.as_array() {
        return arr.iter().filter_map(parse_location_like).collect();
    }
    parse_location_like(result).into_iter().collect()
}

fn parse_symbol_kind(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?).ok()
}

fn parse_document_symbol(
    value: &Value,
    depth: usize,
    remaining: &mut usize,
) -> Option<LspDocumentSymbol> {
    if depth >= MAX_DOCUMENT_SYMBOL_DEPTH || *remaining == 0 {
        return None;
    }

    let name = bounded_string(value.get("name")?.as_str()?, MAX_SYMBOL_NAME_BYTES);
    let kind = parse_symbol_kind(value.get("kind")?)?;
    let (detail, range, selection_range, child_values) =
        if let Some(location) = value.get("location") {
            // SymbolInformation[] is the legacy flat response. containerName is
            // the only useful detail; the location range is also its selection.
            let range = parse_lsp_range(location.get("range")?)?;
            let detail = value
                .get("containerName")
                .and_then(Value::as_str)
                .map(|detail| bounded_string(detail, MAX_SYMBOL_DETAIL_BYTES));
            (detail, range.clone(), range, None)
        } else {
            let range = parse_lsp_range(value.get("range")?)?;
            let selection_range = value
                .get("selectionRange")
                .and_then(parse_lsp_range)
                .unwrap_or_else(|| range.clone());
            let detail = value
                .get("detail")
                .and_then(Value::as_str)
                .map(|detail| bounded_string(detail, MAX_SYMBOL_DETAIL_BYTES));
            (
                detail,
                range,
                selection_range,
                value.get("children").and_then(Value::as_array),
            )
        };

    *remaining -= 1;
    let mut children = Vec::new();
    if let Some(child_values) = child_values {
        for child in child_values {
            if *remaining == 0 {
                break;
            }
            if let Some(child) = parse_document_symbol(child, depth + 1, remaining) {
                children.push(child);
            }
        }
    }
    Some(LspDocumentSymbol {
        name,
        detail,
        kind,
        range,
        selection_range,
        children,
    })
}

fn parse_document_symbols(result: &Value) -> Vec<LspDocumentSymbol> {
    let Some(values) = result.as_array() else {
        return Vec::new();
    };
    let mut remaining = MAX_DOCUMENT_SYMBOLS;
    let mut symbols = Vec::new();
    for value in values {
        if remaining == 0 {
            break;
        }
        if let Some(symbol) = parse_document_symbol(value, 0, &mut remaining) {
            symbols.push(symbol);
        }
    }
    symbols
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum LspKind {
    Definition,
    Declaration,
    TypeDefinition,
    Implementation,
    References,
}

#[tauri::command]
pub async fn lsp_locations(
    project: String,
    language: String,
    path: String,
    line: u32,
    character: u32,
    kind: LspKind,
) -> AppResult<Vec<LspLocation>> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    let (method, with_context) = match kind {
        LspKind::Definition => ("textDocument/definition", false),
        LspKind::Declaration => ("textDocument/declaration", false),
        LspKind::TypeDefinition => ("textDocument/typeDefinition", false),
        LspKind::Implementation => ("textDocument/implementation", false),
        LspKind::References => ("textDocument/references", true),
    };
    let mut params = json!({
        "textDocument": { "uri": path_to_uri(&path) },
        "position": { "line": line, "character": character }
    });
    if with_context {
        params["context"] = json!({ "includeDeclaration": false });
    }
    let result = task::spawn_blocking(move || request(&server, method, params))
        .await
        .map_err(|e| AppError::Lsp(format!("join: {e}")))??;
    Ok(parse_locations(&result))
}

#[tauri::command]
pub async fn lsp_document_symbols(
    project: String,
    language: String,
    path: String,
) -> AppResult<Vec<LspDocumentSymbol>> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    let params = json!({ "textDocument": { "uri": path_to_uri(&path) } });
    // Use the same bounded blocking request path as locations: shutdown drops
    // the response sender, and timeout removes the pending request in 4 s.
    let result =
        task::spawn_blocking(move || request(&server, "textDocument/documentSymbol", params))
            .await
            .map_err(|error| AppError::Lsp(format!("join: {error}")))??;
    Ok(parse_document_symbols(&result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, Cursor};

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

    fn admission_entry(
        project: &str,
        language: &str,
        live: bool,
        idle: bool,
        last_used: u64,
    ) -> AdmissionEntry {
        AdmissionEntry {
            key: ServerKey::new(project, language),
            live,
            idle,
            last_used,
        }
    }

    #[test]
    fn seventh_busy_server_is_rejected_before_admission() {
        let requested = ServerKey::new("/project/seven", "rust");
        let entries = (0..MAX_LSP_SERVERS)
            .map(|index| {
                admission_entry(
                    &format!("/project/{index}"),
                    "rust",
                    true,
                    false,
                    index as u64,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            plan_server_admission(&requested, entries),
            ServerAdmissionPlan {
                stale: Vec::new(),
                action: ServerStartAction::Reject,
            }
        );
        assert_eq!(
            server_limit_error().to_string(),
            format!(
                "lsp: language server limit reached ({MAX_LSP_SERVERS}); close a project before starting another"
            )
        );
    }

    #[test]
    fn admission_cleans_stale_entries_and_evicts_complete_lru_set() {
        let requested = ServerKey::new("/project/new", "rust");
        let stale = ServerKey::new("/project/stale", "go");
        let oldest_idle = ServerKey::new("/project/idle-a", "rust");
        let next_idle = ServerKey::new("/project/idle-b", "rust");
        let mut entries = vec![
            admission_entry(&stale.project, &stale.language, false, true, 0),
            admission_entry(&oldest_idle.project, &oldest_idle.language, true, true, 1),
            admission_entry(&next_idle.project, &next_idle.language, true, true, 2),
        ];
        entries.extend((0..MAX_LSP_SERVERS - 1).map(|index| {
            admission_entry(
                &format!("/project/busy-{index}"),
                "rust",
                true,
                false,
                10 + index as u64,
            )
        }));

        // Seven live servers require two complete idle evictions before the
        // requested server can be admitted under a hard cap of six.
        assert_eq!(
            plan_server_admission(&requested, entries),
            ServerAdmissionPlan {
                stale: vec![stale],
                action: ServerStartAction::Admit {
                    victims: vec![oldest_idle, next_idle],
                },
            }
        );
    }

    #[test]
    fn admission_never_selects_a_partial_eviction_set() {
        let requested = ServerKey::new("/project/new", "rust");
        let only_idle = ServerKey::new("/project/only-idle", "rust");
        let mut entries = vec![admission_entry(
            &only_idle.project,
            &only_idle.language,
            true,
            true,
            0,
        )];
        entries.extend((0..MAX_LSP_SERVERS).map(|index| {
            admission_entry(
                &format!("/project/busy-{index}"),
                "rust",
                true,
                false,
                10 + index as u64,
            )
        }));

        // Seven live entries need two victims to leave a slot for the new
        // server. One idle candidate is insufficient, so nothing is selected.
        assert_eq!(
            plan_server_admission(&requested, entries).action,
            ServerStartAction::Reject
        );
    }

    #[test]
    fn typed_server_keys_and_project_stop_selection_are_collision_safe() {
        // Both pairs produced `a::b::c` with the old delimiter-composed key.
        let nested_project = ServerKey::new("b::c", "a");
        let delimiter_language = ServerKey::new("c", "a::b");
        assert_ne!(nested_project, delimiter_language);

        let suffix_project = ServerKey::new("folder::c", "rust");
        let mut registry = HashMap::new();
        registry.insert(nested_project.clone(), 1);
        registry.insert(delimiter_language.clone(), 2);
        registry.insert(suffix_project, 3);
        let mut selected = server_keys_for_project(&registry, "c");
        selected.sort();

        assert_eq!(selected, vec![delimiter_language]);
        assert!(registry.contains_key(&nested_project));
    }

    #[test]
    fn optional_document_language_id_is_bounded_before_serialization() {
        validate_document_language_id(&"x".repeat(MAX_LSP_LANGUAGE_BYTES))
            .expect("boundary language id");
        assert_eq!(
            validate_document_language_id(&"x".repeat(MAX_LSP_LANGUAGE_BYTES + 1))
                .expect_err("oversized language id")
                .to_string(),
            format!("lsp: document language identifier exceeds {MAX_LSP_LANGUAGE_BYTES} bytes")
        );
        assert!(
            validate_document_language_id(&"é".repeat(MAX_LSP_LANGUAGE_BYTES / 2 + 1)).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_kills_before_waiting_for_a_blocked_document_writer() {
        use std::os::fd::AsRawFd;

        struct ChildCleanup(Arc<Mutex<Option<Child>>>);

        impl Drop for ChildCleanup {
            fn drop(&mut self) {
                kill_and_reap_child(&self.0);
            }
        }

        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn non-reading child");
        let stdin = child.stdin.take().expect("child stdin");

        // Fill the pipe without blocking, then restore blocking mode. The next
        // byte written is guaranteed to wait until the child closes its reader.
        let fd = stdin.as_raw_fd();
        // SAFETY: `fd` is a live ChildStdin descriptor owned by this test.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0, "read pipe flags");
        // SAFETY: the descriptor remains live and the flag combination keeps
        // its existing access mode while temporarily adding O_NONBLOCK.
        assert!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "set nonblocking pipe"
        );
        let chunk = [0_u8; 4_096];
        loop {
            // SAFETY: `chunk` remains valid for the full write call and `fd`
            // still belongs to `stdin` in this scope.
            let written = unsafe { libc::write(fd, chunk.as_ptr().cast(), chunk.len()) };
            if written > 0 {
                continue;
            }
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock,
                "pipe should become full"
            );
            break;
        }
        // SAFETY: `fd` is unchanged and live; restore the exact original flags.
        assert!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } >= 0,
            "restore blocking pipe"
        );

        let child = Arc::new(Mutex::new(Some(child)));
        let _cleanup = ChildCleanup(child.clone());
        let stdin = Arc::new(Mutex::new(Some(stdin)));
        let document_state = Arc::new(Mutex::new(()));
        let (writer_entered_tx, writer_entered_rx) = mpsc::channel();
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        let writer_stdin = stdin.clone();
        let writer_state = document_state.clone();
        let writer = thread::spawn(move || {
            let _documents = writer_state.lock().expect("document state");
            let mut stdin = writer_stdin.lock().expect("stdin state");
            writer_entered_tx.send(()).expect("writer entered");
            let result = stdin
                .as_mut()
                .expect("live stdin")
                .write_all(&[1_u8])
                .map_err(|error| error.kind());
            writer_done_tx.send(result).expect("writer result");
        });
        writer_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("writer acquired document and stdin locks");

        // The old shutdown order waited on `document_state` here forever.
        // Killing first closes the pipe reader, so the blocked writer unwinds.
        kill_and_reap_child(&child);
        assert_eq!(
            writer_done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("blocked writer released"),
            Err(std::io::ErrorKind::BrokenPipe)
        );
        writer.join().expect("writer thread");
        assert!(document_state.try_lock().is_ok());
        stdin.lock().expect("stdin state").take();
    }

    #[test]
    fn open_document_limits_reject_without_leaking_global_slots() {
        let counter = AtomicUsize::new(0);
        let per_server_error = reserve_open_document_slot(MAX_OPEN_DOCUMENTS_PER_SERVER, &counter)
            .expect_err("per-server cap");
        assert_eq!(
            per_server_error.to_string(),
            format!(
                "lsp: open document limit reached for language server ({MAX_OPEN_DOCUMENTS_PER_SERVER})"
            )
        );
        assert_eq!(counter.load(Ordering::Acquire), 0);

        let counter = AtomicUsize::new(MAX_OPEN_DOCUMENTS_GLOBAL);
        let global_error =
            reserve_open_document_slot(0, &counter).expect_err("global document cap");
        assert_eq!(
            global_error.to_string(),
            format!("lsp: global open document limit reached ({MAX_OPEN_DOCUMENTS_GLOBAL})")
        );
        assert_eq!(counter.load(Ordering::Acquire), MAX_OPEN_DOCUMENTS_GLOBAL);

        let counter = AtomicUsize::new(0);
        reserve_open_document_slot(0, &counter).expect("available slot");
        assert_eq!(counter.load(Ordering::Acquire), 1);
        release_counter_slots(&counter, 1);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn pending_request_limits_and_cleanup_are_exact() {
        let pending = Mutex::new(HashMap::new());
        let counter = AtomicUsize::new(0);
        let mut receivers = Vec::new();
        for id in 0..MAX_PENDING_REQUESTS_PER_SERVER as i64 {
            let (sender, receiver) = mpsc::channel();
            insert_pending_request(&pending, &counter, id, sender).expect("pending slot");
            receivers.push(receiver);
        }
        let (sender, _receiver) = mpsc::channel();
        let error = insert_pending_request(
            &pending,
            &counter,
            MAX_PENDING_REQUESTS_PER_SERVER as i64,
            sender,
        )
        .expect_err("per-server pending cap");
        assert_eq!(
            error.to_string(),
            format!(
                "lsp: pending request limit reached for language server ({MAX_PENDING_REQUESTS_PER_SERVER})"
            )
        );
        assert_eq!(
            pending.lock().expect("pending map").len(),
            MAX_PENDING_REQUESTS_PER_SERVER
        );
        assert_eq!(
            counter.load(Ordering::Acquire),
            MAX_PENDING_REQUESTS_PER_SERVER
        );

        assert!(take_pending_request(&pending, &counter, 0).is_some());
        assert_eq!(
            counter.load(Ordering::Acquire),
            MAX_PENDING_REQUESTS_PER_SERVER - 1
        );
        clear_pending_requests(&pending, &counter);
        assert!(pending.lock().expect("pending map").is_empty());
        assert_eq!(counter.load(Ordering::Acquire), 0);
        drop(receivers);

        let pending = Mutex::new(HashMap::new());
        let counter = AtomicUsize::new(MAX_PENDING_REQUESTS_GLOBAL);
        let (sender, _receiver) = mpsc::channel();
        let error =
            insert_pending_request(&pending, &counter, 1, sender).expect_err("global pending cap");
        assert_eq!(
            error.to_string(),
            format!("lsp: global pending request limit reached ({MAX_PENDING_REQUESTS_GLOBAL})")
        );
        assert!(pending.lock().expect("pending map").is_empty());
        assert_eq!(counter.load(Ordering::Acquire), MAX_PENDING_REQUESTS_GLOBAL);

        let pending = Mutex::new(HashMap::new());
        let counter = AtomicUsize::new(0);
        let (sender, _receiver) = mpsc::channel();
        insert_pending_request(&pending, &counter, 7, sender).expect("first request id");
        let (sender, _receiver) = mpsc::channel();
        assert_eq!(
            insert_pending_request(&pending, &counter, 7, sender)
                .expect_err("duplicate request id")
                .to_string(),
            "lsp: duplicate language-server request id"
        );
        assert_eq!(pending.lock().expect("pending map").len(), 1);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        clear_pending_requests(&pending, &counter);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

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

    fn test_range() -> Value {
        json!({
            "start": { "line": 1, "character": 2 },
            "end": { "line": 3, "character": 4 }
        })
    }

    #[test]
    fn diagnostics_payload_is_typed_bounded_and_empty_publish_clears() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("main.rs");
        let uri = Url::from_file_path(&path).unwrap().to_string();
        let mut diagnostics = vec![json!({
            "range": test_range(),
            "severity": 1,
            "code": 42,
            "source": "s".repeat(MAX_DIAGNOSTIC_SOURCE_BYTES + 20),
            "message": "é".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES)
        })];
        diagnostics.extend((1..MAX_DIAGNOSTICS_PER_PUBLISH + 10).map(|index| {
            json!({
                "range": test_range(),
                "severity": 2,
                "message": format!("warning {index}")
            })
        }));

        let payload = parse_diagnostics_payload(
            temp.path().to_string_lossy().as_ref(),
            "rust",
            &json!({ "uri": uri, "version": 7, "diagnostics": diagnostics }),
        )
        .unwrap();
        assert_eq!(payload.path, path.to_string_lossy());
        assert_eq!(payload.version, Some(7));
        assert_eq!(payload.diagnostics.len(), MAX_DIAGNOSTICS_PER_PUBLISH);
        assert_eq!(
            payload.diagnostics[0].severity,
            Some(LspDiagnosticSeverity::Error)
        );
        assert_eq!(payload.diagnostics[0].code.as_deref(), Some("42"));
        assert!(payload.diagnostics[0].message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
        assert!(payload.diagnostics[0]
            .message
            .is_char_boundary(payload.diagnostics[0].message.len()));
        assert!(payload.diagnostics[0]
            .source
            .as_ref()
            .is_some_and(|source| source.len() == MAX_DIAGNOSTIC_SOURCE_BYTES));

        let cleared = parse_diagnostics_payload(
            temp.path().to_string_lossy().as_ref(),
            "rust",
            &json!({ "uri": Url::from_file_path(&path).unwrap(), "diagnostics": [] }),
        )
        .unwrap();
        assert!(cleared.diagnostics.is_empty());
        assert!(parse_diagnostics_payload(
            temp.path().to_string_lossy().as_ref(),
            "rust",
            &json!({ "uri": "https://example.com/main.rs", "diagnostics": [] })
        )
        .is_none());
    }

    #[test]
    fn diagnostic_tracking_replaces_clears_and_caps_paths() {
        let mut tracked = HashMap::new();
        assert!(track_diagnostic_publish(
            &mut tracked,
            "main.rs",
            Some(1),
            true
        ));
        assert!(track_diagnostic_publish(
            &mut tracked,
            "main.rs",
            Some(2),
            true
        ));
        assert_eq!(tracked.get("main.rs"), Some(&Some(2)));
        assert!(track_diagnostic_publish(
            &mut tracked,
            "main.rs",
            Some(2),
            false
        ));
        assert!(!tracked.contains_key("main.rs"));

        for index in 0..MAX_DIAGNOSTIC_FILES_PER_SERVER {
            assert!(track_diagnostic_publish(
                &mut tracked,
                &format!("file-{index}"),
                None,
                true
            ));
        }
        assert!(!track_diagnostic_publish(
            &mut tracked,
            "one-too-many",
            None,
            true
        ));
        assert_eq!(tracked.len(), MAX_DIAGNOSTIC_FILES_PER_SERVER);
    }

    fn hierarchical_symbol(name: &str, children: Vec<Value>) -> Value {
        json!({
            "name": name,
            "detail": "d".repeat(MAX_SYMBOL_DETAIL_BYTES + 20),
            "kind": 12,
            "range": test_range(),
            "selectionRange": test_range(),
            "children": children
        })
    }

    fn symbol_depth(symbol: &LspDocumentSymbol) -> usize {
        1 + symbol.children.iter().map(symbol_depth).max().unwrap_or(0)
    }

    #[test]
    fn document_symbols_support_both_shapes_and_bound_depth_strings_and_count() {
        let mut nested = hierarchical_symbol("leaf", Vec::new());
        for _ in 0..MAX_DOCUMENT_SYMBOL_DEPTH + 5 {
            nested = hierarchical_symbol(&"é".repeat(MAX_SYMBOL_NAME_BYTES), vec![nested]);
        }
        let symbols = parse_document_symbols(&json!([nested]));
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbol_depth(&symbols[0]), MAX_DOCUMENT_SYMBOL_DEPTH);
        assert!(symbols[0].name.len() <= MAX_SYMBOL_NAME_BYTES);
        assert!(symbols[0]
            .detail
            .as_ref()
            .is_some_and(|detail| detail.len() == MAX_SYMBOL_DETAIL_BYTES));

        let flat = parse_document_symbols(&json!([{
            "name": "legacy",
            "kind": 5,
            "containerName": "Container",
            "location": {
                "uri": "file:///tmp/main.rs",
                "range": test_range()
            }
        }]));
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].detail.as_deref(), Some("Container"));
        assert_eq!(flat[0].range, flat[0].selection_range);
        assert!(flat[0].children.is_empty());

        let many = (0..MAX_DOCUMENT_SYMBOLS + 10)
            .map(|index| hierarchical_symbol(&format!("symbol-{index}"), Vec::new()))
            .collect::<Vec<_>>();
        assert_eq!(
            parse_document_symbols(&Value::Array(many)).len(),
            MAX_DOCUMENT_SYMBOLS
        );
    }
}
