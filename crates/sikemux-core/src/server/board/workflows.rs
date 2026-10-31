//! Assertions a named CI workflow shows. The board reads that workflow's runs
//! on the pushed tip of the project's branch that holds a slice's latest
//! commit, as the project's CI check does, and keeps the verdict as each such
//! assertion's evidence. Runs moving in the project's repository wake it.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::slice::Verdict;

use super::slice::moved_by_evidence;
use super::{State, Status};
use crate::server::codehost::ci::{self, Shown};
use crate::server::codehost::repo::{self as codehost, Checks, CI_START_WAIT_MS};
use crate::server::Core;

/// What the board keeps of each project whose slices wait on a workflow's verdict.
#[derive(Default)]
pub(super) struct Watches(HashMap<String, Watch>);

#[derive(Default)]
struct Watch {
    /// The project's repository on its code host, once read.
    repo: Option<Shown>,
    /// A look is under way.
    looking: bool,
    /// What waited at the last look; a look is due when it changes.
    looked: BTreeSet<(u64, String)>,
    /// When each pushed tip was first seen holding a waiting head, so a
    /// workflow that never starts there ends with no verdict.
    tips: HashMap<String, u64>,
}

/// A slice that waits on workflows: its card, its latest commit, and each
/// assertion's id and workflow.
type Waiting = (u64, String, Vec<(u64, String)>);

fn waiting(state: &State) -> HashMap<String, Vec<Waiting>> {
    let mut found: HashMap<String, Vec<Waiting>> = HashMap::new();
    for (project, board) in &state.boards {
        for card in board
            .cards
            .iter()
            .filter(|card| card.status == Status::Progress)
        {
            let Some(slice) = &card.slice else {
                continue;
            };
            let (Some(head), awaiting) = (slice.head(), slice.awaiting_workflows()) else {
                continue;
            };
            if !awaiting.is_empty() {
                found.entry(project.clone()).or_default().push((
                    card.id,
                    head.to_owned(),
                    awaiting,
                ));
            }
        }
    }
    found
}

/// Starts a look at each project whose waiting slices changed, whose runs
/// moved, or whose tip waited long enough for a workflow to start. Answers
/// when a tip next runs out of time.
pub(super) fn due(core: &Arc<Core>) -> Option<u64> {
    let now = epoch_ms();
    let mut next = None;
    let mut looks = Vec::new();
    {
        let mut state = core.boards.state.locked();
        let waiting = waiting(&state);
        let watches = &mut state.workflows.0;
        watches.retain(|project, watch| watch.looking || waiting.contains_key(project));
        for (project, slices) in waiting {
            let watch = watches.entry(project.clone()).or_default();
            let deadline = watch
                .tips
                .values()
                .map(|since| since + CI_START_WAIT_MS)
                .min();
            let overdue = deadline.is_some_and(|deadline| deadline <= now);
            next = crate::server::wake::sooner(next, deadline.filter(|_| !overdue));
            let looked: BTreeSet<(u64, String)> = slices
                .iter()
                .map(|(card, head, _)| (*card, head.clone()))
                .collect();
            if watch.looking || (watch.looked == looked && !overdue) {
                continue;
            }
            watch.looking = true;
            watch.looked = looked;
            looks.push((project, slices));
        }
    }
    for (project, slices) in looks {
        tokio::spawn(look(core.clone(), project, slices));
    }
    next
}

/// Reads each waiting workflow's verdict on the pushed tip that holds the
/// slices' heads, and keeps those it gave.
async fn look(core: Arc<Core>, project: String, slices: Vec<Waiting>) {
    let ruled = read(&core, &project, &slices).await;
    for (card, name, tip, verdict, output) in ruled {
        moved_by_evidence(&core, &project, card, |slice| {
            slice.workflow_ran(&name, &tip, verdict, output)
        });
    }
    let mut state = core.boards.state.locked();
    if let Some(watch) = state.workflows.0.get_mut(&project) {
        watch.looking = false;
    }
    drop(state);
    core.boards.wake.now();
}

/// What each workflow said that the slices wait on: card, workflow, tip, the
/// verdict, and what it said.
async fn read(
    core: &Arc<Core>,
    project: &str,
    slices: &[Waiting],
) -> Vec<(u64, String, String, Option<Verdict>, String)> {
    let Ok(repo) = codehost::of_project(core, project).await else {
        return Vec::new();
    };
    if let Some(watch) = core.boards.state.locked().workflows.0.get_mut(project) {
        watch.repo = Some(repo.watched());
    }
    ci::start(core);
    let (dir, heads) = (
        project.to_owned(),
        slices
            .iter()
            .map(|(_, head, _)| head.clone())
            .collect::<Vec<_>>(),
    );
    let looked = tokio::task::spawn_blocking(move || {
        crate::server::collisions::look(Path::new(&dir), &heads)
    })
    .await
    .unwrap_or_default();
    let Some((tip, held)) = looked.tip else {
        return Vec::new();
    };
    let since = {
        let mut state = core.boards.state.locked();
        let watch = state.workflows.0.entry(project.to_owned()).or_default();
        watch.tips.retain(|known, _| *known == tip);
        *watch.tips.entry(tip.clone()).or_insert_with(epoch_ms)
    };
    let overdue = epoch_ms().saturating_sub(since) >= CI_START_WAIT_MS;
    let names: BTreeSet<&String> = (slices.iter())
        .filter(|(_, head, _)| held.contains(head))
        .flat_map(|(_, _, awaiting)| awaiting.iter().map(|(_, name)| name))
        .collect();
    let mut ruled = Vec::new();
    for name in names {
        let said = match codehost::commit_checks(&repo, None, &tip, std::slice::from_ref(name))
            .await
        {
            Ok(Checks::Passed) => Some((Some(Verdict::Passed), format!("{name} passed on {tip}"))),
            Ok(Checks::Failed(what)) => Some((Some(Verdict::Failed), what)),
            Ok(Checks::Stopped(why)) => Some((None, why)),
            Ok(Checks::None) if overdue => {
                Some((None, format!("no run of {name} started on {tip}")))
            }
            Ok(Checks::None | Checks::Running { .. }) | Err(_) => None,
        };
        let Some((verdict, output)) = said else {
            continue;
        };
        for (card, head, awaiting) in slices {
            if held.contains(head) && awaiting.iter().any(|(_, wanted)| wanted == name) {
                ruled.push((*card, name.clone(), tip.clone(), verdict, output.clone()));
            }
        }
    }
    ruled
}

/// The repositories of projects whose slices wait on a workflow, for the CI watch.
pub(super) fn waiting_on_ci(state: &State) -> Vec<Shown> {
    (state.workflows.0.values())
        .filter_map(|watch| watch.repo.clone())
        .collect()
}

/// Runs moved in `repo`, so the projects waiting on its workflows look again.
pub(super) fn ci_moved(state: &mut State, repo: &Shown) -> bool {
    let mut moved = false;
    for watch in state.workflows.0.values_mut() {
        if watch.repo.as_ref().is_some_and(|known| known.id == repo.id) {
            watch.looked.clear();
            moved = true;
        }
    }
    moved
}
