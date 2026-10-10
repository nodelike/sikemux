use std::fs;
use std::path::Path;

use tauri::async_runtime::spawn_blocking;

use super::context::session_transcript_path;
use crate::agents::config::agent_config_root;
use crate::agents::AgentKind;

// ---- delete a saved session ----------------------------------------------
/// Removes a Claude or Codex session from the provider's own storage for good,
/// so it no longer lists anywhere and can no longer be resumed.
#[tauri::command]
pub async fn agent_session_delete(
    agent: AgentKind,
    cwd: String,
    session_id: String,
    config_path: Option<String>,
) -> Result<(), String> {
    spawn_blocking(move || delete_session(agent, &cwd, &session_id, config_path.as_deref()))
        .await
        .map_err(|error| error.to_string())?
}

fn delete_session(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Result<(), String> {
    let (kind, folder) = match agent {
        AgentKind::Claude => ("claude", "projects"),
        AgentKind::Codex => ("codex", "sessions"),
        _ => return Err(format!("{} chats cannot be deleted", agent.as_str())),
    };
    let root = agent_config_root(kind, config_path).ok_or("session not found")?;
    let transcript = session_transcript_path(agent, cwd, session_id, config_path)
        .and_then(|path| fs::canonicalize(path).ok())
        .filter(|path| path.is_file())
        .ok_or("session not found")?;
    let inside = fs::canonicalize(root.join(folder)).is_ok_and(|dir| transcript.starts_with(dir));
    if !inside {
        return Err("session not found".to_string());
    }
    fs::remove_file(&transcript).map_err(|error| error.to_string())?;
    if matches!(agent, AgentKind::Claude) {
        remove_claude_leftovers(&root, &transcript, session_id);
    }
    Ok(())
}

/// Claude keeps a session's subagent transcripts, file snapshots and todo lists
/// beside the transcript, each named by the session id.
fn remove_claude_leftovers(root: &Path, transcript: &Path, session_id: &str) {
    if let Some(project) = transcript.parent() {
        let _ = fs::remove_dir_all(project.join(session_id));
    }
    let _ = fs::remove_dir_all(root.join("file-history").join(session_id));
    let Ok(todos) = fs::read_dir(root.join("todos")) else {
        return;
    };
    let prefix = format!("{session_id}-");
    for entry in todos.flatten() {
        let name = entry.file_name();
        let ours = name
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json"));
        if ours {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{delete_session, AgentKind};
    use std::fs;
    use std::path::Path;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn a_deleted_claude_session_takes_its_leftovers_with_it() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        let config = base.to_str();
        write(&base.join("projects/-repo/abc-123.jsonl"), "{}\n");
        write(
            &base.join("projects/-repo/abc-123/subagents/agent-1.jsonl"),
            "{}\n",
        );
        write(&base.join("projects/-repo/other.jsonl"), "{}\n");
        write(&base.join("file-history/abc-123/snapshot"), "x");
        write(&base.join("todos/abc-123-agent-abc-123.json"), "[]");
        write(&base.join("todos/other-agent-other.json"), "[]");

        delete_session(AgentKind::Claude, "/repo", "abc-123", config).unwrap();

        assert!(!base.join("projects/-repo/abc-123.jsonl").exists());
        assert!(!base.join("projects/-repo/abc-123").exists());
        assert!(!base.join("file-history/abc-123").exists());
        assert!(!base.join("todos/abc-123-agent-abc-123.json").exists());
        assert!(base.join("projects/-repo/other.jsonl").exists());
        assert!(base.join("todos/other-agent-other.json").exists());
    }

    #[test]
    fn a_deleted_codex_session_removes_only_its_rollout() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        let day = base.join("sessions/2026/10/10");
        write(
            &day.join("rollout-2026-10-10T09-00-00-abc-123.jsonl"),
            "{}\n",
        );
        write(
            &day.join("rollout-2026-10-10T10-00-00-def-456.jsonl"),
            "{}\n",
        );

        delete_session(AgentKind::Codex, "/repo", "abc-123", base.to_str()).unwrap();

        assert!(!day
            .join("rollout-2026-10-10T09-00-00-abc-123.jsonl")
            .exists());
        assert!(day
            .join("rollout-2026-10-10T10-00-00-def-456.jsonl")
            .exists());
    }

    #[test]
    fn a_delete_never_reaches_outside_the_provider_sessions() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        let config = base.to_str();
        write(&base.join("projects/-repo/abc-123.jsonl"), "{}\n");
        write(&base.join("secret.jsonl"), "{}\n");

        assert!(delete_session(AgentKind::Claude, "/repo", "missing", config).is_err());
        assert!(delete_session(AgentKind::Claude, "/repo", "../secret", config).is_err());
        assert!(delete_session(AgentKind::Claude, "/", "../secret", config).is_err());
        assert!(delete_session(AgentKind::Pi, "/repo", "abc-123", config).is_err());
        assert!(base.join("secret.jsonl").exists());
        assert!(base.join("projects/-repo/abc-123.jsonl").exists());
    }
}
