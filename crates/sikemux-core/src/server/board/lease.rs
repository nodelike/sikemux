//! Holding a slice is a lease. An owner that closed, stopped or went quiet loses its slice, so
//! the team doesn't wait on it in silence: the slice keeps its gates and its work, anybody may
//! take it up, and the person is asked to resume the owner or hand the slice on. An owner keeps
//! the lease for as long as something it waits on can still bring it back by itself.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::{
    hand_off, hand_off::HandOnParams, holder, name_of, project_at, project_view, remember_team,
    take, tell_clients, wait, BoardView, Card, CardParams, Status,
};
use crate::server::connection::blocking;
use crate::server::wake::sooner;
use crate::server::workspace::SessionChoice;
use crate::server::{launching, messages, saved, Core};

/// How long an owner with no turn running may leave its slice untouched. Nothing announces that
/// this has passed, so the board wakes at the earliest such moment; it is long enough that the
/// person stepping away from an agent they talk with is no stall.
const QUIET_MS: u64 = 30 * 60 * 1000;

/// Why an owner lost its slice.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Why {
    Closed,
    Stopped,
    Quiet,
}

impl Why {
    pub(crate) fn words(self) -> String {
        match self {
            Why::Closed => "Its chat closed with the work unfinished.".to_owned(),
            Why::Stopped => "Its chat went to sleep or stopped and has not come back.".to_owned(),
            Why::Quiet => format!(
                "It has run no turn and left the slice untouched for {} minutes.",
                QUIET_MS / 60_000
            ),
        }
    }
}

/// The owner a slice lost, kept until someone takes the slice or the person moves it back to Open.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Stalled {
    pub agent: String,
    pub name: String,
    pub why: Why,
}

/// The owner a card lost, for the person to resume or hand it on: the one a slice's lease took,
/// or the closed owner of a goal with no slice, which has no lease to lose.
pub(super) fn stalled_of(core: &Core, card: &Card) -> Option<Stalled> {
    (card.slice.as_ref())
        .and_then(|slice| slice.stalled.clone())
        .filter(|_| card.owner.is_none())
        .or_else(|| super::chat_goal::plain_stalled(core, card))
}

/// A started slice an owner holds, which waits on no check, card, gate or turn.
struct Unattended {
    project: String,
    id: u64,
    owner: String,
    touched: u64,
}

fn unattended(core: &Core, now: u64) -> Vec<Unattended> {
    let state = core.boards.state.locked();
    let born = core.boards.born;
    let mut found = Vec::new();
    for board in state.boards.values() {
        for card in &board.cards {
            let started = card.slice.as_ref().is_some_and(|slice| slice.started);
            let held = card.status == Status::Progress && card.auto.is_none() && started;
            let Some(owner) = card.owner_id().filter(|_| held) else {
                continue;
            };
            if wait::of(core, board, card, now).is_none() {
                found.push(Unattended {
                    project: board.project.clone(),
                    id: card.id,
                    owner: owner.to_owned(),
                    touched: card.updated_at.max(card.acted_at).max(born),
                });
            }
        }
    }
    found
}

/// Whether the owner waits on something that brings it back by itself, or that the person knows
/// of already, such as having stopped everything themselves.
fn waits_elsewhere(core: &Core, owner: &str, running: &HashSet<String>) -> bool {
    core.workspaces.held(owner)
        || core.harness.questions.waits_on_answer(owner)
        || core.harness.mailbox.has_waiting(owner)
        || core.seen.viewed(owner)
        || core.inbox.failing(owner)
        || running.contains(owner)
        || core
            .ship
            .waiting()
            .iter()
            .any(|held| held.agent_id == owner)
        || super::auto::workers_busy(core, owner, owner)
}

