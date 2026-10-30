//! The sessions each agent CLI saves on disk: listing them, their titles,
//! paging them for the Recent list, searching them, renaming, moving and deleting them.

pub mod carry;
pub(crate) mod claude;
pub(crate) mod codex;
pub mod context;
pub mod delete;
pub(crate) mod grok;
pub(crate) mod hermes;
mod history;
pub mod index;
pub(crate) mod muse;
pub(crate) mod omp;
pub(crate) mod opencode;
pub(crate) mod pi;
pub mod recent;
pub mod relocate;
pub mod rename;
pub mod roots;
pub mod search;
pub mod transcript;

use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms_of;
use tokio::process::Command;

use crate::agent_catalog::AgentKind;
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
/// Agents that resume by path also name a session by its transcript's path.
pub fn is_session_id(agent: Option<AgentKind>, id: &str) -> bool {
    let mut chars = id.chars();
    let token = id.len() <= 200
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | ':' | '-'));
    let transcript = agent.is_some_and(|agent| agent.entry().resumes_by_path)
        && id.starts_with('/')
        && id.ends_with(".jsonl")
        && id.len() <= 4_096
        && !id.chars().any(|char| char < ' ');
    token || transcript
}

/// One agent's saved sessions that ran in `cwd`, newest first.
pub fn agent_sessions(agent: AgentKind, cwd: &str, config_path: Option<&str>) -> Vec<AgentSession> {
    agent.ops().sessions(cwd, config_path)
}

/// A saved session as the updates its agent sends when it loads it, for agents whose
/// chats show it before the agent starts.
pub fn history(agent: AgentKind, session_id: &str, cwd: &Path) -> Result<Vec<Value>, String> {
    agent.ops().history(session_id, cwd)
}

/// The provider and model a saved session last ran on, as its agent names them, for
/// agents that keep a provider beside each reply.
pub fn last_model(agent: AgentKind, session_id: &str) -> Option<(String, String)> {
    agent.ops().last_model(session_id)
}

/// What holds the session a file written under `agent`'s session folders belongs
/// to. None for agents that keep their sessions in a database.
pub fn session_place(agent: AgentKind, path: &Path) -> Option<PathBuf> {
    path.file_name()?.to_str()?;
    agent.ops().session_place(path)
}

/// The session `place` holds, when it ran in `cwd`.
pub fn session_at(
    agent: AgentKind,
    place: &Path,
    cwd: &str,
    config_path: Option<&str>,
) -> Option<AgentSession> {
    agent.ops().session_at(place, cwd, config_path)
}

/// The session `found` holds, when it ran in `cwd`.
fn in_project(found: Option<recent::Found>, cwd: &str) -> Option<AgentSession> {
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
pub(crate) struct FileStamp {
    modified_ns: u128,
    len: u64,
}

impl FileStamp {
    /// A stamp for a session kept in a database, which changes when its last write does.
    fn at_secs(secs: u64) -> Self {
        FileStamp {
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
/// ordering, the mtime and the index need.
fn stamped_transcripts(paths: impl Iterator<Item = PathBuf>) -> Vec<(PathBuf, FileStamp)> {
    let mut stamped: Vec<(PathBuf, FileStamp)> = paths
        .map(|path| {
            let stamp = file_stamp(&path);
            (path, stamp)
        })
        .collect();
    stamped.sort_unstable_by_key(|(_, stamp)| std::cmp::Reverse(stamp.modified_ns));
    stamped
}

pub(crate) fn file_stamp(path: &Path) -> FileStamp {
    let metadata = fs::metadata(path).ok();
    FileStamp {
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

const MAX_AGENT_TRANSCRIPT_PATHS: usize = 20_000;
const MAX_AGENT_TRANSCRIPTS_INSPECTED: usize = 5_000;

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

fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
}

/// Codex compresses a session's rollout with zstd once the session closes.
const COMPRESSED_JSONL: &str = ".jsonl.zst";

pub(crate) fn is_compressed(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|path| path.ends_with(COMPRESSED_JSONL))
}

/// A transcript's lines, decompressed when it is compressed.
fn transcript_lines(path: &Path) -> Option<Box<dyn BufRead>> {
    let file = fs::File::open(path).ok()?;
    if is_compressed(path) {
        let decoder = zstd::stream::read::Decoder::new(file).ok()?;
        return Some(Box::new(BufReader::new(decoder)));
    }
    Some(Box::new(BufReader::new(file)))
}

/// Up to `max` bytes of a compressed transcript, decoded: from `from`, or its last ones. Answers
/// them with where they start and how long the whole transcript is decoded. A compressed file
/// can't seek, so it is decoded from its start.
pub(crate) fn decoded_window(
    path: &Path,
    from: Option<u64>,
    max: u64,
) -> Option<(Vec<u8>, u64, u64)> {
    let mut reader = transcript_lines(path)?;
    let mut window = Vec::new();
    let mut start = from.unwrap_or(0);
    let mut decoded = 0;
    loop {
        let chunk = reader.fill_buf().ok()?;
        if chunk.is_empty() {
            break;
        }
        let read = chunk.len() as u64;
        let kept = match from {
            Some(from) => {
                let first = from.saturating_sub(decoded).min(read) as usize;
                let room = (max as usize).saturating_sub(window.len());
                &chunk[first..chunk.len().min(first + room)]
            }
            None => chunk,
        };
        window.extend_from_slice(kept);
        if from.is_none() && window.len() as u64 > 2 * max {
            let cut = window.len() - max as usize;
            window.drain(..cut);
            start = decoded + read - max;
        }
        decoded += read;
        reader.consume(read as usize);
    }
    if from.is_none() && window.len() as u64 > max {
        let cut = window.len() - max as usize;
        window.drain(..cut);
        start = decoded - max;
    }
    Some((window, start.min(decoded), decoded))
}

fn collect_transcripts(dir: &Path, wanted: fn(&Path) -> bool, out: &mut Vec<PathBuf>, depth: u32) {
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
            collect_transcripts(&path, wanted, out, depth + 1);
        } else if wanted(&path) {
            out.push(path);
        }
    }
}
