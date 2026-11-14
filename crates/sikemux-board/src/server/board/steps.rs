//! The places an agent proposes for the person to accept.

use serde_json::Value;
pub use sikemux_calls::board::{BoardProposalParams, PlacesProposal};
use sikemux_process::clock::epoch_ms;

use super::{Board, State};
use crate::server::places::{self, Place};

const MAX_WHY_CHARS: usize = 500;

/// `board_decide` with environments: keeps the places an agent proposed for the person to
/// answer, in place of any it proposed before. It changes nothing else. Each is given as an
/// environment was, with a name, who passes it and the facts that recognise it.
pub fn propose(board: &mut Board, given: Vec<Value>, why: &str, by: String) -> Result<(), String> {
    let why = why.trim();
    if why.is_empty() || why.chars().count() > MAX_WHY_CHARS {
        return Err(format!(
            "why says in at most {MAX_WHY_CHARS} characters why the project should have these"
        ));
    }
    let mut proposed: Vec<Place> = Vec::new();
    for environment in &given {
        let place = places::place::from_environment(environment)
            .ok_or("each environment needs a name, who passes it and what recognises it")?;
        proposed.push(places::place::checked(place, &proposed)?);
    }
    if proposed.is_empty() {
        return Err("name at least one environment".into());
    }
    board.places_proposal = Some(PlacesProposal {
        places: proposed,
        why: why.to_owned(),
        by,
        at: epoch_ms(),
    });
    Ok(())
}

/// The person answers an agent's proposal: the places they accept, none when they turn it down.
pub fn answer_proposal(
    state: &mut State,
    project: &str,
    accept: bool,
) -> Result<Vec<Place>, String> {
    let board = state
        .boards
        .get_mut(project)
        .ok_or("this project's board has no proposal")?;
    let proposal = board
        .places_proposal
        .take()
        .ok_or("that proposal is gone")?;
    Ok(match accept {
        true => proposal.places,
        false => Vec::new(),
    })
}
