mod claude;
mod codex;
pub(crate) mod context;
mod grok;
mod hermes;
mod omp;
pub(super) mod opencode;
pub(super) mod pi;
pub(crate) mod recent;
pub(crate) mod rename;

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tauri::async_runtime::spawn_blocking;
use tokio::process::Command;

use super::executable::{apply_login_environment, apply_process_config, expand_user_path};
use super::{allowed_agent_path, AgentKind, AgentSession};
use claude::claude_sessions;
use codex::codex_sessions;
use grok::grok_sessions;
use hermes::hermes_sessions;
use omp::omp_sessions;
use opencode::opencode_sessions;
use pi::pi_sessions;

fn read_agent_sessions(
    agent: AgentKind,
    cwd: &str,
    config_path: Option<&str>,
) -> Vec<AgentSession> {
    match agent {
        AgentKind::Claude => claude_sessions(cwd, config_path),
        AgentKind::Codex => codex_sessions(cwd, config_path),
        AgentKind::Hermes => hermes_sessions(cwd),
        AgentKind::Pi => pi_sessions(cwd),
        AgentKind::Opencode => opencode_sessions(cwd),
        AgentKind::Omp => omp_sessions(cwd),
        AgentKind::Grok => grok_sessions(cwd),
    }
}

const LIVE_SESSION_TIMEOUT: Duration = Duration::from_secs(6);
const LIVE_SESSION_OUTPUT_LIMIT: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAgentSession {
    pub session_id: String,
    pub status: String,
}

fn parse_live_sessions(text: &str) -> Vec<LiveAgentSession> {
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let session_id = row.get("sessionId")?.as_str()?.to_string();
            let status = row.get("status")?.as_str()?.to_string();
            Some(LiveAgentSession { session_id, status })
        })
        .collect()
}

/// What Claude Code says about the sessions it is running right now. A session
/// reads `idle` only when nothing of its own is left going: a turn in flight,
/// a prompt waiting on the user, or a background shell each report something
/// else, so `idle` is the one status that is safe to end.
#[tauri::command]
pub async fn live_agent_sessions(
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<Vec<LiveAgentSession>, String> {
    let executable = executable_path
        .as_deref()
        .map(expand_user_path)
        .or_else(|| {
            crate::system::find_executable_matching("claude", |candidate| {
                allowed_agent_path("claude", candidate)
            })
        })
        .ok_or_else(|| "claude is not available".to_string())?;
    let mut command = Command::from(sikemux_process::user_environment::command(executable));
    apply_login_environment(&mut command);
    command
        .args(["agents", "--json"])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    apply_process_config(&mut command, "claude", config_path.as_deref());
    let output = tokio::time::timeout(LIVE_SESSION_TIMEOUT, command.output())
        .await
        .map_err(|_| "claude session lookup timed out".to_string())?
        .map_err(|_| "Could not start claude session lookup".to_string())?;
    if !output.status.success() {
        return Err("claude session lookup exited unsuccessfully".to_string());
    }
    if output.stdout.len() > LIVE_SESSION_OUTPUT_LIMIT {
        return Err("claude listed too many sessions".to_string());
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "claude session list was not valid UTF-8".to_string())?;
    Ok(parse_live_sessions(&text))
}

/// Existing on-disk conversations for an agent. The scan reads directories and
/// transcripts, so it runs on a blocking thread rather than the IPC thread.
#[tauri::command]
pub async fn agent_sessions(
    agent: AgentKind,
    cwd: String,
    config_path: Option<String>,
) -> Vec<AgentSession> {
    spawn_blocking(move || read_agent_sessions(agent, &cwd, config_path.as_deref()))
        .await
        .unwrap_or_default()
}

fn mtime_of(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TitleCacheStamp {
    modified_ns: u128,
    len: u64,
}

impl TitleCacheStamp {
    fn unix_secs(self) -> u64 {
        (self.modified_ns / 1_000_000_000) as u64
    }

    fn unix_millis(self) -> u64 {
        (self.modified_ns / 1_000_000) as u64
    }
}

/// Stats each transcript once and hands back the newest first, which is all the
/// ordering, the mtime and the cache keys need.
fn stamped_transcripts(paths: impl Iterator<Item = PathBuf>) -> Vec<(PathBuf, TitleCacheStamp)> {
    let mut stamped: Vec<(PathBuf, TitleCacheStamp)> = paths
        .map(|path| {
            let stamp = title_cache_stamp(&path);
            (path, stamp)
        })
        .collect();
    stamped.sort_unstable_by_key(|(_, stamp)| std::cmp::Reverse(stamp.modified_ns));
    stamped
}

fn title_cache_stamp(path: &Path) -> TitleCacheStamp {
    let metadata = fs::metadata(path).ok();
    TitleCacheStamp {
        modified_ns: metadata
            .as_ref()
            .and_then(|value| value.modified().ok())
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_nanos())
            .unwrap_or(0),
        len: metadata.map(|value| value.len()).unwrap_or(0),
    }
}

/// The longest title a session is listed under.
const MAX_TITLE_CHARS: usize = 72;

fn condense(text: &str) -> Option<String> {
    let c = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if c.is_empty() || c.starts_with('<') {
        return None;
    }
    Some(c.chars().take(MAX_TITLE_CHARS).collect())
}

fn text_from_content(content: &Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    content.as_array()?.iter().find_map(|b| {
        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
            b.get("text").and_then(|t| t.as_str()).map(String::from)
        } else {
            None
        }
    })
}