/// Takes each lapsed slice from its owner, and answers when the next lease runs out.
pub(super) fn settle(core: &Core) -> Option<u64> {
    let now = epoch_ms();
    let slices = unattended(core, now);
    if slices.is_empty() {
        return None;
    }
    let here = core.open_agents();
    let linked = core.names.open_with(HashSet::new());
    let running: HashSet<String> = (core.harness.all_runs().into_iter())
        .filter(|run| run.status.is_active())
        .filter_map(|run| run.agent_id)
        .collect();
    let mut next: Option<u64> = None;
    let mut lost = Vec::new();
    for slice in slices {
        let owner = slice.owner.as_str();
        if linked.contains(owner) {
            continue;
        }
        if !here.contains(owner) {
            lost.push((slice, Why::Closed));
            continue;
        }
        let touched = slice.touched.max(core.seen.quiet_since(owner).unwrap_or(0));
        let due = touched + QUIET_MS;
        if waits_elsewhere(core, owner, &running) {
            // Not all of these say when they end, such as the person looking away from it.
            next = sooner(next, Some(due.max(now + QUIET_MS)));
        } else if now < due {
            next = sooner(next, Some(due));
        } else if core.chats.get(owner).is_some() || core.agent_session(owner).is_some() {
            lost.push((slice, Why::Quiet));
        } else {
            lost.push((slice, Why::Stopped));
        }
    }
    lose(core, lost, now);
    next
}

fn lose(core: &Core, lost: Vec<(Unattended, Why)>, now: u64) {
    let mut state = core.boards.state.locked();
    for (slice, why) in lost {
        let Some(board) = state.boards.get_mut(&slice.project) else {
            continue;
        };
        let Ok(card) = board.card_mut(slice.id) else {
            continue;
        };
        if card.owner_id() != Some(slice.owner.as_str()) || card.status != Status::Progress {
            continue;
        }
        let name = card.owner_name(core);
        let Some(held) = &mut card.slice else {
            continue;
        };
        held.stalled = Some(Stalled {
            name: name.unwrap_or_else(|| slice.owner.clone()),
            agent: slice.owner,
            why,
        });
        card.owner = None;
        card.status = Status::Open;
        card.updated_at = now;
        state
            .offered
            .retain(|(project, id, _)| !(*project == slice.project && *id == slice.id));
        core.boards.save(&mut state, &slice.project);
    }
}

/// The stalled slice the person acts on, by the project it is on.
async fn stalled_card(
    core: &Arc<Core>,
    params: &CardParams,
) -> Result<(String, Stalled, String), String> {
    let (core, folder, id) = (core.clone(), params.project.clone(), params.id);
    blocking(move || {
        let project = project_at(&core, &folder)?;
        let state = core.boards.state.locked();
        let card = state
            .boards
            .get(&project)
            .ok_or_else(|| format!("There is no card #{id} on this project's board"))?
            .card(id)?;
        let stalled = stalled_of(&core, card)
            .ok_or_else(|| format!("{} has no stalled owner", card.label()))?;
        let label = card.label();
        Ok((project.clone(), stalled, label))
    })
    .await
    .map_err(|error| error.to_string())
}

