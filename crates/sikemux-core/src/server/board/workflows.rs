//! Assertions a named CI workflow shows. The board reads that workflow's runs
//! on the pushed tip of the project's branch that holds a slice's latest
//! commit, as the project's CI check does, and keeps the verdict as each such
//! assertion's evidence. Runs moving in the project's repository wake it.

use crate::server::parts::Parts;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::slice::Verdict;

use super::slice::moved_by_evidence;
use super::{CardStatus, State};
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
    /// What waited at the last look; a look is due when something new waits.
    looked: BTreeSet<(u64, String)>,
    /// When to look again though nothing moved.
    next: Option<u64>,
    /// When each pushed tip was first seen holding a waiting head, so a
    /// workflow that never starts there ends with no verdict.
    tips: HashMap<String, u64>,
}

/// How long a look waits for runs to move before looking again by itself.
/// The CI watch wakes it sooner when they do.
const LOOK_AGAIN_MS: u64 = 5 * 60_000;

/// A slice that waits on workflows: its card, its latest commit, and each
/// assertion's id and workflow.
type Waiting = (u64, String, Vec<(u64, String)>);

/// What a workflow said on the tip holding a slice's latest commit.
struct Ruled {
    card: u64,
    head: String,
    name: String,
    tip: String,
    verdict: Option<Verdict>,
    output: String,
}

fn waiting(state: &State) -> HashMap<String, Vec<Waiting>> {
    let mut found: HashMap<String, Vec<Waiting>> = HashMap::new();
    for (project, board) in &state.boards {
        for card in board
            .cards
            .iter()
            .filter(|card| card.status == CardStatus::Progress)
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

/// Starts a look at each project where a slice newly waits, whose runs
/// moved, or whose time to look again came. Answers when one next comes.
pub(super) fn due(core: &Arc<Core>) -> Option<u64> {
    let now = epoch_ms();
    let mut next = None;
    let mut looks = Vec::new();
    {
        let mut state = core.boards().state.locked();
        let waiting = waiting(&state);
        let watches = &mut state.workflows.0;
        watches.retain(|project, watch| watch.looking || waiting.contains_key(project));
        for (project, slices) in waiting {
            let watch = watches.entry(project.clone()).or_default();
            let looked: BTreeSet<(u64, String)> = slices
                .iter()
                .map(|(card, head, _)| (*card, head.clone()))
                .collect();
            let fresh = !looked.is_subset(&watch.looked);
            if watch.looking || !(fresh || watch.next.is_some_and(|at| at <= now)) {
                next = crate::server::wake::sooner(next, watch.next);
                continue;
            }
            watch.looking = true;
            watch.looked = looked;
            watch.next = None;
            looks.push((project, slices));
        }
    }
    for (project, slices) in looks {
        tokio::spawn(look(core.clone(), project, slices));
    }
    next
}

/// Reads each waiting workflow's verdict on the pushed tip that holds the
/// slices' heads, keeps those it gave, and sets when to look again.
async fn look(core: Arc<Core>, project: String, slices: Vec<Waiting>) {
    let (ruled, again) = read(&core, &project, &slices).await;
    for ruled in ruled {
        moved_by_evidence(&core, &project, ruled.card, |slice| {
            slice.workflow_ran(
                &ruled.head,
                &ruled.name,
                &ruled.tip,
                ruled.verdict,
                ruled.output,
            )
        });
    }
    let mut state = core.boards().state.locked();
    if let Some(watch) = state.workflows.0.get_mut(&project) {
        watch.looking = false;
        watch.next = Some(again);
    }
    drop(state);
    core.boards().wake.at(again);
}

/// What each workflow said that the slices wait on, and when to look again:
/// by itself after a while, or sooner once a tip has waited long enough for
/// a workflow to start.
async fn read(core: &Arc<Core>, project: &str, slices: &[Waiting]) -> (Vec<Ruled>, u64) {
    let again = epoch_ms() + LOOK_AGAIN_MS;
    let Ok(repo) = codehost::of_project(core, project).await else {
        return (Vec::new(), again);
    };
    if let Some(watch) = core.boards().state.locked().workflows.0.get_mut(project) {
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
    let looked =
        tokio::task::spawn_blocking(move || crate::server::checks::look(Path::new(&dir), &heads))
            .await
            .unwrap_or_default();
    let tip = looked.tip.as_ref().map(|(tip, _)| tip.clone());
    let since = {
        let mut state = core.boards().state.locked();
        let watch = state.workflows.0.entry(project.to_owned()).or_default();
        watch.tips.retain(|known, _| tip.as_ref() == Some(known));
        tip.map(|tip| *watch.tips.entry(tip).or_insert_with(epoch_ms))
    };
    let (Some((tip, held)), Some(since)) = (looked.tip, since) else {
        return (Vec::new(), again);
    };
    let starts_by = since + CI_START_WAIT_MS;
    let overdue = epoch_ms() >= starts_by;
    let names: BTreeSet<&String> = (slices.iter())
        .filter(|(_, head, _)| held.contains(head))
        .flat_map(|(_, _, awaiting)| awaiting.iter().map(|(_, name)| name))
        .collect();
    let mut ruled = Vec::new();
    let mut again = again;
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
            Ok(Checks::None) => {
                again = again.min(starts_by);
                None
            }
            Ok(Checks::Running { .. }) | Err(_) => None,
        };
        let Some((verdict, output)) = said else {
            continue;
        };
        for (card, head, awaiting) in slices {
            if held.contains(head) && awaiting.iter().any(|(_, wanted)| wanted == name) {
                ruled.push(Ruled {
                    card: *card,
                    head: head.clone(),
                    name: name.clone(),
                    tip: tip.clone(),
                    verdict,
                    output: output.clone(),
                });
            }
        }
    }
    (ruled, again)
}

/// Someone asks after the project's slices, so their workflows are read again.
pub(super) fn look_again(core: &Core, project: &str) {
    let mut state = core.boards().state.locked();
    let Some(watch) = state.workflows.0.get_mut(project) else {
        return;
    };
    watch.looked.clear();
    drop(state);
    core.boards().wake.now();
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
