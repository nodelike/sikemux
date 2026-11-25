//! Each project's board: cards of work the person and agents add, which an
//! agent claims, works and marks done. A card may sit under another and wait
//! on others, and is free to claim once every card it waits on is done. Kept
//! in `boards/` in the data folder, one file per project, so a board outlives
//! a window reload, a restart and an update.

use crate::server::agent_now::AgentNowCore;
use crate::server::board_state::BoardParts;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::board::{
    BoardAddParams, BoardAddTicketParams, BoardCard, BoardCardParams, BoardColumn, BoardParams,
    BoardSettings, BoardSettingsParams, BoardTicketCard, BoardTicketParams, BoardUpdateParams,
    CardCommit, CardDropped, CardPull, CardStatus, CardTicket, DroppedBy, ForgetInheritedParams,
    Landing, ProjectBoard, PullState, Tracker,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::changed::Topic;
use crate::cli::protocol::HarnessRequest;
use crate::text::{clipped, one_line};

use super::connection::blocking;
use super::kept;
use super::wake::{sooner, Wake};
use super::Core;
use sikemux_view::inbox::InboxRecording;
use sikemux_view::slice::SliceState;
use sikemux_wire::protocol::ChatLaunch;

/// How the board starts an agent of its own, as `board_launch` says, on the agent's own model
/// when the org's key it names may not run in `cwd`, with what its chat then says.
fn launch_own(
    core: &Core,
    launcher: Option<&str>,
    cwd: &Path,
    provider: Option<&str>,
    model: Option<String>,
    picked_in: &str,
) -> Result<(ChatLaunch, String, Option<Value>), String> {
    let moved = (core.board_above().org_moved)(core, cwd, provider, model.as_deref());
    let (provider, model) = match moved {
        Some(_) => (None, None),
        None => (provider, model),
    };
    let (launch, launcher) =
        (core.workspaces()).board_launch(launcher, cwd, provider, model, picked_in)?;
    Ok((launch, launcher, moved))
}

mod archive;
mod arrival;
pub mod asked;
pub mod audit;
pub mod briefing;
pub mod chat_goal;
pub mod failing;
pub mod goal;
pub mod hand_off;
mod handlers;
mod key;
mod migrate;
mod mutations;
pub mod own_writes;
pub mod record;
mod spend;
pub mod steps;
mod task_rows;
pub mod tickets;
mod tidy;
mod views;

pub use handlers::*;
pub use mutations::{
    add_card, add_for_trigger, change_card, check_passed, ci_card, claim_card, committed,
    pull_seen, taken_up, triggers_changed, Subject, TriggerCard,
};
pub use views::{
    agent_card, asked_for_name, attach_recording, card_line, cards_done_since, goal_of,
    goals_to_name, ticket_of_agent, titles, told_to_teammate, unnamed_goals, wants_name, Adding,
    AddingExt, BoardTicketParamsExt, Changes, Dropping, GoalRef, Summary,
};

use mutations::*;
use views::*;

pub use super::checks::CiCard;
pub use failing::ci_ruled;
pub use hand_off::hand_off;
pub use hand_off::{BoardHandOffParams, HandedOff};
pub use key::rekey;
pub use steps::{BoardProposalParams, PlacesProposal};
pub use task_rows::task_rows;

const FOLDER: &str = "boards";
const MAX_CARDS: usize = 1000;
const MAX_TITLE_CHARS: usize = 200;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_DEPENDS: usize = 20;
const MAX_KEY_CHARS: usize = 40;
const MAX_URL_CHARS: usize = 2048;
/// How much of a card's details and result `task_list` shows before one
/// card is asked for in full.
const LISTED_TEXT_CHARS: usize = 240;
const RECENT_DONE: usize = 10;
const MAX_RECORDINGS: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Team {
    lead: String,
    name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: u64,
    title: String,
    #[serde(default)]
    details: String,
    /// What the work must do to count as done, in the person's words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    acceptance: String,
    pub status: CardStatus,
    /// The agent working it, or that did.
    #[serde(flatten)]
    owner: Option<Owner>,
    parent: Option<u64>,
    #[serde(default)]
    depends_on: Vec<u64>,
    /// The agent that added it, or none when the person did.
    #[serde(flatten)]
    created_by: Option<Maker>,
    /// The team it was last seen in, for after the team's agents close.
    #[serde(skip_serializing_if = "Option::is_none")]
    team: Option<Team>,
    result: Option<String>,
    ticket: Option<CardTicket>,
    /// The trigger whose firing added it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trigger: Option<u64>,
    /// The host whose trigger it is, since each host counts its own triggers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fired_on: Option<String>,
    /// What its trigger's firings are about, such as one pull request, which keeps every later
    /// firing about it on this card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    about: Option<String>,
    /// What CI kept failing on, on the project's branch, that it was filed
    /// for or took since.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ci: Vec<failing::Failure>,
    /// Its ticket, trigger or CI failure arriving again while it was open,
    /// oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    arrivals: Vec<arrival::Arrival>,
    /// Browser recordings its owner made while on it, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recordings: Vec<InboxRecording>,
    /// Its own rules, for a goal.
    #[serde(default, skip_serializing_if = "goal::Goal::is_none")]
    goal: goal::Goal,
    at: u64,
    updated_at: u64,
    /// When its owner last claimed it, which nothing else changes: the
    /// owner's latest claim is the card it works now.
    #[serde(default)]
    acted_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dropped: Option<CardDropped>,
    /// When the person opened it once it was done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opened_at: Option<u64>,
    /// The landed commit of its owner's that the project's check passed on while the owner worked
    /// it. The card closes by itself once the owner leaves it or waits on the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checked: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    commits: Vec<CardCommit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pulls: Vec<CardPull>,
    /// What the person asked, when it is their ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asked: Option<asked::Asked>,
}

/// Bumped by a change to what a board file holds that an earlier file needs
/// carried over to, which `migrate` does.
const VERSION: u32 = 9;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    #[serde(default)]
    version: u32,
    /// The record it is kept under: its repository's identity, else its folder (ADR 0023).
    project: String,
    /// Its project's folder on this host.
    #[serde(skip)]
    folder: String,
    next: u64,
    pub cards: Vec<Card>,
    #[serde(default)]
    pub settings: BoardSettings,
    /// Places an agent proposed, until the person answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub places_proposal: Option<steps::PlacesProposal>,
    /// The places an earlier core's environments became, until they are kept as the
    /// project's own.
    #[serde(default, rename = "environmentsToPlaces", skip_serializing)]
    carried_over: Option<Vec<crate::server::places::Place>>,
    #[serde(default, skip_serializing_if = "goal::Decided::is_none")]
    decided: goal::Decided,
    /// When its drift audits run and those that ran.
    #[serde(default, skip_serializing_if = "audit::Audits::is_none")]
    audits: audit::Audits,
    /// Its cards are already part of the record its project's hosts share, so it never sends
    /// them to another home to fold in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    shared: bool,
    /// It is a copy of what another host, the project's home, sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    mirrored: bool,
    /// The latest CI run that failed on the project's branch, while the core runs.
    #[serde(skip)]
    ci_failing: failing::Latest,
    #[serde(skip)]
    places: RefCell<Option<Places>>,
}

