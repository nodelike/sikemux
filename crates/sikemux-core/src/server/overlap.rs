//! The files a new agent's brief names that other open agents are already
//! changing, so the lead hears of it while it can still merge or order the work.

use crate::server::parts::Parts;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::edits::{git, owners_at};
use super::roster::{self, Reach};
use super::Core;

/// Names too common to say which file a brief means without its folder.
const SHARED_NAMES: [&str; 8] = [
    "mod.rs",
    "lib.rs",
    "main.rs",
    "index.ts",
    "index.tsx",
    "README.md",
    "package.json",
    "Cargo.toml",
];

/// Each other open agent of the caller's project with changes not yet on the branch it
/// lands on in a file `brief` names, with those files, as `agent_spawn` answers it.
pub(crate) fn of_brief(core: &Core, caller: &str, brief: &str) -> Option<Value> {
    let named = paths_in(brief);
    if named.is_empty() {
        return None;
    }
    let agents = roster::listed(core);
    let project = agents
        .iter()
        .find(|agent| agent.agent_id == caller)?
        .project
        .clone();
    let mut shared_checkouts: HashMap<PathBuf, BTreeMap<String, BTreeSet<String>>> = HashMap::new();
    let mut found = Vec::new();
    for agent in agents {
        if agent.agent_id == caller
            || agent.project != project
            || matches!(agent.reach, Reach::Stopped | Reach::Remote)
        {
            continue;
        }
        let Some(placed) = core.workspaces().agent(&agent.agent_id) else {
            continue;
        };
        let changing = match &placed.worktree {
            Some(worktree) => unlanded_files(&worktree.path, worktree.base.as_deref()),
            None => {
                let Some(top) = top_of(&placed.cwd) else {
                    continue;
                };
                let by_agent = shared_checkouts
                    .entry(top.clone())
                    .or_insert_with(|| changed_by_agent(core, &top));
                by_agent.get(&agent.agent_id).cloned().unwrap_or_default()
            }
        };
        let files: Vec<String> = changing
            .into_iter()
            .filter(|file| named.iter().any(|path| names(path, file)))
            .collect();
        if !files.is_empty() {
            let name = core.names().name_or_id(&agent.agent_id);
            found.push((agent.agent_id, name, files));
        }
    }
    if found.is_empty() {
        return None;
    }
    let said: Vec<String> = found
        .iter()
        .map(|(_, name, files)| format!("@{name} is changing {}", files.join(", ")))
        .collect();
    Some(json!({
        "agents": found.iter().map(|(agent_id, name, files)| json!({
            "agentId": agent_id,
            "name": name,
            "files": files,
        })).collect::<Vec<_>>(),
        "note": format!(
            "This brief names files other agents have uncommitted or unlanded changes in: {}. Consider handing the task to that agent instead, or telling one to wait for the other to land.",
            said.join("; ")
        ),
    }))
}

/// The files the brief names: words with a folder in them or a file's extension.
fn paths_in(brief: &str) -> Vec<String> {
    let mut paths: Vec<String> = brief
        .split(|c: char| c.is_whitespace() || "\"'`()[]{}<>,;|*".contains(c))
        .filter_map(|word| {
            let mut path = word.trim_end_matches(['.', ':', '!', '?']);
            while let Some((rest, tail)) = path.rsplit_once(':') {
                if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
                    break;
                }
                path = rest;
            }
            let path = path.trim_start_matches("./").trim_end_matches('/');
            let has_extension = path
                .rsplit('/')
                .next()
                .and_then(|name| name.rsplit_once('.'))
                .is_some_and(|(stem, extension)| {
                    !stem.is_empty()
                        && (1..=6).contains(&extension.len())
                        && extension.chars().all(|c| c.is_ascii_alphanumeric())
                });
            let folder = path.contains('/') && !path.contains("://");
            let distinct = path.contains('/') || !SHARED_NAMES.contains(&path);
            ((has_extension || folder) && distinct).then(|| path.to_owned())
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Whether the brief's `named` path is the repository's `file`, or a folder holding it.
fn names(named: &str, file: &str) -> bool {
    if !named.contains('/') {
        return file.rsplit('/').next() == Some(named);
    }
    let named = named.trim_start_matches('/');
    file == named
        || file.starts_with(&format!("{named}/"))
        || named.ends_with(&format!("/{file}"))
        || file.ends_with(&format!("/{named}"))
}

fn top_of(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|top| PathBuf::from(top.trim()))
}

/// The files a checkout of its own changes since it left `base`, committed or not.
fn unlanded_files(tree: &Path, base: Option<&str>) -> BTreeSet<String> {
    let since = base
        .and_then(|base| git(tree, &["merge-base", "HEAD", &format!("refs/heads/{base}")]).ok())
        .map_or_else(|| "HEAD".to_owned(), |sha| sha.trim().to_owned());
    let changed = git(tree, &["diff", "--name-only", &since]).unwrap_or_default();
    let new = git(tree, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    changed
        .lines()
        .chain(new.lines())
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The files in a shared checkout each agent has uncommitted lines in.
fn changed_by_agent(core: &Core, top: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut by_agent: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Ok(owners) = owners_at(core, top, false) else {
        return by_agent;
    };
    for file in &owners.files {
        for run in &file.lines {
            if let Some(author) = owners.authors.get(run.author) {
                by_agent
                    .entry(author.agent_id.clone())
                    .or_default()
                    .insert(file.path.clone());
            }
        }
    }
    by_agent
}
