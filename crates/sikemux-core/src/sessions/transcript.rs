//! The transcript an agent's CLI writes of a session: where it is under the account folder it
//! keeps it in, and what an agent running in a terminal said in it. Transcripts only grow, so a
//! byte offset into one is a cursor.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent_catalog::AgentKind;

const MAX_SESSION_ID_CHARS: usize = 200;

/// Whether `id` can name a session's file without leading out of its folder.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The transcript of session `id` under the account folder `root`, looked for
/// in `cwd`'s project first. Other agents keep their history in shapes Sikemux
/// does not read line by line.
pub fn find(kind: AgentKind, root: &Path, cwd: Option<&str>, id: &str) -> Option<PathBuf> {
    if !valid_session_id(id) {
        return None;
    }
    kind.ops().transcript(root, cwd, id)
}

pub(super) fn written(path: PathBuf) -> Option<PathBuf> {
    fs::metadata(&path)
        .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
        .then_some(path)
}

/// One thing said or done in a chat, as another agent reads it.
pub(crate) enum Said {
    /// What the agent was sent: by the person, or another agent's message.
    Asked(String),
    Message(String),
    Tool {
        id: String,
        title: Option<String>,
        failed: bool,
    },
    TurnStarted,
    TurnEnded {
        cancelled: bool,
    },
    Failed(String),
}

/// How much of a file's end is read when no cursor says where to start.
const TAIL_BYTES: u64 = 2 * 1024 * 1024;
/// How much one read takes from a cursor on.
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

/// The session file of `session`, under the agent's account folder or the
/// CLI's own.
pub(crate) fn locate(kind: AgentKind, home: Option<&Path>, session: &str) -> Option<PathBuf> {
    let home = home.map(Path::to_path_buf).or_else(|| kind.home_root())?;
    find(kind, &home, None, session)
}

/// What a session file said from one offset on.
pub(crate) struct Excerpt {
    /// Each with the offset of its line, and whether a turn starts there.
    pub said: Vec<(u64, bool, Said)>,
    /// The offset of each line that said something, oldest first, and when it was written.
    pub times: Vec<(u64, u64)>,
    /// Where reading on starts.
    pub next: u64,
    /// Everything the file holds has been read.
    pub whole: bool,
    /// The offset asked for is past the file's end, so it is not the file the
    /// cursor came from and this is its last turns instead.
    pub restarted: bool,
}

/// The bytes a read takes from the file, from `from` or at its end, with where they start and
/// how long the file is.
fn window(path: &Path, from: Option<u64>) -> std::io::Result<(Vec<u8>, u64, u64)> {
    let max = if from.is_some() {
        MAX_READ_BYTES
    } else {
        TAIL_BYTES
    };
    if super::is_compressed(path) {
        let decoded = |from, max| {
            super::decoded_window(path, from, max)
                .ok_or_else(|| std::io::Error::other("the session file could not be decoded"))
        };
        let read = decoded(from, max)?;
        return match from {
            Some(from) if from > read.2 => decoded(None, TAIL_BYTES),
            _ => Ok(read),
        };
    }
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = from
        .filter(|from| *from <= len)
        .unwrap_or_else(|| len.saturating_sub(TAIL_BYTES));
    let end = len.min(start + max);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(end - start).read_to_end(&mut bytes)?;
    Ok((bytes, start, len))
}

/// What the file said from `from` on, or over its last `turns` turns.
pub(crate) fn read(
    cli: AgentKind,
    path: &Path,
    from: Option<u64>,
    turns: usize,
) -> std::io::Result<Excerpt> {
    let (bytes, start, len) = window(path, from)?;
    let restarted = from.is_some_and(|from| from > len);
    let from = from.filter(|from| *from <= len);
    let mut offset = start;
    let mut rest = bytes.as_slice();
    if from.is_none() && start > 0 {
        let cut = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(rest.len(), |at| at + 1);
        offset += cut as u64;
        rest = &rest[cut..];
    }
    let mut said: Vec<(u64, bool, Said)> = Vec::new();
    let mut times = Vec::new();
    while let Some(at) = rest.iter().position(|byte| *byte == b'\n') {
        let line = &rest[..at];
        if let Ok(value) = serde_json::from_slice::<Value>(line) {
            let mut items = Vec::new();
            if let Some(said) = cli.ops().transcript_said() {
                said(&value, &mut items);
            }
            if !items.is_empty() {
                times.extend(written_at(&value).map(|at| (offset, at)));
            }
            for (starts, item) in items {
                push(&mut said, cli, offset, starts, item);
            }
        }
        offset += at as u64 + 1;
        rest = &rest[at + 1..];
    }
    if from.is_none() {
        let starts: Vec<usize> = said
            .iter()
            .enumerate()
            .filter(|(_, (_, starts, _))| *starts)
            .map(|(position, _)| position)
            .collect();
        if let Some(first) = starts
            .len()
            .checked_sub(turns)
            .and_then(|skip| starts.get(skip))
        {
            said.drain(..*first);
        }
    }
    Ok(Excerpt {
        said,
        times,
        next: offset,
        whole: offset == len,
        restarted,
    })
}

/// When a record of a session file was written, in milliseconds since the epoch.
fn written_at(record: &Value) -> Option<u64> {
    let stamp = record.get("timestamp")?.as_str()?;
    let at = time::OffsetDateTime::parse(stamp, &time::format_description::well_known::Rfc3339);
    u64::try_from(at.ok()?.unix_timestamp_nanos() / 1_000_000).ok()
}

/// Adds `item`, unless it repeats the one before it: a Codex rollout can
/// carry the same message both as an event and as a finished item. In a
/// transcript that marks no turn's end, the next prompt ends the one before.
fn push(
    said: &mut Vec<(u64, bool, Said)>,
    cli: AgentKind,
    offset: u64,
    mut starts: bool,
    item: Said,
) {
    if let Some((_, _, last)) = said.last() {
        let repeat = match (last, &item) {
            (Said::Asked(last), Said::Asked(text)) | (Said::Message(last), Said::Message(text)) => {
                last == text
            }
            _ => false,
        };
        if repeat {
            return;
        }
        if matches!(last, Said::TurnStarted) && matches!(item, Said::Asked(_)) {
            starts = false;
        }
        let open = !matches!(last, Said::TurnEnded { .. } | Said::Failed(_));
        if !cli.ops().transcript_ends_turns() && starts && open {
            said.push((offset, false, Said::TurnEnded { cancelled: false }));
        }
    }
    said.push((offset, starts, item));
}

pub(super) fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.trim().is_empty())
}

pub(super) fn texts(content: &Value) -> Option<String> {
    if let Some(text) = text(content) {
        return Some(text.to_owned());
    }
    let parts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("text" | "Text" | "input_text" | "output_text")
            )
        })
        .filter_map(|part| text(part.get("text")?))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// `text` without the tags Claude Code wraps a long paste in.
fn unpasted(text: &str) -> String {
    let mut text = text.to_owned();
    for tag in ["<pasted_content", "</pasted_content"] {
        while let Some(start) = text.find(tag) {
            let Some(length) = text[start..].find('>') else {
                break;
            };
            text.replace_range(start..=start + length, "");
        }
    }
    text.trim().to_owned()
}

pub(super) fn prompt(text: String, items: &mut Vec<(bool, Said)>) {
    let text = unpasted(&text);
    let start = text.as_str();
    if start.starts_with("[Request interrupted") {
        items.push((false, Said::TurnEnded { cancelled: true }));
    } else if !start.starts_with("<command-") && !start.starts_with("<local-command") {
        items.push((true, Said::Asked(text)));
    }
}