/// Where each card of a board sits and which cards sit under each, by position in its list.
#[derive(Default)]
struct Places {
    at: HashMap<u64, usize>,
    under: HashMap<u64, Vec<usize>>,
}

impl Board {
    pub fn open_working_card(&self, agent: &str) -> Option<&Card> {
        (self.cards.iter())
            .filter(|card| card.owner_id() == Some(agent) && card.status != CardStatus::Done)
            .max_by_key(|card| card.work_rank())
    }

    pub fn empty(key: &str, folder: &str) -> Self {
        Self {
            version: VERSION,
            project: key.to_owned(),
            folder: folder.to_owned(),
            next: 1,
            ..Self::default()
        }
    }

    /// What is saved of it, copied, which is quicker than writing it out as
    /// JSON while its lock is held.
    fn saved(&self) -> Self {
        let Self {
            version,
            project,
            folder: _,
            next,
            cards,
            settings,
            places_proposal,
            carried_over: _,
            decided,
            audits,
            shared,
            mirrored,
            ci_failing: _,
            places: _,
        } = self;
        Self {
            version: *version,
            project: project.clone(),
            next: *next,
            cards: cards.clone(),
            settings: settings.clone(),
            places_proposal: places_proposal.clone(),
            decided: decided.clone(),
            audits: audits.clone(),
            shared: *shared,
            mirrored: *mirrored,
            ..Self::default()
        }
    }

