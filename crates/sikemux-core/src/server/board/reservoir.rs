//! A project's reservoir: a command its `sikemux.json` names that lists what is left to do, one
//! JSON item per line. The board holds only a limited number of them as cards, so a list of tens
//! of thousands costs it no more than a short one, and each landing tops the cards up.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::archive::{self, ROOM_KEPT};
use super::auto::MAX_SLOTS;
use super::slice::{self, Later};
use super::{
    change_in, new_card, send_notices, Adding, Board, By, Card, CardStatus, Changes, Notice, State,
    MAX_CARDS, MAX_TITLE_CHARS,
};
use crate::server::tasks::{ask_person, config_needs};
use crate::server::worktree;
use crate::server::Core;
use crate::tasks::config::{self, ConfigLoad};
use crate::text::{clipped, clipped_line};

/// As many as Auto could start at once, so it never waits on a refill to fill its places.
const MAX_OPEN_CARDS: usize = MAX_SLOTS as usize;
const RUNNING_AT_ONCE: usize = 2;
const TIME_LIMIT: Duration = Duration::from_secs(30 * 60);
const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;
const MAX_KEY_BYTES: usize = 200;
const MAX_DETAILS_CHARS: usize = 4000;
const MAX_FAILURE_CHARS: usize = 400;
/// Landed cards an item may have before it rests, so one that no fix clears stops costing runs.
const MAX_LANDED_TRIES: usize = 3;
const WHO: &str = "The reservoir";

#[derive(Default)]
pub(super) struct Runs {
    running: HashSet<String>,
    /// Projects due a run, each with the card whose landing asked for it, if one did.
    wanted: HashMap<String, Option<u64>>,
    /// Boards with Auto on whose first run since it turned on has been asked for.
    began: HashSet<String>,
    awaiting_trust: HashSet<String>,
}

#[derive(Deserialize)]
struct Item {
    key: String,
    title: String,
    #[serde(default)]
    details: String,
    #[serde(default)]
    priority: i32,
}

/// What a run printed.
struct Listed {
    /// Each key, with its priority.
    keys: HashMap<String, i32>,
    /// Without repeated keys, the highest priority first and otherwise as printed.
    items: Vec<Item>,
}

#[derive(Default)]
struct Applied {
    added: u32,
    closed: u32,
    /// Items still listed that are done trying.
    resting: usize,
}

enum Outcome {
    NoReservoir,
    AwaitsTrust,
    Ran(Applied),
}

/// Starts the reservoirs that are due, as far as the limit allows, and asks for a run of each
/// board Auto has just turned on for, which a core starting with Auto on counts as.
pub(super) fn due(core: &Arc<Core>) {
    let starts: Vec<(String, Option<u64>)> = {
        let mut state = core.boards.state.locked();
        let State {
            boards,
            reservoirs,
            dirty,
            ..
        } = &mut *state;
        for (project, board) in boards.iter_mut() {
            if !board.settings.reservoir && board.reservoir_note.take().is_some() {
                dirty.insert(project.clone());
            }
        }
        let on: HashSet<&String> = (boards.iter())
            .filter(|(_, board)| board.settings.auto && board.settings.reservoir)
            .map(|(project, _)| project)
            .collect();
        reservoirs.began.retain(|project| on.contains(project));
        reservoirs.wanted.retain(|project, _| on.contains(project));
        for project in on {
            if reservoirs.began.insert(project.clone()) {
                reservoirs.wanted.insert(project.clone(), None);
            }
        }
        let free = RUNNING_AT_ONCE.saturating_sub(reservoirs.running.len());
        let ready: Vec<String> = (reservoirs.wanted.keys())
            .filter(|project| !reservoirs.running.contains(*project))
            .take(free)
            .cloned()
            .collect();
        (ready.into_iter())
            .map(|project| {
                let landed = reservoirs.wanted.remove(&project).flatten();
                reservoirs.running.insert(project.clone());
                (project, landed)
            })
            .collect()
    };
    for (project, landed) in starts {
        tokio::spawn(run(core.clone(), project, landed));
    }
}

