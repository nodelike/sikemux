//! The steps a project's places give its slices' ladders, and the places an agent proposes
//! for the person to accept. A place that isn't production passes its step once a deploy
//! there went well, after the project's check where its step says so; a production place's
//! step is the person's while its level is `mine`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_view::slice::Gate;

use super::slice::{Later, Seen};
use super::{Board, By, CardStatus, Changes, Notice, State};
use crate::server::places::{self, Place};
use crate::server::Core;

const MAX_WHY_CHARS: usize = 500;

/// Places an agent proposed, for the person to accept or turn down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacesProposal {
    pub places: Vec<Place>,
    pub why: String,
    /// The agent that proposed them, by name.
    pub by: String,
    pub at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardProposalParams {
    pub project: String,
    pub accept: bool,
}

/// `board_decide` with environments: keeps the places an agent proposed for the person to
/// answer, in place of any it proposed before. It changes nothing else. Each is given as an
/// environment was, with a name, who passes it and the facts that recognise it.
pub(super) fn propose(
    board: &mut Board,
    given: Vec<Value>,
    why: &str,
    by: String,
) -> Result<(), String> {
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

/// The ladder a project's places and landing give a slice. Where the
/// person lands slices, Landed stays first, as nothing counts as landed before they say.
pub(super) fn ladder_steps(person_lands: bool, places: &[Place]) -> Vec<Step> {
    let mut steps = vec![Step::Gate(Gate::Built)];
    if person_lands {
        steps.push(Step::Gate(Gate::Landed));
    }
    steps.extend(
        (places.iter())
            .filter(|place| place.step.is_some())
            .map(|place| Step::Environment(place.shown.name.clone())),
    );
    steps
}

/// A step of a slice's ladder: a gate every project knows, or one of its
/// environments, by name. In the board's file a gate keeps its old key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Step {
    Gate(Gate),
    Environment(String),
}

impl Step {
    /// What a released client reads for it, which knows only the three gates:
    /// an environment comes after what lands, so it reads as Landed.
    pub(crate) fn gate(&self) -> Gate {
        match self {
            Self::Gate(gate) => *gate,
            Self::Environment(_) => Gate::Landed,
        }
    }

    pub(crate) fn environment(&self) -> Option<&str> {
        match self {
            Self::Gate(_) => None,
            Self::Environment(name) => Some(name),
        }
    }

    /// The step a client's `gate` and `environment` name.
    pub(crate) fn named(gate: Gate, environment: Option<&str>) -> Self {
        match environment {
            Some(name) => Self::Environment(name.to_owned()),
            None => Self::Gate(gate),
        }
    }
}

/// What is left to do once the board's lock is let go of, after ladders moved.
#[derive(Default)]
pub(super) struct Settled {
    pub notices: Vec<Notice>,
    pub later: Later,
}

/// The person answers an agent's proposal: the places they accept, none when they turn it down.
pub(super) fn answer_proposal(
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

/// Moves on each open slice of `project` whose ladder just changed, as a step
/// that went may leave it with nothing to wait at.
pub(super) fn settle_cards(core: &Core, state: &mut State, project: &str) -> Settled {
    let open: Vec<u64> = (state.boards.get(project).into_iter())
        .flat_map(|board| board.cards.iter())
        .filter(|card| card.slice.is_some() && card.status != CardStatus::Done)
        .map(|card| card.id)
        .collect();
    let mut settled = Settled::default();
    for id in open {
        let moved = super::change_in(
            core,
            state,
            project,
            &By::Auto("Board"),
            id,
            Changes::default(),
            &Seen::unread(),
        );
        match moved {
            Ok((_, notices, later)) => {
                settled.notices.extend(notices);
                settled.later = std::mem::take(&mut settled.later).and(later);
            }
            Err(error) => {
                eprintln!("sikemux core: #{id} did not move with its new ladder: {error}")
            }
        }
    }
    settled
}

#[cfg(test)]
mod tests {
    use crate::server::parts::Parts;
    use crate::server::ServerCore;
    use serde_json::json;
    use sikemux_process::sync::Locked;

    use super::*;

    const PROJECT: &str = "/tmp/sikemux-places-proposal-test";

    fn production_loosened() -> Vec<Value> {
        vec![json!({
            "name": "Production",
            "passes": "nobody",
            "recognise": [{ "kind": "rundeckJob", "path": "prod/*" }],
        })]
    }

    /// An agent's proposal changes nothing until the person accepts it.
    #[test]
    fn a_proposal_changes_no_setting() {
        let core = Core::new(Default::default(), None).unwrap();
        let mut state = core.boards().state.locked();
        let board = state
            .boards
            .entry(PROJECT.to_owned())
            .or_insert_with(|| Board::empty(PROJECT));
        propose(
            board,
            production_loosened(),
            "We deploy there.",
            "Otter".to_owned(),
        )
        .unwrap();
        assert!(board.settings.places.is_empty());
        assert!(board.places_proposal.is_some());
        answer_proposal(&mut state, PROJECT, false).unwrap();
        let board = state.boards.get_mut(PROJECT).unwrap();
        assert!(board.settings.places.is_empty());
        assert!(board.places_proposal.is_none());
    }

    #[test]
    fn a_proposal_cannot_loosen_production_either() {
        let mut board = Board::empty(PROJECT);
        propose(
            &mut board,
            production_loosened(),
            "Faster.",
            "Otter".to_owned(),
        )
        .unwrap();
        let proposed = &board.places_proposal.unwrap().places;
        assert!(proposed.iter().all(|place| place.shown.production));
    }
}
