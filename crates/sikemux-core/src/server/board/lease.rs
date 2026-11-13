//! Holding a slice is a lease. An owner that closed, stopped or went quiet loses its slice, so
//! the team doesn't wait on it in silence: the slice keeps its gates and its work, anybody may
//! take it up, and Auto takes it on where it can run it, telling the person, who can overrule
//! that. Otherwise, and always for a chat the person works in, they are asked to resume the owner
//! or hand the slice on. An owner keeps the lease for as long as something it waits on can still
//! bring it back by itself. A plain card under another loses such an owner too, and goes back to
//! Open with a line naming who left it, as nobody is asked to bring a plain card's owner back.

use crate::server::agent_now::AgentNowCore;
use crate::server::board_state::BoardParts;
use crate::server::chat_state::ChatParts;
use crate::server::repo_state::RepoParts;
use crate::server::serving::ServeCore;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::{
    hand_off, hand_off::HandOnParams, holder, name_of, project_at, project_view, take_on, wait,
    Board, BoardCardParams, Card, CardStatus, ProjectBoard,
};
use crate::server::connection::blocking;
use crate::server::wake::sooner;
use crate::server::Core;

/// How long an owner with no turn running may leave its slice untouched. Nothing announces that
/// this has passed, so the board wakes at the earliest such moment; it is long enough that the
/// person stepping away from an agent they talk with is no stall.
const QUIET_MS: u64 = 30 * 60 * 1000;

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
    /// Auto leaves it to the person: their own chat lost it, or they overruled Auto taking it on.
    #[serde(default)]
    pub stays: bool,
}

/// The owner a card lost, for the person to resume or hand it on: the one a slice's lease took,
/// or the closed owner of a goal with no slice, which has no lease to lose.
pub(super) fn stalled_of(core: &Core, card: &Card) -> Option<Stalled> {
    (card.slice.as_ref())
        .and_then(|slice| slice.stalled.clone())
        .filter(|_| card.owner.is_none())
        .or_else(|| super::chat_goal::plain_stalled(core, card))
}

/// A started slice an owner holds, which waits on no check, card, gate or turn, or a plain card
/// under another that waits on no card.
struct Unattended {
    project: String,
    id: u64,
    owner: String,
    touched: u64,
    plain: bool,
}

/// Whether the card's owner holds it under a lease that may lapse now: a started slice that
/// waits on nothing, or a plain card under another that waits on no card. A plain card at the
/// top is a goal its chat answers for, which the person resumes when it closes.
fn lapses(core: &Core, board: &Board, card: &Card, now: u64) -> bool {
    match &card.slice {
        Some(slice) => slice.started && wait::of(core, board, card, now).is_none(),
        None => card.parent.is_some() && board.waits_on(card).is_empty(),
    }
}

fn unattended(core: &Core, now: u64) -> Vec<Unattended> {
    let state = core.boards().state.locked();
    let born = core.boards().born;
    let mut found = Vec::new();
    for board in state.boards.values() {
        for card in &board.cards {
            let held = card.status == CardStatus::Progress && card.auto.is_none();
            let Some(owner) = card.owner_id().filter(|_| held) else {
                continue;
            };
            if lapses(core, board, card, now) {
                found.push(Unattended {
                    project: board.project.clone(),
                    id: card.id,
                    owner: owner.to_owned(),
                    touched: card.updated_at.max(card.acted_at).max(born),
                    plain: card.slice.is_none(),
                });
            }
        }
    }
    found
}

/// Whether the owner waits on something that brings it back by itself, or that the person knows
/// of already, such as having stopped everything themselves.
fn waits_elsewhere(core: &Core, owner: &str, running: &HashSet<String>) -> bool {
    let now = core.agent_now(owner);
    core.workspaces().held(owner)
        || now.waits_on_something()
        || core.seen().viewed(owner)
        || (core.board_above().inbox_failing)(core, owner)
        || running.contains(owner)
        || core
            .ship()
            .waiting()
            .iter()
            .any(|held| held.agent_id == owner)
        || now.workers_busy(owner)
}