/// Card `card` of `project` landed, so what its reservoir lists may have changed.
pub(super) fn landed(core: &Core, project: &str, card: u64) {
    (core.boards.state.locked().reservoirs.wanted).insert(project.to_owned(), Some(card));
    core.boards.wake.now();
}

async fn run(core: Arc<Core>, project: String, landed: Option<u64>) {
    let started = Instant::now();
    let outcome = refill(&core, &project).await;
    let took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let note = match &outcome {
        Err(why) => {
            eprintln!("sikemux core: the reservoir of {project} failed: {why}");
            Some(format!("{WHO} changed no cards: {why}"))
        }
        Ok(Outcome::AwaitsTrust) => Some(format!("{WHO} waits for you to trust sikemux.json")),
        Ok(Outcome::Ran(Applied { resting, .. })) if *resting > 0 => Some(format!(
            "{WHO} still lists {resting} items after {MAX_LANDED_TRIES} landed fixes each, and leaves them until they go away"
        )),
        Ok(_) => None,
    };
    let mut state = core.boards.state.locked();
    state.reservoirs.running.remove(&project);
    if let Some(board) = state.boards.get_mut(&project) {
        let noted = board.reservoir_note != note;
        board.reservoir_note = note;
        let recorded = landed.is_some_and(|card| record(board, card, &outcome, took_ms));
        if recorded {
            core.boards.save(&mut state, &project);
        } else if noted {
            state.dirty.insert(project.clone());
        }
    }
    drop(state);
    core.boards.wake.now();
}

/// Adds a run to the run record of the card whose landing started it.
fn record(board: &mut Board, card: u64, outcome: &Result<Outcome, String>, took_ms: u64) -> bool {
    let Some(record) = board
        .card_mut(card)
        .ok()
        .and_then(|card| card.record.as_mut())
    else {
        return false;
    };
    match outcome {
        Ok(Outcome::Ran(applied)) => {
            record.reservoir_added += applied.added;
            record.reservoir_closed += applied.closed;
        }
        Err(_) => record.reservoir_failures += 1,
        Ok(_) => return false,
    }
    record.reservoir_runs += 1;
    record.reservoir_ms += took_ms;
    true
}

async fn refill(core: &Arc<Core>, project: &str) -> Result<Outcome, String> {
    let dir = PathBuf::from(project);
    let reading = {
        let (core, dir) = (core.clone(), dir.clone());
        tokio::task::spawn_blocking(move || {
            let load = config::load(&dir);
            let ask = config_needs(&core, &load);
            match load {
                ConfigLoad::Valid { config, .. } => Ok((config.reservoir, ask)),
                ConfigLoad::Absent { .. } => Ok((None, None)),
                ConfigLoad::Invalid { errors, .. } => Err(config::invalid_words(&errors)),
            }
        })
        .await
        .unwrap_or_else(|error| Err(error.to_string()))?
    };
    let command = match reading {
        (None, _) => return Ok(Outcome::NoReservoir),
        (Some(_), Some(ask)) => {
            ask_trust(core, project, ask);
            return Ok(Outcome::AwaitsTrust);
        }
        (Some(command), None) => command,
    };
    let started_at = epoch_ms();
    let tip = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || worktree::latest_of_base(&dir))
            .await
            .unwrap_or_else(|error| Err(error.to_string()))
            .map_err(|error| format!("its commit could not be found: {error}"))?
            .1
    };
    let checkout = worktree::agent::reservoir(core, &dir, &tip)
        .await
        .map_err(|error| format!("its checkout is not ready: {error}"))?;
    let listed = tokio::task::spawn_blocking(move || listed_by(&checkout, &command))
        .await
        .unwrap_or_else(|error| Err(error.to_string()))?;
    Ok(Outcome::Ran(apply(core, project, &listed, started_at)))
}

