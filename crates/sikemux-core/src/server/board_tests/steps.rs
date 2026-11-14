use serde_json::{json, Value};

use crate::server::board::steps::*;
use crate::server::board::Board;
use crate::server::board_state::BoardParts;
use crate::server::Core;
use crate::server::ServerCore;
use sikemux_process::sync::Locked;

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
