//! Auto's merge queue. A change its reviewer approved merges only after its
//! check passed on top of the latest base, one base of one project at a time.
//! Changes checked by the project's command are stacked and checked together,
//! and a failure is traced to the one that broke it. Changes checked by the
//! code host go one at a time: put on the latest base, pushed, and waited for.
//! An approval moves to the rebased commit only when the patch is the same.

use crate::server::parts::Parts;
use crate::server::repo_state::RepoParts;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::slice::Verdict;

use super::auto::{AutoPull, AutoRun, RunHold, RunStage};
use super::finish::{self, change_run, Step};
use super::record::RunRecord;
use super::{git, Card, Checker, State};
use crate::server::checks::{self, QueueRun};
use crate::server::codehost::repo::{self as codehost, Checks};
use crate::server::connection::blocking;
use crate::server::merge_queue::{self, Blame, Lane, Next, Waiter};
use crate::server::worktree;
use crate::server::Core;

/// A code host can refuse a merge for a moment after a push, while it catches
/// up with the new commit.
const MERGE_TRIES_AFTER_PUSH: u32 = 4;
const MERGE_RETRY_WAIT: Duration = Duration::from_secs(2);

/// A run's place in its queue, kept with the run so it survives a restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Place {
    pub joined: u64,
    /// The base commit the code host is checking it on top of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<String>,
    /// The base commit it was tried alone on, while that base failed its own
    /// check, and did not mend it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_over: Option<String>,
}

type LaneKey = (String, String);

struct Waiting {
    id: u64,
    joined: u64,
    stage: RunStage,
    on_code_host: bool,
    failed_over: Option<String>,
}

/// It left the queue, for whatever reason, and its run record counts the wait.
pub(super) fn leave(run: &mut AutoRun, record: Option<&mut RunRecord>) {
    let place = run.queue.take();
    if let (Some(place), Some(record)) = (place, record) {
        record.queue_ms += epoch_ms().saturating_sub(place.joined);
    }
}

fn bounced(card: &mut Card) {
    if let Some(record) = card.record.as_mut() {
        record.queue_bounces += 1;
    }
}

/// An approved change whose checks passed is here: at the back of the queue
/// the first time, or, when the code host just checked it, at the door.
pub(super) async fn arrived(core: &Arc<Core>, step: &Step, head: &str) {
    let Some(place) = &step.run.queue else {
        let joined = epoch_ms();
        change_run(core, &step.project, step.id, step.agent(), |_, run| {
            run.stage = RunStage::Merging;
            run.queue = Some(Place {
                joined,
                over: None,
                failed_over: None,
            });
            run.note = Some(format!("Waiting in the merge queue for {}", run.base));
        });
        return;
    };
    if place.over.is_some() && latest(step).await.ok() == place.over {
        land(core, step, head, 1).await;
    } else {
        go_again(core, step);
    }
}

/// The base moved under it, so it goes round again at its place in the queue.
fn go_again(core: &Core, step: &Step) {
    change_run(core, &step.project, step.id, step.agent(), |_, run| {
        run.stage = RunStage::Merging;
        if let Some(place) = run.queue.as_mut() {
            place.over = None;
        }
        run.note = Some(format!(
            "{} moved while it was checked, so it goes again",
            run.base
        ));
    });
}

