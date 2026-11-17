use std::collections::HashMap;

use sikemux_process::sync::Locked;

use super::{kept, Board, Core, State};
use crate::server::board_state::BoardParts;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;

/// Keeps each board under its project's record: the repository's identity, else the folder
/// (ADR 0023). A board kept under an older key moves, and two clones' boards fold into one.
pub fn rekey(core: &Core) {
    let keys: HashMap<String, String> = (core.workspaces().projects().into_iter())
        .map(|project| {
            let folder = project.path.to_string_lossy().into_owned();
            let key = project.identity.unwrap_or_else(|| folder.clone());
            (folder, key)
        })
        .collect();
    let boards = core.boards();
    let mut state = boards.state.locked();
    let mut moved: Vec<(String, String)> = Vec::new();
    for (folder, key) in &keys {
        let old = state.key(folder);
        if old == *key || keys.values().any(|still| *still == old) {
            continue;
        }
        let Some(mut board) = state.boards.remove(&old) else {
            continue;
        };
        board.project = key.clone();
        match state.boards.get_mut(key) {
            Some(into) => into.fold(board),
            None => {
                state.boards.insert(key.clone(), board);
            }
        }
        moved.push((old, key.clone()));
    }
    state.keys = keys;
    placed(core, &mut state);
    for (_, key) in &moved {
        boards.save_key(&mut state, key);
    }
    drop(state);
    if moved.is_empty() {
        return;
    }
    boards.writer.flush();
    let Some(folder) = &boards.folder else {
        return;
    };
    for (old, _) in moved {
        let file = folder.join(kept::file_for(&old));
        if let Err(error) = std::fs::remove_file(&file) {
            eprintln!("sikemux core: {} was not removed: {error}", file.display());
        }
    }
}

/// Points each board at its folder on this host, and at the places kept for that folder.
fn placed(core: &Core, state: &mut State) {
    let folders: HashMap<String, String> = (state.keys.iter())
        .map(|(folder, key)| (key.clone(), folder.clone()))
        .collect();
    let mut moved = Vec::new();
    for board in state.boards.values_mut() {
        let folder = folders.get(&board.project).unwrap_or(&board.project);
        if board.folder == *folder {
            continue;
        }
        moved.push((
            std::mem::replace(&mut board.folder, folder.clone()),
            folder.clone(),
        ));
        let kept = core.places().of(folder);
        board.settings.places = kept.places;
        board.settings.places_confirmed = kept.confirmed;
    }
    for (old, new) in moved {
        state.dirty.remove(&old);
        state.dirty.insert(new);
    }
}

impl Board {
    /// Takes in `other`'s cards and decisions, numbered after its own.
    pub(super) fn fold(&mut self, other: Board) {
        let offset = self.next.saturating_sub(1);
        let card = |id: u64| id + offset;
        for mut taken in other.cards {
            taken.id = card(taken.id);
            taken.parent = taken.parent.map(card);
            taken.depends_on = taken.depends_on.into_iter().map(card).collect();
            self.cards.push(taken);
        }
        self.next = self.next.max(other.next + offset);
        self.decided.fold(other.decided, card);
        self.places.take();
    }
}