/// Asks the person to trust the project's `sikemux.json`, once however many things wait on it;
/// each looks again if they do.
pub(super) fn ask_trust(core: &Arc<Core>, project: &str, ask: crate::server::tasks::TrustAsk) {
    let first = (core.boards.state.locked().reservoirs.awaiting_trust).insert(project.to_owned());
    if !first {
        return;
    }
    let (core, project) = (core.clone(), project.to_owned());
    tokio::spawn(async move {
        let trusted = ask_person(&core, &project, ask, None).await;
        let mut state = core.boards.state.locked();
        state.reservoirs.awaiting_trust.remove(&project);
        if trusted {
            state.roles.want(project.clone());
            state.reservoirs.wanted.insert(project, None);
        }
        drop(state);
        core.boards.wake.now();
    });
}

fn listed_by(checkout: &Path, command: &str) -> Result<Listed, String> {
    let mut shell = sikemux_process::user_environment::background_shell();
    shell.args(["-c", command]).current_dir(checkout);
    let output = sikemux_process::run(&mut shell, None, TIME_LIMIT, OUTPUT_LIMIT, None)
        .map_err(|error| format!("it did not finish: {error}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "it failed with {}: {}",
            output.status,
            clipped_line(&said, MAX_FAILURE_CHARS)
        ));
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| "it printed something that is not text".to_owned())?;
    parsed(text)
}

fn parsed(output: &str) -> Result<Listed, String> {
    let mut keys = HashMap::new();
    let mut items = Vec::new();
    for (at, line) in output.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let item = item_of(line).map_err(|why| format!("line {} is not an item: {why}", at + 1))?;
        if keys.insert(item.key.clone(), item.priority).is_none() {
            items.push(item);
        }
    }
    items.sort_by_key(|item| Reverse(item.priority));
    Ok(Listed { keys, items })
}

/// The item a line holds, its words made safe to show: a title is one line, and neither has
/// control characters, which a terminal or a chat would otherwise act on.
fn item_of(line: &str) -> Result<Item, String> {
    let item: Item = serde_json::from_str(line).map_err(|error| error.to_string())?;
    if item.key.is_empty() || item.key.len() > MAX_KEY_BYTES {
        return Err(format!("key must be 1 to {MAX_KEY_BYTES} bytes"));
    }
    let title = item.title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = clipped(&without_control(&title), MAX_TITLE_CHARS);
    if title.is_empty() {
        return Err("title is empty".to_owned());
    }
    let details = clipped(&without_control(&item.details), MAX_DETAILS_CHARS);
    Ok(Item {
        title,
        details,
        ..item
    })
}

fn without_control(text: &str) -> String {
    (text.chars())
        .filter(|at| !at.is_control() || matches!(at, '\n' | '\t'))
        .collect()
}

/// What the board already holds of the keys, which says what a run may add.
#[derive(Default)]
struct Held {
    /// Keys that get no new card: one is unfinished, finished too recently for the run to have
    /// seen it, closed by someone without landing, or out of tries.
    taken: HashSet<String>,
    open: usize,
    resting: usize,
}

fn held(board: &Board, started_at: u64) -> Held {
    let mut held = Held::default();
    let mut landed: HashMap<&str, usize> = HashMap::new();
    for card in &board.cards {
        let Some(key) = card.reservoir.as_deref() else {
            continue;
        };
        held.open += usize::from(card.status == CardStatus::Open);
        let finished_run = (card.record.as_ref()).is_some_and(|run| run.finished_at.is_some());
        if card.status != CardStatus::Done || card.updated_at >= started_at || !finished_run {
            held.taken.insert(key.to_owned());
        } else {
            *landed.entry(key).or_default() += 1;
        }
    }
    for (key, tries) in landed {
        if tries >= MAX_LANDED_TRIES {
            held.resting += usize::from(held.taken.insert(key.to_owned()));
        }
    }
    held
}

/// Nobody works on it and its slice, if any, has not started.
fn unheld(card: &Card, parents: &HashSet<u64>) -> bool {
    card.status == CardStatus::Open
        && card.owner.is_none()
        && card.auto.is_none()
        && !parents.contains(&card.id)
        && (card.slice.as_ref()).is_none_or(|slice| !slice.started)
}

