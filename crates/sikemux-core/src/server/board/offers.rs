//! The board offers an idle agent its team's free cards, and tells a turn that only ignored
//! the offer from one worth reporting.

use crate::server::agent_now::AgentNowCore;
use crate::server::parts::Parts;
use std::sync::Arc;

use super::*;

/// How long an agent has been idle before the board offers it cards.
const IDLE_BEFORE_OFFER_MS: u64 = 5_000;
const MAX_OFFERED: usize = 6;

/// From when the agent has sat idle a while with nothing waiting for it, or
/// none while it is at work or something waits for it.
fn idle_from(core: &Core, agent_id: &str) -> Option<u64> {
    let now = core.agent_now(agent_id);
    if now.waits_on_something() {
        return None;
    }
    now.resting().then(|| {
        core.seen()
            .active_at(agent_id)
            .map_or(0, |at| at + IDLE_BEFORE_OFFER_MS)
    })
}

/// Tells each idle agent another agent started of the open cards of its team
/// that nobody holds and nothing holds up, once per card, unless its lead is
/// at work and so may hand it one itself. Answers when an agent that has such
/// cards will have been idle long enough.
pub(super) fn offer(core: &Arc<Core>) -> Option<u64> {
    forget_stale_offers(core);
    let now = epoch_ms();
    let running = core.running_agents();
    let workers: Vec<(String, String)> = running
        .iter()
        .filter_map(|agent| {
            let named = core.names().get(agent)?;
            let lead = named.parent.as_ref()?;
            idle_from(core, lead)
                .is_some()
                .then(|| (agent.clone(), named.project))
        })
        .collect();
    if workers.is_empty() {
        return None;
    }
    let mut next = None;
    let mut offers: Vec<(String, Vec<String>, String)> = Vec::new();
    {
        let mut state = core.boards().state.locked();
        let State {
            boards,
            offered,
            woken,
            ..
        } = &mut *state;
        for (agent, project) in workers {
            let Some(board) = boards.get(&project) else {
                continue;
            };
            let busy = board.cards.iter().any(|card| {
                card.owner_id() == Some(agent.as_str()) && card.status == CardStatus::Progress
            });
            if busy {
                continue;
            }
            let team = core.names().team_of(&agent);
            let free: Vec<&Card> = board
                .cards
                .iter()
                .filter(|card| {
                    card.status == CardStatus::Open
                        && card.owner.is_none()
                        && board.waits_on(card).is_empty()
                        && !board
                            .cards
                            .iter()
                            .any(|other| other.parent == Some(card.id))
                        && !offered.contains(&(project.clone(), card.id, agent.clone()))
                        && team_of(core, board, card).is_some_and(|ours| ours.lead == team)
                })
                .take(MAX_OFFERED)
                .collect();
            if free.is_empty() {
                continue;
            }
            match idle_from(core, &agent) {
                Some(from) if from <= now => {}
                Some(from) => {
                    next = sooner(next, Some(from));
                    continue;
                }
                None => continue,
            }
            for card in &free {
                offered.insert((project.clone(), card.id, agent.clone()));
            }
            woken.insert(agent.clone());
            offers.push((
                agent,
                free.iter().map(|card| card.label()).collect(),
                name_of(core, &team),
            ));
        }
    }
    for (agent, cards, lead) in offers {
        let text = format!(
            "Open cards on your team's board that nobody has taken:\n\n{}\n\nIf one is yours to do, claim it with board_claim and do it; board_list with card shows one in full. If none is, end your turn without doing anything.",
            cards.join("\n")
        );
        let note = format!(
            "Sent by Sikemux's board, not by another agent, because you are idle and these cards are {lead}'s team's."
        );
        let core = core.clone();
        tokio::spawn(async move {
            let sent =
                (core.board_above().messages_from_board)(&core, agent.clone(), text, note).await;
            let new_turn = sent.is_ok_and(|sent| sent["delivery"] == "newTurn");
            if !new_turn {
                core.boards().state.locked().woken.remove(&agent);
            }
        });
    }
    next
}

/// Forgets offers of cards no longer open, or gone, and offers to agents
/// since closed.
fn forget_stale_offers(core: &Core) {
    let mut state = core.boards().state.locked();
    if state.offered.is_empty() {
        return;
    }
    let offered_to: HashSet<String> = state
        .offered
        .iter()
        .map(|(_, _, agent)| agent.clone())
        .collect();
    let named: HashSet<String> = core
        .names()
        .of(&offered_to)
        .into_iter()
        .map(|named| named.agent_id)
        .collect();
    let State {
        boards, offered, ..
    } = &mut *state;
    offered.retain(|(project, id, agent)| {
        named.contains(agent)
            && boards
                .get(project)
                .and_then(|board| board.card(*id).ok())
                .is_some_and(|card| card.status == CardStatus::Open)
    });
}

/// Whether the agent's turn was one the board started to offer it cards, in
/// which it took none and changed nothing on the board. Ends that turn's mark.
pub(crate) fn offer_turn_ignored(core: &Core, agent_id: &str) -> bool {
    core.boards().state.locked().woken.remove(agent_id)
}

/// Something besides the board's offer reached the agent in the turn the
/// offer started, so that turn's end is worth reporting.
pub(crate) fn reached_by_more_than_offer(core: &Core, agent_id: &str) {
    core.boards().state.locked().woken.remove(agent_id);
}
