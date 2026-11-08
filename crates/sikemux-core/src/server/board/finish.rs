//! How a card Auto runs lands: its work is committed and pushed, a pull
//! request opened, and once CI, or the project's check command when no CI
//! runs, passes, merged. A failure gets one rebase and one fix by its agent;
//! anything more goes back to the person. While it waits for review, its
//! agent is handed reviewers' comments and its replies are posted for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::auto::{AutoPull, AutoRun, BoardSettings, Landing, RunHold, RunStage};
use super::review::{self, MAX_ROUNDS};
use super::{
    blind, change_card, git, guard, queue, recover, send_notices, By, Card, CardStatus, Changes,
    Checker,
};
use crate::server::codehost::ci;
use crate::server::codehost::repo::{self as codehost, Checks, Feedback, CI_START_WAIT_MS};
use crate::server::connection::blocking;
use crate::server::wake::sooner;
use crate::server::worktree;
use crate::server::{messages, Core};
use sikemux_view::slice::Verdict;

/// How long a run whose checks could not be read waits before it reads them again.
const CHECKS_RETRY_MS: u64 = 60_000;
const REVIEW_EVERY_MS: u64 = 60_000;
/// How many times the wait between looks at a pull request doubles while
/// nothing on it changes.
const MAX_QUIET_DOUBLINGS: u64 = 3;
/// How long a run's agent may be missing before Auto takes it that the agent
/// is gone, such as after Sikemux's background process restarted without it.
const GONE_AFTER_MS: u64 = 60_000;
const MAX_TITLE_CHARS: usize = 120;
/// What a check run on a pull request's code host is called on its slice.
const PULL_CHECKS: &str = "its pull request's checks";

/// Changes the card's run, if it is still `agent`'s and Auto may move it on.
/// False when it is not.
pub(super) fn change_run(
    core: &Core,
    project: &str,
    id: u64,
    agent: &str,
    change: impl FnOnce(&mut Card, &mut AutoRun),
) -> bool {
    core.boards
        .edit_if(project, id, |card| {
            if card.run().is_none_or(|run| run.agent != agent) {
                return None;
            }
            let mut run = card.auto.take()?;
            change(card, &mut run);
            card.auto = Some(run);
            Some(())
        })
        .is_some()
}

pub(super) fn note(core: &Core, project: &str, id: u64, agent: &str, stage: RunStage, note: &str) {
    change_run(core, project, id, agent, |_, run| {
        run.stage = stage;
        run.held = None;
        run.note = Some(note.to_owned());
    });
}

fn agent_cost(core: &Core, agent: &str) -> Option<f64> {
    core.chats.get(agent)?.feed.cost_usd()
}

/// What the run cost: all its agents when any reported it, else its last agent's own count.
fn run_cost(core: &Core, run: &AutoRun) -> Option<f64> {
    run.burn.spent().or_else(|| agent_cost(core, &run.agent))
}

/// Marks the run as given back to the person.
pub(super) fn mark_held(
    core: &Core,
    card: &mut Card,
    run: &mut AutoRun,
    need: RunHold,
    why: String,
) {
    queue::leave(run, card.record.as_mut());
    run.verdict = None;
    run.recovery.due = None;
    run.recovery.fresh = false;
    if run.worktree.is_empty() {
        super::to_open(card);
    }
    run.held = Some(need);
    run.note = Some(why);
    if let Some(record) = card.record.as_mut() {
        record.cost_usd = run_cost(core, run).or(record.cost_usd);
    }
}

/// Gives the card back to the person, for `need`, saying why. A run that never
/// got its worktree leaves the card open and nobody's, for the person to start
/// again.
pub(crate) fn hold(core: &Core, project: &str, id: u64, agent: &str, need: RunHold, why: String) {
    let mut reviewer = None;
    change_run(core, project, id, agent, |card, run| {
        reviewer = run.reviewer.take();
        mark_held(core, card, run, need, why);
    });
    if let Some(reviewer) = reviewer {
        blind::dismiss_later(core, reviewer);
    }
}

/// A run Auto no longer holds, because its card was moved or taken off the
/// board, whose agent and worktree are to go.
pub(super) struct Dropped {
    pub project: String,
    pub id: u64,
    pub run: AutoRun,
    /// The card is to be done afresh, so its pull request and pushed branch
    /// go too.
    pub discard: bool,
}

/// Stops the agents of runs Auto no longer holds and clears their worktrees
/// away, each once no step of it is under way.
pub(super) fn clear_dropped(core: &Arc<Core>) {
    blind::clear_dismissed(core);
    let ready: Vec<Dropped> = {
        let mut state = core.boards.state.locked();
        if state.dropped.is_empty() {
            return;
        }
        let dropped = std::mem::take(&mut state.dropped);
        let (ready, waiting): (Vec<Dropped>, Vec<Dropped>) = dropped
            .into_iter()
            .partition(|dropped| !state.busy(&dropped.project, dropped.id));
        state.dropped = waiting;
        for dropped in &ready {
            state.work(&dropped.project, dropped.id).job = true;
        }
        ready
    };
    for dropped in ready {
        let core = core.clone();
        tokio::spawn(async move {
            let (project, id) = (dropped.project.clone(), dropped.id);
            clear(&core, dropped).await;
            core.boards.end_job(&project, id);
        });
    }
}

