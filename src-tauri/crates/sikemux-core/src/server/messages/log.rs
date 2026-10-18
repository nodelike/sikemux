//! A log of the messages between agents in each project: who sent what kind
//! to whom, when, and what became of it. One line of JSON each, never the
//! text, in a file per project that starts over once it fills, keeping the
//! one before.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Kind;

const FOLDER: &str = "agent-messages";
const MAX_FILE_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_READ: usize = 500;

/// What became of a message.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Outcome {
    /// Taken into the turn the agent was working on.
    Steered,
    /// Started a turn of the agent's.
    NewTurn,
    /// Typed into a terminal agent's prompt, not yet seen taken.
    Typed,
    /// Waits for the agent to be free.
    Waiting,
    /// Reached the agent after waiting, or a terminal agent took it.
    Delivered,
    /// The agent did not take it, so it waits again.
    Returned,
    /// The same text went to the same agent a moment before.
    Repeat,
    Refused,
    /// Dropped unread: its agent closed, or it was withdrawn.
    Dropped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub at: u64,
    /// The sending agent's id, `person`, `board` or `auto`; unknown for a
    /// message that came back unread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// What the sender was called then, as its name goes once it closes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_name: Option<String>,
    pub outcome: Outcome,
}

pub(crate) struct MessageLog {
    folder: Option<PathBuf>,
    /// When each project last logged a message, held while writing.
    latest: Mutex<BTreeMap<String, u64>>,
}

impl MessageLog {
    /// Without a data folder nothing is logged.
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            folder: data_dir.map(|dir| dir.join(FOLDER)),
            latest: Mutex::new(BTreeMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, u64>> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// When each project last logged a message since the core started, so a
    /// client reads a project's log again when its time changes.
    pub(crate) fn latest(&self) -> BTreeMap<String, u64> {
        self.lock().clone()
    }

    fn files(&self, project: &str) -> Option<(PathBuf, PathBuf)> {
        let folder = self.folder.as_ref()?;
        let digest = hex::encode(Sha256::digest(project.as_bytes()));
        let stem = digest.get(..16).unwrap_or(&digest);
        Some((
            folder.join(format!("{stem}.jsonl")),
            folder.join(format!("{stem}.old.jsonl")),
        ))
    }

    pub(crate) fn append(&self, project: &str, entry: &Entry) {
        let Some((current, old)) = self.files(project) else {
            return;
        };
        let Ok(mut line) = serde_json::to_vec(entry) else {
            return;
        };
        line.push(b'\n');
        let mut latest = self.lock();
        latest.insert(project.to_owned(), entry.at);
        let full = std::fs::metadata(&current)
            .is_ok_and(|file| file.len() + line.len() as u64 > MAX_FILE_BYTES);
        let written = (|| {
            if full {
                std::fs::rename(&current, &old)?;
            }
            if let Some(folder) = current.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(&current)?.write_all(&line)
        })();
        if let Err(error) = written {
            eprintln!("sikemux core: a message between agents was not logged: {error}");
        }
    }

    /// The project's latest entries, newest first, at most `limit`.
    pub(crate) fn read(&self, project: &str, limit: usize) -> Vec<Entry> {
        let Some((current, old)) = self.files(project) else {
            return Vec::new();
        };
        let _writing = self.lock();
        let mut entries = Vec::new();
        for path in [current, old] {
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            let mut lines: Vec<Entry> = std::io::BufReader::new(file)
                .lines()
                .map_while(Result::ok)
                .filter_map(|line| serde_json::from_str(&line).ok())
                .collect();
            lines.reverse();
            entries.extend(lines);
            if entries.len() >= limit {
                break;
            }
        }
        entries.truncate(limit);
        entries
    }
}