/// Cached title per transcript: `path -> (high-resolution stamp, title)`.
type TitleCache = HashMap<PathBuf, (TitleCacheStamp, Option<String>, u64)>;
const MAX_TITLE_CACHE_ENTRIES: usize = 2_048;
const MAX_AGENT_TRANSCRIPT_PATHS: usize = 20_000;
const MAX_AGENT_TRANSCRIPTS_INSPECTED: usize = 5_000;

fn next_title_cache_access() -> u64 {
    static ACCESS: AtomicU64 = AtomicU64::new(1);
    ACCESS.fetch_add(1, Ordering::Relaxed)
}

/// Per-file title cache keyed by a high-resolution file stamp. Titles are derived from transcript
/// content that only ever grows, so an unchanged nanosecond timestamp and file
/// length means an unchanged title. Length prevents same-tick appends from
/// preserving a cached title-less result on coarse filesystems.
/// This turns the palette's cold scan of every session into a one-time cost:
/// reopening it re-reads only the sessions that have actually changed.
fn title_cache() -> &'static Mutex<TitleCache> {
    static CACHE: OnceLock<Mutex<TitleCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return the cached title for `path` when its stamp matches, otherwise run
/// `compute`, store the result (including `None`, so title-less files are not
/// re-scanned), and return it.
fn cached_title<F>(path: &Path, stamp: TitleCacheStamp, compute: F) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    if let Ok(mut cache) = title_cache().lock() {
        if let Some((cached_stamp, title, access)) = cache.get_mut(path) {
            if *cached_stamp == stamp {
                *access = next_title_cache_access();
                return title.clone();
            }
        }
    }
    let title = compute();
    if let Ok(mut cache) = title_cache().lock() {
        if cache.len() >= MAX_TITLE_CACHE_ENTRIES && !cache.contains_key(path) {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (_, _, access))| *access)
                .map(|(path, _)| path.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            path.to_path_buf(),
            (stamp, title.clone(), next_title_cache_access()),
        );
    }
    title
}

/// Read up to `n` bytes from the start of `file` as lossy UTF-8.
fn read_prefix(file: &mut fs::File, n: u64) -> Option<String> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut buf = Vec::new();
    file.by_ref().take(n).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Read from `start` to the end of `file` as lossy UTF-8.
fn read_suffix(file: &mut fs::File, start: u64) -> Option<String> {
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 6 || out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
            break;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}
