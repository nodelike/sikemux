use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::async_runtime::spawn_blocking;

use super::context::session_transcript_path;
use super::{condense, MAX_TITLE_CHARS};
use crate::agents::config::{grok_root, omp_session_dirs};
use crate::agents::executable::expand_user_path;
use crate::agents::{allowed_agent_path, AgentKind};

// ---- rename a saved session ----------------------------------------------
/// Names a session the way the provider's own rename does, so the new name
/// shows wherever the session is listed, including the provider's resume picker.
#[tauri::command]
pub async fn agent_session_rename(
    agent: AgentKind,
    cwd: String,
    session_id: String,
    title: String,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<(), String> {
    let name = session_name(&title)?;
    match agent {
        AgentKind::Codex => {
            let executable = agent_executable(agent, executable_path.as_deref())?;
            super::codex::rename_codex_session(
                &executable,
                config_path.as_deref(),
                &session_id,
                &name,
            )
            .await
        }
        AgentKind::Hermes => {
            let executable = agent_executable(agent, executable_path.as_deref())?;
            super::hermes::rename_hermes_session(&executable, &session_id, &name).await
        }
        _ => spawn_blocking(move || {
            rename_session(agent, &cwd, &session_id, &name, config_path.as_deref())
        })
        .await
        .map_err(|error| error.to_string())?,
    }
}

fn agent_executable(agent: AgentKind, configured: Option<&str>) -> Result<PathBuf, String> {
    configured
        .map(expand_user_path)
        .or_else(|| {
            crate::system::find_executable_matching(agent.as_str(), |candidate| {
                allowed_agent_path(agent.as_str(), candidate)
            })
        })
        .ok_or_else(|| format!("{} is not available", agent.as_str()))
}

/// The name as sessions are listed: runs of whitespace collapsed, and short
/// enough that the listing shows all of it.
fn session_name(title: &str) -> Result<String, String> {
    let name = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let valid = name.chars().count() <= MAX_TITLE_CHARS && !name.chars().any(char::is_control);
    condense(&name)
        .filter(|_| valid)
        .ok_or_else(|| "invalid session name".to_string())
}

/// Renames a session whose provider keeps its name in files or a database Sikemux can write.
fn rename_session(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    title: &str,
    config_path: Option<&str>,
) -> Result<(), String> {
    let name = session_name(title)?;
    match agent {
        AgentKind::Claude => {
            let path = session_transcript_path(agent, cwd, session_id, config_path)
                .filter(|path| path.is_file())
                .ok_or("session not found")?;
            super::claude::rename_claude_session(&path, session_id, &name)
        }
        AgentKind::Pi => {
            let root = super::pi::pi_session_dir().ok_or("session not found")?;
            super::pi::rename_pi_session(&session_file_within(session_id, &[root])?, &name)
        }
        AgentKind::Omp => {
            let path = session_file_within(session_id, &omp_session_dirs())?;
            super::omp::rename_omp_session(&path, &name, super::omp::omp_title_index().as_deref())
        }
        AgentKind::Grok => super::grok::rename_grok_session(
            &grok_root().ok_or("session not found")?,
            cwd,
            session_id,
            &name,
        ),
        AgentKind::Opencode => super::opencode::rename_opencode_session(session_id, &name),
        AgentKind::Codex | AgentKind::Hermes => Err(format!(
            "{} renames through its own command",
            agent.as_str()
        )),
    }
}

/// Pi and OMP name a session by its file's path, which must sit inside one of their session folders.
fn session_file_within(session_id: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let path = fs::canonicalize(session_id).map_err(|_| "session not found".to_string())?;
    let inside = roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| path.starts_with(root));
    if inside && path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
        Ok(path)
    } else {
        Err("session not found".to_string())
    }
}

/// Adds one whole line, starting it on a fresh line if the file does not end with one.
pub(super) fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let mut last = [0u8; 1];
    let ends_mid_line = file.seek(SeekFrom::End(-1)).is_ok()
        && file.read_exact(&mut last).is_ok()
        && last[0] != b'\n';
    let text = if ends_mid_line {
        format!("\n{line}\n")
    } else {
        format!("{line}\n")
    };
    file.write_all(text.as_bytes())
        .map_err(|error| error.to_string())
}

pub(super) fn now_iso8601() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    crate::autopsy::iso8601(ms)
}

#[cfg(test)]
mod tests {
    use super::super::claude::claude_sessions;
    use super::{rename_session, AgentKind};

    fn transcript(root: &std::path::Path, body: &str) -> std::path::PathBuf {
        let dir = root.join("projects").join("-repo");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc-123.jsonl");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn a_renamed_claude_session_lists_under_its_new_name() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().to_str();
        let path = transcript(
            root.path(),
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"fix the parser\"}}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Parser fixes\"}\n",
            ),
        );

        rename_session(
            AgentKind::Claude,
            "/repo",
            "abc-123",
            "  Parser rewrite  ",
            config,
        )
        .unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.ends_with(
            "{\"type\":\"custom-title\",\"customTitle\":\"Parser rewrite\",\"sessionId\":\"abc-123\"}\n"
        ));
        assert_eq!(claude_sessions("/repo", config)[0].title, "Parser rewrite");
    }

    #[test]
    fn a_transcript_cut_mid_line_gets_the_name_on_a_line_of_its_own() {
        let root = tempfile::tempdir().unwrap();
        let path = transcript(
            root.path(),
            "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}",
        );

        rename_session(
            AgentKind::Claude,
            "/repo",
            "abc-123",
            "Greeting",
            root.path().to_str(),
        )
        .unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 2);
        assert!(body
            .lines()
            .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()));
    }

    #[test]
    fn a_rename_never_creates_or_escapes_a_transcript() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().to_str();
        transcript(root.path(), "");

        assert!(rename_session(AgentKind::Claude, "/repo", "missing", "Name", config).is_err());
        assert!(rename_session(AgentKind::Claude, "/repo", "../abc-123", "Name", config).is_err());
        assert!(rename_session(AgentKind::Claude, "/repo", "abc-123", "   ", config).is_err());
        assert!(rename_session(AgentKind::Claude, "/repo", "abc-123", "<tag>", config).is_err());
        let too_long = "x".repeat(73);
        assert!(rename_session(AgentKind::Claude, "/repo", "abc-123", &too_long, config).is_err());
        assert!(rename_session(AgentKind::Codex, "/repo", "abc-123", "Name", config).is_err());
        assert!(!root.path().join("projects/-repo/missing.jsonl").exists());
    }
}
