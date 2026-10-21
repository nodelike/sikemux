//! Which saved conversation each agent writes to. For the agents in the
//! workspace the core watches each CLI's session folders, binds a terminal
//! agent to the session it starts, keeps titles in step with the ones the CLI
//! saves, and writes a name the person gave back into the CLI's own session.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{Map, Value};
use sikemux_process::sync::Locked;
use tokio::sync::mpsc;

use crate::agent_catalog::AgentKind;
use crate::protocol::WorkspaceAgent;
use crate::sessions::roots::{agent_config_root, muse_data_root, omp_session_dirs};
use crate::sessions::{self, claude_project_dir, opencode_data_dirs, pi_session_dir, AgentSession};

use super::connection::blocking;
use super::Core;

const SETTLE: Duration = Duration::from_millis(200);
/// While a turn runs its transcript is written to constantly, and rereading
/// every session at each write buys nothing.
const SETTLE_STREAMING: Duration = Duration::from_secs(2);
/// A title no longer than this that the session id starts with is the CLI's
/// placeholder, not a name.
const PLACEHOLDER_TITLE_CHARS: usize = 13;

/// One CLI's sessions for one folder and account.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Group {
    kind: AgentKind,
    cwd: String,
    config_path: Option<String>,
}

struct Watched {
    _watchers: Vec<RecommendedWatcher>,
}

#[derive(Default)]
pub(crate) struct SessionWatch {
    groups: Mutex<HashMap<Group, Watched>>,
    /// The sessions there were when a terminal agent started fresh, so it never
    /// takes one it did not start.
    baselines: Mutex<HashMap<String, HashSet<String>>>,
    /// Names already written back, or refused, so neither is tried again.
    named: Mutex<HashSet<(String, String)>>,
}

fn group_of(core: &Core, agent: &WorkspaceAgent) -> Option<Group> {
    let kind = AgentKind::from_name(&agent.provider)?;
    let book = core.workspaces.accounts().unwrap_or_default();
    let config_path = agent
        .profile_id
        .as_deref()
        .and_then(|id| book.profile(id))
        .and_then(|profile| profile.config_path.clone());
    Some(Group {
        kind,
        cwd: agent.cwd.to_string_lossy().into_owned(),
        config_path,
    })
}

fn agents(core: &Core) -> Vec<WorkspaceAgent> {
    core.workspaces
        .agent_ids()
        .into_iter()
        .filter_map(|agent_id| core.workspaces.agent(&agent_id))
        .filter(|agent| !agent.asleep)
        .collect()
}

/// Watches the session folders of every group an agent of the workspace is
/// in, and only those. Runs whenever the workspace changes.
pub(crate) fn follow(core: &Arc<Core>) {
    let wanted: HashSet<Group> = agents(core)
        .iter()
        .filter_map(|agent| group_of(core, agent))
        .collect();
    let mut groups = core.session_watch.groups.locked();
    groups.retain(|group, _| wanted.contains(group));
    for group in wanted {
        if groups.contains_key(&group) {
            continue;
        }
        groups.insert(group.clone(), watch(core, group));
    }
}

fn watch(core: &Arc<Core>, group: Group) -> Watched {
    let (changed, mut heard) = mpsc::unbounded_channel::<()>();
    let owner = Arc::downgrade(core);
    let settling = (owner.clone(), group.clone());
    tokio::spawn(async move {
        let _ = scan(&settling.0, &settling.1).await;
        while heard.recv().await.is_some() {
            loop {
                let wait = match settling.0.upgrade() {
                    Some(core) if streaming(&core, &settling.1).is_empty() => SETTLE,
                    Some(_) => SETTLE_STREAMING,
                    None => return,
                };
                match tokio::time::timeout(wait, heard.recv()).await {
                    Ok(Some(())) => continue,
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            if scan(&settling.0, &settling.1).await.is_none() {
                return;
            }
        }
    });
    let mut watchers = Vec::new();
    for (dir, mode) in folders(&group) {
        let (changed, owner, group) = (changed.clone(), owner.clone(), group.clone());
        let watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            let Ok(event) = event else {
                return;
            };
            let interesting = matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            );
            let Some(core) = owner.upgrade() else {
                return;
            };
            if interesting && !streamed_only(&core, &group, &event.paths) {
                let _ = changed.send(());
            }
        });
        if let Ok(mut watcher) = watcher {
            if watcher.watch(&dir, mode).is_ok() {
                watchers.push(watcher);
            }
        }
    }
    Watched {
        _watchers: watchers,
    }
}

