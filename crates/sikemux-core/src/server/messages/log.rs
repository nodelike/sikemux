//! A log of the messages between agents in each project: who sent what kind
//! to whom, when, and what became of it. One line of JSON each, never the
//! text, in a file per project that starts over once it fills, keeping the
//! one before. Questions asked aside, by this person's agents or a teammate's, are kept the
//! same way in a log of their own, with their text.

pub use sikemux_calls::messages::{MessageOutcome, MessageRow, MessageTone};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;

use super::MessageKind;

const FOLDER: &str = "agent-messages";
const MAX_FILE_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_READ: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageEntry {
    pub at: u64,
    /// The sending agent's id, or a [`super::Sender`]'s; unknown for a message that came back
    /// unread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// What the sender was called then, as its name goes once it closes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<MessageKind>,
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_name: Option<String>,
    pub outcome: MessageOutcome,
}

/// A question one agent asked about another and the answer it got, in place of its turns.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AskEntry {
    pub at: u64,
    /// The asking agent's id; for a teammate's agent, which has no id here, its name.
    pub from: String,
    pub from_name: String,
    /// The agent whose work answered; empty when nobody held the task.
    pub to: String,
    pub to_name: String,
    pub question: String,
    pub answer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teammate: Option<Teammate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<u64>,
}

/// The person in one of this account's orgs whose agent asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Teammate {
    pub org: String,
    pub member: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// The program their agent that asked runs on, which picks its mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_cli: Option<String>,
}

pub(crate) struct MessageLog<T = MessageEntry> {
    folder: Option<PathBuf>,
    /// When each project last logged a message, held while writing.
    latest: Mutex<BTreeMap<String, u64>>,
    entry: std::marker::PhantomData<T>,
}

impl MessageLog {
    /// Without a data folder nothing is logged.
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self::in_folder(data_dir, FOLDER)
    }
}

impl<T: Serialize + DeserializeOwned> MessageLog<T> {
    pub(crate) fn in_folder(data_dir: Option<&Path>, folder: &str) -> Self {
        Self {
            folder: data_dir.map(|dir| dir.join(folder)),
            latest: Mutex::new(BTreeMap::new()),
            entry: std::marker::PhantomData,
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

    pub(crate) fn append(&self, project: &str, at: u64, entry: &T) {
        let Some((current, old)) = self.files(project) else {
            return;
        };
        let Ok(mut line) = serde_json::to_vec(entry) else {
            return;
        };
        line.push(b'\n');
        let mut latest = self.latest.locked();
        latest.insert(project.to_owned(), at);
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
    pub(crate) fn read(&self, project: &str, limit: usize) -> Vec<T> {
        let Some((current, old)) = self.files(project) else {
            return Vec::new();
        };
        let _writing = self.latest.locked();
        let mut entries = Vec::new();
        for path in [current, old] {
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            let mut lines: Vec<T> = std::io::BufReader::new(file)
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

fn still_going(outcome: MessageOutcome) -> bool {
    matches!(
        outcome,
        MessageOutcome::Typed | MessageOutcome::Waiting | MessageOutcome::Returned
    )
}

fn tone(outcome: MessageOutcome) -> Option<MessageTone> {
    match outcome {
        MessageOutcome::Waiting | MessageOutcome::Returned => Some(MessageTone::Held),
        MessageOutcome::Refused | MessageOutcome::Dropped => Some(MessageTone::Failed),
        _ => None,
    }
}

/// The log, newest first, as one row per message. A message is logged when it is sent
/// and again when one that waited is delivered, taken back or dropped; a later line
/// belongs to the oldest message still waiting for the same agent from the same sender.
/// `name_of` gives an open agent's name now, ahead of the one it was logged with.
pub(crate) fn rows(
    entries: &[MessageEntry],
    name_of: impl Fn(&str) -> Option<String>,
) -> Vec<MessageRow> {
    let name = |id: Option<&str>, logged: &Option<String>| {
        let id = id.filter(|id| super::Sender::of(id).is_none())?;
        name_of(id).or_else(|| logged.clone())
    };
    let mut rows: Vec<MessageRow> = Vec::with_capacity(entries.len());
    let mut going: Vec<usize> = Vec::new();
    for entry in entries.iter().rev() {
        let later = matches!(
            entry.outcome,
            MessageOutcome::Delivered | MessageOutcome::Returned | MessageOutcome::Dropped
        );
        let earlier = later
            .then(|| {
                (going.iter().enumerate()).find(|(_, place)| {
                    rows.get(**place).is_some_and(|row| {
                        row.to == entry.to
                            && (entry.from.is_none() || row.from == entry.from)
                            && (entry.kind.is_none() || row.kind == entry.kind)
                    })
                })
            })
            .flatten()
            .map(|(waiting, place)| (waiting, *place));
        if let Some((waiting, place)) = earlier {
            if let Some(row) = rows.get_mut(place) {
                row.waited |= !matches!(row.outcome, MessageOutcome::Typed);
                row.outcome = entry.outcome;
                row.tone = tone(entry.outcome);
                row.at = entry.at;
            }
            if !still_going(entry.outcome) {
                going.remove(waiting);
            }
            continue;
        }
        if still_going(entry.outcome) {
            going.push(rows.len());
        }
        rows.push(MessageRow {
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