async fn clear(core: &Arc<Core>, dropped: Dropped) {
    let Dropped {
        mut run, discard, ..
    } = dropped;
    if let Some(reviewer) = run.reviewer.take() {
        blind::dismiss(core, reviewer).await;
    }
    if core.chats.stop(&run.agent) {
        core.note_agent_gone(&run.agent);
    }
    if let (true, Some(pull)) = (discard, &run.pull) {
        if let Err(error) = codehost::close(core, pull).await {
            eprintln!(
                "sikemux core: Auto left the pull request {} open: {error}",
                pull.url
            );
        }
    }
    if run.worktree.is_empty() {
        return;
    }
    let remote = discard || run.pull.is_none();
    let (repo, tree, branch) = (
        PathBuf::from(&run.repo),
        PathBuf::from(&run.worktree),
        run.branch,
    );
    let _ = blocking(move || {
        worktree::remove_with_remote(&repo, &tree, &branch, remote);
        Ok(())
    })
    .await;
}

/// What a step of a run works from, read once when it starts.
#[derive(Clone)]
pub(super) struct Step {
    pub project: String,
    pub id: u64,
    pub card: Card,
    pub run: AutoRun,
    pub settings: BoardSettings,
}

impl Step {
    /// A check on its pushed head ended, which its slice keeps as evidence.
    fn checked(&self, core: &Arc<Core>, command: &str, verdict: Verdict, output: &str) {
        if let Some(head) = &self.run.head {
            if verdict == Verdict::Passed {
                guard::check_passed(core, self, head);
            }
            let run = super::slice::HeardRun::of(head, command, verdict, output);
            super::slice::auto_checked(core, &self.project, self.id, self.agent(), run);
        }
    }

    pub(super) fn tree(&self) -> PathBuf {
        PathBuf::from(&self.run.worktree)
    }

    pub(super) fn agent(&self) -> &str {
        &self.run.agent
    }

    fn pull_url(&self) -> String {
        self.run
            .pull
            .as_ref()
            .map(|pull| pull.url.clone())
            .unwrap_or_default()
    }

    pub(super) fn name(&self, core: &Core) -> String {
        core.names
            .name(self.agent())
            .or_else(|| self.card.owner.as_ref()?.name.clone())
            .unwrap_or_else(|| "Its agent".to_owned())
    }

    pub(super) fn hold(&self, core: &Core, need: RunHold, why: String) {
        hold(core, &self.project, self.id, self.agent(), need, why);
    }

    pub(super) fn note(&self, core: &Core, stage: RunStage, text: &str) {
        note(core, &self.project, self.id, self.agent(), stage, text);
    }
}

/// Runs `work` on the card unless a step of it is under way already.
fn step<F, Fut>(core: &Arc<Core>, project: String, id: u64, stage: Option<RunStage>, work: F)
where
    F: FnOnce(Arc<Core>, Step) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let step = {
        let mut state = core.boards.state.locked();
        if state.busy(&project, id) {
            if stage == Some(RunStage::Shipping) {
                state.work(&project, id).reship = true;
            }
            return;
        }
        let Some(board) = state.boards.get_mut(&project) else {
            return;
        };
        let settings = board.settings.clone();
        let Ok(card) = board.card_mut(id) else {
            return;
        };
        let restarts = stage.is_some() && card.kept_run().is_some();
        if !restarts && card.run().is_none() {
            return;
        }
        let Some(run) = card.auto.as_mut() else {
            return;
        };
        if let Some(stage) = stage {
            run.stage = stage;
            run.held = None;
            queue::leave(run, card.record.as_mut());
        }
        let step = Step {
            project: project.clone(),
            id,
            run: run.clone(),
            card: card.clone(),
            settings,
        };
        state.work(&project, id).job = true;
        if stage.is_some() {
            core.boards.save(&mut state, &project);
        }
        step
    };
    let core = core.clone();
    tokio::spawn(async move {
        let (project, id) = (step.project.clone(), step.id);
        if !recover::rescued(&core, &step) {
            work(core.clone(), step).await;
        }
        end_step(&core, project, id);
    });
}

/// A step of the card ended, so whatever waited for it can go on.
pub(super) fn end_step(core: &Arc<Core>, project: String, id: u64) {
    let mut again = false;
    core.boards
        .state
        .locked()
        .change_work(&project, id, |work| {
            again = std::mem::take(&mut work.reship)
        });
    core.boards.end_job(&project, id);
    if again {
        ship(core, project, id);
    }
}

