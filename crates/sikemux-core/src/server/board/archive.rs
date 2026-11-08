//! A full board moves its oldest finished cards to a file beside the boards,
//! so a large reservoir keeps taking cards without the person clearing any.

use std::io::Write;

use super::*;

const FOLDER: &str = "board-archive";
/// Archiving starts this far below the limit and frees as many again, so it
/// does not run for every card added and the board keeps room for fix cards.
pub(super) const ROOM_KEPT: usize = 100;

fn may_archive(state: &State, project: &str, parents: &HashSet<u64>, card: &Card) -> bool {
    card.status == CardStatus::Done
        && !parents.contains(&card.id)
        && !state.work.contains_key(&(project.to_owned(), card.id))
        && (card.ticket.as_ref()).is_none_or(|ticket| ticket.synced == Some(CardStatus::Done))
}

/// Moves finished cards off a full board, the oldest first. A finished card
/// with cards under it stays until they have gone.
pub(super) fn make_room(boards: &Boards, state: &mut State, project: &str) {
    let Some(board) = state.boards.get(project) else {
        return;
    };
    if board.cards.len() + ROOM_KEPT < MAX_CARDS {
        return;
    }
    let parents: HashSet<u64> = board.cards.iter().filter_map(|card| card.parent).collect();
    let mut oldest: Vec<(u64, u64)> = (board.cards.iter())
        .filter(|card| may_archive(state, project, &parents, card))
        .map(|card| (card.updated_at, card.id))
        .collect();
    oldest.sort_unstable();
    oldest.truncate(board.cards.len() + 2 * ROOM_KEPT - MAX_CARDS);
    if oldest.is_empty() {
        return;
    }
    let going: HashSet<u64> = oldest.into_iter().map(|(_, id)| id).collect();
    let Some(board) = state.boards.get_mut(project) else {
        return;
    };
    let (gone, kept): (Vec<Card>, Vec<Card>) = std::mem::take(&mut board.cards)
        .into_iter()
        .partition(|card| going.contains(&card.id));
    board.cards = kept;
    for card in &mut board.cards {
        card.depends_on.retain(|other| !going.contains(other));
    }
    for card in &gone {
        board.ci_failing.forget(&card.ci);
    }
    slice::rebind(board);
    boards.save(state, project);
    if let Some(data) = boards.folder.as_deref().and_then(Path::parent) {
        append(file(data, project), gone);
    }
}

fn file(data: &Path, project: &str) -> PathBuf {
    (data.join(FOLDER))
        .join(kept::file_for(project))
        .with_extension("jsonl")
}

/// The project's finished cards that left its board.
pub(super) fn archived(boards: &Boards, project: &str) -> Vec<Card> {
    let Some(data) = boards.folder.as_deref().and_then(Path::parent) else {
        return Vec::new();
    };
    std::fs::read_to_string(file(data, project))
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Adds the cards to their board's file, off the thread that holds the boards.
fn append(path: PathBuf, cards: Vec<Card>) {
    let written = move || -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        for card in &cards {
            writeln!(file, "{}", serde_json::to_string(card)?)?;
        }
        Ok(())
    };
    let spawned = std::thread::Builder::new()
        .name("sikemux-board-archive".into())
        .spawn(move || {
            if let Err(error) = written() {
                eprintln!("sikemux core: finished cards were not archived: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("sikemux core: finished cards were not archived: {error}");
    }
}