/// Starts each base's next piece of work, for the bases not at one already.
pub(super) fn due(core: &Arc<Core>) -> Option<u64> {
    let mut next = None;
    let mut started = Vec::new();
    {
        let mut state = core.boards().state.locked();
        let lanes = waiting_lanes(&state);
        state
            .lanes
            .retain(|key, lane| lane.running || lanes.contains_key(key));
        for (key, waiting) in lanes {
            if waiting.iter().any(|w| w.stage != RunStage::Merging) {
                continue;
            }
            let lane = state.lanes.entry(key.clone()).or_default();
            if lane.running {
                continue;
            }
            let waiters: Vec<Waiter> = (waiting.iter())
                .map(|w| Waiter {
                    failed_over: w.failed_over.as_deref(),
                    alone: w.on_code_host,
                })
                .collect();
            let taken: &[Waiting] = match lane.next(&waiters) {
                None => continue,
                Some((_, Next::LookAt(at))) => {
                    next = super::sooner(next, Some(at));
                    continue;
                }
                Some((at, Next::Take(count))) => &waiting[at..at + count],
            };
            if taken.iter().any(|w| state.busy(&key.0, w.id)) {
                continue;
            }
            let steps: Vec<Step> = taken
                .iter()
                .filter_map(|w| step_of(&state, &key.0, w.id))
                .collect();
            for step in &steps {
                state.work(&step.project, step.id).job = true;
            }
            state.lanes.entry(key.clone()).or_default().running = true;
            started.push((key, steps));
        }
    }
    for (key, steps) in started {
        let core = core.clone();
        tokio::spawn(async move {
            run_lane(&core, &key, &steps).await;
            if let Some(lane) = core.boards().state.locked().lanes.get_mut(&key) {
                lane.running = false;
            }
            for step in &steps {
                finish::end_step(&core, step.project.clone(), step.id);
            }
        });
    }
    next
}

/// Each queue's runs, in the order they joined. A run that stopped to wait on
/// someone keeps its place but is passed over.
fn waiting_lanes(state: &State) -> BTreeMap<LaneKey, Vec<Waiting>> {
    let mut lanes: BTreeMap<LaneKey, Vec<Waiting>> = BTreeMap::new();
    for (project, board) in &state.boards {
        if !board.settings.lands_itself() {
            continue;
        }
        for card in &board.cards {
            let Some((run, place)) = card.run().and_then(|run| Some((run, run.queue.as_ref()?)))
            else {
                continue;
            };
            if !matches!(run.stage, RunStage::Merging | RunStage::Checking) {
                continue;
            }
            lanes
                .entry((project.clone(), run.base.clone()))
                .or_default()
                .push(Waiting {
                    id: card.id,
                    joined: place.joined,
                    stage: run.stage,
                    on_code_host: run.checked_on_host,
                    failed_over: place.failed_over.clone(),
                });
        }
    }
    for waiting in lanes.values_mut() {
        waiting.sort_by_key(|w| (w.joined, w.id));
    }
    lanes
}

fn step_of(state: &State, project: &str, id: u64) -> Option<Step> {
    let board = state.boards.get(project)?;
    let card = board.card(id).ok()?;
    Some(Step {
        project: project.to_owned(),
        id,
        run: card.run()?.clone(),
        card: card.clone(),
        settings: board.settings.clone(),
    })
}

async fn run_lane(core: &Arc<Core>, key: &LaneKey, steps: &[Step]) {
    let open = still_open(core, steps).await;
    let Some(front) = open.first() else {
        return;
    };
    let red = red_tip(core, key);
    if let Some(red) = &red {
        match latest(front).await {
            Ok(tip) if tip != *red => return clear_red(core, key),
            Ok(_) if failed_on(front, red) => return mark_red(core, key, red),
            Ok(_) => {}
            Err(why) => return could_not_rebase(core, front, &why),
        }
    }
    if front.run.checked_on_host {
        return put_on_latest(core, key, front).await;
    }
    check_together(core, key, open, red.is_some()).await;
}

fn failed_on(step: &Step, tip: &str) -> bool {
    (step.run.queue.as_ref()).is_some_and(|place| place.failed_over.as_deref() == Some(tip))
}

fn red_tip(core: &Core, key: &LaneKey) -> Option<String> {
    let state = core.boards().state.locked();
    state.lanes.get(key)?.red_tip().map(str::to_owned)
}

fn clear_red(core: &Core, key: &LaneKey) {
    if let Some(lane) = core.boards().state.locked().lanes.get_mut(key) {
        lane.clear_red();
    }
}

fn mark_red(core: &Core, key: &LaneKey, tip: &str) {
    if let Some(lane) = core.boards().state.locked().lanes.get_mut(key) {
        lane.mark_red(tip);
    }
}