/// Closes the cards of keys no longer listed that nobody holds, and forgets the keys of finished
/// cards no longer listed, so a problem that comes back gets a fresh start.
fn close_gone(
    core: &Core,
    state: &mut State,
    project: &str,
    listed: &Listed,
) -> (u32, Vec<Notice>, Later) {
    let (mut closed, mut notices, mut later) = (0, Vec::new(), Later::default());
    let Some(board) = state.boards.get(project) else {
        return (closed, notices, later);
    };
    let parents: HashSet<u64> = (board.cards.iter())
        .filter(|card| card.status != CardStatus::Done)
        .filter_map(|card| card.parent)
        .collect();
    let gone = (board.cards.iter())
        .filter(|card| (card.reservoir.as_ref()).is_some_and(|key| !listed.keys.contains_key(key)));
    let (closing, forgetting): (Vec<_>, Vec<_>) = gone
        .filter(|card| card.status == CardStatus::Done || unheld(card, &parents))
        .map(|card| (card.id, card.status == CardStatus::Done))
        .partition(|(_, done)| !done);
    let seen = slice::Seen::unread();
    for (id, _) in &closing {
        let changes = Changes {
            status: Some(CardStatus::Done),
            result: Some("The project's reservoir no longer lists it".to_owned()),
            ..Changes::default()
        };
        if let Ok((_, said, then)) =
            change_in(core, state, project, &By::Auto(WHO), *id, changes, &seen)
        {
            closed += 1;
            notices.extend(said);
            later = later.and(then);
        }
    }
    if let Some(board) = state.boards.get_mut(project) {
        let ids: HashSet<u64> = closing
            .iter()
            .chain(&forgetting)
            .map(|(id, _)| *id)
            .collect();
        for card in board.cards.iter_mut().filter(|card| ids.contains(&card.id)) {
            card.reservoir = None;
        }
    }
    (closed, notices, later)
}

/// Brings the board in line with what a run that began at `started_at` listed, under one lock and
/// one save: its time goes with the board, never with the length of the list.
fn apply(core: &Arc<Core>, project: &str, listed: &Listed, started_at: u64) -> Applied {
    let boards = &core.boards;
    let mut state = boards.state.locked();
    let wanted = (state.boards.get(project))
        .is_some_and(|board| board.settings.auto && board.settings.reservoir);
    if !wanted {
        return Applied::default();
    }
    archive::make_room(boards, &mut state, project);
    let (closed, notices, later) = close_gone(core, &mut state, project, listed);
    let Some(board) = state.boards.get_mut(project) else {
        return Applied::default();
    };
    let reranked = rerank(board, listed);
    let standing = held(board, started_at);
    let added = add_cards(core, board, listed, &standing);
    if closed > 0 || added > 0 || reranked {
        boards.save(&mut state, project);
    }
    drop(state);
    later.run(core);
    send_notices(core, notices);
    Applied {
        added,
        closed,
        resting: standing.resting,
    }
}

/// Gives the open cards nobody holds their item's latest priority. Their words stay as filed.
fn rerank(board: &mut Board, listed: &Listed) -> bool {
    let mut changed = false;
    for card in board
        .cards
        .iter_mut()
        .filter(|card| card.status == CardStatus::Open)
    {
        let priority = (card.reservoir.as_ref()).and_then(|key| listed.keys.get(key));
        if let Some(&priority) = priority.filter(|priority| **priority != card.priority) {
            card.priority = priority;
            changed = true;
        }
    }
    changed
}

fn add_cards(core: &Core, board: &mut Board, listed: &Listed, standing: &Held) -> u32 {
    let room = (MAX_OPEN_CARDS.saturating_sub(standing.open))
        .min(MAX_CARDS.saturating_sub(ROOM_KEPT + board.cards.len()));
    let settings = board.settings.clone();
    let mut added = 0;
    for item in (listed.items.iter())
        .filter(|item| !standing.taken.contains(&item.key))
        .take(room)
    {
        let id = board.next.max(1);
        board.next = id + 1;
        let adding = Adding {
            title: item.title.clone(),
            details: item.details.clone(),
            ..Adding::default()
        };
        let mut card = new_card(core, id, adding, None, &settings);
        card.reservoir = Some(item.key.clone());
        card.priority = item.priority;
        board.cards.push(card);
        added += 1;
    }
    added
}
