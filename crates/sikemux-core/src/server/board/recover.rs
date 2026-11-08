//! When one of Auto's agents fails, dies or hangs, the board brings it back without the
//! person: its own session first, so it keeps its context, and a fresh agent in the same
//! worktree when that cannot work. A few tries with growing waits; the same failure twice
//! in a row, or too many, go to the person once, with the reason. What is tried and what is
//! due sits on the run, so a core that restarts carries on from the card.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::auto::{self, AutoRun, RunHold, RunStage};
use super::elsewhere;
use super::finish::{self, Dropped, Step};
use super::lease::Stalled;
use super::Card;
use crate::git::Git;
use crate::server::chat::carry_on_prompt;
use crate::server::inbox::Fault;
use crate::server::launching;
use crate::server::messages;
use crate::server::spawn::Ending;
use crate::server::worktree::{self, agent::set_up, MadeAgain, Tree};
use crate::server::Core;

const MAX_TRIES: u32 = 3;
const FIRST_WAIT_MS: u64 = 30_000;
const WAIT_GROWS: u64 = 4;
/// How long a turn may go without a word from its agent before Auto takes it to have hung.
/// The longest tools agents run alone stay well under this; those they run through
/// Sikemux's processes count as work.
pub(super) const HUNG_AFTER_MS: u64 = 30 * 60 * 1000;
const MAX_CAUSES: usize = 8;
const MAX_CAUSE_CHARS: usize = 300;
const MAX_LOG_CHARS: usize = 3_000;
const MAX_ANSWER_CHARS: usize = 2_000;
const KEY_CHARS: usize = 120;

/// Words that mean the agent's program is not there to run, which no retry mends.
const CANNOT_RUN: [&str; 5] = [
    "no such file or directory",
    "program is missing",
    "not installed",
    "can no longer start",
    "permission denied",
];

/// Where Auto stands on bringing a run's agent back.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Recovery {
    /// Tries since its agent last finished a turn.
    pub tries: u32,
    /// The failure the last try answered, for telling whether the next is the same.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// When the next try is due, in epoch ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<u64>,
    /// What the next try tells the agent happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The stalled slice this run took on, for the person to overrule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub took_on: Option<TookOn>,
    /// The next try is a fresh agent, as the progress guard tripped.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fresh: bool,
}

/// Auto took a slice whose owner lost the lease, recorded as a decision the person can overrule.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TookOn {
    pub from: Stalled,
    pub goal: Option<u64>,
    pub decision: u64,
    /// It works in the worktree its owner left, which is not Auto's to clear away.
    pub reused: bool,
}

enum Sorted {
    NeedsPerson,
    /// The chat sends its prompt again by itself once a limit resets or the network is back.
    WaitsByItself,
    Tries,
}

fn cannot_run(reason: &str) -> bool {
    let reason = reason.to_lowercase();
    CANNOT_RUN.iter().any(|words| reason.contains(words))
}

fn sort(core: &Core, agent: &str, reason: &str) -> Sorted {
    match core.inbox.fault(agent) {
        Some(Fault::Account) => Sorted::NeedsPerson,
        Some(Fault::Retrying) => Sorted::WaitsByItself,
        _ if cannot_run(reason) => Sorted::NeedsPerson,
        _ => Sorted::Tries,
    }
}