/// Starts a card Auto gave back to the person again: from the start when it
/// never got a worktree, else by shipping its work afresh.
pub(super) fn retry(core: &Arc<Core>, project: &str, id: u64) -> Result<(), String> {
    let ships = {
        let mut state = core.boards.state.locked();
        let card = state
            .boards
            .get_mut(project)
            .ok_or_else(|| format!("There is no card #{id} on this project's board"))?
            .card_mut(id)?;
        let run = card
            .auto
            .as_mut()
            .filter(|run| run.held.is_some() || run.stage == RunStage::Watching)
            .ok_or("Auto has not given that card back to you")?;
        let ships = !run.worktree.is_empty();
        if ships {
            run.rebased = false;
            run.fixed = false;
            run.review.rounds = 0;
            run.recovery.tries = 0;
            run.recovery.failure = None;
            run.recovery.why = None;
            run.recovery.fresh = false;
            run.burn.start_over();
        } else {
            card.auto = None;
        }
        card.updated_at = epoch_ms();
        core.boards.save(&mut state, project);
        ships
    };
    if ships {
        ship(core, project.to_owned(), id);
    }
    Ok(())
}

/// Ships the card's work: commit, push, pull request, then its checks.
pub(super) fn ship(core: &Arc<Core>, project: String, id: u64) {
    step(
        core,
        project,
        id,
        Some(RunStage::Shipping),
        |core, step| async move {
            step.note(&core, RunStage::Shipping, "Committing and pushing its work");
            if let Err(why) = shipping(&core, &step).await {
                step.hold(&core, RunHold::Fix, why);
            }
        },
    );
}

fn commit_message(card: &Card) -> String {
    let title: String = card.title.chars().take(MAX_TITLE_CHARS).collect();
    let mut message = title;
    if let Some(ticket) = &card.ticket {
        message.push_str(&format!("\n\n{}", ticket.key));
    }
    message
}

fn pull_body(core: &Core, step: &Step) -> String {
    let card = &step.card;
    let mut body = String::new();
    if !card.details.is_empty() {
        body.push_str(&card.details);
        body.push_str("\n\n");
    }
    if let Some(result) = current_result(core, step) {
        body.push_str(&result);
        body.push_str("\n\n");
    }
    if let Some(ticket) = &card.ticket {
        match &ticket.url {
            Some(url) => body.push_str(&format!("Ticket: [{}]({url})\n\n", ticket.key)),
            None => body.push_str(&format!("Ticket: {}\n\n", ticket.key)),
        }
    }
    if let Some(evidence) = current_evidence(core, step) {
        body.push_str(&evidence);
        body.push('\n');
    }
    body.push_str(&format!(
        "Done by {} for card #{} of the board, opened by Sikemux's Auto.",
        step.name(core),
        card.id
    ));
    body
}

/// The evidence the card's slice holds now, as its pull request's description lists it.
fn current_evidence(core: &Core, step: &Step) -> Option<String> {
    let host = core.notify.host_name();
    let state = core.boards.state.locked();
    let card = state.boards.get(&step.project)?.card(step.id).ok()?;
    card.slice.as_ref()?.pull_evidence(&host)
}

fn current_result(core: &Core, step: &Step) -> Option<String> {
    let state = core.boards.state.locked();
    state
        .boards
        .get(&step.project)?
        .card(step.id)
        .ok()?
        .result
        .clone()
}