/// Puts `agent` on the slice, unless someone took it meanwhile.
pub(super) fn give(core: &Core, project: &str, id: u64, agent: &str) -> Result<(), String> {
    let mut state = core.boards.state.locked();
    let board = state
        .boards
        .get_mut(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let card: &mut Card = board.card_mut(id)?;
    if let Some(owner) = holder(core, card) {
        return Err(format!("{} has #{id} now", name_of(core, owner)));
    }
    take(core, card, agent);
    remember_team(core, board, id);
    core.boards.save(&mut state, project);
    Ok(())
}

/// The chat that lost the slice `id` by sitting idle is working again, so it holds the slice
/// again, unless someone took it meanwhile.
pub(super) fn back_at_work(core: &Core, project: &str, id: u64, agent: &str) {
    let mut state = core.boards.state.locked();
    let Some(board) = state.boards.get_mut(project) else {
        return;
    };
    let Ok(card) = board.card_mut(id) else {
        return;
    };
    let lost_it = (card.slice.as_ref())
        .and_then(|slice| slice.stalled.as_ref())
        .is_some_and(|stalled| stalled.agent == agent);
    if lost_it && card.owner.is_none() {
        take(core, card, agent);
        core.boards.save(&mut state, project);
    }
}

async fn tell(core: &Arc<Core>, agent: &str, text: String) -> Result<(), String> {
    let note = "Sent by Sikemux's board, because the person chose it.".to_owned();
    messages::from_board(core, agent.to_owned(), text, note)
        .await
        .map(drop)
}

/// The agent again, running: asleep or stopped it wakes where it is, closed its session is taken
/// up again, under the agent that has it open if one does.
async fn bring_back(core: &Arc<Core>, stalled: &Stalled) -> Result<String, String> {
    let agent = stalled.agent.clone();
    if core.open_agents().contains(&agent) {
        if core.chats.get(&agent).is_none() && core.agent_session(&agent).is_none() {
            launching::wake(core, agent.clone())
                .await
                .map_err(|error| format!("@{} did not start again: {error}", stalled.name))?;
        }
        return Ok(agent);
    }
    let ran = core.names.ran_session(&agent).ok_or_else(|| {
        format!(
            "@{}'s session isn't kept any more; hand the slice to someone else",
            stalled.name
        )
    })?;
    let found = {
        let (core, session, cli) = (core.clone(), ran.session_id.clone(), ran.provider.clone());
        blocking(move || saved::locate(&core, &session, Some(&cli)).map_err(Into::into))
            .await
            .ok()
    };
    if let Some((open, _)) = found.as_ref().and_then(|saved| saved.open.clone()) {
        return Ok(open);
    }
    let started = launching::resume_session(
        core,
        SessionChoice {
            provider: ran.provider,
            session_id: ran.session_id,
            project: PathBuf::from(&ran.project),
            cwd: found.map(|saved| saved.project),
            title: None,
        },
    )
    .await
    .map_err(|error| format!("@{}'s session did not start again: {error}", stalled.name))?;
    let resumed = (started["agentId"].as_str().map(str::to_owned))
        .ok_or_else(|| "The session started with no agent".to_owned())?;
    let (owner, kept, name) = (core.clone(), resumed.clone(), stalled.name.clone());
    let _ = blocking(move || {
        // Only a name the person chose comes back by itself; another agent may have this one now.
        let _ = (owner.names).rename(&kept, &name, owner.open_agents());
        owner.view_changed();
        Ok(())
    })
    .await;
    Ok(resumed)
}

/// The person resumes the owner a slice lost, which holds it again. Answers with the board.
pub(crate) async fn person_resume(
    core: &Arc<Core>,
    params: CardParams,
) -> Result<BoardView, String> {
    let (project, stalled, label) = stalled_card(core, &params).await?;
    let agent = bring_back(core, &stalled).await?;
    give(core, &project, params.id, &agent)?;
    tell_clients(core);
    tell(core, &agent, format!(
        "You lost your slice {label} because you stalled, and the person gave it back to you. board_list with card {} shows where it stands: carry on with it.",
        params.id
    ))
    .await?;
    Ok(project_view(core, &project))
}

/// The person hands a slice whose owner stalled to another agent. Answers with the board.
pub(crate) async fn person_hand_on(
    core: &Arc<Core>,
    params: HandOnParams,
) -> Result<BoardView, String> {
    let card = CardParams {
        project: params.project,
        id: params.id,
    };
    let (project, stalled, label) = stalled_card(core, &card).await?;
    let id = card.id;
    let from = format!(
        "{label} was @{}'s until it stalled. {}",
        stalled.name,
        stalled.why.words()
    );
    let Some(agent) = params.to else {
        hand_off::hand_on(core, &project, id, params.launcher, from).await?;
        return Ok(project_view(core, &project));
    };
    if !core.open_agents().contains(&agent) {
        return Err(format!("{} isn't open any more", name_of(core, &agent)));
    }
    give(core, &project, id, &agent)?;
    tell_clients(core);
    tell(core, &agent, format!(
        "The person handed you a slice. {from} It is yours now: board_list with card {id} shows where it stands, so carry it on."
    ))
    .await?;
    Ok(project_view(core, &project))
}
