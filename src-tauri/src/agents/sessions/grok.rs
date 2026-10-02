use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde_json::Value;

use super::recent::{Found, Hit, Listed, PageScan};
use super::rename::now_iso8601;
use super::{condense, mtime_of, MAX_AGENT_TRANSCRIPTS_INSPECTED, MAX_AGENT_TRANSCRIPT_PATHS};
use crate::agents::config::grok_root;
use crate::agents::AgentSession;

fn collect_grok_session_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(groups) = fs::read_dir(root.join("sessions")) else {
        return out;
    };
    for group in groups
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
    {
        if out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
            break;
        }
        let Ok(sessions) = fs::read_dir(group) else {
            continue;
        };
        for session in sessions
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
        {
            if out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
                break;
            }
            if session.join("updates.jsonl").is_file()
                || session.join("chat_history.jsonl").is_file()
            {
                out.push(session);
            }
        }
    }
    out
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        let hex = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        };
        out.push(hex(*bytes.get(index + 1)?)? * 16 + hex(*bytes.get(index + 2)?)?);
        index += 3;
    }
    String::from_utf8(out).ok()
}

fn grok_group_cwd(group: &Path) -> Option<PathBuf> {
    let explicit = fs::read_to_string(group.join(".cwd")).ok();
    if let Some(cwd) = explicit
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
    {
        return Some(PathBuf::from(cwd));
    }
    let name = group.file_name()?.to_str()?;
    percent_decode(name)
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
}

fn grok_content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    content.as_array().map(|parts| {
        parts
            .iter()
            .filter_map(|part| {
                part.as_str()
                    .or_else(|| part.get("text").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("")
    })
}

fn grok_user_query(text: &str) -> &str {
    text.split_once("<user_query>")
        .and_then(|(_, rest)| rest.split_once("</user_query>"))
        .map(|(body, _)| body)
        .unwrap_or(text)
}

fn grok_first_user(session_dir: &Path) -> Option<String> {
    if let Ok(file) = fs::File::open(session_dir.join("updates.jsonl")) {
        let mut chunks = String::new();
        for line in BufReader::new(file).lines().take(300).map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(update) = value.pointer("/params/update") else {
                continue;
            };
            match update.get("sessionUpdate").and_then(Value::as_str) {
                Some("user_message_chunk") => {
                    if let Some(text) = update.get("content").and_then(grok_content_text) {
                        chunks.push_str(&text);
                    }
                }
                Some(_) if !chunks.is_empty() => break,
                _ => {}
            }
        }
        if let Some(title) = condense(grok_user_query(&chunks)) {
            return Some(title);
        }
    }
    let file = fs::File::open(session_dir.join("chat_history.jsonl")).ok()?;
    for line in BufReader::new(file).lines().take(300).map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("user")
            || value.get("synthetic_reason").is_some()
        {
            continue;
        }
        let text = value.get("content").and_then(grok_content_text)?;
        if let Some(title) = condense(grok_user_query(&text)) {
            return Some(title);
        }
    }
    None
}

fn grok_session_mtime(session_dir: &Path) -> u64 {
    ["summary.json", "updates.jsonl", "chat_history.jsonl"]
        .iter()
        .map(|name| mtime_of(&session_dir.join(name)))
        .max()
        .unwrap_or(0)
}

fn grok_session(session_dir: &Path, cwd: &str) -> Option<AgentSession> {
    grok_found(session_dir)
        .filter(|found| Path::new(&found.project) == Path::new(cwd))
        .map(|found| found.session)
}