async fn shipping(core: &Arc<Core>, step: &Step) -> Result<(), String> {
    let tree = step.tree();
    let repo = PathBuf::from(&step.run.repo);
    let message = commit_message(&step.card);
    let start = step.run.start_sha.clone();
    let (ahead, origin) = {
        let (tree, repo) = (tree.clone(), repo.clone());
        let (core, agent) = (core.clone(), step.agent().to_owned());
        blocking(move || {
            git::commit_all(&core, &tree, &message, &agent)?;
            let ahead = git::ahead(&tree, &start)?;
            Ok((ahead, crate::git::origin(&repo)))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    if ahead == 0 {
        return Err(format!(
            "{} finished without a change to merge; read its answer, then mark the card done, or move it back to Open for Auto to start it over",
            step.name(core)
        ));
    }
    let origin = origin.ok_or("the project has no origin remote to open a pull request on")?;
    let picks = codehost::picks(core, Path::new(&step.project));
    let host = codehost::find(core, &origin, &picks).await?;
    let (branch, pushed) = (step.run.branch.clone(), step.run.head.clone());
    let head = {
        let tree = tree.clone();
        blocking(move || {
            git::push(&tree, &branch, pushed.as_deref())?;
            git::head(&tree).map_err(Into::into)
        })
        .await
        .map_err(|error| format!("the push failed: {error}"))?
    };
    let pull = match &step.run.pull {
        Some(pull) => pull.clone(),
        None => codehost::open(
            &host,
            &step.card.title,
            &step.run.branch,
            &step.run.base,
            &pull_body(core, step),
        )
        .await
        .map_err(|error| format!("the pull request did not open: {error}"))?,
    };
    let asked = &step.run.review.asked;
    let answered = if asked.is_empty() {
        None
    } else {
        let said = core
            .chats
            .get(step.agent())
            .and_then(|chat| chat.feed.last_answer());
        let answers = review::answers(asked, said.as_deref(), &step.name(core), &head);
        codehost::answer(core, &pull, &answers.replies, &answers.reviewers)
            .await
            .map_err(|error| {
                format!(
                    "It pushed its answers to the review, but its replies did not post: {error}"
                )
            })?;
        Some(answers)
    };
    let stuck = answered
        .as_ref()
        .map(|answers| answers.stuck.join("\n"))
        .filter(|stuck| !stuck.is_empty());
    let stop = if let Some(stuck) = &stuck {
        Some(format!(
            "{} could not settle these review comments:\n{stuck}",
            step.name(core)
        ))
    } else if let (Landing::Person, Some(answers)) = (step.settings.landing, &answered) {
        Some(format!(
            "It answered {} review comment{}, and its pull request waits for review again",
            answers.replies.len(),
            if answers.replies.len() == 1 { "" } else { "s" }
        ))
    } else if step.settings.landing == Landing::Person {
        Some("Its pull request waits for you to review and merge it".to_owned())
    } else {
        unanswered(core, step)
    };
    let now = epoch_ms();
    change_run(core, &step.project, step.id, step.agent(), |card, run| {
        super::slice::auto_shipped(card, &run.start_sha, &head);
        run.pull = Some(pull);
        run.head = Some(head);
        run.checks_since = Some(now);
        run.review.asked.clear();
        match stop {
            Some(why) => {
                run.stage = RunStage::Watching;
                run.held = stuck.is_some().then_some(RunHold::Fix);
                run.note = Some(why);
            }
            None => {
                run.stage = RunStage::Checking;
                run.note = Some("Waiting for its checks".to_owned());
            }
        }
    });
    Ok(())
}

/// The pull request of each run waiting on its checks, and the repository of
/// each project whose slices wait on a workflow, as the code-host
/// repositories the core's CI check watches for them.
pub(crate) fn waiting_on_ci(core: &Core) -> Vec<ci::Shown> {
    let state = core.boards.state.locked();
    state
        .boards
        .values()
        .flat_map(|board| &board.cards)
        .filter_map(|card| {
            let run = card.run().filter(|run| run.stage == RunStage::Checking)?;
            let pull = run.pull.as_ref()?;
            Some(ci::Shown {
                id: pull.repo_id(),
                account: pull.account.clone(),
                host: None,
            })
        })
        .chain(super::workflows::waiting_on_ci(&state))
        .collect()
}

/// The runs on `repo` moved, so the runs waiting on their checks there look
/// again now rather than at their next fallback.
pub(crate) fn ci_moved(core: &Core, repo: &ci::Shown) {
    let mut state = core.boards.state.locked();
    let waiting: Vec<(String, u64)> = state
        .boards
        .iter()
        .flat_map(|(project, board)| board.cards.iter().map(move |card| (project, card)))
        .filter(|(_, card)| {
            card.run().is_some_and(|run| {
                run.stage == RunStage::Checking
                    && run
                        .pull
                        .as_ref()
                        .is_some_and(|pull| pull.repo_id() == repo.id)
            })
        })
        .map(|(project, card)| (project.clone(), card.id))
        .collect();
    for (project, id) in &waiting {
        state.change_work(project, *id, |work| work.looked = None);
    }
    let workflows = super::workflows::ci_moved(&mut state, repo);
    drop(state);
    if !waiting.is_empty() || workflows {
        core.boards.wake.now();
    }
}

/// A run whose agent is in a turn, for what watches turns as they go.
struct WatchedTurn {
    project: String,
    id: u64,
    stage: RunStage,
    agent: String,
    recovers: bool,
    timed_since: Option<u64>,
    /// Where its agent works, when losing it brings the worktree back.
    worktree: Option<PathBuf>,
}

/// Looks at the checks of runs waiting on them, at pull requests waiting on
/// the person, and takes up steps a restart cut short. Answers when a run is
/// next due a look.
pub(super) fn due(core: &Arc<Core>) -> Option<u64> {
    super::backup::start_due(core);
    let now = epoch_ms();
    let running = core.running_agents();
    let mut next = None;
    let mut checking_any = false;
    let due: (Vec<(String, u64, RunStage)>, Vec<WatchedTurn>) = {
        let mut state = core.boards.state.locked();
        let mut due = Vec::new();
        let mut present = Vec::new();
        let mut missing = Vec::new();
        let mut watched_turns = Vec::new();
        for (project, board) in &state.boards {
            for card in &board.cards {
                let Some(run) = card.run() else { continue };
                let work = state.work_of(project, card.id);
                if work.is_some_and(|work| work.job) {
                    continue;
                }
                if run.stage == RunStage::Merging && run.queue.is_some() && board.settings.queues()
                {
                    continue;
                }
                let every = match run.stage {
                    RunStage::Checking => {
                        checking_any = true;
                        let looked = work.and_then(|work| work.looked);
                        let unread = work
                            .and_then(|work| work.checks_unread)
                            .map(|at| at + CHECKS_RETRY_MS);
                        let start_deadline = run.checks_since.unwrap_or(0) + CI_START_WAIT_MS;
                        let at_deadline = looked
                            .is_some_and(|looked| looked < start_deadline)
                            .then_some(start_deadline);
                        let again = match looked {
                            None => Some(now),
                            Some(_) => sooner(unread, at_deadline),
                        };
                        match again {
                            Some(at) if at <= now => {
                                due.push((project.clone(), card.id, run.stage))
                            }
                            at => next = sooner(next, at),
                        }
                        continue;
                    }
                    RunStage::Watching => {
                        let quiet = work.map_or(0, |work| work.quiet);
                        REVIEW_EVERY_MS << quiet.min(MAX_QUIET_DOUBLINGS)
                    }
                    RunStage::Shipping | RunStage::Merging | RunStage::Starting => 0,
                    RunStage::Reviewing | RunStage::Contracting
                        if run.verdict.is_some() || run.reviewer.is_none() =>
                    {
                        0
                    }
                    RunStage::Reviewing
                        if run
                            .reviewer
                            .as_ref()
                            .is_some_and(|reviewer| reviewer.waiting_since.is_some()) =>
                    {
                        continue;
                    }
                    RunStage::Working | RunStage::Fixing if run.recovery.due.is_some() => {
                        match run.recovery.due {
                            Some(at) if at <= now => {
                                due.push((project.clone(), card.id, run.stage))
                            }
                            at => next = sooner(next, at),
                        }
                        continue;
                    }
                    RunStage::Working
                    | RunStage::Fixing
                    | RunStage::Reviewing
                    | RunStage::Contracting => {
                        let key = (project.clone(), card.id);
                        let watched = match (&run.reviewer, run.stage) {
                            (Some(reviewer), RunStage::Reviewing | RunStage::Contracting) => {
                                &reviewer.agent
                            }
                            _ => &run.agent,
                        };
                        if running.contains(watched) {
                            if (board.settings.recover
                                || board.settings.progress_guard
                                || board.settings.backs_up())
                                && matches!(
                                    run.stage,
                                    RunStage::Working | RunStage::Fixing | RunStage::Reviewing
                                )
                            {
                                watched_turns.push(WatchedTurn {
                                    project: key.0.clone(),
                                    id: key.1,
                                    stage: run.stage,
                                    agent: watched.clone(),
                                    recovers: board.settings.recover,
                                    timed_since: guard::timed_since(board, card, run),
                                    worktree: (board.settings.backs_up()
                                        && matches!(
                                            run.stage,
                                            RunStage::Working | RunStage::Fixing
                                        ))
                                    .then(|| PathBuf::from(&run.worktree)),
                                });
                            }
                            present.push(key);
                        } else {
                            missing.push((key, run.stage));
                        }
                        continue;
                    }
                };
                let last = work.and_then(|work| work.looked).unwrap_or(0);
                if every > 0 && now.saturating_sub(last) < every {
                    next = sooner(next, Some(last + every));
                    continue;
                }
                due.push((project.clone(), card.id, run.stage));
            }
        }
        for (project, id, _) in &due {
            let work = state.work(project, *id);
            work.looked = Some(now);
            work.checks_unread = None;
        }
        for (project, id) in present {
            state.change_work(&project, id, |work| work.missing_since = None);
        }
        for ((project, id), stage) in missing {
            let since = *state.work(&project, id).missing_since.get_or_insert(now);
            if now.saturating_sub(since) >= GONE_AFTER_MS {
                state.change_work(&project, id, |work| work.missing_since = None);
                due.push((project, id, stage));
            } else {
                next = sooner(next, Some(since + GONE_AFTER_MS));
            }
        }
        (due, watched_turns)
    };
    let (mut due, watched_turns) = due;
    for turn in watched_turns {
        let WatchedTurn {
            project,
            id,
            stage,
            agent,
            recovers,
            timed_since,
            worktree,
        } = turn;
        if worktree.is_some_and(|tree| !tree.is_dir()) {
            due.push((project, id, stage));
            continue;
        }
        if let Some(looked) = timed_since {
            next = sooner(
                next,
                Some(guard::look(core, &project, id, &agent, looked, recovers)),
            );
        }
        let hangs = recovers.then(|| recover::hangs_at(core, &agent)).flatten();
        match hangs {
            Some(at) if at <= now => due.push((project, id, stage)),
            at => next = sooner(next, at),
        }
    }
    if checking_any {
        ci::start(core);
    }
    for (project, id) in blind::admit(core) {
        step(core, project, id, None, |core, step| async move {
            blind::take_place(&core, step).await;
        });
    }
    for (project, id, stage) in due {
        match stage {
            RunStage::Checking | RunStage::Merging => {
                step(core, project, id, None, |core, step| async move {
                    checking(&core, step).await;
                })
            }
            RunStage::Watching => step(core, project, id, None, |core, step| async move {
                reviewing(&core, step).await;
            }),
            RunStage::Reviewing | RunStage::Contracting => {
                step(core, project, id, None, |core, step| async move {
                    blind::settle(&core, step).await;
                })
            }
            RunStage::Shipping => ship(core, project, id),
            RunStage::Starting => step(core, project, id, None, |core, step| async move {
                step.hold(
                    &core,
                    RunHold::Unblock,
                    "Sikemux's background process restarted while Auto was starting it".to_owned(),
                );
            }),
            RunStage::Working | RunStage::Fixing => {
                step(core, project, id, None, |core, step| async move {
                    recover::settle(&core, step).await;
                })
            }
        }
    }
    next
}

async fn checking(core: &Arc<Core>, step: Step) {
    let (Some(pull), Some(head)) = (step.run.pull.clone(), step.run.head.clone()) else {
        ship(core, step.project.clone(), step.id);
        return;
    };
    let checks = match codehost::checks(core, &pull, &step.run.branch, &head).await {
        Ok(checks) => checks,
        Err(error) => {
            core.boards
                .state
                .locked()
                .work(&step.project, step.id)
                .checks_unread = Some(epoch_ms());
            step.note(
                core,
                RunStage::Checking,
                &format!("Could not read its checks: {error}"),
            );
            return;
        }
    };
    let waited = epoch_ms().saturating_sub(step.run.checks_since.unwrap_or(0));
    match checks {
        Checks::Running { .. } => step.note(core, RunStage::Checking, "Its checks are running"),
        Checks::Passed => {
            step.checked(core, PULL_CHECKS, Verdict::Passed, "");
            passed(core, &step, &pull, &head, true).await
        }
        Checks::Failed(what) => {
            step.checked(core, PULL_CHECKS, Verdict::Failed, &what);
            if !queue::base_answers_for(core, &step, &pull).await {
                failed(core, &step, &what).await
            }
        }
        Checks::Stopped(why) => step.hold(
            core,
            RunHold::Unblock,
            format!("Its checks were cancelled or never started:\n{why}"),
        ),
        Checks::None if waited < CI_START_WAIT_MS => {
            step.note(core, RunStage::Checking, "Waiting for its checks to start");
        }
        Checks::None => {
            let (owner, project) = (core.clone(), step.project.clone());
            let check = blocking(move || Ok(super::project_check(&owner, &project).ok())).await;
            let check = check.map(|check| {
                check.and_then(|check| {
                    let named = matches!(check, Checker::Named(_));
                    Some((check.label(), check.command()?.to_owned(), named))
                })
            });
            match check {
            Ok(Some((label, _, true)))
                if !crate::server::tasks::config_trusted(core, Path::new(&step.project), None).await =>
            {
                step.hold(
                    core,
                    RunHold::Unblock,
                    format!("sikemux.json is not trusted, so its check, {label}, did not run; trust it and Ship it again"),
                );
            }
            Ok(Some((label, command, _))) => {
                step.note(core, RunStage::Checking, &format!("Running {label}"));
                let (cwd, running) = (step.tree(), command.clone());
                let checked = blocking(move || crate::server::checks::run_command(&cwd, &running).map_err(Into::into)).await;
                match checked {
                    Ok((true, output)) => {
                        step.checked(core, &command, Verdict::Passed, &output);
                        passed(core, &step, &pull, &head, false).await
                    }
                    Ok((false, output)) => {
                        step.checked(core, &command, Verdict::Failed, &output);
                        failed(core, &step, &format!("The check command failed:\n{output}")).await;
                    }
                    Err(error) => step.hold(core, RunHold::Fix, format!("The check command did not run: {error}")),
                }
            }
            Ok(_) => step.note(
                core,
                RunStage::Watching,
                "No CI ran on its pull request and this project has no check command, so it waits for you",
            ),
            Err(error) => step.hold(
                core,
                RunHold::Fix,
                format!("The check command could not be read: {error}"),
            ),
            }
        }
    }
}

/// The checks passed: the change merges once a reviewer approved this very
/// commit, and goes to one first otherwise.
async fn passed(core: &Arc<Core>, step: &Step, pull: &AutoPull, head: &str, on_host: bool) {
    change_run(core, &step.project, step.id, step.agent(), |_, run| {
        run.checked_on_host = on_host;
    });
    if step.run.approved.as_deref() == Some(head) {
        return merge(core, step, pull, head).await;
    }
    if let Some(why) = unanswered(core, step) {
        step.note(core, RunStage::Watching, &why);
        return;
    }
    blind::begin(core, step, head).await;
}

/// The checks failed: the agent gets one go at fixing them.
pub(super) async fn failed(core: &Arc<Core>, step: &Step, what: &str) {
    if guard::check_failed(core, step, what) {
        return;
    }
    if step.run.fixed {
        step.hold(
            core,
            RunHold::Fix,
            format!(
                "Its checks failed again after a fix:\n{}",
                crate::git::tail(what)
            ),
        );
        return;
    }
    let url = step.pull_url();
    let text = format!(
        "The checks on your pull request {url} failed:\n\n{}\n\nFix them on {} and commit. When your turn ends, Sikemux pushes and checks again. This is the one fix Auto asks for: if they fail again, the card goes back to the person.",
        crate::git::tail(what),
        step.run.branch
    );
    ask(
        core,
        step,
        text,
        "Its checks failed, so it is fixing them",
        |run| {
            run.fixed = true;
        },
    )
    .await;
}

/// Sends the agent back to work on its card with `text`.
pub(super) async fn ask(
    core: &Arc<Core>,
    step: &Step,
    text: String,
    note: &str,
    change: impl FnOnce(&mut AutoRun),
) {
    let name = step.name(core);
    change_run(core, &step.project, step.id, step.agent(), |card, run| {
        change(run);
        queue::leave(run, card.record.as_mut());
        run.stage = RunStage::Fixing;
        run.note = Some(note.to_owned());
    });
    if let Err(error) = messages::from_auto(core, step.agent().to_owned(), text).await {
        step.hold(
            core,
            RunHold::Unblock,
            format!("{name} could not be reached: {error}"),
        );
    }
}

/// Why the card waits rather than merging: its agent, or an agent working under
/// it, has a question nobody has answered.
pub(super) fn unanswered(core: &Core, step: &Step) -> Option<String> {
    let team = core.names.team(step.agent());
    let asker = team
        .iter()
        .find(|agent| core.harness.questions.has_open(agent))?;
    let who = if asker == step.agent() {
        step.name(core)
    } else {
        let name = core.names.name_or_id(asker);
        format!("{name}, working under {},", step.name(core))
    };
    Some(format!(
        "{who} has a question nobody has answered, so its pull request waits"
    ))
}

pub(super) async fn merge(core: &Arc<Core>, step: &Step, pull: &AutoPull, head: &str) {
    if let Some(why) = unanswered(core, step) {
        step.note(core, RunStage::Watching, &why);
        return;
    }
    if step.settings.queues() {
        return queue::arrived(core, step, head).await;
    }
    step.note(core, RunStage::Merging, "Merging its pull request");
    let error = match codehost::merge(core, pull, head).await {
        Ok(()) => return merged(core, step, false).await,
        Err(error) => error,
    };
    if codehost::look(core, pull)
        .await
        .is_ok_and(|looked| looked.state == "merged")
    {
        return merged(core, step, false).await;
    }
    if step.run.rebased {
        step.hold(
            core,
            RunHold::Fix,
            format!("Its pull request did not merge: {error}"),
        );
        return;
    }
    let (tree, base, branch) = (step.tree(), step.run.base.clone(), step.run.branch.clone());
    let pushed = head.to_owned();
    let rebased = blocking(move || {
        if !git::rebase(&tree, &base)? {
            return Ok(None);
        }
        git::push(&tree, &branch, Some(&pushed))?;
        let head = git::head(&tree)?;
        let from = git::fork_point(&tree, &base, &head)?;
        Ok(Some((from, head)))
    })
    .await;
    match rebased {
        Ok(Some((from, head))) => {
            let now = epoch_ms();
            change_run(core, &step.project, step.id, step.agent(), |card, run| {
                super::slice::auto_shipped(card, &from, &head);
                run.rebased = true;
                run.head = Some(head);
                run.checks_since = Some(now);
                run.stage = RunStage::Checking;
                run.note = Some(format!(
                    "It did not merge, so Auto put it on the latest {} and waits for its checks again",
                    run.base
                ));
            });
        }
        Ok(None) => conflicted(core, step, &error).await,
        Err(failure) => step.hold(
            core,
            RunHold::Fix,
            format!("It did not merge ({error}), and rebasing it failed: {failure}"),
        ),
    }
}

/// It does not sit on the latest base, for `why`: its agent gets one go at
/// rebasing it, and the person after that.
pub(super) async fn conflicted(core: &Arc<Core>, step: &Step, why: &str) {
    if step.run.fixed {
        step.hold(
            core,
            RunHold::Fix,
            format!(
                "It did not merge and conflicts with the latest {}: {why}",
                step.run.base
            ),
        );
        return;
    }
    let text = format!(
        "Your pull request did not merge ({why}), and {} does not rebase cleanly onto origin/{}. Rebase it onto origin/{}, resolve the conflicts, run the checks and commit. When your turn ends, Sikemux pushes and checks again. This is the one fix Auto asks for.",
        step.run.branch, step.run.base, step.run.base
    );
    ask(
        core,
        step,
        text,
        "It conflicts with the latest base, so it is rebasing",
        |run| run.fixed = true,
    )
    .await;
}

/// Its pull request merged, by the person when `by_person`: its slice lands
/// and the card is done, its agent stops and its worktree goes.
pub(super) async fn merged(core: &Arc<Core>, step: &Step, by_person: bool) {
    let url = step.pull_url();
    let mut all_passed = true;
    let kept = change_run(core, &step.project, step.id, step.agent(), |card, run| {
        if let Some(slice) = card.slice.as_mut() {
            all_passed = slice.merged(by_person, &step.settings);
        }
        queue::leave(run, card.record.as_mut());
        if let Some(record) = card.record.as_mut() {
            record.finished(run.review.rounds, run_cost(core, run));
        }
        run.note = Some("Merged".to_owned());
    });
    if !kept {
        return;
    }
    let result = match current_result(core, step) {
        Some(result) if !result.contains(&url) => format!("{result}\n\nMerged in {url}"),
        Some(result) => result,
        None => format!("Merged in {url}"),
    };
    let changes = Changes {
        status: all_passed.then_some(CardStatus::Done),
        result: Some(result),
        ..Changes::default()
    };
    let (core2, project, id) = (core.clone(), step.project.clone(), step.id);
    let notices = blocking(move || {
        change_card(&core2, &project, &By::Auto("Auto"), id, changes)
            .map(|(_, notices)| notices)
            .map_err(Into::into)
    })
    .await
    .unwrap_or_default();
    send_notices(core, notices);
    super::reservoir::landed(core, &step.project, step.id);
    if !all_passed {
        // Auto's part ended at the merge; what it deploys next is for whoever takes the card.
        core.boards.edit(&step.project, step.id, Card::auto_lets_go);
    }
    if core.chats.stop(step.agent()) {
        core.note_agent_gone(step.agent());
    }
    let (repo, tree, branch) = (
        PathBuf::from(&step.run.repo),
        step.tree(),
        step.run.branch.clone(),
    );
    let _ = blocking(move || {
        worktree::remove_with_remote(&repo, &tree, &branch, true);
        Ok(())
    })
    .await;
}

/// A pull request left for the person: merged by them closes the card,
/// closed without merging hands it back, and new review comments go to its
/// agent.
async fn reviewing(core: &Arc<Core>, step: Step) {
    let Some(pull) = step.run.pull.clone() else {
        return;
    };
    let Ok(looked) = codehost::look(core, &pull).await else {
        return;
    };
    match looked.state.as_str() {
        "merged" => return merged(core, &step, true).await,
        "closed" | "declined" | "superseded" => {
            return step.hold(
                core,
                RunHold::Unblock,
                "Its pull request was closed without merging".to_owned(),
            );
        }
        _ => {}
    }
    let mut seen = step.run.review.clone();
    if !seen.moved(&looked.mark) {
        return quiet(core, &step, true);
    }
    let Ok(found) = codehost::feedback(core, &pull, seen.since()).await else {
        return;
    };
    let new = seen.take(looked.mark, found);
    quiet(core, &step, new.is_empty());
    if new.is_empty() {
        change_run(core, &step.project, step.id, step.agent(), |_, run| {
            run.review = seen;
        });
    } else if seen.rounds >= MAX_ROUNDS {
        step.hold(
            core,
            RunHold::Unblock,
            format!(
                "Reviewers left more comments after {} answered {MAX_ROUNDS} rounds of them; read them on {}, then Ship it again for more rounds",
                step.name(core),
                pull.url
            ),
        );
    } else {
        hand_review(core, &step, seen, new).await;
    }
}

/// Counts the looks in a row at the card's pull request that found nothing new.
fn quiet(core: &Core, step: &Step, nothing_new: bool) {
    core.boards
        .state
        .locked()
        .change_work(&step.project, step.id, |work| {
            work.quiet = if nothing_new { work.quiet + 1 } else { 0 };
        });
}

async fn hand_review(core: &Arc<Core>, step: &Step, mut seen: review::Review, new: Vec<Feedback>) {
    let text = review::task(&new, &step.pull_url(), &step.run.branch, seen.rounds + 1);
    let note = format!(
        "Answering {} review comment{}",
        new.len(),
        if new.len() == 1 { "" } else { "s" }
    );
    seen.hand(new);
    ask(core, step, text, &note, |run| run.review = seen).await;
}
