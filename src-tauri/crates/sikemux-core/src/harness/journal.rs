//! What happened to a project's harness tasks, as numbered events an agent can
//! wait on. Each project appends to its own JSON-lines file, so an event
//! cursor still means the same place after the core restarts.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_JOURNAL_BYTES: u64 = 5 * 1024 * 1024;
/// What a rotation keeps, so the next one is a while away.
const ROTATED_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;
/// Events kept in memory per project. Older ones are only on disk, and a wait
/// from before them comes back truncated.
pub const RETAINED_EVENTS: usize = 1024;
const TAIL_READ_BYTES: u64 = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub seq: u64,
    pub ts: u64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventsPage {
    pub events: Vec<JournalEntry>,
    pub cursor: u64,
    pub truncated: bool,
}

struct ProjectJournal {
    path: Option<PathBuf>,
    last_seq: u64,
    size: u64,
    events: VecDeque<JournalEntry>,
}

/// Every project's journal, loaded the first time the project is named.
/// Without a directory the journals live only in memory.
pub struct Journals {
    directory: Option<PathBuf>,
    projects: HashMap<String, ProjectJournal>,
}

pub fn journal_file_name(project: &str) -> String {
    let digest = hex::encode(Sha256::digest(project.as_bytes()));
    format!("{}.jsonl", digest.get(..16).unwrap_or(&digest))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// The last whole lines of the file, at most `TAIL_READ_BYTES` of them.
fn read_tail(path: &Path) -> std::io::Result<(Vec<JournalEntry>, u64)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(TAIL_READ_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0 {
        text = text
            .split_once('\n')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default();
    }
    let entries = text
        .lines()
        .filter_map(|line| serde_json::from_str::<JournalEntry>(line).ok())
        .collect();
    Ok((entries, size))
}

/// Keeps the newest `ROTATED_JOURNAL_BYTES`, cut at a line boundary.
fn rotate(path: &Path) -> std::io::Result<u64> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(ROTATED_JOURNAL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let kept = match bytes.iter().position(|byte| *byte == b'\n') {
        Some(newline) if start > 0 => bytes.get(newline + 1..).unwrap_or_default(),
        _ => &bytes[..],
    };
    let staging = path.with_extension("jsonl.rotating");
    std::fs::write(&staging, kept)?;
    std::fs::rename(&staging, path)?;
    Ok(kept.len() as u64)
}

impl ProjectJournal {
    fn load(path: Option<PathBuf>) -> Self {
        let (entries, size) = path
            .as_deref()
            .and_then(|path| read_tail(path).ok())
            .unwrap_or_default();
        let last_seq = entries.iter().map(|entry| entry.seq).max().unwrap_or(0);
        let skip = entries.len().saturating_sub(RETAINED_EVENTS);
        Self {
            path,
            last_seq,
            size,
            events: entries.into_iter().skip(skip).collect(),
        }
    }

    fn write(&mut self, entry: &JournalEntry) -> std::io::Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(path)?.write_all(&line)?;
        self.size += line.len() as u64;
        if self.size > MAX_JOURNAL_BYTES {
            self.size = rotate(path)?;
        }
        Ok(())
    }
}

/// One project's journal as kept in memory, for handing over to a
/// replacement core.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalRecord {
    pub project: String,
    pub last_seq: u64,
    pub size: u64,
    pub events: Vec<JournalEntry>,
}

impl Journals {
    pub fn record(&self) -> Vec<JournalRecord> {
        self.projects
            .iter()
            .map(|(project, journal)| JournalRecord {
                project: project.clone(),
                last_seq: journal.last_seq,
                size: journal.size,
                events: journal.events.iter().cloned().collect(),
            })
            .collect()
    }

