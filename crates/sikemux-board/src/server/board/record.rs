//! A project's board as its hosts share it (ADR 0023): its home keeps it and sends it to the
//! others, which each keep a copy for reading.

use serde_json::Value;
use sikemux_process::sync::Locked;

use super::{archive, Board, Core, VERSION};
use crate::server::board_state::BoardParts;
use crate::server::repo_state::RepoParts;

/// The board kept under `key`, as its home sends it.
pub fn record(core: &Core, key: &str) -> Option<Value> {
    let mut state = core.boards().state.locked();
    let board = state.boards.get_mut(key)?;
    board.shared = true;
    serde_json::to_value(board.saved()).ok()
}

/// What this host kept before a home's copy replaced it, for that home to take in.
pub enum Replaced {
    /// Cards this host kept on its own, never shared, to fold in under new ids.
    Unshared(Value),
    /// The board this host kept as the project's home, to merge by card.
    KeptAsHome(Value),
}

/// Whether the board under `key` is a copy of what another host, the project's home, sent.
pub fn is_copy(core: &Core, key: &str) -> bool {
    (core.boards().state.locked().boards.get(key)).is_some_and(|board| board.mirrored)
}

/// This host keeps the board under `key` as the project's home from now on.
pub fn kept_here(core: &Core, key: &str) {
    let boards = core.boards();
    let mut state = boards.state.locked();
    if let Some(board) = state.boards.get_mut(key).filter(|board| board.mirrored) {
        board.mirrored = false;
        boards.save_key(&mut state, key);
    }
}

/// Takes `record`, which the project's home sent, as this host's board under `key`. Answers what
/// this host kept until now that the home lacks.
pub fn mirror(core: &Core, key: &str, record: Value) -> Result<Option<Replaced>, String> {
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
    sent.mirrored = true;
    sent.settings.places = kept.places;
    sent.settings.places_confirmed = kept.confirmed;
    let unshared = own.and_then(|mut own| {
        sent.places_proposal = own.places_proposal.take();
        sent.ci_failing = std::mem::take(&mut own.ci_failing);
        let saved = serde_json::to_value(own.saved()).ok()?;
        match (own.shared, own.mirrored) {
            (false, _) if !own.cards.is_empty() => Some(Replaced::Unshared(saved)),
            (true, false) => Some(Replaced::KeptAsHome(saved)),
            _ => None,
        }
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

/// Merges in the board another host kept as the project's home before this one took over.
pub fn merge_in(core: &Core, key: &str, board: Value) -> Result<(), String> {
    let theirs: Board = serde_json::from_value(board).map_err(|error| error.to_string())?;
    let boards = core.boards();
    let folder = (boards.state.locked().boards.get(key)).map(|board| board.folder.clone());
    let Some(folder) = folder else {
        return Err("this host keeps no board for that project".into());
    };
    let archived = archive::archived(boards, &folder);
    let mut state = boards.state.locked();
    let Some(board) = state.boards.get_mut(key) else {
        return Err("this host keeps no board for that project".into());
    };
    board.merge(theirs, &archived);
    boards.save_key(&mut state, key);
    Ok(())
}
