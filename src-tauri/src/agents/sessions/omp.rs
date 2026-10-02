use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::claude::{CLAUDE_HEAD_BYTES, CLAUDE_TAIL_BYTES};
use super::recent::{Found, Hit, Listed, PageScan};
use super::rename::now_iso8601;
use super::{
    cached_title, collect_jsonl, condense, mtime_of, read_prefix, read_suffix, text_from_content,
    title_cache_stamp, MAX_AGENT_TRANSCRIPTS_INSPECTED,
};
use crate::agents::config::{omp_agent_root, omp_session_dirs};
use crate::agents::AgentSession;

pub(super) fn omp_sessions(cwd: &str) -> Vec<AgentSession> {
    omp_sessions_from_dirs(cwd, omp_session_dirs())
}

fn omp_sessions_from_dirs(cwd: &str, roots: Vec<PathBuf>) -> Vec<AgentSession> {
    let mut files = Vec::new();
    for root in roots {
        collect_jsonl(&root, &mut files, 0);
    }
    files.sort_unstable_by_key(|path| std::cmp::Reverse(mtime_of(path)));
    files.dedup();
    files.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|path| {
            omp_session(path)
                .filter(|found| found.project == cwd)
                .map(|found| found.session)
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

pub(super) fn omp_recent(scan: &PageScan<'_>) -> Vec<Hit> {
    let mut files = Vec::new();
    for root in omp_session_dirs() {
        collect_jsonl(&root, &mut files, 0);
    }
    files.sort_unstable();
    files.dedup();
    let listed = files
        .into_iter()
        .map(|path| Listed {
            at_ms: title_cache_stamp(&path).unix_millis(),
            key: path.to_string_lossy().into_owned(),
            item: path,
        })
        .collect();
    scan.collect(listed, |path| omp_session(path))
}

/// The session in one transcript and the project its header names.
fn omp_session(path: &Path) -> Option<Found> {
    let file = fs::File::open(path).ok()?;
    let header = BufReader::new(file)
        .lines()
        .take(40)
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .find(|value| value.get("type").and_then(Value::as_str) == Some("session"))?;
    let project = header.get("cwd").and_then(Value::as_str)?.to_string();
    let mtime = mtime_of(path);
    let title = cached_title(path, title_cache_stamp(path), || omp_title(path))
        .or_else(|| {
            header
                .get("title")
                .and_then(Value::as_str)
                .and_then(condense)
        })
        .or_else(|| header.get("id").and_then(Value::as_str).and_then(condense))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("session")
                .chars()
                .take(13)
                .collect()
        });
    Some(Found {
        project,
        session: AgentSession {
            id: path.to_string_lossy().into_owned(),
            title,
            mtime,
        },
    })
}

fn scan_omp_line(line: &str, named: &mut Option<String>, first_user: &mut Option<String>) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("title" | "session_info") => {
            if let Some(title) = value
                .get("title")
                .or_else(|| value.get("name"))
                .and_then(Value::as_str)
                .and_then(condense)
            {
                *named = Some(title);
            }
        }
        Some("session") if named.is_none() => {
            *named = value
                .get("title")
                .and_then(Value::as_str)
                .and_then(condense);
        }
        Some("message") if first_user.is_none() => {
            let Some(message) = value.get("message") else {
                return;
            };
            if message.get("role").and_then(Value::as_str) == Some("user") {
                *first_user = message
                    .get("content")
                    .and_then(text_from_content)
                    .and_then(|text| condense(&text));
            }
        }
        _ => {}
    }
}

fn omp_title(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut named = None;
    let mut first_user = None;
    let head = read_prefix(&mut file, CLAUDE_HEAD_BYTES.min(len))?;
    for line in head.lines() {
        scan_omp_line(line, &mut named, &mut first_user);
    }
    if len > CLAUDE_HEAD_BYTES {
        if let Some(tail) = read_suffix(&mut file, len.saturating_sub(CLAUDE_TAIL_BYTES)) {
            for line in tail.lines().skip(1) {
                scan_omp_line(line, &mut named, &mut first_user);
            }
        }
    }
    named.or(first_user)
}

/// OMP keeps a session's name in a first line padded to exactly this many bytes, so it can be rewritten in place.
const OMP_TITLE_SLOT_BYTES: usize = 256;

/// Names a session the way OMP's `/rename` does: its title line, and the title index its welcome screen reads.
pub(super) fn rename_omp_session(
    path: &Path,
    name: &str,
    title_index: Option<&Path>,
) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let mut slot = vec![0u8; OMP_TITLE_SLOT_BYTES];
    file.read_exact(&mut slot)
        .map_err(|_| "this OMP session has no title line".to_string())?;
    let current = serde_json::from_slice::<Value>(&slot[..OMP_TITLE_SLOT_BYTES - 1]).ok();
    let is_slot = slot[OMP_TITLE_SLOT_BYTES - 1] == b'\n'
        && current
            .as_ref()
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            == Some("title");
    if !is_slot {
        return Err("this OMP session has no title line".to_string());
    }
    let line = omp_title_slot(name, &now_iso8601()).ok_or("this OMP session has no title line")?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    file.write_all(line.as_bytes())
        .map_err(|error| error.to_string())?;
    if let (Some(db), Some(session_id)) = (title_index, omp_session_id(path)) {
        record_omp_title(db, &session_id, name);
    }
    Ok(())
}