    /// Card `id`, when it is a goal whose log can keep a decision.
    fn goal_log(&mut self, id: u64) -> Option<&mut Card> {
        let goal = self.card(id).is_ok_and(|card| self.is_goal(card));
        goal.then(|| self.card_mut(id).ok()).flatten()
    }

    /// The goal `card` belongs to: the card at the top above it, or itself
    /// when nothing is above it.
    fn goal_above<'a>(&'a self, card: &'a Card) -> &'a Card {
        self.up_from(card).last().unwrap_or(card)
    }

    /// `card` and the cards above it, the nearest first.
    fn up_from<'a>(&'a self, card: &'a Card) -> impl Iterator<Item = &'a Card> {
        let steps = std::iter::successors(Some(card), |card| {
            card.parent.and_then(|id| self.card(id).ok())
        });
        steps.take(self.cards.len() + 1)
    }

    /// Whether `card` is a goal: at the top of the board, with an agent on it or on a card
    /// under it.
    fn is_goal(&self, card: &Card) -> bool {
        card.parent.is_none()
            && (card.owner.is_some()
                || (self.under(card.id).into_iter()).any(|under| under.owner.is_some()))
    }

    pub fn card(&self, id: u64) -> Result<&Card, String> {
        let placed = (self.places.borrow().as_ref()).map(|places| places.at.get(&id).copied());
        let found = match placed {
            Some(at) => at.and_then(|at| self.cards.get(at)),
            None => self.cards.iter().find(|card| card.id == id),
        };
        found.ok_or_else(|| format!("There is no card #{id} on this project's board"))
    }

    /// The cards right under card `id`.
    fn under(&self, id: u64) -> Vec<&Card> {
        match self.places.borrow().as_ref() {
            Some(places) => (places.under.get(&id).into_iter().flatten())
                .filter_map(|at| self.cards.get(*at))
                .collect(),
            None => (self.cards.iter())
                .filter(|card| card.parent == Some(id))
                .collect(),
        }
    }

    /// Answers looking up a card from an index until [`Self::forget_places`], so reading the
    /// whole board costs one pass. No card may be added, removed or moved until then.
    fn place_cards(&self) {
        let mut places = Places::default();
        for (at, card) in self.cards.iter().enumerate() {
            places.at.insert(card.id, at);
            if let Some(parent) = card.parent {
                places.under.entry(parent).or_default().push(at);
            }
        }
        *self.places.borrow_mut() = Some(places);
    }

    fn forget_places(&self) {
        *self.places.borrow_mut() = None;
    }

    fn blocked(&self) -> Vec<u64> {
        self.cards
            .iter()
            .filter(|card| !self.waits_on(card).is_empty())
            .map(|card| card.id)
            .collect()
    }

    pub fn card_mut(&mut self, id: u64) -> Result<&mut Card, String> {
        self.cards
            .iter_mut()
            .find(|card| card.id == id)
            .ok_or_else(|| format!("There is no card #{id} on this project's board"))
    }

    /// The cards `card` waits on that are not done.
    fn waits_on(&self, card: &Card) -> Vec<u64> {
        card.depends_on
            .iter()
            .copied()
            .filter(|id| {
                self.card(*id)
                    .is_ok_and(|other| other.status != CardStatus::Done)
            })
            .collect()
    }

    fn subcards(&self, id: u64) -> (usize, usize) {
        let under = self.under(id);
        let done = under
            .iter()
            .filter(|card| card.status == CardStatus::Done)
            .count();
        (done, under.len())
    }

