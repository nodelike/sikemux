//! A saved session carried to another host: its files gathered here, named from where the
//! provider keeps them, and filed there under that host's own folder for the project.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::claude::claude_project_dir;
use super::delete::saved_transcript;
use super::relocate::moved_cwds;
use super::roots::account_root;
use crate::agent_catalog::AgentKind;

/// One file of a saved session: where it sits from the provider's folder for it, and here.
pub struct Gathered {
    pub name: String,
    pub path: PathBuf,
}

/// The files of the saved session `session_id` of `agent`, started in `cwd`: Claude's
/// transcript with the subagents' folder beside it, named from the project's folder, or
/// Codex's transcript, named from the account's folder.
pub fn gather(
    agent: AgentKind,
    cwd: &str,
    session_id: &str,
    config_path: Option<&str>,
) -> Result<Vec<Gathered>, String> {
    let (root, transcript) = saved_transcript(agent, cwd, session_id, config_path)?;
    let base = match agent {
        AgentKind::Claude => transcript
            .parent()
            .ok_or("the session's folder is missing")?
            .to_path_buf(),
        _ => fs::canonicalize(&root).map_err(|error| error.to_string())?,
    };
    let mut paths = vec![transcript];
    if agent == AgentKind::Claude {
        walk(&base.join(session_id), &mut paths);
    }
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(&base)
                .map_err(|_| "a session file lies outside its folder".to_owned())?
                .to_string_lossy()
                .into_owned();
            Ok(Gathered { name, path })
        })
        .collect()
}

fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&path, into),
            Ok(kind) if kind.is_file() => into.push(path),
            _ => {}
        }
    }
}

/// Files a session of `agent` that ran in `from_cwd` on another host into this host's own
/// store for the project at `to_cwd`, each working directory in it moved there. A file
/// already here stays as it is.
pub fn file(
    agent: AgentKind,
    from_cwd: &str,
    to_cwd: &str,
    files: Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let root = account_root(agent, None, &BTreeMap::new())
        .ok_or("this host has no folder for that agent's sessions")?;
    let base = match agent {
        AgentKind::Claude => claude_project_dir(&root, to_cwd),
        _ => root,
    };
    for (name, bytes) in files {
        let name = PathBuf::from(name);
        if !name
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err("a session file is named outside its folder".into());
        }
        let target = base.join(&name);
        if target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = match String::from_utf8(bytes) {
            Ok(text) if name.extension().is_some_and(|ext| ext == "jsonl") => {
                moved_cwds(&text, from_cwd, to_cwd).into_bytes()
            }
            Ok(text) => text.into_bytes(),
            Err(raw) => raw.into_bytes(),
        };
        sikemux_process::files::write_atomic(&target, &bytes, false)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