async fn latest(step: &Step) -> Result<String, String> {
    let (tree, base) = (step.tree(), step.run.base.clone());
    blocking(move || git::latest(&tree, &base).map_err(Into::into))
        .await
        .map_err(|error| error.to_string())
}

/// It was tried alone on a base that fails its own check and did not mend
/// it. That is not its fault, so it keeps its place, and goes again once the
/// base moves.
fn did_not_mend(core: &Core, step: &Step, tip: &str) {
    change_run(core, &step.project, step.id, step.agent(), |_, run| {
        run.stage = RunStage::Merging;
        if let Some(place) = run.queue.as_mut() {
            place.over = None;
            place.failed_over = Some(tip.to_owned());
        }
        run.note = Some(format!(
            "{} fails its own check and this change does not mend it, so it waits",
            run.base
        ));
    });
}

/// A queued change failed its checks on the code host. True when that is not
/// down to the change, because the base fails them too or may yet.
pub(super) async fn base_answers_for(core: &Arc<Core>, step: &Step, pull: &AutoPull) -> bool {
    let Some(over) = step.run.queue.as_ref().and_then(|place| place.over.clone()) else {
        return false;
    };
    match codehost::checks(core, pull, &step.run.base, &over).await {
        Ok(Checks::Failed(_)) => {
            mark_red(core, &(step.project.clone(), step.run.base.clone()), &over);
            did_not_mend(core, step, &over);
            true
        }
        Ok(Checks::Running { .. }) => {
            core.boards()
                .state
                .locked()
                .work(&step.project, step.id)
                .checks_unread = Some(epoch_ms());
            step.note(
                core,
                RunStage::Checking,
                &format!(
                    "Its checks failed, and {}'s own are still running to say whether that is its doing",
                    step.run.base
                ),
            );
            true
        }
        _ => false,
    }
}

/// The runs that can still merge. One whose pull request merged while it
/// waited is finished, and one closed goes back to the person.
async fn still_open(core: &Arc<Core>, steps: &[Step]) -> Vec<Step> {
    let mut open = Vec::with_capacity(steps.len());
    for step in steps {
        let Some(pull) = &step.run.pull else {
            continue;
        };
        if let Some(why) = finish::unanswered(core, step) {
            step.note(core, RunStage::Watching, &why);
            continue;
        }
        match codehost::look(core, pull).await.map(|look| look.state) {
            Ok(state) if state == "merged" => landed(core, step).await,
            Ok(state) if matches!(state.as_str(), "closed" | "declined" | "superseded") => step
                .hold(
                    core,
                    RunHold::Unblock,
                    "Its pull request was closed without merging".to_owned(),
                ),
            _ => open.push(step.clone()),
        }
    }
    open
}

struct Moved {
    tip: String,
    head: String,
    /// The rebase left the patch as the reviewer approved it.
    carried: bool,
}

enum Rebased {
    Moved(Moved),
    Conflict,
    /// Every commit of it is on the base already.
    NothingLeft,
}

/// Puts the run's worktree on the latest base, and says whether its approval
/// still stands for what that made.
async fn rebase_onto_latest(step: &Step) -> Result<Rebased, String> {
    let (tree, base) = (step.tree(), step.run.base.clone());
    let approved = step.run.approved.clone();
    blocking(move || {
        if !git::rebase(&tree, &base)? {
            return Ok(Rebased::Conflict);
        }
        let tip = git::latest_known(&tree, &base)?;
        let head = git::head(&tree)?;
        if head == tip {
            return Ok(Rebased::NothingLeft);
        }
        let carried = match approved {
            Some(approved) => {
                let from = git::fork_point(&tree, &base, &approved)?;
                git::same_patch(&tree, (&from, &approved), (&tip, &head))?
            }
            None => false,
        };
        Ok(Rebased::Moved(Moved { tip, head, carried }))
    })
    .await
    .map_err(|error| error.to_string())
}

