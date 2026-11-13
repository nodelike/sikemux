//! Standing roles: agents a project's `sikemux.json` names, each with a charter and how many
//! commits must land between its runs. A run is an ordinary card, so Auto takes it through its
//! usual contract, review and queue, and it waits for a slot like any other.

use crate::server::board_state::BoardParts;
use crate::server::repo_state::RepoParts;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::{new_card, Adding, Board, CardStatus, State, MAX_CARDS};
use crate::git::Git;
use crate::server::worktree;
use crate::server::Core;
use crate::tasks::config::{self, ConfigLoad, ProjectRole};

const LOOKING_AT_ONCE: usize = 2;
const SHORT_SHA: usize = 10;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    /// The commit its count of landed commits starts from.
    since: String,
    /// Its last run found nothing to do, so it waits twice as long.
    #[serde(default)]
    idle: bool,
    /// Its last run ended, so the count starts again at the next look, past that run's own commits.
    #[serde(default)]
    ended: bool,
}

#[derive(Default)]
pub struct Runs {
    looking: HashSet<String>,
    wanted: HashSet<String>,
    /// Boards with Auto on whose first look since it turned on has been asked for.
    began: HashSet<String>,
}

impl Runs {
    pub fn want(&mut self, project: String) {
        self.wanted.insert(project);
    }
}

/// Starts the looks that are due, as far as the limit allows, and asks for one of each board
/// Auto has just turned on for.
pub fn due(core: &Arc<Core>) {
    let starts: Vec<String> = {
        let mut state = core.boards().state.locked();
        let State { boards, roles, .. } = &mut *state;
        let on: HashSet<&String> = (boards.iter())
            .filter(|(_, board)| board.settings.auto)
            .map(|(project, _)| project)
            .collect();
        roles.began.retain(|project| on.contains(project));
        roles.wanted.retain(|project| on.contains(project));
        for project in on {
            if roles.began.insert(project.clone()) {
                roles.wanted.insert(project.clone());
            }
        }
        let free = LOOKING_AT_ONCE.saturating_sub(roles.looking.len());
        let ready: Vec<String> = (roles.wanted.iter())
            .filter(|project| !roles.looking.contains(*project))
            .take(free)
            .cloned()
            .collect();
        for project in &ready {
            roles.wanted.remove(project);
            roles.looking.insert(project.clone());
        }
        ready
    };
    for project in starts {
        tokio::spawn(look(core.clone(), project));
    }
}

/// A commit landed on `project`'s branch, so a role may have reached its threshold.
pub fn landed(core: &Core, project: &str) {
    core.boards().state.locked().roles.want(project.to_owned());
    core.boards().wake.now();
}

/// Card `id`'s run ended, having found nothing to do when `idle`. Nothing happens for any card
/// but a standing role's.
pub fn ended(core: &Core, project: &str, id: u64, idle: bool) {
    let mut state = core.boards().state.locked();
    let Some(board) = state.boards.get_mut(project) else {
        return;
    };
    let Some(name) = board.card(id).ok().and_then(|card| card.role.clone()) else {
        return;
    };
    let Some(role) = board.roles.get_mut(&name) else {
        return;
    };
    role.idle = idle;
    role.ended = true;
    core.boards().save(&mut state, project);
    state.roles.want(project.to_owned());
    drop(state);
    core.boards().wake.now();
}

/// Whether card `id` is a standing role's whose agent has changed nothing so far.
pub fn found_nothing(core: &Core, project: &str, id: u64) -> bool {
    let tree = {
        let state = core.boards().state.locked();
        let card = state
            .boards
            .get(project)
            .and_then(|board| board.card(id).ok());
        card.filter(|card| card.role.is_some())
            .and_then(|card| card.run())
            .map(|run| (PathBuf::from(&run.worktree), run.start_sha.clone()))
    };
    tree.is_some_and(|(tree, start)| super::git::untouched(&tree, &start))
}

async fn look(core: Arc<Core>, project: String) {
    if let Err(why) = refresh(&core, &project).await {
        eprintln!("sikemux core: the standing roles of {project} were not looked at: {why}");
    }
    core.boards().state.locked().roles.looking.remove(&project);
    core.boards().wake.now();
}

