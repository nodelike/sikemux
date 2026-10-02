use std::fs;
use std::path::Path;

use rayon::prelude::*;
use serde_json::Value;

use super::recent::{Found, Hit, Listed, PageScan};
use super::rename::append_line;
use super::{
    cached_title, condense, read_prefix, read_suffix, stamped_transcripts, text_from_content,
    title_cache_stamp, MAX_AGENT_TRANSCRIPTS_INSPECTED,
};
use crate::agents::config::agent_config_root;
use crate::agents::AgentSession;

// ---- claude — ~/.claude/projects/<cwd-dashed>/<uuid>.jsonl --------------
pub(super) fn claude_sessions(cwd: &str, config_path: Option<&str>) -> Vec<AgentSession> {
    let Some(root) = agent_config_root("claude", config_path) else {
        return Vec::new();
    };
    let dir = root.join("projects").join(cwd.replace('/', "-"));
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths = stamped_transcripts(
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl")),
    );
    paths.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);
    // Titles come from reading each transcript, so fan the per-file work out
    // across rayon's pool instead of scanning sessions one at a time.
    let mut out: Vec<AgentSession> = paths
        .par_iter()
        .filter_map(|(path, stamp)| {
            let id = path.file_stem().and_then(|s| s.to_str())?;
            let title = cached_title(path, *stamp, || claude_title(path))?;
            Some(AgentSession {
                id: id.to_string(),
                title,
                mtime: stamp.unix_secs(),
            })
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

/// Each project has its own transcript folder, so only the projects asked for are read.
pub(super) fn claude_recent(scan: &PageScan<'_>, config_path: Option<&str>) -> Vec<Hit> {
    let Some(root) = agent_config_root("claude", config_path) else {
        return Vec::new();
    };
    let mut listed = Vec::new();
    for project in scan.projects() {
        let Ok(entries) = fs::read_dir(root.join("projects").join(project.replace('/', "-")))
        else {
            continue;
        };
        for path in entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        {
            let stamp = title_cache_stamp(&path);
            listed.push(Listed {
                at_ms: stamp.unix_millis(),
                key: path.to_string_lossy().into_owned(),
                item: (path, stamp, project.clone()),
            });
        }
    }
    scan.collect(listed, |(path, stamp, project)| {
        let id = path.file_stem().and_then(|s| s.to_str())?;
        let title = cached_title(path, *stamp, || claude_title(path))?;
        Some(Found {
            project: project.clone(),
            session: AgentSession {
                id: id.to_string(),
                title,
                mtime: stamp.unix_secs(),
            },
        })
    })
}

#[derive(Default)]
struct ClaudeTitles {
    custom: Option<String>,
    ai: Option<String>,
    first_user: Option<String>,
    /// Text Claude Code wrote into the conversation itself, such as a message
    /// relayed from another session. It names a session only when nothing was typed.
    first_harness: Option<String>,
}

impl ClaudeTitles {
    /// `custom-title` is what `/rename` and the Claude desktop app write, so it
    /// outranks the title Claude generates for itself.
    fn resolve(self) -> Option<String> {
        self.custom
            .or(self.ai)
            .or(self.first_user)
            .or(self.first_harness)
    }
}

/// Pull a stored title out of one transcript line. Both kinds are appended as the
/// session grows, so the last one wins.
fn scan_claude_title_line(line: &str, titles: &mut ClaudeTitles) {
    if line.contains("\"type\":\"custom-title\"") {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(t) = v
                .get("customTitle")
                .and_then(|t| t.as_str())
                .and_then(condense)
            {
                titles.custom = Some(t);
            }
        }
    } else if line.contains("\"type\":\"ai-title\"") {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(t) = v.get("aiTitle").and_then(|t| t.as_str()).and_then(condense) {
                titles.ai = Some(t);
            }
        }
    }
}

/// As above, plus `first_user` (first write wins) for sessions with no stored title.
fn scan_claude_line(line: &str, titles: &mut ClaudeTitles) {
    scan_claude_title_line(line, titles);
    if titles.first_user.is_none() && line.contains("\"type\":\"user\"") {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("user") {
                let text = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(text_from_content)
                    .and_then(|text| condense(&text));
                if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    titles.first_harness = titles.first_harness.take().or(text);
                } else {
                    titles.first_user = text;
                }
            }
        }
    }
}

/// Names a session the way Claude's `/rename` does.
pub(super) fn rename_claude_session(
    path: &Path,
    session_id: &str,
    name: &str,
) -> Result<(), String> {
    let line = format!(
        r#"{{"type":"custom-title","customTitle":{},"sessionId":{}}}"#,
        Value::from(name),
        Value::from(session_id),
    );
    append_line(path, &line)
}

// Bound the per-file read: the first user prompt sits near the top and Claude
// emits `ai-title` entries continuously (so the freshest title sits at the end),
// which lets us skip the middle of multi-MB transcripts.
pub(super) const CLAUDE_HEAD_BYTES: u64 = 128 * 1024;
pub(super) const CLAUDE_TAIL_BYTES: u64 = 128 * 1024;