/// Pushes what a rebase made and has the run say so.
async fn push_moved(
    core: &Arc<Core>,
    step: &Step,
    moved: &Moved,
    change: impl FnOnce(&mut Card, &mut AutoRun),
) -> Result<(), String> {
    let (tree, branch, head) = (step.tree(), step.run.branch.clone(), moved.head.clone());
    let pushed = step.run.head.clone().unwrap_or_default();
    blocking(move || git::push_rebased(&tree, &branch, &pushed, &head).map_err(Into::into))
        .await
        .map_err(|error| format!("the push failed: {error}"))?;
    change_run(core, &step.project, step.id, step.agent(), |card, run| {
        super::slice::auto_shipped(card, &moved.tip, &moved.head);
        run.approved = moved.carried.then(|| moved.head.clone());
        run.head = Some(moved.head.clone());
        change(card, run);
    });
    Ok(())
}

fn is_pushed(step: &Step, moved: &Moved) -> bool {
    step.run.head.as_deref() == Some(moved.head.as_str())
}

/// The next change of a base checked by the code host: on the latest base,
/// pushed, and left to its checks. Already there and green, it merges.
async fn put_on_latest(core: &Arc<Core>, key: &LaneKey, step: &Step) {
    let base = step.run.base.clone();
    step.note(
        core,
        RunStage::Merging,
        &format!("Putting it on the latest {base}"),
    );
    let moved = match rebase_onto_latest(step).await {
        Err(why) => return could_not_rebase(core, step, &why),
        Ok(Rebased::Conflict) => return conflicted(core, step).await,
        Ok(Rebased::NothingLeft) => return nothing_left(core, step),
        Ok(Rebased::Moved(moved)) => moved,
    };
    if !moved.carried {
        return review_again(core, step, moved).await;
    }
    if is_pushed(step, &moved) {
        if failed_on(step, &moved.tip) {
            return mark_red(core, key, &moved.tip);
        }
        land(core, step, &moved.head, 1).await;
        return;
    }
    let tip = moved.tip.clone();
    let now = epoch_ms();
    let pushed = push_moved(core, step, &moved, |_, run| {
        run.stage = RunStage::Checking;
        run.checks_since = Some(now);
        if let Some(place) = run.queue.as_mut() {
            place.over = Some(tip);
        }
        run.note = Some(format!(
            "On the latest {}, so its checks run there before it merges",
            run.base
        ));
    });
    if let Err(why) = pushed.await {
        step.hold(core, RunHold::Fix, why);
    }
}

fn could_not_rebase(core: &Core, step: &Step, why: &str) {
    step.hold(
        core,
        RunHold::Fix,
        format!("Putting it on the latest {} failed: {why}", step.run.base),
    );
}

fn nothing_left(core: &Core, step: &Step) {
    step.hold(
        core,
        RunHold::Fix,
        format!(
            "Its change is on {} already, so there is nothing left to merge; mark the card done",
            step.run.base
        ),
    );
}

async fn conflicted(core: &Arc<Core>, step: &Step) {
    core.boards().edit(&step.project, step.id, bounced);
    finish::conflicted(core, step, "its turn in the merge queue came").await;
}

/// The rebase changed the patch, so its approval does not carry: it is
/// checked and reviewed afresh, and joins the queue again after.
async fn review_again(core: &Arc<Core>, step: &Step, moved: Moved) {
    let now = epoch_ms();
    let base = step.run.base.clone();
    let pushed = push_moved(core, step, &moved, |card, run| {
        bounced(card);
        leave(run, card.record.as_mut());
        run.stage = RunStage::Checking;
        run.checks_since = Some(now);
        run.note = Some(format!(
            "The latest {base} changes its patch, so it is checked and reviewed again"
        ));
    });
    if let Err(why) = pushed.await {
        step.hold(core, RunHold::Fix, why);
    }
}

/// It is on the base, so the base has moved and any verdict on the old one is
/// over.
async fn landed(core: &Arc<Core>, step: &Step) {
    finish::merged(core, step, false).await;
    clear_red(core, &(step.project.clone(), step.run.base.clone()));
}