async fn refresh(core: &Arc<Core>, project: &str) -> Result<(), String> {
    let dir = PathBuf::from(project);
    let reading = {
        let (core, dir) = (core.clone(), dir.clone());
        tokio::task::spawn_blocking(move || {
            let load = config::load(&dir);
            let ask = (core.repo_above().tasks_config_needs)(&core, &load);
            match load {
                ConfigLoad::Valid { config, .. } => Some((config.roles, ask)),
                ConfigLoad::Absent { .. } => Some((Vec::new(), None)),
                ConfigLoad::Invalid { .. } => None,
            }
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let Some((named, ask)) = reading else {
        return Ok(());
    };
    if named.is_empty() {
        forget_all(core, project);
        return Ok(());
    }
    if let Some(ask) = ask {
        super::reservoir::ask_trust(core, project, ask);
        return Ok(());
    }
    let sinces: BTreeMap<String, String> = {
        let state = core.boards().state.locked();
        (state.boards.get(project).into_iter())
            .flat_map(|board| board.roles.iter())
            .map(|(name, role)| (name.clone(), role.since.clone()))
            .collect()
    };
    let (tip, counts) = tokio::task::spawn_blocking(move || {
        let (_, tip) = worktree::latest_of_base(&dir)?;
        let counts: HashMap<String, Option<u32>> = (sinces.into_iter())
            .map(|(name, since)| {
                let count = landed_between(&dir, &since, &tip);
                (name, count)
            })
            .collect();
        Ok::<_, String>((tip, counts))
    })
    .await
    .map_err(|error| error.to_string())??;
    apply(core, project, &named, &tip, &counts);
    Ok(())
}

fn landed_between(repo: &Path, since: &str, tip: &str) -> Option<u32> {
    Git::quick(repo)
        .trimmed(&["rev-list", "--count", &format!("{since}..{tip}")])
        .ok()?
        .parse()
        .ok()
}

fn forget_all(core: &Core, project: &str) {
    let mut state = core.boards().state.locked();
    let forgot = state.boards.get_mut(project).is_some_and(|board| {
        let had = !board.roles.is_empty();
        board.roles.clear();
        had
    });
    if forgot {
        core.boards().save(&mut state, project);
    }
}

fn apply(
    core: &Core,
    project: &str,
    named: &[ProjectRole],
    tip: &str,
    counts: &HashMap<String, Option<u32>>,
) {
    let boards = core.boards();
    let mut state = boards.state.locked();
    let Some(board) = state
        .boards
        .get_mut(project)
        .filter(|board| board.settings.auto)
    else {
        return;
    };
    let mut changed = false;
    let before = board.roles.len();
    board
        .roles
        .retain(|name, _| named.iter().any(|role| role.name == *name));
    changed |= board.roles.len() != before;
    let open: HashSet<String> = (board.cards.iter())
        .filter(|card| card.status != CardStatus::Done)
        .filter_map(|card| card.role.clone())
        .collect();
    for role in named {
        let Some(standing) = board.roles.get(&role.name).cloned() else {
            let started = Role {
                since: tip.to_owned(),
                idle: false,
                ended: false,
            };
            board.roles.insert(role.name.clone(), started);
            changed = true;
            continue;
        };
        let count = counts.get(&role.name).copied().flatten();
        if standing.ended || count.is_none() {
            if let Some(kept) = board.roles.get_mut(&role.name) {
                kept.since = tip.to_owned();
                kept.ended = false;
            }
            changed = true;
            continue;
        }
        let needed = role.every.saturating_mul(if standing.idle { 2 } else { 1 });
        let reached = count.is_some_and(|count| count >= needed);
        if reached && !open.contains(&role.name) && board.cards.len() < MAX_CARDS {
            add_run(core, board, role, &standing.since);
            if let Some(kept) = board.roles.get_mut(&role.name) {
                kept.since = tip.to_owned();
            }
            changed = true;
        }
    }
    if changed {
        boards.save(&mut state, project);
    }
}

fn add_run(core: &Core, board: &mut Board, role: &ProjectRole, since: &str) {
    let id = board.next.max(1);
    board.next = id + 1;
    let from = since.get(..SHORT_SHA).unwrap_or(since);
    let adding = Adding {
        title: format!("Standing role: {}", role.name),
        details: format!(
            "{}\n\nLook at what landed since {from}. If nothing needs doing, end your turn without proposing a contract or changing anything, and this run closes by itself.",
            role.charter
        ),
        ..Adding::default()
    };
    let settings = board.settings.clone();
    let mut card = new_card(core, id, adding, None, &settings);
    card.role = Some(role.name.clone());
    board.cards.push(card);
}
