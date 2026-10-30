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
use sikemux_process::sync::Locked;

use super::Kind;

const FOLDER: &str = "agent-messages";
const MAX_FILE_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_READ: usize = 500;

/// What became of a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// What the sender was called then, as its name goes once it closes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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

    /// When each project last logged a message since the core started, so a
    /// client reads a project's log again when its time changes.
    pub(crate) fn latest(&self) -> BTreeMap<String, u64> {
        self.latest.locked().clone()
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
        let mut latest = self.latest.locked();
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
        let _writing = self.latest.locked();
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

/// Whether a message still waits for its agent, or never reached it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    Held,
    Failed,
}

/// One message between agents, from being sent to what last became of it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub sent_at: u64,
    pub at: u64,
    /// The sending agent's id, or `person`, `board`, `auto` or `sikemux`.
    pub from: Option<String>,
    /// The sending agent's name now, else when it was logged; none for those that are
    /// not agents.
    pub from_name: Option<String>,
    pub to: String,
    pub to_name: Option<String>,
    pub kind: Option<Kind>,
    pub outcome: Outcome,
    /// It waited for its agent before it got where it is.
    pub waited: bool,
    pub tone: Option<Tone>,
}

const NOT_AGENTS: [&str; 4] = ["person", "board", "auto", "sikemux"];

fn still_going(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Typed | Outcome::Waiting | Outcome::Returned
    )
}

fn tone(outcome: Outcome) -> Option<Tone> {
    match outcome {
        Outcome::Waiting | Outcome::Returned => Some(Tone::Held),
        Outcome::Refused | Outcome::Dropped => Some(Tone::Failed),
        _ => None,
    }
}

/// The log, newest first, as one row per message. A message is logged when it is sent
/// and again when one that waited is delivered, taken back or dropped; a later line
/// belongs to the oldest message still waiting for the same agent from the same sender.
/// `name_of` gives an open agent's name now, ahead of the one it was logged with.
pub(crate) fn rows(entries: &[Entry], name_of: impl Fn(&str) -> Option<String>) -> Vec<Row> {
    let name = |id: Option<&str>, logged: &Option<String>| {
        let id = id.filter(|id| !NOT_AGENTS.contains(id))?;
        name_of(id).or_else(|| logged.clone())
    };
    let mut rows: Vec<Row> = Vec::with_capacity(entries.len());
    let mut going: Vec<usize> = Vec::new();
    for entry in entries.iter().rev() {
        let later = matches!(
            entry.outcome,
            Outcome::Delivered | Outcome::Returned | Outcome::Dropped
        );
        let earlier = later
            .then(|| {
                going.iter().position(|&place| {
                    let row = &rows[place];
                    row.to == entry.to
                        && (entry.from.is_none() || row.from == entry.from)
                        && (entry.kind.is_none() || row.kind == entry.kind)
                })
            })
            .flatten();
        if let Some(waiting) = earlier {
            let row = &mut rows[going[waiting]];
            row.waited |= !matches!(row.outcome, Outcome::Typed);
            row.outcome = entry.outcome;
            row.tone = tone(entry.outcome);
            row.at = entry.at;
            if !still_going(entry.outcome) {
                going.remove(waiting);
            }
            continue;
        }
        if still_going(entry.outcome) {
            going.push(rows.len());
        }
        rows.push(Row {
            sent_at: entry.at,
            at: entry.at,
            from: entry.from.clone(),
            from_name: name(entry.from.as_deref(), &entry.from_name),
            to: entry.to.clone(),
            to_name: name(Some(&entry.to), &entry.to_name),
            kind: entry.kind,
            outcome: entry.outcome,
            waited: false,
            tone: tone(entry.outcome),
        });
    }
    rows.reverse();
    rows
}
