use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde_json::Value;

use super::recent::{Found, Hit, Listed, PageScan};
use super::rename::{append_line, now_iso8601};
use super::{
    cached_title, collect_jsonl, condense, mtime_of, read_suffix, text_from_content,
    title_cache_stamp, MAX_AGENT_TRANSCRIPTS_INSPECTED,
};

const PI_TAIL_BYTES: u64 = 128 * 1024;
use crate::agents::AgentSession;

// ---- pi — ~/.pi/agent/sessions/**/<session>.jsonl ----------------------
pub(in crate::agents) fn pi_session_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_SESSION_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        return Some(PathBuf::from(dir).join("sessions"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".pi/agent/sessions"))
}

pub(super) fn pi_sessions(cwd: &str) -> Vec<AgentSession> {
    let Some(root) = pi_session_dir() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_jsonl(&root, &mut files, 0);
    files.sort_unstable_by_key(|path| std::cmp::Reverse(mtime_of(path)));
    files.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|path| {
            pi_session(path)
                .filter(|found| found.project == cwd)
                .map(|found| found.session)
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

pub(super) fn pi_recent(scan: &PageScan<'_>) -> Vec<Hit> {
    let Some(root) = pi_session_dir() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_jsonl(&root, &mut files, 0);
    let listed = files
        .into_iter()
        .map(|path| Listed {
            at_ms: title_cache_stamp(&path).unix_millis(),
            key: path.to_string_lossy().into_owned(),
            item: path,
        })
        .collect();
    scan.collect(listed, |path| pi_session(path))
}

/// The session in one transcript and the project its header names.
fn pi_session(path: &Path) -> Option<Found> {
    let file = fs::File::open(path).ok()?;
    let mut first = String::new();
    BufReader::new(file).read_line(&mut first).ok()?;
    let v = serde_json::from_str::<Value>(first.trim()).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("session") {
        return None;
    }
    let project = v.get("cwd").and_then(|c| c.as_str())?.to_string();
    let id = path.to_string_lossy().to_string();
    let mtime = mtime_of(path);
    let title = cached_title(path, title_cache_stamp(path), || {
        pi_latest_name(path).or_else(|| pi_title(path))
    })
    .or_else(|| v.get("id").and_then(|i| i.as_str()).and_then(condense))
    .unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
            .chars()
            .take(13)
            .collect()
    });
    Some(Found {
        project,
        session: AgentSession { id, title, mtime },
    })
}

fn pi_title(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut first_user: Option<String> = None;
    let mut named: Option<String> = None;
    for line in BufReader::new(file).lines().take(220).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("session_info") => {
                if let Some(name) = v.get("name").and_then(|n| n.as_str()).and_then(condense) {
                    named = Some(name);
                }
            }
            Some("message") if first_user.is_none() => {
                let Some(message) = v.get("message") else {
                    continue;
                };
                if message.get("role").and_then(|r| r.as_str()) != Some("user") {
                    continue;
                }
                if let Some(text) = message
                    .get("content")
                    .and_then(text_from_content)
                    .and_then(|t| condense(&t))
                {
                    first_user = Some(text);
                }
            }
            _ => {}
        }
    }
    named.or(first_user)
}

/// The name pi's `/name` gave the session last, which may sit past the lines read for its first prompt.
fn pi_latest_name(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(PI_TAIL_BYTES);
    let tail = read_suffix(&mut file, start)?;
    tail.lines()
        .skip(usize::from(start > 0))
        .filter(|line| line.contains("\"session_info\""))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("session_info"))
        .filter_map(|value| value.get("name").and_then(Value::as_str).and_then(condense))
        .last()
}

/// Names a session the way pi's `/name` does: a `session_info` entry that continues its last entry.
pub(super) fn rename_pi_session(path: &Path, name: &str) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let start = file
        .metadata()
        .map_err(|error| error.to_string())?
        .len()
        .saturating_sub(PI_TAIL_BYTES);
    let tail = read_suffix(&mut file, start).unwrap_or_default();
    let parent = tail.lines().rev().find_map(|line| {
        serde_json::from_str::<Value>(line)
            .ok()?
            .get("id")?
            .as_str()
            .map(str::to_string)
    });
    let entry = serde_json::json!({
        "type": "session_info",
        "id": uuid::Uuid::new_v4().simple().to_string()[..8],
        "parentId": parent,
        "timestamp": now_iso8601(),
        "name": name,
    });
    append_line(path, &entry.to_string())
}

#[cfg(test)]
mod tests {
    use super::{pi_latest_name, pi_title, rename_pi_session};
    use serde_json::Value;

    const SESSION: &str = concat!(
        "{\"type\":\"session\",\"version\":3,\"id\":\"aaaa1111\",\"timestamp\":\"2026-09-29T10:00:00.000Z\",\"cwd\":\"/repo\"}\n",
        "{\"type\":\"message\",\"id\":\"a1b2c3d4\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Fix the flaky test\"}]}}\n",
        "{\"type\":\"message\",\"id\":\"b2c3d4e5\",\"parentId\":\"a1b2c3d4\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done\"}]}}\n",
    );

    #[test]
    fn a_renamed_pi_session_continues_its_last_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, SESSION).unwrap();

        rename_pi_session(&path, "Flaky test fix").unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let entry: Value = serde_json::from_str(body.lines().last().unwrap()).unwrap();
        assert_eq!(entry["type"], "session_info");
        assert_eq!(entry["parentId"], "b2c3d4e5");
        assert_eq!(entry["name"], "Flaky test fix");
        assert_eq!(entry["id"].as_str().unwrap().len(), 8);
        assert_eq!(pi_latest_name(&path).as_deref(), Some("Flaky test fix"));
    }

    #[test]
    fn a_name_given_late_in_a_long_session_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let filler = "{\"type\":\"custom\",\"id\":\"f\"}\n".repeat(300);
        std::fs::write(&path, format!("{SESSION}{filler}")).unwrap();

        rename_pi_session(&path, "Late name").unwrap();

        assert_eq!(pi_title(&path).as_deref(), Some("Fix the flaky test"));
        assert_eq!(pi_latest_name(&path).as_deref(), Some("Late name"));
    }
}
