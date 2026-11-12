//! Each project's calls as a file of lines, one JSON entry each: raised, shown on a client,
//! answered, what came of it. Lines are only ever added. A file that fills starts over, keeping the
//! one before, and opens with every call still open so none is lost.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_view::calls::{CallAnswer, CallAsker, CallClient, CallOutcome, PersonCall};

use super::Source;

const FOLDER: &str = "person-calls";
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "event")]
pub(super) enum Entry {
    /// Put to the person, or written again whole at the top of a new file.
    #[serde(rename_all = "camelCase")]
    Raised {
        call: Box<PersonCall>,
        source: Source,
        /// What else raised the same call and waits on its answer.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also: Vec<Source>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    /// Another agent waits on it, raising it again from `source`.
    #[serde(rename_all = "camelCase")]
    Joined {
        id: String,
        asker: CallAsker,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<Source>,
        at: u64,
    },
    /// A client drew it.
    #[serde(rename_all = "camelCase")]
    Shown { id: String, by: CallClient, at: u64 },
    #[serde(rename_all = "camelCase")]
    Answered { id: String, answer: CallAnswer },
    /// It waits on the person again, as when an answer could not reach its asker.
    #[serde(rename_all = "camelCase")]
    Reopened { id: String, at: u64 },
    /// An agent that waited on it no longer does, while others still do.
    #[serde(rename_all = "camelCase")]
    Left { id: String, source: Source, at: u64 },
    /// What came of it; with no answer, it closes.
    #[serde(rename_all = "camelCase")]
    Came { id: String, outcome: CallOutcome },
}

impl Entry {
    pub(super) fn id(&self) -> &str {
        match self {
            Entry::Raised { call, .. } => &call.id,
            Entry::Joined { id, .. }
            | Entry::Shown { id, .. }
            | Entry::Answered { id, .. }
            | Entry::Reopened { id, .. }
            | Entry::Left { id, .. }
            | Entry::Came { id, .. } => id,
        }
    }
}

pub(super) struct CallLog {
    folder: Option<PathBuf>,
}

impl CallLog {
    /// Without a data folder nothing is kept.
    pub(super) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            folder: data_dir.map(|dir| dir.join(FOLDER)),
        }
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

    /// Adds `entry` to its project's file. A file too full to take it starts over with what
    /// `still_open` gives first.
    pub(super) fn append(
        &self,
        project: &str,
        entry: &Entry,
        still_open: impl FnOnce() -> Vec<Entry>,
    ) {
        let Some((current, old)) = self.files(project) else {
            return;
        };
        let Ok(line) = line_of(entry) else {
            return;
        };
        let full = std::fs::metadata(&current)
            .is_ok_and(|file| file.len() + line.len() as u64 > MAX_FILE_BYTES);
        let written = (|| {
            let mut lines = Vec::new();
            if full {
                std::fs::rename(&current, &old)?;
                for carried in still_open() {
                    lines.extend(line_of(&carried)?);
                }
            }
            if !full && !ends_a_line(&current) {
                lines.push(b'\n');
            }
            lines.extend(line);
            if let Some(folder) = current.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(&current)?.write_all(&lines)
        })();
        if let Err(error) = written {
            eprintln!("sikemux core: a call was not written down: {error}");
        }
    }

    /// Every entry kept, oldest first.
    pub(super) fn read_all(&self) -> Vec<Entry> {
        let Some(folder) = &self.folder else {
            return Vec::new();
        };
        let Ok(listed) = std::fs::read_dir(folder) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = listed.flatten().map(|entry| entry.path()).collect();
        let older = |path: &PathBuf| !path.to_string_lossy().ends_with(".old.jsonl");
        files.sort_by_key(|path| (older(path), path.clone()));
        let mut entries = Vec::new();
        for path in files {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            entries.extend(
                (bytes.split(|byte| *byte == b'\n'))
                    .filter_map(|line| serde_json::from_slice::<Entry>(line).ok()),
            );
        }
        entries
    }
}

/// Whether the file at `path` is empty or its last line is whole, as one cut off by a crash is
/// not: the next entry then starts a line of its own.
fn ends_a_line(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };
    let mut last = [0u8; 1];
    match file.seek(SeekFrom::End(-1)) {
        Ok(_) => file.read_exact(&mut last).is_err() || last[0] == b'\n',
        Err(_) => true,
    }
}

fn line_of(entry: &Entry) -> std::io::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    Ok(line)
}
