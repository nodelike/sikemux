#![allow(unused_imports)]

use crate::server::board::slice::{Later, Seen};
use crate::server::board::{Board, By, CardStatus, Changes, Notice, State};
use crate::server::board_state::BoardParts;
use crate::server::places::{self, Place};
use crate::server::Core;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_calls::board::{BoardProposalParams, PlacesProposal};
use sikemux_process::clock::epoch_ms;
use sikemux_view::slice::Gate;

use crate::server::ServerCore;
use serde_json::json;
use sikemux_process::sync::Locked;

use crate::server::board::steps::*;

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
