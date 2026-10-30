use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::roots::account_root;
use super::transcript;
use super::{decoded_window, is_compressed, read_suffix};
use crate::agent_catalog::AgentKind;

// ---- context window of a saved session ---------------------------------
/// How full a session's context window was when its transcript was last written.
/// Claude does not record the window's size, so `size` is only known for Codex.
#[derive(Serialize, Debug, PartialEq)]
pub struct SessionContext {
    pub used: u64,
    pub size: Option<u64>,
}

/// A resumed chat hears nothing about its context window until it runs a turn,
/// so its transcript on disk fills the gap.
pub fn read_session_context(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Option<SessionContext> {
    let path = session_transcript_path(agent, cwd, session_id, config_path)?;
    agent.ops().context(&path)
}

/// Where `agent` writes a session's transcript, for agents whose transcripts Sikemux reads.
pub fn session_transcript_path(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Option<PathBuf> {
    let root = account_root(agent, config_path, &BTreeMap::new())?;
    transcript::find(agent, &root, Some(cwd), session_id)
}

/// The last line of a transcript that `parse` accepts, read from the end so a
/// long session costs its tail rather than its whole history.
pub(super) fn last_line_matching<T>(path: &Path, parse: impl Fn(&str) -> Option<T>) -> Option<T> {
    const FIRST_WINDOW: u64 = 256 * 1024;
    const LAST_WINDOW: u64 = 32 * 1024 * 1024;
    if is_compressed(path) {
        let (window, start, _) = decoded_window(path, None, LAST_WINDOW)?;
        let text = String::from_utf8_lossy(&window);
        let mut lines = text.lines();
        if start > 0 {
            lines.next();
        }
        return lines.rev().find_map(parse);
    }
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
