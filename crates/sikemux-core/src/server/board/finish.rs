//! How a card Auto runs lands: its work is committed and pushed, a pull
//! request opened, and once CI, or the project's check command when no CI
//! runs, passes, merged. A failure gets one rebase and one fix by its agent;
//! anything more goes back to the person. While it waits for review, its
//! agent is handed reviewers' comments and its replies are posted for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::auto::{Finish, Pull, Run, Settings, Stage};
use super::review::{self, MAX_ROUNDS};
use super::{blind, change_card, git, send_notices, By, Card, Changes, Status};
use crate::server::codehost::ci;
use crate::server::codehost::repo::{self as codehost, Checks, Feedback};
use crate::server::connection::blocking;
use crate::server::wake::sooner;
use crate::server::worktree;
use crate::server::{messages, Core};

/// How long a pushed commit may go without a CI run before Auto takes it
/// that the project has none.
const CI_START_WAIT_MS: u64 = 3 * 60 * 1000;
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

/// Changes the card's run, if it is still `agent`'s. False when it is not.
pub(super) fn change_run(
    core: &Core,
    project: &str,
    id: u64,
    agent: &str,
    change: impl FnOnce(&mut Card, &mut Run),
) -> bool {
    let mut state = core.boards.state.locked();
    let Some(card) = state
        .boards
        .get_mut(project)
        .and_then(|board| board.card_mut(id).ok())
    else {
        return false;
    };
    let Some(mut run) = card
        .auto
        .take()
        .filter(|run| run.agent == agent && !run.stage.is_over())
    else {
        return false;
    };
    change(card, &mut run);
    card.auto = Some(run);
    card.updated_at = epoch_ms();
    core.boards.save(&mut state, project);
    true
}

pub(super) fn note(core: &Core, project: &str, id: u64, agent: &str, stage: Stage, note: &str) {
    change_run(core, project, id, agent, |_, run| {
        run.stage = stage;
        run.note = Some(note.to_owned());
    });
}

