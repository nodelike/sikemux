#![allow(unused_imports)]

use crate::cli::protocol::HarnessRequest;
use crate::server::agent_now::AgentNowCore;
use crate::server::board::auto::AutoRun;
use crate::server::board::auto::BoardSettingsExt;
use crate::server::board::auto::{
    BoardReviewerParams, BoardSettings, BoardSettingsParams, CardPull, CardRun, Landing,
    ReviewerChoice, RunHold, RunStage,
};
use crate::server::board::contract::{CardContract, ContractState, ContractTerm};
use crate::server::board::failing::ci_ruled;
use crate::server::board::finish::{ci_moved, waiting_on_ci};
use crate::server::board::goal::AssertParams;
use crate::server::board::hand_off::hand_off;
use crate::server::board::hand_off::{BoardHandOffParams, HandOnParams, HandedOff};
use crate::server::board::lease::{person_hand_on, person_resume};
use crate::server::board::record::record_check;
use crate::server::board::slice::is_page_file;
use crate::server::board::slice::{
    check_stands, follow, gate_key, gate_words, person_note, person_pass, screenshot_taken,
    slice_screenshots,
};
use crate::server::board::slice::{turn_pictures, verdict_on};
use crate::server::board::slice::{BoardNoteParams, BoardPassParams};
use crate::server::board::steps::Step;
use crate::server::board::steps::{BoardProposalParams, PlacesProposal};
use crate::server::board::wait::owners_on_check;
use crate::server::board_state::BoardParts;
use crate::server::changed::ChangedCore;
use crate::server::changed::Topic;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::checks::CheckRan;
use crate::server::checks::CiCard;
use crate::server::checks::{project_check, Checker, NoCheck};
use crate::server::codehost::repo::AutoPull;
use crate::server::connection::blocking;
use crate::server::kept;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_events::DeployRan;
use crate::server::repo_state::RepoParts;
use crate::server::wake::{sooner, Wake};
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;
use crate::text::clipped;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_calls::board::{
    BoardAddParams, BoardAddTicketParams, BoardCard, BoardCardParams, BoardColumn, BoardParams,
    BoardTicketCard, BoardTicketParams, BoardUpdateParams, CardStatus, CardTicket,
    ForgetInheritedParams, ProjectBoard, Tracker,
};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::inbox::InboxRecording;
use sikemux_view::slice::{Assertion, Collect, SliceState, Wait};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use crate::server::board::*;
use crate::server::ServerCore;

const PROJECT: &str = "/project";

fn done() -> Changes {
    Changes {
        status: Some(CardStatus::Done),
        ..Changes::default()
    }
}

/// On 2026-11-01 a card stayed in progress after its owner closed: neither the owner's lead,
/// its reopened session nor the card's creator could mark it done.
#[test]
fn a_closed_owners_card_is_free_and_a_live_owners_is_not() {
    let core = Core::new(Default::default(), None).unwrap();
    let creator = By::Agent("creator".to_owned());
    let adding = Adding {
        title: "Stuck".to_owned(),
        ..Adding::default()
    };
    let (id, _) = add_card(&core, PROJECT, &creator, adding, None).unwrap();
    core.names().pin(HashSet::from(["owner".to_owned()]));
    claim_card(&core, PROJECT, "owner", id).unwrap();

    let refused = change_card(&core, PROJECT, &creator, id, done()).map(|_| ());
    assert!(refused.unwrap_err().contains("is owner's"));
    let refused = claim_card(&core, PROJECT, "creator", id);
    assert!(refused.unwrap_err().contains("has #"));

    core.names().pin(HashSet::new());

    let (view, _) = change_card(&core, PROJECT, &creator, id, done()).unwrap();
    assert_eq!(view["status"], "done");
}

fn claimed_card(core: &Arc<Core>, owner: &str) -> u64 {
    let adding = Adding {
        title: "Work".to_owned(),
        ..Adding::default()
    };
    let creator = By::Agent("creator".to_owned());
    let (id, _) = add_card(core, PROJECT, &creator, adding, None).unwrap();
    core.names().pin(HashSet::from([owner.to_owned()]));
    claim_card(core, PROJECT, owner, id).unwrap();
    id
}

fn stalled_on(core: &Core, id: u64) -> Option<String> {
    let state = core.boards().state.locked();
    let card = state.boards[PROJECT].card(id).unwrap();
    lease::stalled_of(core, card).map(|stalled| stalled.agent)
}

/// On 2026-11-06 the notch kept asking to resume a chat the person had deleted for good.
#[test]
fn a_deleted_chats_card_stops_asking_for_it() {
    let core = Core::new(Default::default(), None).unwrap();
    core.name_agent("owner", Path::new(PROJECT), Some("claude"), None);
    core.names().note_session("owner", "deleted");
    let id = claimed_card(&core, "owner");
    core.names().pin(HashSet::new());
    assert_eq!(stalled_on(&core, id).as_deref(), Some("owner"));

    lease::session_deleted(&core, "another");
    assert_eq!(stalled_on(&core, id).as_deref(), Some("owner"));

    lease::session_deleted(&core, "deleted");
    assert_eq!(stalled_on(&core, id), None);
    let state = core.boards().state.locked();
    let card = state.boards[PROJECT].card(id).unwrap();
    assert_eq!(card.status, CardStatus::Open);
    assert_eq!(card.owner_id(), None);
}

/// On 2026-11-08 a slice marked done again on the commit whose check failed, after its fix
/// card was done with no new commit, got a second fix card for that same commit.
#[test]
fn a_failing_commit_gets_one_fix_card() {
    let core = Core::new(Default::default(), None).unwrap();
    let adding = Adding {
        title: "Slice".to_owned(),
        promise: Some("It works.".to_owned()),
        ..Adding::default()
    };
    let creator = By::Agent("creator".to_owned());
    let (id, _) = add_card(&core, PROJECT, &creator, adding, None).unwrap();
    core.names().pin(HashSet::from(["owner".to_owned()]));
    claim_card(&core, PROJECT, "owner", id).unwrap();
    let sha = "c1f0c6e0000000000000000000000000000000000";
    let mut state = core.boards().state.locked();
    let board = state.boards.get_mut(PROJECT).unwrap();

    slice::file_fix(&core, board, id, sha).unwrap();
    let fix = board.cards.last().unwrap().id;
    board.card_mut(fix).unwrap().status = CardStatus::Done;
    let said = slice::file_fix(&core, board, id, sha).unwrap();

    assert_eq!(board.cards.len(), 2);
    assert!(said.contains("new commit"));
}

/// The same day, cards of chats closed weeks before all came up as stalled at once.
#[test]
fn stalls_from_before_are_cleared_once_and_later_ones_still_ask() {
    let core = Core::new(Default::default(), None).unwrap();
    let old = claimed_card(&core, "gone");
    let live = claimed_card(&core, "live");
    core.names().pin(HashSet::from(["live".to_owned()]));
    core.boards()
        .state
        .locked()
        .old_stalls
        .push(PROJECT.to_owned());

    lease::clear_old_stalls(&core);
    assert_eq!(stalled_on(&core, old), None);
    assert_eq!(stalled_on(&core, live), None);

    core.names().pin(HashSet::new());
    assert_eq!(stalled_on(&core, live).as_deref(), Some("live"));
    lease::clear_old_stalls(&core);
    assert_eq!(stalled_on(&core, live).as_deref(), Some("live"));
}