/// The session in one folder and the project Grok recorded for it.
fn grok_found(session_dir: &Path) -> Option<Found> {
    let summary_path = session_dir.join("summary.json");
    let summary = fs::read_to_string(&summary_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let workspace = summary
        .as_ref()
        .and_then(|value| value.pointer("/info/cwd"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| session_dir.parent().and_then(grok_group_cwd))?;
    let id = summary
        .as_ref()
        .and_then(|value| value.pointer("/info/id"))
        .and_then(Value::as_str)
        .or_else(|| session_dir.file_name().and_then(|name| name.to_str()))?;
    let title = summary
        .as_ref()
        .and_then(|value| value.get("generated_title"))
        .and_then(Value::as_str)
        .and_then(condense)
        .or_else(|| {
            summary
                .as_ref()
                .and_then(|value| value.get("session_summary"))
                .and_then(Value::as_str)
                .and_then(condense)
        })
        .or_else(|| grok_first_user(session_dir))
        .unwrap_or_else(|| id.chars().take(13).collect());
    Some(Found {
        project: workspace.to_string_lossy().into_owned(),
        session: AgentSession {
            id: id.to_string(),
            title,
            mtime: grok_session_mtime(session_dir),
        },
    })
}

pub(super) fn grok_sessions(cwd: &str) -> Vec<AgentSession> {
    let Some(root) = grok_root() else {
        return Vec::new();
    };
    let mut dirs = collect_grok_session_dirs(&root);
    dirs.sort_unstable_by_key(|path| std::cmp::Reverse(grok_session_mtime(path)));
    dirs.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);
    let mut out: Vec<_> = dirs
        .par_iter()
        .filter_map(|path| grok_session(path, cwd))
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

pub(super) fn grok_recent(scan: &PageScan<'_>) -> Vec<Hit> {
    let Some(root) = grok_root() else {
        return Vec::new();
    };
    let listed = collect_grok_session_dirs(&root)
        .into_iter()
        .map(|dir| Listed {
            at_ms: grok_session_mtime(&dir) * 1000,
            key: dir.to_string_lossy().into_owned(),
            item: dir,
        })
        .collect();
    scan.collect(listed, |dir| grok_found(dir))
}

/// Names a session the way Grok's `/rename` does: a title marked manual, so generated titles never replace it.
pub(super) fn rename_grok_session(
    root: &Path,
    cwd: &str,
    session_id: &str,
    name: &str,
) -> Result<(), String> {
    let session_dir = collect_grok_session_dirs(root)
        .into_iter()
        .find(|dir| grok_session(dir, cwd).is_some_and(|session| session.id == session_id))
        .ok_or("session not found")?;
    let summary_path = session_dir.join("summary.json");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(session_dir.join("summary.json.lock"))
        .map_err(|error| error.to_string())?;
    lock_exclusive(&lock)?;
    let mut summary = fs::read_to_string(&summary_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .ok_or("session not found")?;
    summary["generated_title"] = Value::from(name);
    summary["title_is_manual"] = Value::Bool(true);
    summary["updated_at"] = Value::from(now_iso8601());
    if summary
        .get("session_summary")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        summary["session_summary"] = Value::from(name);
    }
    let text = serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?;
    let staged = session_dir.join("summary.json.tmp");
    fs::write(&staged, text).map_err(|error| error.to_string())?;
    fs::rename(&staged, &summary_path).map_err(|error| error.to_string())
}

/// Grok takes this lock around every change to a session's summary; it is released when the file closes.
#[cfg(unix)]
fn lock_exclusive(file: &fs::File) -> Result<(), String> {
    rustix::fs::flock(file, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn lock_exclusive(_file: &fs::File) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{grok_session, percent_decode, rename_grok_session};

    #[test]
    fn grok_summary_maps_workspace_title_and_resume_id() {
        let root = tempfile::tempdir().unwrap();
        let session = root.path().join("%2Frepo").join("session-1");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("summary.json"),
            r#"{"info":{"id":"session-1","cwd":"/repo"},"generated_title":"Fix flaky tests"}"#,
        )
        .unwrap();
        std::fs::write(session.join("updates.jsonl"), "{}\n").unwrap();

        let result = grok_session(&session, "/repo").unwrap();
        assert_eq!(result.id, "session-1");
        assert_eq!(result.title, "Fix flaky tests");
        assert_eq!(percent_decode("%2Frepo").as_deref(), Some("/repo"));
        assert!(grok_session(&session, "/another").is_none());
    }

    #[test]
    fn a_renamed_grok_session_is_pinned_as_a_manual_title() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("sessions")
            .join("%2Frepo")
            .join("session-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("updates.jsonl"), "").unwrap();
        std::fs::write(
            dir.join("summary.json"),
            r#"{"info":{"id":"session-1","cwd":"/repo"},"session_summary":"Fix flaky tests","generated_title":"Fix flaky tests","num_messages":2}"#,
        )
        .unwrap();

        rename_grok_session(root.path(), "/repo", "session-1", "Flaky test fix").unwrap();

        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["generated_title"], "Flaky test fix");
        assert_eq!(summary["title_is_manual"], true);
        assert_eq!(summary["session_summary"], "Fix flaky tests");
        assert_eq!(summary["num_messages"], 2);
        assert!(summary["updated_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(grok_session(&dir, "/repo").unwrap().title, "Flaky test fix");
        assert!(rename_grok_session(root.path(), "/elsewhere", "session-1", "Name").is_err());
    }
}