/// What makes two failures the same, whatever ids, times or counts each one carries.
pub(super) fn sameness(reason: &str) -> String {
    let words: String = reason
        .to_lowercase()
        .chars()
        .filter(|letter| !letter.is_ascii_digit())
        .take(KEY_CHARS)
        .collect();
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wait_before(tries: u32) -> u64 {
    FIRST_WAIT_MS * WAIT_GROWS.pow(tries)
}

pub(super) fn clipped(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

fn noted(card: &mut Card, cause: &str) {
    if let Some(record) = card.record.as_mut() {
        record.recoveries += 1;
        if record.recovery_causes.len() < MAX_CAUSES {
            record.recovery_causes.push(clipped(cause, MAX_CAUSE_CHARS));
        }
    }
}

/// Holds the run for the person, or sets its next try, for a failure described as `why`.
/// `cause` is the failure alone, which is compared with the last one: an agent's name is not
/// part of it, as a fresh agent has another.
fn schedule(core: &Core, project: &str, id: u64, agent: &str, why: String, cause: &str) {
    finish::change_run(core, project, id, agent, |card, run| {
        let key = sameness(cause);
        let again = run.recovery.failure.as_deref() == Some(key.as_str());
        if again || run.recovery.tries >= MAX_TRIES {
            let tried = match run.recovery.tries {
                1 => "once".to_owned(),
                tries => format!("{tries} times"),
            };
            let what = if again {
                format!("It failed the same way after Auto tried again {tried}")
            } else {
                format!("Auto tried again {tried}")
            };
            run.recovery.due = None;
            run.held = Some(RunHold::Unblock);
            run.note = Some(format!("{why}. {what}."));
            return;
        }
        noted(card, &why);
        let wait = wait_before(run.recovery.tries);
        run.recovery.tries += 1;
        run.recovery.failure = Some(key);
        run.recovery.due = Some(epoch_ms() + wait);
        run.note = Some(format!("{why}. Auto tries again in {}s", wait / 1000));
        run.recovery.why = Some(why);
    });
}

/// The next try is a fresh agent, due now, told `why`. The old agent keeps running until then,
/// and `due` being set is what keeps its ending from counting as a failure.
pub(super) fn take_over(run: &mut AutoRun, why: String) {
    run.recovery.fresh = true;
    run.recovery.due = Some(epoch_ms());
    run.recovery.why = Some(why);
}

fn worktree_lost(run: &AutoRun) -> bool {
    !run.worktree.is_empty() && !Path::new(&run.worktree).is_dir()
}

/// A run whose worktree is gone gets a fresh agent in one made again, instead of the step it
/// was due. True when that is what happens.
pub(super) fn rescued(core: &Core, step: &Step) -> bool {
    let takes_up = matches!(
        step.run.stage,
        RunStage::Working
            | RunStage::Fixing
            | RunStage::Shipping
            | RunStage::Checking
            | RunStage::Merging
    );
    if !takes_up
        || !step.settings.backs_up()
        || step.run.recovery.fresh
        || !worktree_lost(&step.run)
    {
        return false;
    }
    finish::change_run(core, &step.project, step.id, step.agent(), |_, run| {
        if run.stage != RunStage::Working {
            run.stage = RunStage::Fixing;
        }
        run.note = Some("Its worktree was lost, so Auto is making it again".to_owned());
        take_over(run, "Its worktree was lost".to_owned());
    });
    true
}

/// The turn of one of Auto's agents ended without finishing: what comes next depends on why.
pub(super) fn ended(
    core: &Arc<Core>,
    project: &str,
    id: u64,
    agent: &str,
    name: &str,
    stage: RunStage,
    ending: &Ending,
) {
    let held = |note: String| finish::hold(core, project, id, agent, RunHold::Unblock, note);
    let on = {
        let state = core.boards.state.locked();
        state
            .boards
            .get(project)
            .is_some_and(|board| board.settings.recover)
    };
    let (why, cause) = match ending {
        Ending::Finished => return,
        Ending::Stopped | Ending::Gone { asked: true, .. } => {
            held(format!("{name} was stopped before it finished"));
            return;
        }
        Ending::Failed(reason) => (format!("{name}'s turn failed: {reason}"), reason.clone()),
        Ending::Gone { reason, .. } => match reason {
            Some(reason) if !reason.trim().is_empty() => {
                (format!("{name} stopped running: {reason}"), reason.clone())
            }
            _ => (
                format!("{name} stopped running"),
                "stopped running".to_owned(),
            ),
        },
    };
    if !on || !matches!(stage, RunStage::Working | RunStage::Fixing) {
        held(why);
        return;
    }
    match sort(core, agent, &why) {
        Sorted::NeedsPerson => held(why),
        Sorted::WaitsByItself => {
            finish::change_run(core, project, id, agent, |_, run| {
                run.note = Some(format!("{why}. It goes on by itself once that clears"));
            });
        }
        Sorted::Tries => schedule(core, project, id, agent, why, &cause),
    }
    core.boards.wake.now();
}

/// A usage limit's wait gave up, so a run it held waits on the person after all.
pub(crate) fn stopped_waiting(core: &Core, agent: &str) {
    let found = {
        let state = core.boards.state.locked();
        state.boards.iter().find_map(|(project, board)| {
            board.cards.iter().find_map(|card| {
                let run = card.run()?;
                (run.agent == agent).then(|| (project.clone(), card.id))
            })
        })
    };
    if let Some((project, id)) = found {
        let why =
            "Its account is out of usage, and Sikemux cannot tell when that resets".to_owned();
        finish::hold(core, &project, id, agent, RunHold::Unblock, why);
    }
}

/// An ending that came while Auto itself is bringing the agent back says nothing new.
pub(super) fn bringing_back(run: &AutoRun) -> bool {
    run.recovery.due.is_some()
}

/// Its agent finished a turn, so what was tried before no longer counts against it.
pub(super) fn finished_turn(core: &Core, project: &str, id: u64, agent: &str) {
    let tried = {
        let state = core.boards.state.locked();
        state
            .boards
            .get(project)
            .and_then(|board| board.card(id).ok())
            .and_then(|card| card.run())
            .is_some_and(|run| run.recovery.tries > 0)
    };
    if tried {
        finish::change_run(core, project, id, agent, |_, run| {
            run.recovery.tries = 0;
            run.recovery.failure = None;
            run.recovery.due = None;
            run.recovery.why = None;
        });
    }
}

fn hold(core: &Core, step: &Step, why: String) {
    step.hold(core, RunHold::Unblock, why);
}

/// The agent is at work where only the person or its own workers can tell it is not hung.
fn keeps_busy(core: &Core, agent: &str) -> bool {
    let now = core.agent_now(agent);
    now.waits_on_something()
        || now.workers_busy(agent)
        || (core.chats.get(agent)).is_some_and(|chat| !chat.pending_permissions().is_empty())
        || (core.harness.all_runs().into_iter())
            .any(|run| run.status.is_active() && run.agent_id.as_deref() == Some(agent))
}

/// When the agent's turn counts as hung unless it says something first; none when not in one.
pub(super) fn hangs_at(core: &Core, agent: &str) -> Option<u64> {
    let chat = core.chats.get(agent)?;
    (chat.turn_running() && !keeps_busy(core, agent)).then(|| chat.feed.heard_at() + HUNG_AFTER_MS)
}

/// Looks at a run Auto holds that is due attention: a try whose time came, a turn that hung,
/// or an agent that is gone with no ending to say why.
pub(super) async fn settle(core: &Arc<Core>, step: Step) {
    let agent = step.agent().to_owned();
    if step.run.recovery.due.is_some_and(|due| due <= epoch_ms()) {
        attempt(core, step).await;
        return;
    }
    if hangs_at(core, &agent).is_some_and(|at| at <= epoch_ms()) {
        hung(core, step);
        return;
    }
    if core.running_agents().contains(&agent) || elsewhere::works_now(core, &agent) {
        return;
    }
    let name = step.name(core);
    let why = elsewhere::why_gone(core, &step.run, &name)
        .unwrap_or_else(|| format!("{name} is no longer running"));
    let cut_short = format!("{why}, so it did not finish; Ship it again sends what it committed");
    let asleep = core
        .workspaces
        .agent(&agent)
        .is_some_and(|known| known.asleep);
    if !step.settings.recover || asleep || closed_here(core, &step) || core.workspaces.held(&agent)
    {
        hold(core, &step, cut_short);
        return;
    }
    schedule(
        core,
        &step.project,
        step.id,
        &agent,
        why,
        "no longer running",
    );
    core.boards.wake.now();
}

/// The person closed the run's agent. One on a linked host is never in this host's workspace.
fn closed_here(core: &Core, step: &Step) -> bool {
    step.run.elsewhere.is_none() && core.workspaces.closed(step.agent())
}

fn stop_agent(core: &Core, agent: &str) {
    if let Some(chat) = core.chats.get(agent) {
        core.workspaces.note_stopping(&chat);
    }
    if core.chats.stop(agent) {
        core.note_agent_gone(agent);
    }
    crate::server::hosts::route::stop_worker(core, agent);
}

/// Stops an agent whose turn hung, which then has a try coming like any other failure.
fn hung(core: &Arc<Core>, step: Step) {
    let agent = step.agent().to_owned();
    let why = format!(
        "{} said nothing for {} minutes in the middle of a turn",
        step.name(core),
        HUNG_AFTER_MS / 60_000
    );
    schedule(core, &step.project, step.id, &agent, why, "hung");
    stop_agent(core, &agent);
    core.boards.wake.now();
}

/// A try whose time came: the agent's own session if that can work, else a fresh agent.
async fn attempt(core: &Arc<Core>, step: Step) {
    let agent = step.agent().to_owned();
    let name = step.name(core);
    let now = core.agent_now(&agent);
    let why = step.run.recovery.why.clone().unwrap_or_default();
    let cleared = |note: Option<String>| {
        finish::change_run(core, &step.project, step.id, &agent, |_, run| {
            run.recovery.due = None;
            run.note = note;
        });
    };
    let replaces = step.run.recovery.fresh;
    if now.mid_turn() && !replaces {
        cleared(None);
        return;
    }
    if core.workspaces.held(&agent) {
        hold(
            core,
            &step,
            format!("{why}, and the person stopped everything before Auto tried again"),
        );
        return;
    }
    if closed_here(core, &step) {
        hold(
            core,
            &step,
            format!("{name} was closed before Auto tried again"),
        );
        return;
    }
    if replaces {
        for worker in core.names.team(&agent).iter().filter(|id| **id != agent) {
            stop_agent(core, worker);
        }
        fresh(core, &step, &why).await;
        return;
    }
    cleared(Some(format!("Bringing {name} back")));
    let running = core.chats.get(&agent).is_some() || elsewhere::works_now(core, &agent);
    let resumable = running
        || core
            .workspaces
            .agent(&agent)
            .is_some_and(|known| known.resume_id.is_some() && !known.terminal);
    if !resumable {
        fresh(core, &step, &why).await;
        return;
    }
    if !running {
        if let Err(error) = launching::wake_chat(core, agent.clone()).await {
            let error = error.to_string();
            if cannot_run(&error) {
                hold(core, &step, format!("{name} cannot start again: {error}"));
            } else {
                fresh(core, &step, &why).await;
            }
            return;
        }
    }
    if core.agent_now(&agent).waits_on_something() {
        cleared(None);
        return;
    }
    let prompt = carry_on_prompt(&format!("Sikemux's Auto brought you back ({why})"));
    match messages::from_auto(core, agent.clone(), prompt.text).await {
        Ok(_) => cleared(None),
        Err(_) => fresh(core, &step, &why).await,
    }
}

/// What the next agent is told of the work so far, from the branch and the last agent's words.
fn handover(
    tree: &std::path::Path,
    start_sha: &str,
    name: &str,
    why: &str,
    said: Option<String>,
) -> String {
    let range = format!("{start_sha}..HEAD");
    let commits = Git::quick(tree)
        .trimmed(&["log", "--oneline", &range])
        .unwrap_or_default();
    let status = Git::quick(tree)
        .trimmed(&["status", "--short"])
        .unwrap_or_default();
    let mut text = format!(
        "\n\nYou take over from @{name}, an earlier agent on this card, which could not go on: {why}. Its work is yours to carry on, in this same worktree and branch, so do not start again."
    );
    let commits = if commits.is_empty() {
        "none yet".to_owned()
    } else {
        clipped(&commits, MAX_LOG_CHARS)
    };
    text.push_str(&format!("\n\nCommits on the branch so far:\n{commits}"));
    if !status.is_empty() {
        text.push_str(&format!(
            "\n\nChanges it left uncommitted:\n{}",
            clipped(&status, MAX_LOG_CHARS)
        ));
    }
    if let Some(said) = said.filter(|said| !said.trim().is_empty()) {
        text.push_str(&format!(
            "\n\nThe last thing it wrote:\n{}",
            clipped(&said, MAX_ANSWER_CHARS)
        ));
    }
    text.push_str(&format!(
        "\n\nCheck what got done, since a step that was cut off may not have finished. agent_read on @{name} shows its whole conversation."
    ));
    text
}

/// A fresh agent in the run's worktree, handed the card, the branch and the last agent's words.
async fn fresh(core: &Arc<Core>, step: &Step, why: &str) {
    let agent = step.agent().to_owned();
    let name = step.name(core);
    let lost = worktree_lost(&step.run);
    if step.run.worktree.is_empty() || (lost && !step.settings.backs_up()) {
        hold(
            core,
            step,
            format!("{why}. Its worktree is gone, so Auto cannot carry it on"),
        );
        return;
    }
    if let Err(error) = elsewhere::take_in(core, step).await {
        hold(core, step, format!("{why}. {error}"));
        return;
    }
    let said = core
        .chats
        .get(&agent)
        .and_then(|chat| chat.feed.last_answer());
    stop_agent(core, &agent);
    let (tree, remade) = match lost {
        false => (auto::tree_of(&step.run, &step.project), None),
        true => match make_again(core, step).await {
            Ok((tree, remade)) => (tree, Some(remade)),
            Err(error) => {
                hold(
                    core,
                    step,
                    format!("{why}. Auto could not make its worktree again: {error}"),
                );
                return;
            }
        },
    };
    let note = match &remade {
        Some(Remade {
            from_base: true,
            told,
        }) => told.clone(),
        _ => {
            let (cwd, start_sha, name, why) = (
                tree.cwd.clone(),
                step.run.start_sha.clone(),
                name.clone(),
                why.to_owned(),
            );
            let handed = crate::server::connection::blocking(move || {
                Ok(handover(&cwd, &start_sha, &name, &why, said))
            })
            .await
            .unwrap_or_default();
            let told = remade.as_ref().map_or("", |remade| remade.told.as_str());
            format!("{handed}{told}")
        }
    };
    if let Err(error) = auto::replace_agent(core, step, tree, note).await {
        hold(
            core,
            step,
            format!("{why}. A fresh agent could not start: {error}"),
        );
    }
}

/// What a worktree made again means for the next agent.
struct Remade {
    from_base: bool,
    told: String,
}

/// Makes the run's lost worktree again and runs the project's setup in it, saying so on the card.
async fn make_again(core: &Arc<Core>, step: &Step) -> Result<(Tree, Remade), String> {
    let lost = auto::tree_of(&step.run, &step.project);
    let (kept, shipped) = (step.run.backed_up.clone(), step.run.head.clone());
    let (tree, made) = crate::server::connection::blocking(move || {
        worktree::make_again(lost, kept.as_deref(), shipped.as_deref()).map_err(Into::into)
    })
    .await
    .map_err(|error| error.to_string())?;
    let setup_failed = set_up(
        core,
        Path::new(&step.project),
        Some(step.agent()),
        &tree.cwd,
    )
    .await;
    let (card_note, mut told, from_base) = match made {
        MadeAgain::AtCommit(sha) => {
            let short = crate::git::short(&sha);
            (
                format!("Its worktree was lost, so Auto made it again at {short} and put a fresh agent on it. Anything not committed is gone"),
                format!("\n\nThis worktree was lost and made again at commit {short}. Anything the last agent had not committed is gone."),
                false,
            )
        }
        MadeAgain::FromBase => (
            format!("Its worktree was lost with nothing pushed, so a fresh agent starts it again from the latest {}", tree.base),
            format!("\n\nThis card's worktree was lost with nothing pushed, so it was made again from the latest {}. Earlier work on it is gone: start the card from the beginning.", tree.base),
            true,
        ),
    };
    if let Some(failed) = setup_failed {
        told.push_str(&format!(
            "\n\nThe project's worktree setup did not finish: {failed}"
        ));
    }
    finish::change_run(core, &step.project, step.id, step.agent(), |card, run| {
        run.note = Some(card_note);
        run.recovery.fresh = true;
        if let Some(record) = card.record.as_mut() {
            record.worktree_rebuilds += 1;
        }
    });
    Ok((tree, Remade { from_base, told }))
}

/// The handover for an agent taking a stalled slice on, from the branch its owner left.
pub(super) fn takeover_note(tree: &std::path::Path, start_sha: &str, stalled: &Stalled) -> String {
    handover(
        tree,
        start_sha,
        &stalled.name,
        &format!("it lost the slice. {}", stalled.why.words()),
        None,
    )
}

/// The person overruled Auto taking a stalled slice on: its agent stops, and the slice waits for
/// them as it did.
pub(super) fn overruled(core: &Core, project: &str, goal: Option<u64>, decision: u64) {
    let mut state = core.boards.state.locked();
    let Some(board) = state.boards.get_mut(project) else {
        return;
    };
    let found = board.cards.iter_mut().find(|card| {
        card.kept_run()
            .and_then(|run| run.recovery.took_on.as_ref())
            .is_some_and(|took| took.goal == goal && took.decision == decision)
    });
    let Some(card) = found else {
        return;
    };
    let Some(mut run) = card.auto.take() else {
        return;
    };
    let took = run.recovery.took_on.take();
    card.owner = None;
    card.team = None;
    card.status = super::CardStatus::Open;
    card.updated_at = epoch_ms();
    if let Some(record) = card.record.as_mut() {
        record.overruled = true;
    }
    let id = card.id;
    if let (Some(slice), Some(took)) = (card.slice.as_mut(), took.as_ref()) {
        let mut from = took.from.clone();
        from.stays = true;
        slice.stalled = Some(from);
    }
    if took.is_some_and(|took| took.reused) {
        run.worktree.clear();
    }
    state.dropped.push(Dropped {
        project: project.to_owned(),
        id,
        run,
        discard: false,
    });
    core.boards.save(&mut state, project);
    drop(state);
    core.boards.wake.now();
}
