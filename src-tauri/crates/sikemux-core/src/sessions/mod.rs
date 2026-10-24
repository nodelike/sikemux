//! The sessions each agent CLI saves on disk: listing them, their titles,
//! paging them for the Recent list, searching them, renaming, moving and deleting them.

pub mod carry;
mod claude;
mod codex;
pub mod context;
pub mod delete;
mod grok;
mod hermes;
mod muse;
mod omp;
mod opencode;
mod pi;
pub mod recent;
pub mod relocate;
pub mod rename;
pub mod roots;
pub mod search;
pub mod transcript;

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms_of;
use sikemux_process::sync::Locked;
use tokio::process::Command;

use crate::agent_catalog::AgentKind;
use claude::claude_sessions;
use codex::codex_sessions;
use grok::grok_sessions;
use hermes::hermes_sessions;
use muse::muse_sessions;
use omp::omp_sessions;
use opencode::opencode_sessions;
use pi::pi_sessions;
use sikemux_pty::user_shell::login_shell_environment;

pub use claude::claude_project_dir;
pub use opencode::opencode_data_dirs;
pub use pi::pi_session_dir;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub title: String,
    pub mtime: u64,
}

/// Whether `id` has the shape `agent`'s own sessions take, so it can never pass for a flag.
/// Pi and OMP also name a session by its transcript's path.
pub fn is_session_id(agent: Option<AgentKind>, id: &str) -> bool {
    let mut chars = id.chars();
    let token = id.len() <= 200
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | ':' | '-'));
    let transcript = matches!(agent, Some(AgentKind::Pi | AgentKind::Omp))
        && id.starts_with('/')
        && id.ends_with(".jsonl")
        && id.len() <= 4_096
        && !id.chars().any(|char| char < ' ');
    token || transcript
}

/// One agent's saved sessions that ran in `cwd`, newest first.
pub fn agent_sessions(agent: AgentKind, cwd: &str, config_path: Option<&str>) -> Vec<AgentSession> {
    match agent {
        AgentKind::Claude => claude_sessions(cwd, config_path),
        AgentKind::Codex => codex_sessions(cwd, config_path),
        AgentKind::Hermes => hermes_sessions(cwd),
        AgentKind::Pi => pi_sessions(cwd),
        AgentKind::Opencode => opencode_sessions(cwd),
        AgentKind::Omp => omp_sessions(cwd),
        AgentKind::Grok => grok_sessions(cwd),
        AgentKind::Muse => muse_sessions(cwd),
    }
}

/// What holds the session a file written under `agent`'s session folders belongs
/// to: Grok's folder for each session, else the transcript itself. None for agents
/// that keep their sessions in a database.
pub fn session_place(agent: AgentKind, path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    match agent {
        AgentKind::Claude | AgentKind::Codex | AgentKind::Pi | AgentKind::Omp => {
            name.ends_with(".jsonl").then(|| path.to_path_buf())
        }
        AgentKind::Grok => path
            .parent()
            .filter(|_| grok::GROK_SESSION_FILES.contains(&name))
            .map(Path::to_path_buf),
        AgentKind::Hermes | AgentKind::Opencode | AgentKind::Muse => None,
    }
}

/// The session `place` holds, when it ran in `cwd`. Claude files each project's
/// transcripts in a folder of their own, so it trusts the caller on the folder.
pub fn session_at(
    agent: AgentKind,
    place: &Path,
    cwd: &str,
    config_path: Option<&str>,
) -> Option<AgentSession> {
    let found = match agent {
        AgentKind::Claude => return claude::claude_session(place, title_cache_stamp(place)),
        AgentKind::Codex => return codex::codex_session_at(place, cwd, config_path),
        AgentKind::Grok => return grok::grok_session(place, cwd),
        AgentKind::Pi => pi::pi_session(place),
        AgentKind::Omp => omp::omp_session(place),
        AgentKind::Hermes | AgentKind::Opencode | AgentKind::Muse => None,
    };
    found
        .filter(|found| found.project == cwd)
        .map(|found| found.session)
}

/// Points a CLI started on one of the person's accounts at that account's folder.
pub fn apply_process_config(command: &mut Command, agent: &str, config_path: Option<&str>) {
    let (Some(variable), Some(path)) = (crate::launch::account_variable(agent), config_path) else {
        return;
    };
    command.env(variable, sikemux_pty::launch::account_dir(path));
}

pub fn apply_login_environment(command: &mut Command) {
    for (key, value) in login_shell_environment() {
        command.env(key, value);
    }
}

fn mtime_of(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map_or(0, |modified| epoch_ms_of(modified) / 1000)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TitleCacheStamp {
    modified_ns: u128,
    len: u64,
}

impl TitleCacheStamp {
    /// A stamp for a session kept in a database, which changes when its last write does.
    fn at_secs(secs: u64) -> Self {
        TitleCacheStamp {
            modified_ns: u128::from(secs) * 1_000_000_000,
            len: 0,
        }
    }

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

/// Cached answer per transcript: `path -> (high-resolution stamp, answer, last use)`.
type TitleCache = HashMap<PathBuf, (TitleCacheStamp, Option<String>, u64)>;
const MAX_TITLE_CACHE_ENTRIES: usize = 2_048;
const MAX_AGENT_TRANSCRIPT_PATHS: usize = 20_000;
const MAX_AGENT_TRANSCRIPTS_INSPECTED: usize = 5_000;

fn next_title_cache_access() -> u64 {
    static ACCESS: AtomicU64 = AtomicU64::new(1);
    ACCESS.fetch_add(1, Ordering::Relaxed)
}

/// What was read from each transcript, keyed by a high-resolution file stamp. Transcripts
/// only ever grow, so an unchanged nanosecond timestamp and file length means an unchanged
/// answer. Length prevents same-tick appends from preserving a cached empty answer on
/// coarse filesystems. This turns a cold scan of every session into a one-time cost:
/// later scans re-read only the sessions that have actually changed.
struct FileCache(OnceLock<Mutex<TitleCache>>);

impl FileCache {
    const fn new() -> Self {
        FileCache(OnceLock::new())
    }

    /// The cached answer for `path` when its stamp matches, otherwise `compute`'s, stored
    /// even when it is `None` so a file without one is not read again.
    fn get_or<F>(&self, path: &Path, stamp: TitleCacheStamp, compute: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>,
    {
        let cache = self.0.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some((cached_stamp, value, access)) = cache.locked().get_mut(path) {
            if *cached_stamp == stamp {
                *access = next_title_cache_access();
                return value.clone();
            }
        }
        let value = compute();
        let mut cache = cache.locked();
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
            (stamp, value.clone(), next_title_cache_access()),
        );
        value
    }
}

fn cached_title<F>(path: &Path, stamp: TitleCacheStamp, compute: F) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    static TITLES: FileCache = FileCache::new();
    TITLES.get_or(path, stamp, compute)
}

/// A message's text blocks, or its content when that is plain text, leaving
/// out text the CLI wrote itself in tags.
fn push_texts(content: &Value, prose: &mut search::Prose) {
    let mut push = |text: &str| {
        if !text.trim_start().starts_with('<') {
            prose.push(text);
        }
    };
    match content {
        Value::String(text) => push(text),
        Value::Array(blocks) => {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        push(text);
                    }
                }
            }
        }
        _ => {}
    }
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