/// Takes each lapsed slice from its owner, and answers when the next lease runs out.
pub(super) fn settle(core: &Core) -> Option<u64> {
    let now = epoch_ms();
    let slices = unattended(core, now);
    if slices.is_empty() {
        return None;
    }
    let here = core.open_agents();
    let linked = core.names().open_with(HashSet::new());
    let running = (core.board_above().harness_running_agents)(core);
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
        if slice.plain && persons_own(core, owner, Why::Quiet) {
            continue;
        }
        let touched = slice
            .touched
            .max(core.seen().quiet_since(owner).unwrap_or(0));
        let due = touched + QUIET_MS;
        if waits_elsewhere(core, owner, &running) {
            // Not all of these say when they end, such as the person looking away from it.
            next = sooner(next, Some(due.max(now + QUIET_MS)));
        } else if now < due {
            next = sooner(next, Some(due));
        } else if core.chats().get(owner).is_some() || core.agent_session(owner).is_some() {
            lost.push((slice, Why::Quiet));
        } else {
            lost.push((slice, Why::Stopped));
        }
    }
    lose(core, lost, now);
    next
}

/// Whether the person works on the slice with this agent, in a chat of their own still open, so
/// its going quiet means they stepped away rather than that the work is stuck.
fn persons_own(core: &Core, owner: &str, why: Why) -> bool {
    !matches!(why, Why::Closed)
        && core
            .names()
            .get(owner)
            .is_none_or(|named| named.parent.is_none())
}

fn lose(core: &Core, lost: Vec<(Unattended, Why)>, now: u64) {
    let mut state = core.boards().state.locked();
    let mut owners = Vec::new();
    for (slice, why) in lost {
        let Some(board) = state.boards.get_mut(&slice.project) else {
            continue;
        };
        let Ok(card) = board.card_mut(slice.id) else {
            continue;
        };
        if card.owner_id() != Some(slice.owner.as_str()) || card.status != CardStatus::Progress {
            continue;
        }
        let name = (card.owner_name(core)).unwrap_or_else(|| slice.owner.clone());
        match &mut card.slice {
            Some(held) => {
                held.stalled = Some(Stalled {
                    name,
                    stays: persons_own(core, &slice.owner, why),
                    agent: slice.owner.clone(),
                    why,
                });
            }
            None => {
                let left = format!("@{name} left it unfinished.");
                card.result = Some(match card.result.take() {
                    Some(result) => format!("{result}\n\n{left}"),
                    None => left,
                });
            }
        }
        owners.push(slice.owner);
        card.lose_owner(now);
        state
            .offered
            .retain(|(project, id, _)| !(*project == slice.project && *id == slice.id));
        core.boards().save(&mut state, &slice.project);
    }
    drop(state);
    for owner in owners {
        super::chat_goal::lease_lapsed(core, &owner);
    }
}

/// Lets go of each unfinished card of `projects` that `lost` picks: it stays on the board with its
/// slice's work and gates, nobody's and on no team, so nothing asks the person or an agent about it.
fn let_go(core: &Core, projects: Vec<String>, lost: impl Fn(&Card) -> bool) {
    let now = epoch_ms();
    let mut state = core.boards().state.locked();
    for project in projects {
        let Some(board) = state.boards.get_mut(&project) else {
            continue;
        };
        let mut changed = false;
        for card in &mut board.cards {
            if card.status == CardStatus::Done || !lost(card) {
                continue;
            }
            card.lose_owner(now);
            card.team = None;
            if let Some(slice) = &mut card.slice {
                slice.stalled = None;
            }
            changed = true;
        }
        if changed {
            core.boards().save(&mut state, &project);
        }
    }
}

