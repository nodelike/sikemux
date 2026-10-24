use std::fs::{self, File};
use std::path::Path;

use tokio::task::spawn_blocking;

use super::claude::claude_project_dir;
use super::delete::saved_transcript;
use crate::agent_catalog::AgentKind;

// ---- move a saved session to another project -----------------------------
/// Files a stopped Claude or Codex session under another project folder, so it
/// lists there and resumes with that folder as its working directory.
pub async fn relocate(
    agent: AgentKind,
    from_cwd: String,
    to_cwd: String,
    session_id: String,
    config_path: Option<String>,
) -> Result<(), String> {
    spawn_blocking(move || {
        relocate_files(
            agent,
            &from_cwd,
            &to_cwd,
            &session_id,
            config_path.as_deref(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

fn relocate_files(
    agent: AgentKind,
    from_cwd: &str,
    to_cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Result<(), String> {
    if from_cwd == to_cwd {
        return Ok(());
    }
    let (root, transcript) = saved_transcript(agent, from_cwd, session_id, config_path)?;
    if !matches!(agent, AgentKind::Claude) {
        return rewrite(&transcript, &transcript, from_cwd, to_cwd);
    }
    let project = claude_project_dir(&root, to_cwd);
    fs::create_dir_all(&project).map_err(|error| error.to_string())?;
    let target = project.join(format!("{session_id}.jsonl"));
    rewrite(&transcript, &target, from_cwd, to_cwd)?;
    if target != transcript {
        fs::remove_file(&transcript).map_err(|error| error.to_string())?;
    }
    let Some(old_project) = transcript.parent() else {
        return Ok(());
    };
    // Subagent transcripts sit in a folder named by the session, beside it.
    let (leftovers, moved) = (old_project.join(session_id), project.join(session_id));
    if leftovers.is_dir() && !moved.exists() {
        fs::rename(&leftovers, &moved).map_err(|error| error.to_string())?;
        let subagents = fs::read_dir(moved.join("subagents"))
            .into_iter()
            .flatten()
            .flatten();
        for path in subagents.map(|entry| entry.path()) {
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                rewrite(&path, &path, from_cwd, to_cwd)?;
            }
        }
    }
    Ok(())
}

/// Writes `from` to `to` with every recorded working directory under `from_cwd`
/// moved under `to_cwd`, keeping the file's time so the chat keeps its place in Recent.
fn rewrite(from: &Path, to: &Path, from_cwd: &str, to_cwd: &str) -> Result<(), String> {
    let text = fs::read_to_string(from).map_err(|error| error.to_string())?;
    let modified = fs::metadata(from).and_then(|meta| meta.modified()).ok();
    sikemux_process::files::write_atomic(to, moved_cwds(&text, from_cwd, to_cwd).as_bytes(), false)
        .map_err(|error| error.to_string())?;
    if let Some(modified) = modified {
        let _ = File::options()
            .write(true)
            .open(to)
            .and_then(|file| file.set_modified(modified));
    }
    Ok(())
}

pub(super) fn moved_cwds(text: &str, from_cwd: &str, to_cwd: &str) -> String {
    let quoted = |cwd: &str| format!("\"cwd\":{}", serde_json::Value::from(cwd));
    let (from_exact, to_exact) = (quoted(from_cwd), quoted(to_cwd));
    let open = |exact: &str| format!("{}/", &exact[..exact.len() - 1]);
    text.replace(&from_exact, &to_exact)
        .replace(&open(&from_exact), &open(&to_exact))
}