/// Merges it, in up to `tries` goes. False when it did not, whether it goes
/// round again or was held.
async fn land(core: &Arc<Core>, step: &Step, head: &str, tries: u32) -> bool {
    let Some(pull) = &step.run.pull else {
        return false;
    };
    step.note(core, RunStage::Merging, "Merging its pull request");
    let mut left = tries;
    let error = loop {
        left -= 1;
        match codehost::merge(core, pull, head).await {
            Ok(()) => {
                landed(core, step).await;
                return true;
            }
            Err(error) if left == 0 => break error,
            Err(_) => tokio::time::sleep(MERGE_RETRY_WAIT).await,
        }
    };
    if codehost::look(core, pull)
        .await
        .is_ok_and(|looked| looked.state == "merged")
    {
        landed(core, step).await;
        return true;
    }
    let (tree, base) = (step.tree(), step.run.base.clone());
    let fork = head.to_owned();
    let moved = blocking(move || {
        let tip = git::latest(&tree, &base)?;
        Ok(git::fork_point(&tree, &base, &fork)? != tip)
    })
    .await;
    if moved.unwrap_or(false) {
        go_again(core, step);
    } else {
        step.hold(
            core,
            RunHold::Fix,
            format!("Its pull request did not merge: {error}"),
        );
    }
    false
}

/// A change put on the stack, and where the stack stood after it.
struct Laid {
    at: usize,
    commits: Vec<String>,
    tip: String,
}

enum Trouble {
    Conflict,
    PatchChanged,
    NothingLeft,
}

#[derive(Default)]
struct Stack {
    laid: Vec<Laid>,
    /// Changes that cannot go on the base itself, by their place in the batch.
    troubled: Vec<(usize, Trouble)>,
}

/// Lays each change on the base and the ones before it, in `checkout`. A
/// change that only fails to sit on another queued change waits for that one
/// to land.
fn stack(checkout: &Path, base: &str, approved: &[String]) -> Result<Stack, String> {
    let mut stacked = Stack::default();
    let mut tip = base.to_owned();
    for (at, head) in approved.iter().enumerate() {
        let from = git::merge_base(checkout, base, head)?;
        let Some(new) = git::lay(checkout, &tip, &from, head)? else {
            if stacked.laid.is_empty() {
                stacked.troubled.push((at, Trouble::Conflict));
            }
            continue;
        };
        if new == tip {
            if stacked.laid.is_empty() {
                stacked.troubled.push((at, Trouble::NothingLeft));
            }
            continue;
        }
        if !git::same_patch(checkout, (&from, head), (&tip, &new))? {
            git::go_to(checkout, &tip)?;
            if stacked.laid.is_empty() {
                stacked.troubled.push((at, Trouble::PatchChanged));
            }
            continue;
        }
        let commits = git::commits_between(checkout, &tip, &new)?;
        stacked.laid.push(Laid {
            at,
            commits,
            tip: new.clone(),
        });
        tip = new;
    }
    Ok(stacked)
}

/// The command that decides this project's changes, once it may run. None
/// when it may not, and the front of the queue was given back saying why.
async fn command_of(core: &Arc<Core>, step: &Step) -> Option<(String, String)> {
    let (owner, project) = (core.clone(), step.project.clone());
    let check = blocking(move || Ok(super::project_check(&owner, &project).ok()))
        .await
        .ok()
        .flatten();
    let named = check
        .as_ref()
        .is_some_and(|check| matches!(check, Checker::Named(_)));
    let Some((label, command)) = check.and_then(|check| {
        let command = check.command()?.to_owned();
        Some((check.label(), command))
    }) else {
        step.hold(
            core,
            RunHold::Unblock,
            "This project no longer has a check command, so the merge queue cannot check it"
                .to_owned(),
        );
        return None;
    };
    if named
        && !(core.repo_above().tasks_config_trusted)(core, Path::new(&step.project), None).await
    {
        step.hold(
            core,
            RunHold::Unblock,
            format!("sikemux.json is not trusted, so its check, {label}, did not run; trust it and Ship it again"),
        );
        return None;
    }
    Some((label, command))
}