/// The sessions of the group's chats with a turn running.
fn streaming(core: &Core, group: &Group) -> Vec<String> {
    core.chats
        .ids()
        .into_iter()
        .filter_map(|agent_id| core.chats.get(&agent_id))
        .filter(|chat| {
            chat.provider() == group.kind.as_str()
                && chat.launch.cwd.to_string_lossy() == group.cwd
                && chat.turn_running()
        })
        .filter_map(|chat| Some(chat.feed.start()?.session_id))
        .collect()
}

/// Whether every changed file is the transcript of a chat at work, which the
/// core already has every word of. Each CLI names the file after its session.
fn streamed_only(core: &Core, group: &Group, paths: &[PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let sessions = streaming(core, group);
    !sessions.is_empty()
        && paths.iter().all(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| sessions.iter().any(|id| name.contains(id.as_str())))
        })
}

fn folders(group: &Group) -> Vec<(PathBuf, RecursiveMode)> {
    let mut out: Vec<(PathBuf, RecursiveMode)> = Vec::new();
    let mut add = |path: PathBuf, mode: RecursiveMode| {
        let (dir, mode) = if path.is_dir() {
            (path, mode)
        } else if let Some(parent) = path.parent().filter(|parent| parent.is_dir()) {
            (parent.to_path_buf(), RecursiveMode::NonRecursive)
        } else {
            return;
        };
        match out.iter_mut().find(|(known, _)| *known == dir) {
            Some(known) if mode == RecursiveMode::Recursive => known.1 = mode,
            Some(_) => {}
            None => out.push((dir, mode)),
        }
    };
    let config_path = group.config_path.as_deref();
    match group.kind {
        AgentKind::Claude => {
            if let Some(root) = agent_config_root("claude", config_path) {
                let project = claude_project_dir(&root, &group.cwd);
                if project.is_dir() {
                    add(project, RecursiveMode::Recursive);
                } else {
                    add(root.join("projects"), RecursiveMode::NonRecursive);
                }
            }
        }
        AgentKind::Codex => {
            if let Some(root) = agent_config_root("codex", config_path) {
                add(root.join("sessions"), RecursiveMode::Recursive);
                add(root, RecursiveMode::NonRecursive);
            }
        }
        AgentKind::Hermes => {
            if let Some(root) = AgentKind::Hermes.home_root() {
                add(root, RecursiveMode::NonRecursive);
            }
        }
        AgentKind::Pi => {
            if let Some(root) = pi_session_dir() {
                add(root, RecursiveMode::Recursive);
            }
        }
        AgentKind::Opencode => {
            for dir in opencode_data_dirs() {
                add(dir, RecursiveMode::NonRecursive);
            }
        }
        AgentKind::Omp => {
            for dir in omp_session_dirs() {
                add(dir, RecursiveMode::Recursive);
            }
        }
        AgentKind::Grok => {
            if let Some(root) = AgentKind::Grok.home_root() {
                add(root.join("sessions"), RecursiveMode::Recursive);
            }
        }
        AgentKind::Muse => {
            if let Some(root) = muse_data_root() {
                add(root, RecursiveMode::NonRecursive);
            }
        }
    }
    out
}

/// Notes the sessions there are as a terminal agent starts without one, so
/// the one it binds to is the one it starts.
pub(crate) async fn note_launch(core: &Arc<Core>, agent: &WorkspaceAgent) {
    let Some(group) = group_of(core, agent) else {
        return;
    };
    let listed = listed(group).await;
    let ids = listed.into_iter().map(|session| session.id).collect();
    core.session_watch
        .baselines
        .locked()
        .insert(agent.id.clone(), ids);
}

async fn listed(group: Group) -> Vec<AgentSession> {
    blocking(move || {
        Ok(sessions::agent_sessions(
            group.kind,
            &group.cwd,
            group.config_path.as_deref(),
        ))
    })
    .await
    .unwrap_or_default()
}

