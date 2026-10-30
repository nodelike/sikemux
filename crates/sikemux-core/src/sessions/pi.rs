use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde_json::Value;

use crate::agent_catalog::AgentKind;

use super::index::{indexed, Indexed};
use super::recent::{Found, Hit, Listed, PageScan};
use super::rename::{append_line, now_iso8601};
use super::search::Prose;
use super::{
    collect_transcripts, condense, file_stamp, is_jsonl, push_texts, read_suffix,
    stamped_transcripts, text_from_content, FileStamp, MAX_AGENT_TRANSCRIPTS_INSPECTED,
};

const PI_TAIL_BYTES: u64 = 128 * 1024;
use super::in_project;
use super::rename::session_file_within;
use super::roots::environment_path;
use super::search::each_line;
use super::AgentSession;
use crate::agent_accounts::defaults::{json_effort, qualify_model};
use crate::agent_accounts::status::{self, AgentStatus};
use crate::agent_ops::Pending;
use crate::agent_ops::{AgentOps, SessionPlaces};
use crate::launch::skills;
use crate::launch::tools::TerminalSetup;
use notify::RecursiveMode;
use std::collections::BTreeMap;
use std::future::ready;

// ---- pi — ~/.pi/agent/sessions/**/<session>.jsonl ----------------------
pub fn pi_session_dir() -> Option<PathBuf> {
    environment_path("PI_CODING_AGENT_SESSION_DIR")
        .or_else(|| Some(AgentKind::Pi.home_root()?.join("sessions")))
}

pub(super) fn pi_sessions(cwd: &str) -> Vec<AgentSession> {
    let Some(root) = pi_session_dir() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_transcripts(&root, is_jsonl, &mut files, 0);
    let mut files = stamped_transcripts(files.into_iter());
    files.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|(path, stamp)| {
            pi_session(path, *stamp)
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
    collect_transcripts(&root, is_jsonl, &mut files, 0);
    let listed = files
        .into_iter()
        .map(|path| {
            let stamp = file_stamp(&path);
            Listed {
                at_ms: stamp.unix_millis(),
                key: path.to_string_lossy().into_owned(),
                item: (path, stamp),
            }
        })
        .collect();
    scan.collect(listed, |(path, stamp)| pi_session(path, *stamp))
}

/// The session in one transcript and the project its header names.
pub(super) fn pi_session(path: &Path, stamp: FileStamp) -> Option<Found> {
    let read = indexed(AgentKind::Pi, path, stamp, || read_pi(path));
    let project = read.cwd.clone()?;
    let title = read.title.clone().unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
            .chars()
            .take(13)
            .collect()
    });
    Some(Found {
        project,
        session: AgentSession {
            id: path.to_string_lossy().to_string(),
            title,
            mtime: stamp.unix_secs(),
        },
        place: Some(path.to_path_buf()),
    })
}

fn read_pi(path: &Path) -> Indexed {
    let header = fs::File::open(path).ok().and_then(|file| {
        let mut first = String::new();
        BufReader::new(file).read_line(&mut first).ok()?;
        let header = serde_json::from_str::<Value>(first.trim()).ok()?;
        (header.get("type").and_then(|t| t.as_str()) == Some("session")).then_some(header)
    });
    let Some(header) = header else {
        return Indexed::default();
    };
    Indexed {
        id: None,
        cwd: header
            .get("cwd")
            .and_then(|c| c.as_str())
            .map(str::to_string),
        title: pi_latest_name(path)
            .or_else(|| pi_title(path))
            .or_else(|| header.get("id").and_then(|i| i.as_str()).and_then(condense)),
    }
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

/// What one Pi or OMP session line adds to what the person and the agent wrote.
pub(super) fn message_prose(line: &str, prose: &mut Prose) {
    if !line.contains("\"message\"") {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if value.get("type").and_then(Value::as_str) != Some("message") {
        return;
    }
    let Some(message) = value.get("message") else {
        return;
    };
    if matches!(
        message.get("role").and_then(Value::as_str),
        Some("user" | "assistant")
    ) {
        if let Some(content) = message.get("content") {
            push_texts(content, prose);
        }
    }
}

pub(crate) struct Pi;

impl AgentOps for Pi {
    fn sessions(&self, cwd: &str, _config_path: Option<&str>) -> Vec<AgentSession> {
        pi_sessions(cwd)
    }

    fn recent(&self, scan: &PageScan<'_>, _config_path: Option<&str>) -> Vec<Hit> {
        pi_recent(scan)
    }

    fn session_place(&self, path: &Path) -> Option<PathBuf> {
        is_jsonl(path).then(|| path.to_path_buf())
    }

    fn session_at(
        &self,
        place: &Path,
        cwd: &str,
        _config_path: Option<&str>,
    ) -> Option<AgentSession> {
        in_project(pi_session(place, file_stamp(place)), cwd)
    }

    fn session_places(&self, _cwd: &str, _config_path: Option<&str>) -> SessionPlaces {
        SessionPlaces {
            sessions: pi_session_dir()
                .map(|dir| (dir, RecursiveMode::Recursive))
                .into_iter()
                .collect(),
            listings: Vec::new(),
        }
    }

    fn prose(&self, found: &Found, prose: &mut Prose) -> Option<()> {
        each_line(found.place.as_deref()?, |line| message_prose(line, prose));
        Some(())
    }

    fn rename_saved(
        &self,
        _cwd: &str,
        session_id: &str,
        name: &str,
        _config_path: Option<&str>,
    ) -> Option<Result<(), String>> {
        let Some(root) = pi_session_dir() else {
            return Some(Err("session not found".to_string()));
        };
        Some(
            session_file_within(session_id, &[root])
                .and_then(|path| rename_pi_session(&path, name)),
        )
    }

    fn terminal_tools<'a>(
        &'a self,
        setup: &'a mut TerminalSetup<'_>,
    ) -> Pending<'a, Result<Vec<String>, String>> {
        Box::pin(ready(setup.extension_args(|data_dir| {
            skills::skill_args(data_dir, |dir| Ok(skills::pi_args(dir)))
        })))
    }

    fn default_model(&self, _config_path: Option<&str>) -> Option<String> {
        let settings = fs::read_to_string(AgentKind::Pi.home_root()?.join("settings.json")).ok()?;
        let value: Value = serde_json::from_str(&settings).ok()?;
        qualify_model(
            value.get("defaultProvider").and_then(Value::as_str),
            value.get("defaultModel").and_then(Value::as_str),
        )
    }

    fn default_effort(&self, _config_path: Option<&str>) -> Option<String> {
        json_effort(
            &AgentKind::Pi.home_root()?.join("settings.json"),
            "defaultThinkingLevel",
            "pi",
        )
    }

    fn sign_in_status<'a>(
        &self,
        program: &'a Path,
        config_path: Option<&'a str>,
        _providers: BTreeMap<String, String>,
    ) -> Pending<'a, AgentStatus> {
        Box::pin(status::pi_sign_in(program, config_path))
    }

    fn skills_folder(&self) -> Option<PathBuf> {
        Some(AgentKind::Pi.home_root()?.join("skills"))
    }
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