/// Cuts the title to fit the slot, as OMP does, rather than refusing a long name.
fn omp_title_slot(title: &str, updated_at: &str) -> Option<String> {
    let line = |title: &str, pad: &str| {
        let slot = serde_json::json!({
            "type": "title",
            "v": 1,
            "title": title,
            "source": "user",
            "updatedAt": updated_at,
            "pad": pad,
        });
        format!("{slot}\n")
    };
    let chars: Vec<char> = title.chars().collect();
    (0..=chars.len()).rev().find_map(|count| {
        let title: String = chars[..count].iter().collect();
        let pad = OMP_TITLE_SLOT_BYTES.checked_sub(line(&title, "").len())?;
        Some(line(&title, &" ".repeat(pad)))
    })
}

fn omp_session_id(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .take(40)
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .find(|value| value.get("type").and_then(Value::as_str) == Some("session"))?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

/// Where OMP's welcome screen looks up session names before it reads any session file.
pub(super) fn omp_title_index() -> Option<PathBuf> {
    omp_agent_root()
        .map(|root| root.join("history.db"))
        .filter(|db| db.is_file())
}

/// OMP treats its title index as a cache it can rebuild, so a failed write here does not fail the rename.
fn record_omp_title(db: &Path, session_id: &str, title: &str) {
    let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE) else {
        return;
    };
    let _ = conn.busy_timeout(Duration::from_secs(5));
    let _ = conn.execute(
        "INSERT INTO session_titles (session_id, title, updated_at) \
         VALUES (?1, ?2, CAST(strftime('%s','now') AS INTEGER)) \
         ON CONFLICT(session_id) DO UPDATE SET title = excluded.title, updated_at = excluded.updated_at",
        (session_id, title),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        omp_sessions_from_dirs, omp_title, omp_title_slot, rename_omp_session, OMP_TITLE_SLOT_BYTES,
    };
    use std::io::Write;

    #[test]
    fn omp_title_prefers_the_latest_explicit_name() {
        let mut transcript = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            transcript,
            "{}",
            serde_json::json!({"type":"title","title":"Initial title"})
        )
        .unwrap();
        writeln!(
            transcript,
            "{}",
            serde_json::json!({"type":"session","id":"session-1","cwd":"/repo"})
        )
        .unwrap();
        writeln!(
            transcript,
            "{}",
            serde_json::json!({"type":"message","message":{"role":"user","content":"Fallback prompt"}})
        )
        .unwrap();
        writeln!(
            transcript,
            "{}",
            serde_json::json!({"type":"title","title":"Renamed session"})
        )
        .unwrap();
        transcript.flush().unwrap();

        assert_eq!(
            omp_title(transcript.path()).as_deref(),
            Some("Renamed session")
        );
    }

    #[test]
    fn omp_session_listing_accepts_title_before_header() {
        let root = tempfile::tempdir().unwrap();
        let transcript = root.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"title\",\"title\":\"Ship OMP support\"}\n",
                "{\"type\":\"session\",\"id\":\"session-1\",\"cwd\":\"/repo\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"Fallback\"}}\n"
            ),
        )
        .unwrap();

        let sessions = omp_sessions_from_dirs("/repo", vec![root.path().to_path_buf()]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, transcript.to_string_lossy());
        assert_eq!(sessions[0].title, "Ship OMP support");
    }

    fn omp_session_with_slot(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("session.jsonl");
        let body = format!(
            "{}{}\n{}\n",
            omp_title_slot("Auto title", "2026-09-30T11:45:52.506Z").unwrap(),
            r#"{"type":"session","version":3,"id":"01a0f222","timestamp":"2026-09-30T11:45:52.506Z","cwd":"/repo"}"#,
            r#"{"type":"message","id":"96defca8","parentId":null,"message":{"role":"user","content":"Fix the flaky test"}}"#,
        );
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn a_renamed_omp_session_keeps_its_fixed_title_line_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = omp_session_with_slot(dir.path());
        let before = std::fs::read(&path).unwrap();
        let index = dir.path().join("history.db");
        rusqlite::Connection::open(&index)
            .unwrap()
            .execute_batch("CREATE TABLE session_titles (session_id TEXT PRIMARY KEY, title TEXT NOT NULL, updated_at INTEGER NOT NULL)")
            .unwrap();

        rename_omp_session(&path, "Flaky test fix", Some(&index)).unwrap();

        let after = std::fs::read(&path).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(
            after[OMP_TITLE_SLOT_BYTES..],
            before[OMP_TITLE_SLOT_BYTES..]
        );
        assert_eq!(after[OMP_TITLE_SLOT_BYTES - 1], b'\n');
        assert_eq!(omp_title(&path).as_deref(), Some("Flaky test fix"));
        let indexed: String = rusqlite::Connection::open(&index)
            .unwrap()
            .query_row(
                "SELECT title FROM session_titles WHERE session_id = '01a0f222'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed, "Flaky test fix");
    }

    #[test]
    fn a_long_name_is_cut_to_fit_the_title_line() {
        let slot = omp_title_slot(&"名".repeat(72), "2026-09-30T11:45:52.506Z").unwrap();
        assert_eq!(slot.len(), OMP_TITLE_SLOT_BYTES);
        assert!(serde_json::from_str::<serde_json::Value>(slot.trim_end()).is_ok());
    }

    #[test]
    fn an_omp_session_without_a_title_line_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let body = format!(
            "{}\n",
            r#"{"type":"session","version":3,"id":"01a0f222","cwd":"/repo"}"#
        )
        .repeat(8);
        std::fs::write(&path, &body).unwrap();

        assert!(rename_omp_session(&path, "Name", None).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }
}