async fn check_together(core: &Arc<Core>, key: &LaneKey, steps: Vec<Step>, repairing: bool) {
    let _turn = merge_queue::checkout_turn().await;
    let front = steps[0].clone();
    let base = front.run.base.clone();
    let tip = match latest(&front).await {
        Ok(tip) => tip,
        Err(why) => return could_not_rebase(core, &front, &why),
    };
    let Some((label, command)) = command_of(core, &front).await else {
        return;
    };
    let dir = PathBuf::from(&front.project);
    let checkout = match worktree::agent::merge_queue(core, &dir, &tip).await {
        Ok(checkout) => checkout,
        Err(why) => {
            return front.hold(
                core,
                RunHold::Unblock,
                format!("The merge queue's checkout is not ready: {why}"),
            )
        }
    };
    let approved: Vec<String> = (steps.iter())
        .map(|step| step.run.approved.clone().unwrap_or_default())
        .collect();
    let on = tip.clone();
    let stacked = match blocking(move || stack(&checkout, &on, &approved).map_err(Into::into)).await
    {
        Ok(stacked) => stacked,
        Err(error) => return could_not_rebase(core, &front, &error.to_string()),
    };
    for (at, trouble) in &stacked.troubled {
        let step = &steps[*at];
        match trouble {
            Trouble::Conflict => conflicted(core, step).await,
            Trouble::NothingLeft => nothing_left(core, step),
            Trouble::PatchChanged => match rebase_onto_latest(step).await {
                Ok(Rebased::Moved(moved)) => review_again(core, step, moved).await,
                Ok(Rebased::Conflict) => conflicted(core, step).await,
                Ok(Rebased::NothingLeft) => nothing_left(core, step),
                Err(why) => could_not_rebase(core, step, &why),
            },
        }
    }
    let laid = stacked.laid;
    let Some(newest) = laid.last().map(|laid| laid.tip.clone()) else {
        return;
    };
    let words = match laid.len() {
        1 if repairing => format!("{base} fails its own check, so this is tried alone on it"),
        1 => format!("Running {label} on the latest {base}"),
        n => format!(
            "Running {label} on the latest {base}, with {} others",
            n - 1
        ),
    };
    for one in &laid {
        steps[one.at].note(core, RunStage::Merging, &words);
    }
    let batch = Batch {
        key,
        steps: &steps,
        laid: &laid,
        tip: &tip,
        label: &label,
        command: &command,
    };
    match checks::run_in_queue(core, &dir, &newest, &command, front.agent()).await {
        Ok(ran) if ran.passed => land_together(core, &batch, &ran.output).await,
        Ok(_) if repairing => did_not_mend(core, &front, &tip),
        Ok(ran) => failed_together(core, &batch, ran).await,
        Err(why) => front.hold(
            core,
            RunHold::Fix,
            format!("The merge queue's check did not run: {why}"),
        ),
    }
}

/// One check of some changes stacked on the latest base.
struct Batch<'a> {
    key: &'a LaneKey,
    steps: &'a [Step],
    laid: &'a [Laid],
    tip: &'a str,
    label: &'a str,
    command: &'a str,
}

impl Batch<'_> {
    fn trust_one_at_a_time(&self, core: &Core, one_at_a_time: bool) {
        if let Some(lane) = core.boards().state.locked().lanes.get_mut(self.key) {
            lane.one_at_a_time = one_at_a_time;
        }
    }

    fn step(&self, laid: &Laid) -> &Step {
        &self.steps[laid.at]
    }
}

/// The check passed, so each change lands in turn, as long as what lands is
/// exactly what was checked. Those it does not reach wait for the next check.
async fn land_together(core: &Arc<Core>, batch: &Batch<'_>, output: &str) {
    batch.trust_one_at_a_time(core, false);
    let landed = land_in_turn(core, batch, output).await;
    for laid in &batch.laid[landed..] {
        let step = batch.step(laid);
        step.note(
            core,
            RunStage::Merging,
            &format!("Waiting in the merge queue for {}", step.run.base),
        );
    }
}