    fn reaches(&self, from: u64, to: u64, next: impl Fn(&Card) -> Vec<u64>) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![from];
        while let Some(id) = stack.pop() {
            if id == to {
                return true;
            }
            if !seen.insert(id) {
                continue;
            }
            if let Ok(card) = self.card(id) {
                stack.extend(next(card));
            }
        }
        false
    }

    fn check_depends(&self, id: Option<u64>, depends_on: &[u64]) -> Result<(), String> {
        if depends_on.len() > MAX_DEPENDS {
            return Err(format!("A card waits on at most {MAX_DEPENDS} others"));
        }
        for other in depends_on {
            self.card(*other)?;
            if let Some(id) = id {
                if *other == id || self.reaches(*other, id, |card| card.depends_on.clone()) {
                    return Err(format!(
                        "#{id} cannot wait on #{other}, since #{other} already waits on #{id}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_parent(&self, id: Option<u64>, parent: Option<u64>) -> Result<(), String> {
        let Some(parent) = parent else {
            return Ok(());
        };
        self.card(parent)?;
        if let Some(id) = id {
            if parent == id || self.reaches(parent, id, |card| card.parent.into_iter().collect()) {
                return Err(format!(
                    "#{id} cannot sit under #{parent}, which is under it"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct State {
    /// By the key each is kept under.
    pub boards: BTreeMap<String, Board>,
    /// The key of each project on this host, by its folder.
    keys: HashMap<String, String>,
    /// The folders of boards that changed since clients were last told.
    dirty: BTreeSet<String>,
    /// The keys of boards that changed since the project's other hosts were last sent them.
    unsent: BTreeSet<String>,
    /// What is kept about each card while the ticket sync or an agent's start works on it, by
    /// project and card.
    work: HashMap<(String, u64), CardWork>,
}

/// What the core keeps about a card only while it runs, never saved. It goes
/// with the card, and once nothing is left in it.
#[derive(Default, PartialEq)]
pub struct CardWork {
    /// Its ticket is being updated.
    pub syncing: bool,
    /// When its ticket was last looked at in its tracker.
    pub ticket_looked: Option<u64>,
    /// The status its ticket last failed to move to, and when.
    pub ticket_failed: Option<(CardStatus, u64)>,
    /// An agent is being started or reopened on it for its trigger.
    pub taking_up: bool,
}

impl State {
    /// The key the board of the project at `folder` is kept under.
    pub fn key(&self, folder: &str) -> String {
        self.keys
            .get(folder)
            .cloned()
            .unwrap_or_else(|| folder.to_owned())
    }

    /// The folder on this host of the project kept under `key`.
    fn folder_of(&self, key: &str) -> String {
        (self.keys.iter())
            .find(|(_, kept)| *kept == key)
            .map_or_else(|| key.to_owned(), |(folder, _)| folder.clone())
    }

    /// The folder `folder` and every other folder on this host whose project is kept under the
    /// same key, such as a second clone of one repository.
    fn sharing_board(&self, folder: &str) -> Vec<String> {
        let key = self.key(folder);
        let clones = (self.keys.iter()).filter(|(_, kept)| **kept == key);
        std::iter::once(folder.to_owned())
            .chain(clones.map(|(clone, _)| clone.clone()))
            .collect()
    }

    /// The board of the project at `folder`.
    pub fn of(&self, folder: &str) -> Option<&Board> {
        self.boards.get(&self.key(folder))
    }

    pub fn of_mut(&mut self, folder: &str) -> Option<&mut Board> {
        let key = self.key(folder);
        self.boards.get_mut(&key)
    }

    /// The board of the project at `folder`, made empty when it has none.
    pub fn made(&mut self, folder: &str) -> &mut Board {
        let key = self.key(folder);
        (self.boards.entry(key.clone())).or_insert_with(|| Board::empty(&key, folder))
    }

    fn work(&mut self, project: &str, id: u64) -> &mut CardWork {
        self.work.entry((project.to_owned(), id)).or_default()
    }

    fn work_of(&self, project: &str, id: u64) -> Option<&CardWork> {
        self.work.get(&(project.to_owned(), id))
    }

    fn change_work(&mut self, project: &str, id: u64, change: impl FnOnce(&mut CardWork)) {
        let key = (project.to_owned(), id);
        let work = self.work.entry(key.clone()).or_default();
        change(work);
        if *work == CardWork::default() {
            self.work.remove(&key);
        }
    }
}

pub struct Boards {
    pub state: Mutex<State>,
    folder: Option<PathBuf>,
    writer: Arc<Writer>,
    /// Keeps tickets, audits and the window up to date.
    pub wake: Wake,
    chat_goals: chat_goal::ChatGoals,
    /// How many steps of its project's spend amount each chat had passed when last looked at.
    spend_told: Mutex<HashMap<String, u32>>,
}

/// The file in the data folder an earlier core kept the environments set for folders in,
/// which each board under such a folder takes as its own places.
const FOLDERS_FILE: &str = "folder-environments.json";

/// The version whose boards keep places where earlier ones kept environments.
const PLACES_VERSION: u32 = 6;

/// Keeps the board file of `project` as an earlier core wrote it beside the one this core
/// writes, under a name no core reads as a board.
fn keep_before_places(folder: &Path, project: &str) {
    let file = folder.join(kept::file_for(project));
    let kept = file.with_extension("json.v5");
    if kept.exists() {
        return;
    }
    if let Err(error) = std::fs::copy(&file, &kept) {
        eprintln!("sikemux core: {} was not kept: {error}", file.display());
    }
}

/// Writes boards out in the order they are handed over, at once, on a thread
/// of its own, so nobody holding a board's lock waits on JSON or the disk.
#[derive(Default)]
struct Writer {
    writes: Mutex<Writes>,
    done: Condvar,
}

#[derive(Default)]
struct Writes {
    /// One write per file: a newer one takes the place of the one waiting.
    waiting: VecDeque<(PathBuf, Board)>,
    writing: bool,
}

impl Writer {
    fn hand(self: &Arc<Self>, path: PathBuf, board: Board) {
        let mut writes = self.writes.locked();
        match writes
            .waiting
            .iter_mut()
            .find(|(waiting, _)| *waiting == path)
        {
            Some((_, waiting)) => *waiting = board,
            None => writes.waiting.push_back((path, board)),
        }
        if writes.writing {
            return;
        }
        writes.writing = true;
        drop(writes);
        let writer = self.clone();
        let spawned = std::thread::Builder::new()
            .name("sikemux-boards".into())
            .spawn(move || writer.write_all());
        if spawned.is_err() {
            self.write_all();
        }
    }

    fn write_all(&self) {
        loop {
            let next = {
                let mut writes = self.writes.locked();
                let next = writes.waiting.pop_front();
                if next.is_none() {
                    writes.writing = false;
                    self.done.notify_all();
                }
                next
            };
            let Some((path, board)) = next else {
                return;
            };
            let bytes = match serde_json::to_vec(&board) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("sikemux core: the board was not saved: {error}");
                    continue;
                }
            };
            if let Err(error) = sikemux_process::files::write_atomic(&path, &bytes, true) {
                eprintln!("sikemux core: {} was not saved: {error}", path.display());
            }
        }
    }

    fn flush(&self) {
        let writes = self.writes.locked();
        drop(
            self.done
                .wait_while(writes, |writes| writes.writing)
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
}

#[derive(Clone)]
pub enum By {
    Person,
    Agent(String),
    /// Sikemux itself, such as the ticket sync or a trigger, acting for the person.
    Auto(&'static str),
}

/// What an agent is told because of a change someone else made.
pub struct Notice {
    to: String,
    text: String,
}

impl Boards {
    pub fn new(data_dir: Option<&Path>) -> Self {
        let folder = data_dir.map(|dir| dir.join(FOLDER));
        let mut state = State::default();
        let mut upgraded = Vec::new();
        let folders = data_dir.map(|dir| dir.join(FOLDERS_FILE));
        let inherited = folders.as_deref().map(migrate::folder_environments);
        let carry_over = |board| migrate::carry_over(board, inherited.as_ref());
        for mut board in kept::load_all_upgraded::<Board>(folder.as_deref(), carry_over) {
            board.folder = board.project.clone();
            if let Some(places) = board.carried_over.take() {
                crate::server::places::store::carried_over(data_dir, &board.project, places);
            }
            if board.version < VERSION {
                if let (Some(folder), true) = (&folder, board.version < PLACES_VERSION) {
                    keep_before_places(folder, &board.project);
                }
                board.version = VERSION;
                upgraded.push(board.project.clone());
            }
            state.dirty.insert(board.folder.clone());
            state.boards.insert(board.project.clone(), board);
        }
        for kept in crate::server::places::store::read_all(data_dir) {
            if let Some(board) = state.of_mut(&kept.project) {
                board.settings.places = kept.places;
                board.settings.places_confirmed = kept.confirmed;
            }
        }
        if let Some(Err(error)) = folder.as_deref().map(std::fs::create_dir_all) {
            eprintln!("sikemux core: boards will not be saved: {error}");
        }
        let boards = Self {
            state: Mutex::new(state),
            folder,
            writer: Arc::default(),
            wake: Wake::default(),
            chat_goals: chat_goal::ChatGoals::new(data_dir),
            spend_told: Mutex::default(),
        };
        {
            let mut state = boards.state.locked();
            for project in upgraded {
                boards.save(&mut state, &project);
            }
        }
        if let Some(file) = folders.filter(|file| file.exists()) {
            boards.writer.flush();
            let kept = (boards.folder.as_ref())
                .map(|folder| folder.join(format!("{FOLDERS_FILE}.v5")))
                .map(|kept| std::fs::copy(&file, kept));
            match kept {
                Some(Err(error)) => {
                    eprintln!("sikemux core: {} was not kept: {error}", file.display())
                }
                _ => {
                    if let Err(error) = std::fs::remove_file(&file) {
                        eprintln!("sikemux core: {} was not removed: {error}", file.display());
                    }
                }
            }
        }
        boards.wake.now();
        boards
    }

    pub fn save(&self, state: &mut State, project: &str) {
        let key = state.key(project);
        self.save_key(state, &key);
    }

    fn save_key(&self, state: &mut State, key: &str) {
        let Some(board) = state.boards.get(key) else {
            return;
        };
        state.dirty.insert(board.folder.clone());
        state.unsent.insert(key.to_owned());
        self.wake.now();
        if let Some(folder) = &self.folder {
            self.writer
                .hand(folder.join(kept::file_for(key)), board.saved());
        }
    }
}

/// Moves the cards' tickets and audits on, and sends the window the boards that changed. Runs
/// when a board or the agents change, and again when a ticket or audit is next due.
pub fn settle(core: &Arc<Core>) {
    let mut next = tidy::settle(core);
    next = sooner(next, tickets::due(core));
    next = sooner(next, audit::due(core));
    if let Some(next) = next {
        core.boards().wake.at(next);
    }
    chat_goal::settle(core);
    spend::settle(core);
    if !core.boards().state.locked().dirty.is_empty() {
        (core.board_above().questions_boards_changed)(core);
    }
    tell_clients(core);
    let unsent = std::mem::take(&mut core.boards().state.locked().unsent);
    if !unsent.is_empty() {
        (core.board_above().records_changed)(core, unsent.into_iter().collect());
    }
}

/// Tells every client which boards changed, so each reads them again.
fn tell_clients(core: &Core) {
    let dirty: BTreeSet<String> = {
        let mut state = core.boards().state.locked();
        let dirty = std::mem::take(&mut state.dirty);
        (dirty.iter())
            .flat_map(|folder| state.sharing_board(folder))
            .collect()
    };
    if !dirty.is_empty() {
        (core.board_above().hosts_rows_moved)(core);
    }
    for project in dirty {
        core.changed(What::ReadIn(Topic::Boards, Some(&project), None));
    }
}

impl super::part::Part for Boards {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![super::part::on_wake(
            core,
            |core| &core.boards().wake,
            settle,
        )]
    }

    fn flush(&self) {
        self.writer.flush();
        self.chat_goals.flush();
    }
}