/// A chat deleted for good can't be resumed, so the cards it held or lost stop waiting on it.
pub(crate) fn session_deleted(core: &Core, session_id: &str) {
    let agents = core.names().agents_of_session(session_id);
    if agents.is_empty() {
        return;
    }
    let projects = core
        .boards()
        .state
        .locked()
        .boards
        .keys()
        .cloned()
        .collect();
    let_go(core, projects, |card| {
        let closed_owner = || card.owner_id().filter(|_| holder(core, card).is_none());
        match stalled_of(core, card) {
            Some(stalled) => agents.contains(&stalled.agent),
            None => closed_owner().is_some_and(|owner| agents.contains(owner)),
        }
    });
}

/// Lets go of the cards stalled on chats that closed before this version, once, as weeks-old
/// chats may be long gone. Only a chat closing from now on asks the person about its card.
pub(super) fn clear_old_stalls(core: &Core) {
    let projects = std::mem::take(&mut core.boards().state.locked().old_stalls);
    if projects.is_empty() {
        return;
    }
    let_go(core, projects, |card| {
        stalled_of(core, card).is_some_and(|stalled| !core.agent_now(&stalled.agent).open())
    });
}

/// The stalled slice the person acts on, by the project it is on.
async fn stalled_card(
    core: &Arc<Core>,
    params: &BoardCardParams,
) -> Result<(String, Stalled, String), String> {
    let (core, folder, id) = (core.clone(), params.project.clone(), params.id);
    blocking(move || {
        let project = project_at(&core, &folder)?;
        let state = core.boards().state.locked();
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
    let mut state = core.boards().state.locked();
    let board = state
        .boards
        .get_mut(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let card: &mut Card = board.card_mut(id)?;
    if let Some(owner) = holder(core, card).filter(|owner| *owner != agent) {
        return Err(format!("{} has #{id} now", name_of(core, owner)));
    }
    take_on(core, board, id, agent)?;
    core.boards().save(&mut state, project);
    Ok(())
}

/// A chat taken up again on the provider session of an owner that lost its slices holds them
/// again, unless someone took them meanwhile, and answers with where its goal and card stand,
/// for it to be handed with its next message.
pub(crate) fn resumed_session(
    core: &Core,
    agent: &str,
    provider: &str,
    session: &str,
) -> Option<String> {
    let ran = |stalled: &Stalled| {
        core.names()
            .ran_session(&stalled.agent)
            .is_some_and(|ran| ran.provider == provider && ran.session_id == session)
    };
    let mut taken = false;
    {
        let mut state = core.boards().state.locked();
        let projects: Vec<String> = state.boards.keys().cloned().collect();
        for project in projects {
            let Some(board) = state.boards.get_mut(&project) else {
                continue;
            };
            let lost: Vec<u64> = (board.cards.iter())
                .filter(|card| card.owner.is_none())
                .filter(|card| {
                    (card.slice.as_ref())
                        .and_then(|slice| slice.stalled.as_ref())
                        .is_some_and(ran)
                })
                .map(|card| card.id)
                .collect();
            let mut changed = false;
            for id in lost {
                changed |= take_on(core, board, id, agent).is_ok();
            }
            if changed {
                core.boards().save(&mut state, &project);
                taken = true;
            }
        }
    }
    let standing = standing_for(core, agent);
    (!standing.is_empty()).then(|| {
        let lead = match taken {
            true => "You lost your slice while your chat was closed and hold it again now that it is taken up.",
            false => "Your chat was closed and is taken up again.",
        };
        format!("{lead} Where your work stands now, as Sikemux keeps it:\n\n{standing}")
    })
}

/// The chat that lost the slice `id` by sitting idle is working again, so it holds the slice
/// again, unless someone took it meanwhile.
pub(super) fn back_at_work(core: &Core, project: &str, id: u64, agent: &str) {
    let mut state = core.boards().state.locked();
    let Some(board) = state.boards.get_mut(project) else {
        return;
    };
    let Ok(card) = board.card(id) else {
        return;
    };
    let lost_it = (card.slice.as_ref())
        .and_then(|slice| slice.stalled.as_ref())
        .is_some_and(|stalled| stalled.agent == agent);
    if lost_it && card.owner.is_none() && take_on(core, board, id, agent).is_ok() {
        core.boards().save(&mut state, project);
    }
}

/// Where `agent`'s goal and card stand now, for an agent handed a slice it did not start with.
fn standing_for(core: &Core, agent: &str) -> String {
    (super::briefing::goal_for(core, agent))
        .map(|goal| super::briefing::sections(core, agent, &goal, false).join("\n\n"))
        .unwrap_or_default()
}

async fn tell(core: &Arc<Core>, agent: &str, text: String) -> Result<(), String> {
    let note = "Sent by Sikemux's board, because the person chose it.".to_owned();
    (core.board_above().messages_from_board)(core, agent.to_owned(), text, note)
        .await
        .map(drop)
}

/// The agent again, running: asleep or stopped it wakes where it is, closed its session is taken
/// up again, under the agent that has it open if one does.
async fn bring_back(core: &Arc<Core>, stalled: &Stalled) -> Result<String, String> {
    let agent = stalled.agent.clone();
    if core.open_agents().contains(&agent) {
        if core.chats().get(&agent).is_none() && core.agent_session(&agent).is_none() {
            (core.board_above().launching_wake)(core, agent.clone())
                .await
                .map_err(|error| format!("@{} did not start again: {error}", stalled.name))?;
        }
        return Ok(agent);
    }
    if core.names().ran_session(&agent).is_none() {
        return Err(format!(
            "@{}'s session isn't kept any more; hand the slice to someone else",
            stalled.name
        ));
    }
    let started = (core.board_above().launching_reopen_agent)(core, agent)
        .await
        .map_err(|error| format!("@{}'s session did not start again: {error}", stalled.name))?;
    Ok(started)
}

/// The person resumes the owner a slice lost, which holds it again. Answers with the board.
pub(crate) async fn person_resume(
    core: &Arc<Core>,
    params: BoardCardParams,
) -> Result<ProjectBoard, String> {
    let (project, stalled, label) = stalled_card(core, &params).await?;
    let agent = bring_back(core, &stalled).await?;
    give(core, &project, params.id, &agent)?;
    if let Some(chat) = core.chats().get(&agent) {
        chat.take_brief();
    }
    let standing = standing_for(core, &agent);
    tell(core, &agent, format!(
        "You lost your slice {label} because you stalled, and the person gave it back to you. board_list with card {} shows where it stands: carry on with it.\n\n{standing}",
        params.id
    ))
    .await?;
    Ok(project_view(core, &project))
}

pub(crate) async fn person_hand_on(
    core: &Arc<Core>,
    params: HandOnParams,
) -> Result<ProjectBoard, String> {
    let card = BoardCardParams {
        project: params.project,
        id: params.id,
    };
    let (project, stalled, label) = stalled_card(core, &card).await?;
    let id = card.id;
    let said = super::recover::last_said(core, &stalled.agent);
    let left = {
        let (core, stalled) = (core.clone(), stalled.clone());
        blocking(move || Ok(super::recover::left_elsewhere(&core, &stalled, said)))
            .await
            .unwrap_or_default()
    };
    let from = format!("{label} was @{}'s until it stalled.{left}", stalled.name);
    let Some(agent) = params.to else {
        hand_off::hand_on(core, &project, id, params.launcher, from).await?;
        return Ok(project_view(core, &project));
    };
    if !core.open_agents().contains(&agent) {
        return Err(format!("{} isn't open any more", name_of(core, &agent)));
    }
    give(core, &project, id, &agent)?;
    let standing = standing_for(core, &agent);
    tell(core, &agent, format!(
        "The person handed you a slice, which is yours now: board_list with card {id} shows where it stands, so carry it on. {from}\n\n{standing}"
    ))
    .await?;
    Ok(project_view(core, &project))
}