/// Reads the group's sessions again and settles its agents on them. None once
/// the core is gone.
async fn scan(core: &Weak<Core>, group: &Group) -> Option<()> {
    let rows = listed(group.clone()).await;
    let core = core.upgrade()?;
    if !rows.is_empty() {
        settle(&core, group, rows).await;
    }
    Some(())
}

fn usable_title(row: &AgentSession) -> Option<String> {
    let title = row.title.trim();
    let placeholder = title.chars().count() <= PLACEHOLDER_TITLE_CHARS && row.id.starts_with(title);
    (!title.is_empty() && !placeholder).then(|| title.to_owned())
}

async fn settle(core: &Arc<Core>, group: &Group, rows: Vec<AgentSession>) {
    let members: Vec<WorkspaceAgent> = agents(core)
        .into_iter()
        .filter(|agent| group_of(core, agent).as_ref() == Some(group))
        .collect();
    let by_id: HashMap<&str, &AgentSession> =
        rows.iter().map(|row| (row.id.as_str(), row)).collect();
    let mut claimed: HashSet<String> = HashSet::new();
    let mut names = Vec::new();
    for agent in &members {
        let Some(resume_id) = &agent.resume_id else {
            continue;
        };
        claimed.insert(resume_id.clone());
        let Some(row) = by_id.get(resume_id.as_str()) else {
            continue;
        };
        if agent.renamed {
            if row.title != agent.title {
                names.push((resume_id.clone(), agent.title.clone()));
            }
        } else {
            super::workspace::retitle(core, &agent.id, usable_title(row), false);
        }
    }
    let mut fresh: Vec<&WorkspaceAgent> = members
        .iter()
        .filter(|agent| agent.resume_id.is_none() && agent.pty_id.is_some())
        .collect();
    fresh.sort_by_key(|agent| std::cmp::Reverse(agent.created_at));
    let mut candidates: Vec<&AgentSession> = rows
        .iter()
        .filter(|row| !claimed.contains(&row.id))
        .collect();
    candidates.sort_by_key(|row| std::cmp::Reverse(row.mtime));
    for agent in fresh {
        let baseline = core
            .session_watch
            .baselines
            .locked()
            .get(&agent.id)
            .cloned();
        let launched = agent.created_at / 1000;
        let found = candidates.iter().position(|row| match &baseline {
            Some(baseline) => !baseline.contains(&row.id),
            None => row.mtime >= launched,
        });
        let Some(index) = found else {
            continue;
        };
        let row = candidates.remove(index);
        let patch = Map::from_iter([("resumeId".to_owned(), Value::from(row.id.clone()))]);
        if core.workspaces.update_agent(&agent.id, patch).is_err() {
            continue;
        }
        core.session_watch.baselines.locked().remove(&agent.id);
        if agent.renamed {
            names.push((row.id.clone(), agent.title.clone()));
        } else {
            super::workspace::retitle(core, &agent.id, usable_title(row), false);
        }
        core.view_changed();
    }
    for (session_id, name) in names {
        name_session(core, group, session_id, name).await;
    }
}

/// Writes a name the person gave an agent into its CLI's own session, once.
async fn name_session(core: &Core, group: &Group, session_id: String, name: String) {
    let key = (session_id.clone(), name.clone());
    let first = core.session_watch.named.locked().insert(key);
    if !first {
        return;
    }
    let renamed = sessions::rename::rename(
        group.kind,
        group.cwd.clone(),
        session_id,
        name,
        None,
        group.config_path.clone(),
    )
    .await;
    if let Err(error) = renamed {
        eprintln!("sikemux core: an agent's name was not saved with its session: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{usable_title, AgentSession};

    fn row(id: &str, title: &str) -> AgentSession {
        AgentSession {
            id: id.into(),
            title: title.into(),
            mtime: 0,
        }
    }

    #[test]
    fn the_cli_s_placeholder_is_no_title_and_its_own_name_is() {
        let id = "019fd81d-c861-7920-bc88-a41a6b17aca4";
        assert_eq!(usable_title(&row(id, &id[..8])), None);
        assert_eq!(usable_title(&row(id, "  ")), None);
        assert_eq!(usable_title(&row(id, "Hello")).as_deref(), Some("Hello"));
    }
}
