//! A project's board as its hosts share it (ADR 0023): its home keeps it and sends it to the
//! others, which each keep a copy for reading.

use serde_json::Value;
use sikemux_process::sync::Locked;

use super::{Board, Core, VERSION};
use crate::server::board_state::BoardParts;
use crate::server::repo_state::RepoParts;

/// The board kept under `key`, as its home sends it.
pub fn record(core: &Core, key: &str) -> Option<Value> {
    let mut state = core.boards().state.locked();
    let board = state.boards.get_mut(key)?;
    board.shared = true;
    serde_json::to_value(board.saved()).ok()
}

/// Takes `record`, which the project's home sent, as this host's board under `key`. Answers the
/// cards this host kept on its own until now, for the home to fold in.
pub fn mirror(core: &Core, key: &str, record: Value) -> Result<Option<Value>, String> {
    let mut sent: Board = serde_json::from_value(record).map_err(|error| error.to_string())?;
    if sent.version > VERSION {
        return Err("this host runs an older Sikemux than the project's home".into());
    }
    let boards = core.boards();
    let mut state = boards.state.locked();
    let own = state.boards.remove(key);
    let folder = state.folder_of(key);
    let kept = core.places().of(&folder);
    sent.version = VERSION;
    sent.project = key.to_owned();
    sent.folder = folder;
    sent.shared = true;
    sent.settings.places = kept.places;
    sent.settings.places_confirmed = kept.confirmed;
    let unshared = own.and_then(|mut own| {
        sent.places_proposal = own.places_proposal.take();
        sent.ci_failing = std::mem::take(&mut own.ci_failing);
        (!own.shared && !own.cards.is_empty()).then(|| serde_json::to_value(own.saved()).ok())?
    });
    state.boards.insert(key.to_owned(), sent);
    boards.save_key(&mut state, key);
    Ok(unshared)
}

/// Folds in the cards another host kept of its own before it shared this board.
pub fn fold_in(core: &Core, key: &str, cards: Value) -> Result<(), String> {
    let theirs: Board = serde_json::from_value(cards).map_err(|error| error.to_string())?;
    let boards = core.boards();
    let mut state = boards.state.locked();
    let Some(board) = state.boards.get_mut(key) else {
        return Err("this host keeps no board for that project".into());
    };
    board.fold(theirs);
    boards.save_key(&mut state, key);
    Ok(())
}
