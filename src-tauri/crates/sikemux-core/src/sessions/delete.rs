use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use tokio::task::spawn_blocking;

use super::context::session_transcript_path;
use super::rename::agent_executable;
use super::roots::account_root;
use crate::agent_catalog::AgentKind;

// ---- delete a saved session ----------------------------------------------
/// Removes a session from the provider's own storage for good,
/// so it no longer lists anywhere and can no longer be resumed.
pub async fn delete(
    agent: AgentKind,
    cwd: String,
    session_id: String,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<(), String> {
    let command = agent.ops().delete_command().and_then(|command| {
        let executable = agent_executable(agent, executable_path.as_deref()).ok()?;
        Some((command, executable))
    });
    let Some((command, executable)) = command else {
        return spawn_blocking(move || {
            delete_session_files(agent, &cwd, &session_id, config_path.as_deref())
        })
        .await
        .map_err(|error| error.to_string())?;
    };
    let (id, config) = (session_id.clone(), config_path.clone());
    spawn_blocking(move || saved_transcript(agent, &cwd, &id, config.as_deref()))
        .await
        .map_err(|error| error.to_string())??;
    command(&executable, config_path.as_deref(), &session_id).await
}

/// The session's transcript and the provider's folder it sits in, refusing any
/// path that would lead outside that folder.
pub(super) fn saved_transcript(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Result<(PathBuf, PathBuf), String> {
    let entry = agent.entry();
    let folder = entry
        .home
        .as_ref()
        .and_then(|home| home.sessions)
        .filter(|_| entry.deletable_chats)
        .ok_or_else(|| format!("{} chats cannot be deleted", entry.label))?;
    let root = account_root(agent, config_path, &BTreeMap::new()).ok_or("session not found")?;
    let transcript = session_transcript_path(agent, cwd, session_id, config_path)
        .and_then(|path| fs::canonicalize(path).ok())
        .filter(|path| path.is_file())
        .ok_or("session not found")?;
    let inside = fs::canonicalize(root.join(folder)).is_ok_and(|dir| transcript.starts_with(dir));
    if !inside {
        return Err("session not found".to_string());
    }
    Ok((root, transcript))
}

fn delete_session_files(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Result<(), String> {
    let (root, transcript) = saved_transcript(agent, cwd, session_id, config_path)?;
    fs::remove_file(&transcript).map_err(|error| error.to_string())?;
    agent.ops().delete_leftovers(&root, &transcript, session_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{delete, delete_session_files, AgentKind};
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

        delete_session_files(AgentKind::Claude, "/repo", "abc-123", config).unwrap();

        assert!(!base.join("projects/-repo/abc-123.jsonl").exists());
        assert!(!base.join("projects/-repo/abc-123").exists());
        assert!(!base.join("file-history/abc-123").exists());
        assert!(!base.join("todos/abc-123-agent-abc-123.json").exists());
        assert!(base.join("projects/-repo/other.jsonl").exists());
        assert!(base.join("todos/other-agent-other.json").exists());
    }

    #[test]
    fn without_codex_a_deleted_codex_session_removes_only_its_rollout() {
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

        delete_session_files(AgentKind::Codex, "/repo", "abc-123", base.to_str()).unwrap();

        assert!(!day
            .join("rollout-2026-10-10T09-00-00-abc-123.jsonl")
            .exists());
        assert!(day
            .join("rollout-2026-10-10T10-00-00-def-456.jsonl")
            .exists());
    }

    #[test]
    fn a_codex_session_is_deleted_by_codex_itself() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        let rollout = base.join("sessions/2026/10/10/rollout-2026-10-10T09-00-00-abc-123.jsonl");
        write(&rollout, "{}\n");
        let codex = base.join("codex");
        write(
            &codex,
            concat!(
                "#!/bin/sh\n",
                "while IFS= read -r line; do\n",
                "  case \"$line\" in\n",
                "    *'\"method\":\"initialize\"'*) echo '{\"id\":1,\"result\":{}}' ;;\n",
                "    *'\"id\":2'*) echo \"$line\" > \"$0.asked\"; echo '{\"id\":2,\"result\":{}}' ;;\n",
                "  esac\n",
                "done\n",
            ),
        );
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(delete(
                AgentKind::Codex,
                "/repo".into(),
                "abc-123".into(),
                codex.to_str().map(String::from),
                base.to_str().map(String::from),
            ))
            .unwrap();

        let asked: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(base.join("codex.asked")).unwrap()).unwrap();
        assert_eq!(asked["method"], "thread/delete");
        assert_eq!(asked["params"]["threadId"], "abc-123");
    }

    #[test]
    fn a_delete_never_reaches_outside_the_provider_sessions() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        let config = base.to_str();
        write(&base.join("projects/-repo/abc-123.jsonl"), "{}\n");
        write(&base.join("secret.jsonl"), "{}\n");

        assert!(delete_session_files(AgentKind::Claude, "/repo", "missing", config).is_err());
        assert!(delete_session_files(AgentKind::Claude, "/repo", "../secret", config).is_err());
        assert!(delete_session_files(AgentKind::Claude, "/", "../secret", config).is_err());
        assert!(delete_session_files(AgentKind::Pi, "/repo", "abc-123", config).is_err());
        assert!(base.join("secret.jsonl").exists());
        assert!(base.join("projects/-repo/abc-123.jsonl").exists());
    }
}