/// Gives the card back to the person, saying why. A run that never got its
/// worktree leaves the card open and nobody's, for the person to start again.
pub(crate) fn hold(core: &Core, project: &str, id: u64, agent: &str, why: String) {
    let mut reviewer = None;
    change_run(core, project, id, agent, |card, run| {
        reviewer = run.reviewer.take();
        run.verdict = None;
        if run.worktree.is_empty() {
            card.owner = None;
            card.team = None;
            card.status = Status::Open;
        }
        run.stage = Stage::Held;
        run.note = Some(why);
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
    pub run: Run,
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
    pub run: Run,
    pub settings: Settings,
}

impl Step {
    fn tree(&self) -> PathBuf {
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

    pub(super) fn hold(&self, core: &Core, why: String) {
        hold(core, &self.project, self.id, self.agent(), why);
    }

    pub(super) fn note(&self, core: &Core, stage: Stage, text: &str) {
        note(core, &self.project, self.id, self.agent(), stage, text);
    }
}

/// Runs `work` on the card unless a step of it is under way already.
fn step<F, Fut>(core: &Arc<Core>, project: String, id: u64, stage: Option<Stage>, work: F)
where
    F: FnOnce(Arc<Core>, Step) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let step = {
        let mut state = core.boards.state.locked();
        if state.busy(&project, id) {
            if stage == Some(Stage::Shipping) {
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
        let Some(run) = card.auto.as_mut().filter(|run| !run.stage.is_over()) else {
            return;
        };
        if let Some(stage) = stage {
            run.stage = stage;
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
        work(core.clone(), step).await;
        let mut again = false;
        core.boards
            .state
            .locked()
            .change_work(&project, id, |work| {
                again = std::mem::take(&mut work.reship)
            });
        core.boards.end_job(&project, id);
        if again {
            ship(&core, project, id);
        }
    });
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
            .filter(|run| matches!(run.stage, Stage::Held | Stage::Review))
            .ok_or("Auto has not given that card back to you")?;
        let ships = !run.worktree.is_empty();
        if ships {
            run.rebased = false;
            run.fixed = false;
            run.review.rounds = 0;
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
        Some(Stage::Shipping),
        |core, step| async move {
            step.note(&core, Stage::Shipping, "Committing and pushing its work");
            if let Err(why) = shipping(&core, &step).await {
                step.hold(&core, why);
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
    body.push_str(&format!(
        "Done by {} for card #{} of the board, opened by Sikemux's Auto.",
        step.name(core),
        card.id
    ));
    body
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
    } else if let (Finish::Pr, Some(answers)) = (step.settings.finish, &answered) {
        Some(format!(
            "It answered {} review comment{}, and its pull request waits for review again",
            answers.replies.len(),
            if answers.replies.len() == 1 { "" } else { "s" }
        ))
    } else if step.settings.finish == Finish::Pr {
        Some("Its pull request waits for you to review and merge it".to_owned())
    } else {
        unanswered(core, step)
    };
    let now = epoch_ms();
    change_run(core, &step.project, step.id, step.agent(), |_, run| {
        run.pull = Some(pull);
        run.head = Some(head);
        run.checks_since = Some(now);
        run.review.asked.clear();
        match stop {
            Some(why) => {
                run.stage = if stuck.is_some() {
                    Stage::Held
                } else {
                    Stage::Review
                };
                run.note = Some(why);
            }
            None => {
                run.stage = Stage::Checking;
                run.note = Some("Waiting for its checks".to_owned());
            }
        }
    });
    Ok(())
}

/// The pull request of each run waiting on its checks, as the code-host
/// repository the core's CI check watches for it.
pub(crate) fn waiting_on_ci(core: &Core) -> Vec<ci::Shown> {
    let state = core.boards.state.locked();
    state
        .boards
        .values()
        .flat_map(|board| &board.cards)
        .filter_map(|card| {
            let run = card
                .auto
                .as_ref()
                .filter(|run| run.stage == Stage::Checking)?;
            let pull = run.pull.as_ref()?;
            Some(ci::Shown {
                id: pull.repo_id(),
                account: pull.account.clone(),
                host: None,
            })
        })
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
            card.auto.as_ref().is_some_and(|run| {
                run.stage == Stage::Checking
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
    drop(state);
    if !waiting.is_empty() {
        core.boards.wake.now();
    }
}

/// Looks at the checks of runs waiting on them, at pull requests waiting on
/// the person, and takes up steps a restart cut short. Answers when a run is
/// next due a look.
pub(super) fn due(core: &Arc<Core>) -> Option<u64> {
    let now = epoch_ms();
    let running = core.running_agents();
    let mut next = None;
    let mut checking_any = false;
    let due: Vec<(String, u64, Stage)> = {
        let mut state = core.boards.state.locked();
        let mut due = Vec::new();
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for (project, board) in &state.boards {
            for card in &board.cards {
                let Some(run) = &card.auto else { continue };
                let work = state.work_of(project, card.id);
                if work.is_some_and(|work| work.job) {
                    continue;
                }
                let every = match run.stage {
                    Stage::Checking => {
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
                    Stage::Review => {
                        let quiet = work.map_or(0, |work| work.quiet);
                        REVIEW_EVERY_MS << quiet.min(MAX_QUIET_DOUBLINGS)
                    }
                    Stage::Shipping | Stage::Merging | Stage::Starting => 0,
                    Stage::Reviewing if run.verdict.is_some() || run.reviewer.is_none() => 0,
                    Stage::Working | Stage::Fixing | Stage::Reviewing => {
                        let key = (project.clone(), card.id);
                        let watched = match (&run.reviewer, run.stage) {
                            (Some(reviewer), Stage::Reviewing) => &reviewer.agent,
                            _ => &run.agent,
                        };
                        if running.contains(watched) {
                            present.push(key);
                        } else {
                            missing.push((key, run.stage));
                        }
                        continue;
                    }
                    _ => continue,
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
        due
    };
    if checking_any {
        ci::start(core);
    }
    for (project, id, stage) in due {
        match stage {
            Stage::Checking | Stage::Merging => {
                step(core, project, id, None, |core, step| async move {
                    checking(&core, step).await;
                })
            }
            Stage::Review => step(core, project, id, None, |core, step| async move {
                reviewing(&core, step).await;
            }),
            Stage::Reviewing => step(core, project, id, None, |core, step| async move {
                blind::settle(&core, step).await;
            }),
            Stage::Shipping => ship(core, project, id),
            Stage::Starting => step(core, project, id, None, |core, step| async move {
                step.hold(
                    &core,
                    "Sikemux's background process restarted while Auto was starting it".to_owned(),
                );
            }),
            Stage::Working | Stage::Fixing => {
                step(core, project, id, None, |core, step| async move {
                    if core.running_agents().contains(step.agent()) {
                        return;
                    }
                    step.hold(
                        &core,
                        format!(
                            "{} is no longer running, so it did not finish; Ship it again sends what it committed",
                            step.name(&core)
                        ),
                    );
                })
            }
            _ => {}
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
                Stage::Checking,
                &format!("Could not read its checks: {error}"),
            );
            return;
        }
    };
    let waited = epoch_ms().saturating_sub(step.run.checks_since.unwrap_or(0));
    match checks {
        Checks::Running => step.note(core, Stage::Checking, "Its checks are running"),
        Checks::Passed => passed(core, &step, &pull, &head).await,
        Checks::Failed(what) => failed(core, &step, &what).await,
        Checks::None if waited < CI_START_WAIT_MS => {
            step.note(core, Stage::Checking, "Waiting for its checks to start");
        }
        Checks::None => {
            let (owner, project) = (core.clone(), step.project.clone());
            let command = blocking(move || Ok(super::check_command(&owner, &project))).await;
            match command {
            Ok(Some(command)) => {
                step.note(core, Stage::Checking, &format!("Running {command}"));
                let cwd = step.tree();
                let checked = blocking(move || git::check(&cwd, &command).map_err(Into::into)).await;
                match checked {
                    Ok((true, _)) => passed(core, &step, &pull, &head).await,
                    Ok((false, output)) => {
                        failed(core, &step, &format!("The check command failed:\n{output}")).await;
                    }
                    Err(error) => step.hold(core, format!("The check command did not run: {error}")),
                }
            }
            Ok(None) => step.note(
                core,
                Stage::Review,
                "No CI ran on its pull request and this project has no check command, so it waits for you",
            ),
            Err(error) => step.hold(core, format!("The check command could not be read: {error}")),
            }
        }
    }
}

/// The checks passed: the change merges once a reviewer approved this very
/// commit, and goes to one first otherwise.
async fn passed(core: &Arc<Core>, step: &Step, pull: &Pull, head: &str) {
    if step.run.approved.as_deref() == Some(head) {
        return merge(core, step, pull, head).await;
    }
    if let Some(why) = unanswered(core, step) {
        step.note(core, Stage::Review, &why);
        return;
    }
    blind::begin(core, step, head).await;
}

/// The checks failed: the agent gets one go at fixing them.
async fn failed(core: &Arc<Core>, step: &Step, what: &str) {
    if step.run.fixed {
        step.hold(
            core,
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
    change: impl FnOnce(&mut Run),
) {
    let name = step.name(core);
    change_run(core, &step.project, step.id, step.agent(), |_, run| {
        change(run);
        run.stage = Stage::Fixing;
        run.note = Some(note.to_owned());
    });
    if let Err(error) = messages::from_auto(core, step.agent().to_owned(), text).await {
        step.hold(core, format!("{name} could not be reached: {error}"));
    }
}

/// Why the card waits for the person rather than merging: its agent, or an
/// agent working under it, has a question or an assumption they have not
/// answered.
fn unanswered(core: &Core, step: &Step) -> Option<String> {
    let team = core.names.team(step.agent());
    let asker = team
        .iter()
        .find(|agent| core.harness.questions.has_open(agent))?;
    let who = if asker == step.agent() {
        step.name(core)
    } else {
        let name = core.names.name(asker).unwrap_or_else(|| asker.clone());
        format!("{name}, working under {},", step.name(core))
    };
    Some(format!(
        "{who} has a question or an assumption you have not answered, so its pull request waits for you"
    ))
}

pub(super) async fn merge(core: &Arc<Core>, step: &Step, pull: &Pull, head: &str) {
    if let Some(why) = unanswered(core, step) {
        step.note(core, Stage::Review, &why);
        return;
    }
    step.note(core, Stage::Merging, "Merging its pull request");
    let error = match codehost::merge(core, pull, head).await {
        Ok(()) => return merged(core, step).await,
        Err(error) => error,
    };
    if codehost::look(core, pull)
        .await
        .is_ok_and(|looked| looked.state == "merged")
    {
        return merged(core, step).await;
    }
    if step.run.rebased {
        step.hold(core, format!("Its pull request did not merge: {error}"));
        return;
    }
    let (tree, base, branch) = (step.tree(), step.run.base.clone(), step.run.branch.clone());
    let pushed = head.to_owned();
    let rebased = blocking(move || {
        if !git::rebase(&tree, &base)? {
            return Ok(None);
        }
        git::push(&tree, &branch, Some(&pushed))?;
        git::head(&tree).map(Some).map_err(Into::into)
    })
    .await;
    match rebased {
        Ok(Some(head)) => {
            let now = epoch_ms();
            change_run(core, &step.project, step.id, step.agent(), |_, run| {
                run.rebased = true;
                run.head = Some(head);
                run.checks_since = Some(now);
                run.stage = Stage::Checking;
                run.note = Some(format!(
                    "It did not merge, so Auto put it on the latest {} and waits for its checks again",
                    run.base
                ));
            });
        }
        Ok(None) if !step.run.fixed => {
            let text = format!(
                "Your pull request did not merge ({error}), and {} does not rebase cleanly onto origin/{}. Rebase it onto origin/{}, resolve the conflicts, run the checks and commit. When your turn ends, Sikemux pushes and checks again. This is the one fix Auto asks for.",
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
        Ok(None) => step.hold(
            core,
            format!(
                "It did not merge and conflicts with the latest {}: {error}",
                step.run.base
            ),
        ),
        Err(failure) => step.hold(
            core,
            format!("It did not merge ({error}), and rebasing it failed: {failure}"),
        ),
    }
}

/// Its pull request merged: the card is done, its agent stops and its
/// worktree goes.
async fn merged(core: &Arc<Core>, step: &Step) {
    let url = step.pull_url();
    let kept = change_run(core, &step.project, step.id, step.agent(), |_, run| {
        run.stage = Stage::Merged;
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
        status: Some(Status::Done),
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
        "merged" => return merged(core, &step).await,
        "closed" | "declined" | "superseded" => {
            return step.hold(
                core,
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
