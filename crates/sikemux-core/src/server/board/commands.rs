//! Assertions a command shows. Once the person approved the command, the board
//! runs it on a slice's latest commit in a clean checkout, as the project's
//! check runs, and keeps what it said as each such assertion's evidence. Its
//! exit code is the verdict. A new commit runs it again, and so does the owner
//! marking the card done again; a verdict stands until then.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use sikemux_process::sync::Locked;
use sikemux_view::slice::Verdict;

use super::slice::{moved_by_evidence, short};
use super::{CardStatus, Notice, State};
use crate::server::Core;

/// Commands running at once, across projects. Each project runs one at a time,
/// as its one checkout for them can hold only one commit.
const MAX_RUNNING: usize = 2;

/// A card, its latest commit, and a command that has not passed there.
type Waiting = (u64, String, String);

#[derive(Default)]
pub(super) struct Runs {
    /// The commit and command running now in each project running one.
    running: HashMap<String, (String, String)>,
    /// What each project already ran or is running.
    ran: HashMap<String, BTreeSet<Waiting>>,
}

fn waiting(state: &State) -> HashMap<String, BTreeSet<Waiting>> {
    let mut found: HashMap<String, BTreeSet<Waiting>> = HashMap::new();
    for (project, board) in &state.boards {
        for card in board
            .cards
            .iter()
            .filter(|card| card.status == CardStatus::Progress)
        {
            let Some(slice) = &card.slice else {
                continue;
            };
            let Some(head) = slice.head() else {
                continue;
            };
            for command in slice.awaiting_commands() {
                found.entry(project.clone()).or_default().insert((
                    card.id,
                    head.to_owned(),
                    command,
                ));
            }
        }
    }
    found
}

/// Starts the command next in line in each project with none running, as far as the limit allows.
pub(super) fn due(core: &Arc<Core>) {
    let mut starts = Vec::new();
    {
        let mut state = core.boards.state.locked();
        let waiting = waiting(&state);
        let runs = &mut state.commands;
        runs.ran.retain(|project, _| waiting.contains_key(project));
        for (project, slices) in waiting {
            let ran = runs.ran.entry(project.clone()).or_default();
            ran.retain(|one| slices.contains(one));
            if runs.running.contains_key(&project) || runs.running.len() >= MAX_RUNNING {
                continue;
            }
            let Some((_, head, command)) = slices.iter().find(|one| !ran.contains(*one)) else {
                continue;
            };
            let (head, command) = (head.clone(), command.clone());
            ran.extend(
                (slices.iter())
                    .filter(|(_, at, wanted)| *at == head && *wanted == command)
                    .cloned(),
            );
            (runs.running).insert(project.clone(), (head.clone(), command.clone()));
            starts.push((project, head, command));
        }
    }
    for (project, head, command) in starts {
        tokio::spawn(run(core.clone(), project, head, command));
    }
}

async fn run(core: Arc<Core>, project: String, head: String, command: String) {
    let outcome =
        match crate::server::worktree::agent::gates(&core, Path::new(&project), &head).await {
            Ok(checkout) => {
                let running = command.clone();
                tokio::task::spawn_blocking(move || {
                    crate::server::checks::run_command(&checkout, &running)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            }
            Err(error) => Err(format!("its checkout is not ready: {error}")),
        };
    let (verdict, output) = match outcome {
        Ok((true, output)) => (Some(Verdict::Passed), output),
        Ok((false, output)) => (Some(Verdict::Failed), output),
        Err(why) => (None, why),
    };
    keep(&core, &project, &head, &command, verdict, output);
    core.boards.state.locked().commands.running.remove(&project);
    core.boards.wake.now();
}

/// Each slice still at `head` that waits on `command` keeps the run, and its owner hears when it
/// did not pass.
fn keep(
    core: &Arc<Core>,
    project: &str,
    head: &str,
    command: &str,
    verdict: Option<Verdict>,
    output: String,
) {
    let cards: Vec<(u64, Option<String>, String)> = {
        let state = core.boards.state.locked();
        (state.boards.get(project).into_iter())
            .flat_map(|board| board.cards.iter())
            .filter(|card| {
                (card.slice.as_ref()).is_some_and(|slice| {
                    slice.head() == Some(head)
                        && slice.awaiting_commands().iter().any(|one| one == command)
                })
            })
            .map(|card| (card.id, card.owner_id().map(str::to_owned), card.label()))
            .collect()
    };
    let mut notices = Vec::new();
    for (card, owner, label) in cards {
        let mut assertions = Vec::new();
        moved_by_evidence(core, project, card, |slice| {
            assertions =
                slice.command_ran(owner.as_deref(), head, command, verdict, output.clone());
        });
        let Some(owner) = owner.filter(|_| !assertions.is_empty()) else {
            continue;
        };
        let at = short(head);
        let then = match verdict {
            Some(Verdict::Passed) => continue,
            Some(Verdict::Failed) => format!(
                "failed on {at}. It runs again on your next commit, or when you mark the card done again"
            ),
            None => format!(
                "did not finish on {at}. Mark the card done again to run it again"
            ),
        };
        notices.push(Notice {
            to: owner,
            text: format!(
                "Your card {label}: `{command}`, which assertion {} needs, {then}.\n\n{output}",
                assertions
                    .iter()
                    .map(|id| format!("#{id}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    super::send_notices(core, notices);
}

/// The owner of slice `card` marked it done again, so what did not pass on it runs again, other
/// than what is running now.
pub(super) fn run_again(core: &Core, project: &str, card: u64) {
    let mut state = core.boards.state.locked();
    let runs = &mut state.commands;
    let running = runs.running.get(project).cloned();
    if let Some(ran) = runs.ran.get_mut(project) {
        ran.retain(|(of, head, command)| {
            *of != card
                || running
                    .as_ref()
                    .is_some_and(|(at, wanted)| at == head && wanted == command)
        });
    }
    drop(state);
    core.boards.wake.now();
}