/// How many of the batch landed before one could not.
async fn land_in_turn(core: &Arc<Core>, batch: &Batch<'_>, output: &str) -> usize {
    for (landed, laid) in batch.laid.iter().enumerate() {
        let step = batch.step(laid);
        let Ok(Rebased::Moved(moved)) = rebase_onto_latest(step).await else {
            return landed;
        };
        let (tree, head, tested) = (step.tree(), moved.head.clone(), laid.tip.clone());
        let same = blocking(move || git::same_tree(&tree, &head, &tested).map_err(Into::into))
            .await
            .unwrap_or(false);
        if !same || !moved.carried {
            return landed;
        }
        let tries = match is_pushed(step, &moved) {
            true => 1,
            false => MERGE_TRIES_AFTER_PUSH,
        };
        if !is_pushed(step, &moved) {
            if let Err(why) = push_moved(core, step, &moved, |_, _| {}).await {
                step.hold(core, RunHold::Fix, why);
                return landed;
            }
            let run =
                super::slice::HeardRun::of(&moved.head, batch.command, Verdict::Passed, output);
            super::slice::auto_checked(core, &step.project, step.id, step.agent(), run);
        }
        if !land(core, step, &moved.head, tries).await {
            return landed;
        }
    }
    batch.laid.len()
}

/// The check failed: finds whose change broke it, and sends only that one
/// back. The rest stay in line, for the next check without it.
async fn failed_together(core: &Arc<Core>, batch: &Batch<'_>, failed: QueueRun) {
    let front = &batch.steps[0];
    let dir = PathBuf::from(&front.project);
    let stacked: Vec<Vec<String>> = batch.laid.iter().map(|laid| laid.commits.clone()).collect();
    let blamed = merge_queue::blame(
        core,
        &dir,
        batch.tip,
        &stacked,
        batch.command,
        front.agent(),
        failed.took_ms,
    )
    .await;
    match blamed {
        Err(why) => front.hold(
            core,
            RunHold::Fix,
            format!("The merge queue's check did not run: {why}"),
        ),
        Ok(Blame::BaseRed) => base_red(core, batch),
        Ok(Blame::Broke(at)) => send_back(core, batch, &batch.laid[at], &failed).await,
        Ok(Blame::Unknown) => {
            batch.trust_one_at_a_time(core, true);
            for laid in batch.laid {
                batch.step(laid).note(
                    core,
                    RunStage::Merging,
                    "Its check failed with others and could not say whose change broke it, so the queue goes one at a time",
                );
            }
        }
    }
}

async fn send_back(core: &Arc<Core>, batch: &Batch<'_>, culprit: &Laid, failed: &QueueRun) {
    batch.trust_one_at_a_time(core, false);
    let step = batch.step(culprit);
    let ahead = match batch.laid.first() {
        Some(first) if first.at != culprit.at => {
            ", with the changes ahead of it in the merge queue"
        }
        _ => "",
    };
    let what = format!(
        "{}\n\nThat is {} on top of the latest {base}{ahead}, and its reviewer already approved your change. Put {} on origin/{base} to see it.",
        crate::git::tail(&failed.output),
        batch.label,
        step.run.branch,
        base = step.run.base,
    );
    core.boards().edit(&step.project, step.id, bounced);
    finish::failed(core, step, &what).await;
}

/// The base fails its own check, not these changes. A change already tried
/// alone has had its try.
fn base_red(core: &Core, batch: &Batch<'_>) {
    mark_red(core, batch.key, batch.tip);
    if let [only] = batch.laid {
        return did_not_mend(core, batch.step(only), batch.tip);
    }
    for laid in batch.laid {
        let step = batch.step(laid);
        step.note(
            core,
            RunStage::Merging,
            &format!(
                "{} fails its own check, so each change is tried alone for one that mends it",
                step.run.base
            ),
        );
    }
}

pub(super) type Lanes = HashMap<LaneKey, Lane>;