fn claude_title(path: &Path) -> Option<String> {
    // Cheap substring guards keep us from JSON-parsing every line.
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    let mut titles = ClaudeTitles::default();

    // Head: captures the first user prompt and any early title. For small
    // transcripts this covers the whole file, keeping the result exact.
    let head = read_prefix(&mut file, CLAUDE_HEAD_BYTES.min(len))?;
    for line in head.lines() {
        scan_claude_line(line, &mut titles);
    }

    // Tail: the freshest title lives at the end of large transcripts. Skip the
    // first (likely partial) line.
    if len > CLAUDE_HEAD_BYTES {
        if let Some(tail) = read_suffix(&mut file, len.saturating_sub(CLAUDE_TAIL_BYTES)) {
            for line in tail.lines().skip(1) {
                scan_claude_title_line(line, &mut titles);
            }
        }
    }

    titles.resolve()
}

#[cfg(test)]
mod tests {
    use super::{claude_sessions, CLAUDE_HEAD_BYTES};

    #[test]
    fn claude_sessions_omit_titleless_command_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let sessions_dir = root.path().join("projects").join("-repo");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("conversation.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"Fix top bar overlaps\"}}\n",
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("usage-command.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"<command-name>/usage</command-name>\"}}\n",
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("cancelled-resume.jsonl"),
            "{\"type\":\"system\",\"subtype\":\"local_command\",\"content\":\"<command-name>/resume</command-name>\"}\n",
        )
        .unwrap();

        let sessions = claude_sessions("/repo", root.path().to_str());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "conversation");
        assert_eq!(sessions[0].title, "Fix top bar overlaps");
    }

    #[test]
    fn claude_titles_skip_text_the_harness_wrote_before_the_first_prompt() {
        let root = tempfile::tempdir().unwrap();
        let sessions_dir = root.path().join("projects").join("-repo");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let skill = "{\"type\":\"user\",\"isMeta\":true,\"message\":{\"content\":\"Base directory for this skill: /skills/x\"}}\n";
        std::fs::write(
            sessions_dir.join("typed.jsonl"),
            format!(
                "{skill}{{\"type\":\"user\",\"message\":{{\"content\":\"Polish the rail\"}}}}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("relayed.jsonl"),
            "{\"type\":\"user\",\"isMeta\":true,\"message\":{\"content\":\"Another Claude session sent a message\"}}\n",
        )
        .unwrap();

        let sessions = claude_sessions("/repo", root.path().to_str());
        let title = |id: &str| {
            sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.title.as_str())
        };
        assert_eq!(title("typed"), Some("Polish the rail"));
        assert_eq!(
            title("relayed"),
            Some("Another Claude session sent a message")
        );
    }

    #[test]
    fn claude_titles_prefer_the_stored_title_over_the_generated_one() {
        let root = tempfile::tempdir().unwrap();
        let sessions_dir = root.path().join("projects").join("-repo");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("renamed.jsonl"),
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"Rate this ContentIQ project codebase\"}}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Codebase rating\"}\n",
                "{\"type\":\"custom-title\",\"customTitle\":\"ContentIQ codebase quality assessment\"}\n",
            ),
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("generated.jsonl"),
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"check jira whats amitoj working on?\"}}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Amitoj work status\"}\n",
            ),
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("untitled.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"What happens in this stage?\"}}\n",
        )
        .unwrap();

        let sessions = claude_sessions("/repo", root.path().to_str());
        let title = |id: &str| {
            sessions
                .iter()
                .find(|session| session.id == id)
                .map(|session| session.title.as_str())
        };

        assert_eq!(
            title("renamed"),
            Some("ContentIQ codebase quality assessment")
        );
        assert_eq!(title("generated"), Some("Amitoj work status"));
        assert_eq!(title("untitled"), Some("What happens in this stage?"));
    }

    #[test]
    fn claude_titles_find_a_rename_past_the_head_of_a_long_transcript() {
        let root = tempfile::tempdir().unwrap();
        let sessions_dir = root.path().join("projects").join("-repo");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let mut transcript =
            String::from("{\"type\":\"user\",\"message\":{\"content\":\"Handoff details\"}}\n");
        while transcript.len() < (CLAUDE_HEAD_BYTES as usize) + 4096 {
            transcript.push_str("{\"type\":\"assistant\",\"message\":{\"content\":\"filler\"}}\n");
        }
        transcript
            .push_str("{\"type\":\"custom-title\",\"customTitle\":\"SwishX ContentIQ V2 manual test pass\"}\n");
        std::fs::write(sessions_dir.join("long.jsonl"), transcript).unwrap();

        let sessions = claude_sessions("/repo", root.path().to_str());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "SwishX ContentIQ V2 manual test pass");
    }
}
