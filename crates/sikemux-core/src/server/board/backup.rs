//! Auto keeps a card's commits on the remote as they are made, so a lost worktree can be made
//! again. They go to a ref of the card's own, outside branches and tags, where code hosts start
//! no checks; the card's branch is only pushed when its work ships.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sikemux_process::sync::Locked;

use super::git;
use crate::server::connection::blocking;
use crate::server::Core;

type CardKey = (String, u64);

#[derive(Default)]
pub(super) struct Backups {
    due: HashSet<CardKey>,
    pushing: HashSet<CardKey>,
}

/// `agent` made a commit: the run it works for, if any, has something new to keep.
pub(crate) fn committed(core: &Core, agent: &str) {
    let lead = core.names.team_of(agent);
    let mut state = core.boards.state.locked();
    let found = state.boards.iter().find_map(|(project, board)| {
        if !board.settings.backs_up() {
            return None;
        }
        let card = board.cards.iter().find(|card| {
            card.run()
                .is_some_and(|run| run.agent == lead && !run.worktree.is_empty())
        })?;
        Some((project.clone(), card.id))
    });
    let Some(key) = found else {
        return;
    };
    state.backups.due.insert(key);
    drop(state);
    core.boards.wake.now();
}

/// Pushes the commits of each run that has new ones, one push per run at a time.
pub(super) fn start_due(core: &Arc<Core>) {
    let starts: Vec<(CardKey, PathBuf, String, Option<String>)> = {
        let mut state = core.boards.state.locked();
        if state.backups.due.is_empty() {
            return;
        }
        let due = std::mem::take(&mut state.backups.due);
        let (waiting, ready): (HashSet<CardKey>, HashSet<CardKey>) = due
            .into_iter()
            .partition(|key| state.backups.pushing.contains(key));
        state.backups.due = waiting;
        let starts: Vec<_> = ready
            .into_iter()
            .filter_map(|key| {
                let board = state.boards.get(&key.0)?;
                let run = board.card(key.1).ok()?.run()?.clone();
                Some((key, PathBuf::from(run.worktree), run.branch, run.backed_up))
            })
            .collect();
        state.backups.pushing = (state.backups.pushing.iter().cloned())
            .chain(starts.iter().map(|(key, ..)| key.clone()))
            .collect();
        starts
    };
    for (key, tree, branch, kept) in starts {
        let core = core.clone();
        tokio::spawn(async move {
            let pushed = blocking(move || push(&tree, &branch, kept).map_err(Into::into)).await;
            match pushed {
                Ok(Some(head)) => note_kept(&core, &key, head),
                Ok(None) => {}
                Err(error) => eprintln!("sikemux core: Auto could not back up a card: {error}"),
            }
            core.boards.state.locked().backups.pushing.remove(&key);
            core.boards.wake.now();
        });
    }
}

fn push(tree: &Path, branch: &str, kept: Option<String>) -> Result<Option<String>, String> {
    if !tree.is_dir() {
        return Ok(None);
    }
    git::back_up(tree, branch, kept.as_deref())
}

fn note_kept(core: &Core, (project, id): &CardKey, head: String) {
    core.boards.edit_if(project, *id, |card| {
        let run = card.auto.as_mut()?;
        run.backed_up = Some(head);
        if let Some(record) = card.record.as_mut() {
            record.branch_backups += 1;
        }
        Some(())
    });
}