    pub fn restore(&mut self, records: Vec<JournalRecord>) {
        for record in records {
            let path = self
                .directory
                .as_deref()
                .map(|directory| directory.join(journal_file_name(&record.project)));
            let skip = record.events.len().saturating_sub(RETAINED_EVENTS);
            self.projects.insert(
                record.project,
                ProjectJournal {
                    path,
                    last_seq: record.last_seq,
                    size: record.size,
                    events: record.events.into_iter().skip(skip).collect(),
                },
            );
        }
    }

    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            projects: HashMap::new(),
        }
    }

    fn project(&mut self, project: &str) -> &mut ProjectJournal {
        let directory = self.directory.as_deref();
        self.projects.entry(project.to_owned()).or_insert_with(|| {
            ProjectJournal::load(
                directory.map(|directory| directory.join(journal_file_name(project))),
            )
        })
    }

    pub fn append(
        &mut self,
        project: &str,
        kind: &str,
        execution_id: Option<&str>,
        task_id: Option<&str>,
    ) -> JournalEntry {
        let journal = self.project(project);
        journal.last_seq += 1;
        let entry = JournalEntry {
            seq: journal.last_seq,
            ts: now_ms(),
            kind: kind.to_owned(),
            execution_id: execution_id.map(str::to_owned),
            task_id: task_id.map(str::to_owned),
        };
        if let Err(error) = journal.write(&entry) {
            eprintln!("sikemux core: could not write the harness journal: {error}");
        }
        journal.events.push_back(entry.clone());
        if journal.events.len() > RETAINED_EVENTS {
            journal.events.pop_front();
        }
        entry
    }

    pub fn cursor(&mut self, project: &str) -> u64 {
        self.project(project).last_seq
    }

    /// Events after `after`, optionally only one execution's. A cursor this
    /// journal never handed out is an error.
    pub fn since(
        &mut self,
        project: &str,
        after: u64,
        execution_id: Option<&str>,
    ) -> Result<EventsPage, String> {
        let journal = self.project(project);
        if after > journal.last_seq {
            return Err("Event cursor is invalid; take a fresh one from workspace_inspect".into());
        }
        let oldest = journal.events.front().map(|entry| entry.seq);
        let truncated = match oldest {
            Some(oldest) => after + 1 < oldest,
            None => after < journal.last_seq,
        };
        let events = journal
            .events
            .iter()
            .filter(|entry| entry.seq > after)
            .filter(|entry| execution_id.is_none_or(|id| entry.execution_id.as_deref() == Some(id)))
            .cloned()
            .collect();
        Ok(EventsPage {
            events,
            cursor: journal.last_seq,
            truncated,
        })
    }

    /// Whether the kept history mentions the task, though no run of it is
    /// known any more.
    pub fn mentions_task(&mut self, project: &str, task_id: &str) -> bool {
        self.project(project)
            .events
            .iter()
            .any(|entry| entry.task_id.as_deref() == Some(task_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journals(dir: &Path) -> Journals {
        Journals::new(Some(dir.to_path_buf()))
    }

    #[test]
    fn cursors_count_up_per_project_and_filter_by_execution() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = journals(dir.path());
        assert_eq!(journal.cursor("/one"), 0);
        journal.append("/one", "task.starting", Some("a"), Some("dev"));
        journal.append("/two", "ui.opened", None, None);
        journal.append("/one", "task.running", Some("b"), Some("test"));
        assert_eq!(journal.cursor("/one"), 2);
        assert_eq!(journal.cursor("/two"), 1);
        let page = journal.since("/one", 0, Some("b")).unwrap();
        assert_eq!(page.cursor, 2);
        assert!(!page.truncated);
        assert_eq!(
            page.events
                .iter()
                .map(|entry| entry.kind.as_str())
                .collect::<Vec<_>>(),
            ["task.running"]
        );
        assert!(journal.since("/one", 2, None).unwrap().events.is_empty());
        assert!(journal.since("/one", 3, None).is_err());
    }

    #[test]
    fn a_new_core_reads_the_tail_and_keeps_counting_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = journals(dir.path());
        first.append("/one", "task.starting", Some("a"), Some("dev"));
        first.append("/one", "task.running", Some("a"), Some("dev"));
        drop(first);

        let mut second = journals(dir.path());
        assert_eq!(second.cursor("/one"), 2);
        let page = second.since("/one", 1, None).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].kind, "task.running");
        assert!(second.mentions_task("/one", "dev"));
        assert!(!second.mentions_task("/one", "other"));
        assert_eq!(
            second.append("/one", "task.stopped", Some("a"), None).seq,
            3
        );
    }

    #[test]
    fn a_wait_from_before_the_kept_events_is_truncated() {
        let mut journal = Journals::new(None);
        for _ in 0..RETAINED_EVENTS + 10 {
            journal.append("/one", "task.output", Some("a"), None);
        }
        let page = journal.since("/one", 3, None).unwrap();
        assert!(page.truncated);
        assert_eq!(page.events.len(), RETAINED_EVENTS);
        assert!(!journal.since("/one", 10, None).unwrap().truncated);
    }

    #[test]
    fn a_full_journal_keeps_its_newest_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = journals(dir.path());
        let padding = "x".repeat(4000);
        let mut last = 0;
        while journal.project("/one").size <= MAX_JOURNAL_BYTES - 8000 {
            last = journal
                .append("/one", "task.output", Some(&padding), None)
                .seq;
        }
        last = journal
            .append("/one", "task.output", Some(&padding), None)
            .seq
            .max(last);
        last = journal
            .append("/one", "task.output", Some(&padding), None)
            .seq
            .max(last);
        let path = dir.path().join(journal_file_name("/one"));
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size <= ROTATED_JOURNAL_BYTES + 8192, "kept {size} bytes");
        let text = std::fs::read_to_string(&path).unwrap();
        let first: JournalEntry = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert!(first.seq > 1);
        drop(journal);
        assert_eq!(journals(dir.path()).cursor("/one"), last);
    }
}
