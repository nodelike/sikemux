//! Where Claude Code and Codex write a saved session's transcript, under the
//! account folder they keep it in.

use std::fs;
use std::path::{Path, PathBuf};

use super::claude::claude_project_dir;
use crate::agent_catalog::AgentKind;

const MAX_SESSION_ID_CHARS: usize = 200;

/// Whether `id` can name a session's file without leading out of its folder.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The transcript of session `id` under the account folder `root`, looked for
/// in `cwd`'s project first. Other agents keep their history in shapes Sikemux
/// does not read line by line.
pub fn find(kind: AgentKind, root: &Path, cwd: Option<&str>, id: &str) -> Option<PathBuf> {
    if !valid_session_id(id) {
        return None;
    }
    match kind {
        AgentKind::Claude => claude(root, cwd, id),
        AgentKind::Codex => codex(&root.join("sessions"), id),
        _ => None,
    }
}

fn written(path: PathBuf) -> Option<PathBuf> {
    fs::metadata(&path)
        .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
        .then_some(path)
}

/// Claude keeps each session in `projects/<project>/<session>.jsonl`.
fn claude(root: &Path, cwd: Option<&str>, id: &str) -> Option<PathBuf> {
    let file_name = format!("{id}.jsonl");
    if let Some(path) = cwd.and_then(|cwd| written(claude_project_dir(root, cwd).join(&file_name)))
    {
        return Some(path);
    }
    let mut projects: Vec<PathBuf> = fs::read_dir(root.join("projects"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    projects.sort();
    projects
        .into_iter()
        .find_map(|project| written(project.join(&file_name)))
}

fn newest_first(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    found.sort_unstable_by(|a, b| b.cmp(a));
    found
}

/// Codex keeps each session in `YYYY/MM/DD/rollout-<time>-<session>.jsonl`.
fn codex(sessions: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.jsonl");
    newest_first(sessions)
        .iter()
        .flat_map(|year| newest_first(year))
        .flat_map(|month| newest_first(&month))
        .flat_map(|day| newest_first(&day))
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&suffix))
        })
}
