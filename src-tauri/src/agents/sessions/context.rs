use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tauri::async_runtime::spawn_blocking;

use super::{collect_jsonl, read_suffix};
use crate::agents::config::agent_config_root;
use crate::agents::AgentKind;

// ---- context window of a saved session ---------------------------------
/// How full a session's context window was when its transcript was last written.
/// Claude does not record the window's size, so `size` is only known for Codex.
#[derive(Serialize, Debug, PartialEq)]
pub struct SessionContext {
    used: u64,
    size: Option<u64>,
}

/// A resumed chat hears nothing about its context window until it runs a turn,
/// so its transcript on disk fills the gap.
#[tauri::command]
pub async fn agent_session_context(
    agent: AgentKind,
    cwd: String,
    session_id: String,
    config_path: Option<String>,
) -> Option<SessionContext> {
    spawn_blocking(move || read_session_context(agent, &cwd, &session_id, config_path.as_deref()))
        .await
        .ok()
        .flatten()
}

fn read_session_context(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Option<SessionContext> {
    let path = session_transcript_path(agent, cwd, session_id, config_path)?;
    match agent {
        AgentKind::Claude => last_line_matching(&path, claude_context_line),
        AgentKind::Codex => last_line_matching(&path, codex_context_line),
        _ => None,
    }
}

/// Where Claude or Codex writes a session's transcript. Other agents keep
/// their history in shapes Sikemux does not read line by line.
pub(crate) fn session_transcript_path(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Option<PathBuf> {
    let safe_id = !session_id.is_empty()
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !safe_id {
        return None;
    }
    match agent {
        AgentKind::Claude => {
            let root = agent_config_root("claude", config_path)?;
            Some(
                root.join("projects")
                    .join(cwd.replace('/', "-"))
                    .join(format!("{session_id}.jsonl")),
            )
        }
        AgentKind::Codex => {
            let root = agent_config_root("codex", config_path)?;
            let mut paths = Vec::new();
            collect_jsonl(&root.join("sessions"), &mut paths, 0);
            let suffix = format!("-{session_id}.jsonl");
            paths.into_iter().find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&suffix))
            })
        }
        _ => None,
    }
}

/// The last line of a transcript that `parse` accepts, read from the end so a
/// long session costs its tail rather than its whole history.
fn last_line_matching<T>(path: &Path, parse: impl Fn(&str) -> Option<T>) -> Option<T> {
    const FIRST_WINDOW: u64 = 256 * 1024;
    const LAST_WINDOW: u64 = 32 * 1024 * 1024;
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut window = FIRST_WINDOW;
    loop {
        let start = len.saturating_sub(window);
        let text = read_suffix(&mut file, start)?;
        let mut lines = text.lines().rev().peekable();
        while let Some(line) = lines.next() {
            let partial = start > 0 && lines.peek().is_none();
            if partial {
                break;
            }
            if let Some(found) = parse(line) {
                return Some(found);
            }
        }
        if start == 0 || window >= LAST_WINDOW {
            return None;
        }
        window = (window * 4).min(LAST_WINDOW);
    }
}

/// Mirrors the Claude ACP adapter: a main-thread assistant message fills the
/// window with everything it read and wrote.
fn claude_context_line(line: &str) -> Option<SessionContext> {
    if !line.contains("\"type\":\"assistant\"") {
        return None;
    }
    let value = serde_json::from_str::<Value>(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("assistant")
        || value.get("isSidechain").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let message = value.get("message")?;
    if message.get("model").and_then(Value::as_str) == Some("<synthetic>") {
        return None;
    }
    let usage = message.get("usage")?;
    let used = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
    ]
    .iter()
    .map(|key| usage.get(key).and_then(Value::as_u64).unwrap_or(0))
    .sum();
    Some(SessionContext { used, size: None })
}

/// Mirrors the Codex ACP adapter: the last turn's total against the model's window.
fn codex_context_line(line: &str) -> Option<SessionContext> {
    if !line.contains("\"token_count\"") {
        return None;
    }
    let value = serde_json::from_str::<Value>(line).ok()?;
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let info = payload.get("info")?;
    let used = info
        .get("last_token_usage")?
        .get("total_tokens")?
        .as_u64()?;
    let size = info
        .get("model_context_window")?
        .as_u64()
        .filter(|size| *size > 0)?;
    Some(SessionContext {
        used,
        size: Some(size),
    })
}

#[cfg(test)]
mod tests {
    use super::{read_session_context, AgentKind, SessionContext};

    #[test]
    fn a_saved_claude_session_reports_its_last_main_thread_turn() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("projects").join("-repo");
        std::fs::create_dir_all(&dir).unwrap();
        let turn = |sidechain: bool, model: &str, read: u64| {
            format!(
                r#"{{"type":"assistant","isSidechain":{sidechain},"message":{{"model":"{model}","usage":{{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":{read},"output_tokens":40}}}}}}"#
            )
        };
        let lines = [
            turn(false, "claude-opus-5-5", 50_000),
            turn(true, "claude-haiku-4-5", 9),
            turn(false, "<synthetic>", 0),
            r#"{"type":"user","message":{"content":"next"}}"#.to_string(),
        ];
        std::fs::write(dir.join("abc-123.jsonl"), lines.join("\n") + "\n").unwrap();

        let context =
            read_session_context(AgentKind::Claude, "/repo", "abc-123", root.path().to_str());
        assert_eq!(
            context,
            Some(SessionContext {
                used: 50_142,
                size: None
            })
        );
        assert_eq!(
            read_session_context(
                AgentKind::Claude,
                "/repo",
                "../abc-123",
                root.path().to_str()
            ),
            None
        );
    }

    #[test]
    fn a_saved_codex_session_reports_its_last_turn_against_the_model_window() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("sessions")
            .join("2026")
            .join("09")
            .join("24");
        std::fs::create_dir_all(&dir).unwrap();
        let count = |total: u64| {
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"total_tokens":{total}}},"model_context_window":258400}}}}}}"#
            )
        };
        let lines = [
            r#"{"type":"session_meta","payload":{"id":"0199-abc","cwd":"/repo"}}"#.to_string(),
            count(40_000),
            count(96_852),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#.to_string(),
        ];
        std::fs::write(
            dir.join("rollout-2026-09-24T04-00-00-0199-abc.jsonl"),
            lines.join("\n") + "\n",
        )
        .unwrap();

        let context =
            read_session_context(AgentKind::Codex, "/repo", "0199-abc", root.path().to_str());
        assert_eq!(
            context,
            Some(SessionContext {
                used: 96_852,
                size: Some(258_400)
            })
        );
    }
}
