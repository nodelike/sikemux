//! Each project's board: cards of work the person and agents add, which an
//! agent claims, works and marks done. A card may sit under another and wait
//! on others, and is free to claim once every card it waits on is done. Kept
//! in `boards/` in the data folder, one file per project, so a board outlives
//! a window reload, a restart and an update.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cli::protocol::HarnessRequest;
use crate::launch::providers::{self, ROUTED_ENV};
use crate::protocol::WindowCall;

use super::connection::{blocking, ClientId};
use super::inbox::Recording;
use super::messages;
use super::names::project_of;
use super::remote::unix_ms;
use super::wake::{sooner, Wake};
use super::Core;

mod auto;
mod blind;
pub(crate) mod codehost;
mod finish;
pub(crate) mod git;
mod review;
mod tickets;

pub(crate) use auto::Settings;
use auto::{Run, Stage};

const FOLDER: &str = "boards";
const MAX_CARDS: usize = 1000;
const MAX_TITLE_CHARS: usize = 200;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_DEPENDS: usize = 20;
const MAX_KEY_CHARS: usize = 40;
const MAX_URL_CHARS: usize = 2048;
/// How much of a card's details and result `board_list` shows before one
/// card is asked for in full.
const LISTED_TEXT_CHARS: usize = 240;
/// How long an agent has been idle before the board offers it cards.
const IDLE_BEFORE_OFFER_MS: u64 = 5_000;
const MAX_OFFERED: usize = 6;
const MAX_RECORDINGS: usize = 8;
const WINDOW_ANSWER_WAIT: Duration = Duration::from_secs(10);
const ONLY_PERSON_ACCEPTS: &str =
    "Only the person writes a card's acceptance criteria; leave out acceptance";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Open,
    Progress,
    Done,
}

impl Status {
    fn words(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Progress => "in progress",
            Status::Done => "done",
        }
    }

    fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_lowercase().as_str() {
            "open" => Ok(Status::Open),
            "progress" | "in progress" | "in_progress" => Ok(Status::Progress),
            "done" => Ok(Status::Done),
            _ => Err("status must be open, progress or done".into()),
        }
    }
}

/// The Jira or Linear ticket the person put a card on the board for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ticket {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// What the ticket is in its tracker, such as Epic, Story or Bug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub provider: String,
    /// The status last put on the ticket in its tracker, so each change goes
    /// there once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced: Option<Status>,
    /// Why the last change did not reach the ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Card {
    id: u64,
    title: String,
    #[serde(default)]
    details: String,
    /// What the work must do to count as done, written only by the person.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    acceptance: String,
    status: Status,
    /// The agent working it, or that did.
    #[serde(default)]
    owner: Option<String>,
    /// What the owner was called and which CLI it ran when it claimed the
    /// card, for after it has gone.
    #[serde(default)]
    owner_name: Option<String>,
    #[serde(default)]
    owner_cli: Option<String>,
    #[serde(default)]
    parent: Option<u64>,
    #[serde(default)]
    depends_on: Vec<u64>,
    /// The agent that added it, or none when the person did.
    #[serde(default)]
    created_by: Option<String>,
    #[serde(default)]
    created_by_name: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    ticket: Option<Ticket>,
    /// The run Auto gave the card, in a worktree of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auto: Option<Run>,
    /// Browser recordings its owner made while on it, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recordings: Vec<Recording>,
    at: u64,
    updated_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Board {
    project: String,
    next: u64,
    cards: Vec<Card>,
    #[serde(default)]
    settings: Settings,
    /// Why Auto cannot start anything here now.
    #[serde(skip)]
    auto_note: Option<String>,
}

impl Board {
    fn empty(project: &str) -> Self {
        Self {
            project: project.to_owned(),
            next: 1,
            ..Self::default()
        }
    }

    fn card(&self, id: u64) -> Result<&Card, String> {
        self.cards
            .iter()
            .find(|card| card.id == id)
            .ok_or_else(|| format!("There is no card #{id} on this project's board"))
    }

    fn blocked(&self) -> Vec<u64> {
        self.cards
            .iter()
            .filter(|card| !self.waits_on(card).is_empty())
            .map(|card| card.id)
            .collect()
    }

    fn card_mut(&mut self, id: u64) -> Result<&mut Card, String> {
        self.cards
            .iter_mut()
            .find(|card| card.id == id)
            .ok_or_else(|| format!("There is no card #{id} on this project's board"))
    }

    /// The cards `card` waits on that are not done yet.
    fn waits_on(&self, card: &Card) -> Vec<u64> {
        card.depends_on
            .iter()
            .copied()
            .filter(|id| {
                self.cards
                    .iter()
                    .any(|other| other.id == *id && other.status != Status::Done)
            })
            .collect()
    }

    fn subcards(&self, id: u64) -> (usize, usize) {
        let under: Vec<&Card> = self
            .cards
            .iter()
            .filter(|card| card.parent == Some(id))
            .collect();
        let done = under
            .iter()
            .filter(|card| card.status == Status::Done)
            .count();
        (done, under.len())
    }

    /// Whether `from` reaches `to` by following `next`.
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
struct State {
    boards: BTreeMap<String, Board>,
    /// Boards that changed since the window was last sent them.
    dirty: BTreeSet<String>,
    told_window: Option<ClientId>,
    /// Each card the board told an idle agent of, by project, card and agent.
    offered: HashSet<(String, u64, String)>,
    /// Agents whose turn the board started to offer them cards, and that have
    /// done nothing on the board since.
    woken: HashSet<String>,
    /// Cards Auto is busy with, by project and card.
    jobs: HashSet<(String, u64)>,
    /// Cards whose work is to ship again once the step under way ends.
    reship: HashSet<(String, u64)>,
    /// Runs Auto no longer holds, whose agents and worktrees are to go.
    dropped: Vec<finish::Dropped>,
    /// Reviewers whose review ended, whose agents and checkouts are to go.
    dismissed: Vec<blind::Reviewer>,
    /// Cards whose ticket is being updated.
    syncing: HashSet<(String, u64)>,
    /// When each card's pull request or ticket was last asked about.
    polled: BTreeMap<(String, u64, &'static str), u64>,
}

pub(crate) struct Boards {
    state: Mutex<State>,
    folder: Option<PathBuf>,
    /// Offers cards, runs Auto and keeps tickets and the window up to date.
    pub(crate) wake: Wake,
}

/// Who changes a board.
#[derive(Clone)]
pub(crate) enum By {
    Person,
    Agent(String),
    /// Auto, or the ticket sync, acting for the person.
    Auto(&'static str),
}

/// What an agent is told because of a change someone else made.
struct Notice {
    to: String,
    text: String,
}

fn file_name(project: &str) -> String {
    let digest = hex::encode(Sha256::digest(project.as_bytes()));
    format!("{}.json", digest.get(..16).unwrap_or(&digest))
}

impl Boards {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let folder = data_dir.map(|dir| dir.join(FOLDER));
        let mut state = State::default();
        if let Some(entries) = folder
            .as_deref()
            .and_then(|dir| std::fs::read_dir(dir).ok())
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                match std::fs::read(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| {
                        serde_json::from_slice::<Board>(&bytes).map_err(|error| error.to_string())
                    }) {
                    Ok(board) => {
                        state.dirty.insert(board.project.clone());
                        state.boards.insert(board.project.clone(), board);
                    }
                    Err(error) => eprintln!(
                        "sikemux core: the board in {} could not be read: {error}",
                        path.display()
                    ),
                }
            }
        }
        Self {
            state: Mutex::new(state),
            folder,
            wake: Wake::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, state: &mut State, project: &str) {
        state.dirty.insert(project.to_owned());
        self.wake.now();
        let (Some(folder), Some(board)) = (&self.folder, state.boards.get(project)) else {
            return;
        };
        let written = std::fs::create_dir_all(folder)
            .and_then(|()| serde_json::to_vec(board).map_err(std::io::Error::from))
            .and_then(|bytes| {
                sikemux_process::files::write_atomic(&folder.join(file_name(project)), &bytes, true)
            });
        if let Err(error) = written {
            eprintln!("sikemux core: the board was not saved: {error}");
        }
    }

    /// A step Auto took on the card is over.
    fn end_job(&self, key: &(String, u64)) {
        self.lock().jobs.remove(key);
        self.wake.now();
    }
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn clipped(text: &str, max: usize) -> String {
    let text = text.trim();
    if chars(text) <= max {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

fn title_of(value: &str) -> Result<String, String> {
    let title = value.trim();
    if title.is_empty() || chars(title) > MAX_TITLE_CHARS || title.contains('\n') {
        return Err(format!(
            "title must be one line of at most {MAX_TITLE_CHARS} characters"
        ));
    }
    Ok(title.to_owned())
}

fn long_text(key: &str, value: &str) -> Result<String, String> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "{key} is {} bytes, more than the {MAX_TEXT_BYTES} it may hold; put long material in a file and name its path",
            value.len()
        ));
    }
    Ok(value.trim().to_owned())
}

fn card_id(value: &Value, key: &str) -> Result<u64, String> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().trim_start_matches('#').parse().ok(),
        _ => None,
    }
    .filter(|id| *id > 0)
    .ok_or_else(|| format!("{key} must be a card number, such as 3"))
}

fn card_ids(value: &Value, key: &str) -> Result<Vec<u64>, String> {
    let Value::Array(list) = value else {
        return Err(format!("{key} must be a list of card numbers"));
    };
    let mut ids = Vec::new();
    for item in list {
        let id = card_id(item, key)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn optional_id(value: Option<&Value>, key: &str) -> Result<Option<Option<u64>>, String> {
    match value {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(value) if value.as_u64() == Some(0) => Ok(Some(None)),
        Some(value) => card_id(value, key).map(|id| Some(Some(id))),
    }
}

fn optional_text(params: &Value, key: &str) -> Result<Option<String>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => long_text(key, text).map(Some),
        Some(_) => Err(format!("{key} must be text")),
    }
}

/// A new card, as the person or an agent asked for it.
struct Adding {
    title: String,
    details: String,
    acceptance: String,
    parent: Option<u64>,
    depends_on: Vec<u64>,
    ticket: Option<Ticket>,
}

impl Adding {
    fn from_params(params: &Value) -> Result<Self, String> {
        let title = match params.get("title") {
            Some(Value::String(title)) => title_of(title)?,
            _ => return Err("title must be the card's one-line title".into()),
        };
        Ok(Self {
            title,
            details: optional_text(params, "details")?.unwrap_or_default(),
            acceptance: optional_text(params, "acceptance")?.unwrap_or_default(),
            parent: optional_id(params.get("parent"), "parent")?.flatten(),
            depends_on: match params.get("dependsOn") {
                None | Some(Value::Null) => Vec::new(),
                Some(value) => card_ids(value, "dependsOn")?,
            },
            ticket: None,
        })
    }
}

/// What a change to a card asks for.
#[derive(Default)]
struct Changes {
    title: Option<String>,
    details: Option<String>,
    acceptance: Option<String>,
    status: Option<Status>,
    result: Option<String>,
    depends_on: Option<Vec<u64>>,
    parent: Option<Option<u64>>,
}

impl Changes {
    fn from_params(params: &Value) -> Result<Self, String> {
        Ok(Self {
            title: match params.get("title") {
                None | Some(Value::Null) => None,
                Some(Value::String(title)) => Some(title_of(title)?),
                Some(_) => return Err("title must be text".into()),
            },
            details: optional_text(params, "details")?,
            acceptance: optional_text(params, "acceptance")?,
            status: match params.get("status") {
                None | Some(Value::Null) => None,
                Some(Value::String(status)) => Some(Status::parse(status)?),
                Some(_) => return Err("status must be open, progress or done".into()),
            },
            result: optional_text(params, "result")?,
            depends_on: match params.get("dependsOn") {
                None | Some(Value::Null) => None,
                Some(value) => Some(card_ids(value, "dependsOn")?),
            },
            parent: optional_id(params.get("parent"), "parent")?,
        })
    }

    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.details.is_none()
            && self.acceptance.is_none()
            && self.status.is_none()
            && self.result.is_none()
            && self.depends_on.is_none()
            && self.parent.is_none()
    }
}

fn ticket_of(value: &Value) -> Result<Option<Ticket>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let field = |key: &str, max: usize| -> Result<Option<String>, String> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if !text.trim().is_empty() && chars(text) <= max => {
                Ok(Some(text.trim().to_owned()))
            }
            Some(_) => Err(format!(
                "the ticket's {key} must be text of at most {max} characters"
            )),
        }
    };
    let key = field("key", MAX_KEY_CHARS)?.ok_or("a ticket needs its key, such as PROJ-12")?;
    if key.contains(char::is_whitespace) {
        return Err("a ticket's key is one word, such as PROJ-12".into());
    }
    let url = field("url", MAX_URL_CHARS)?;
    if url
        .as_deref()
        .is_some_and(|url| !url.starts_with("https://") && !url.starts_with("http://"))
    {
        return Err("a ticket's url must be a web address".into());
    }
    let provider = field("provider", 20)?.unwrap_or_else(|| "jira".to_owned());
    if !matches!(provider.as_str(), "jira" | "linear") {
        return Err("a ticket comes from jira or linear".into());
    }
    Ok(Some(Ticket {
        key,
        url,
        source: field("source", 40)?,
        provider,
        synced: None,
        sync_note: None,
    }))
}

/// The project a board call names, as agents' names place it.
fn board_project(core: &Core, path: &str) -> String {
    project_of(Path::new(path), &core.workspaces.project_paths())
}

fn caller_project(core: &Core, request: &HarnessRequest) -> String {
    request
        .agent_id
        .as_deref()
        .and_then(|agent_id| core.names.get(agent_id))
        .map(|named| named.project)
        .unwrap_or_else(|| board_project(core, &request.project))
}

fn cli_of(core: &Core, agent_id: &str) -> Option<String> {
    core.chats
        .get(agent_id)
        .map(|chat| chat.provider().to_owned())
        .or_else(|| {
            core.agent_session(agent_id)
                .and_then(|session| session.info().agent_type)
        })
}

/// The provider paying for the agent's turns, by name, when not its own account.
fn paid_by(core: &Core, agent_id: &str) -> Option<&'static str> {
    let chat = core.chats.get(agent_id)?;
    let provider = providers::find(chat.launch.env.get(ROUTED_ENV)?)?;
    Some(&provider.label)
}

fn name_of(core: &Core, agent_id: &str) -> String {
    core.names
        .name(agent_id)
        .unwrap_or_else(|| agent_id.to_owned())
}

/// Whether `agent_id` may change a card `owner` holds: its own, or one held by
/// an agent it started, or one nobody holds.
fn may_change(core: &Core, agent_id: &str, owner: Option<&str>) -> bool {
    match owner {
        None => true,
        Some(owner) => owner == agent_id || core.names.leads(agent_id, owner),
    }
}

impl Card {
    fn owner_name(&self, core: &Core) -> Option<String> {
        let owner = self.owner.as_deref()?;
        core.names
            .name(owner)
            .or_else(|| self.owner_name.clone())
            .or_else(|| Some(owner.to_owned()))
    }

    fn creator_name(&self, core: &Core) -> String {
        match &self.created_by {
            None => "the person".to_owned(),
            Some(agent) => core
                .names
                .name(agent)
                .or_else(|| self.created_by_name.clone())
                .unwrap_or_else(|| agent.clone()),
        }
    }

    fn label(&self) -> String {
        format!("#{} {}", self.id, self.title)
    }
}

/// The team a card belongs to: the agent at the top of its owner's chain of
/// leads, else its maker's, else its parent card's. None for a card only the
/// person has touched.
fn team_of(core: &Core, board: &Board, card: &Card) -> Option<String> {
    let mut current = Some(card);
    let mut steps = 0;
    while let Some(card) = current {
        if let Some(agent) = card.owner.as_deref().or(card.created_by.as_deref()) {
            return Some(core.names.team_of(agent));
        }
        steps += 1;
        if steps > board.cards.len() {
            break;
        }
        current = card.parent.and_then(|parent| board.card(parent).ok());
    }
    None
}

/// The card as the app draws it.
fn app_view(core: &Core, board: &Board, card: &Card) -> Value {
    let team = team_of(core, board, card);
    json!({
        "id": card.id,
        "title": card.title,
        "details": card.details,
        "acceptance": card.acceptance,
        "status": card.status,
        "owner": card.owner,
        "ownerName": card.owner_name(core),
        "ownerCli": card.owner.as_deref().and_then(|owner| cli_of(core, owner)).or_else(|| card.owner_cli.clone()),
        "paidBy": card.owner.as_deref().and_then(|owner| paid_by(core, owner)),
        "team": team,
        "teamName": team.as_deref().map(|team| name_of(core, team)),
        "parent": card.parent,
        "dependsOn": card.depends_on,
        "waitsOn": board.waits_on(card),
        "createdBy": card.created_by,
        "createdByName": card.creator_name(core),
        "result": card.result,
        "ticket": card.ticket,
        "auto": card.auto.as_ref().map(Run::view),
        "recordings": card.recordings,
        "at": card.at,
        "updatedAt": card.updated_at,
    })
}

fn board_view(core: &Core, board: &Board) -> Value {
    json!({
        "project": board.project,
        "cards": board.cards.iter().map(|card| app_view(core, board, card)).collect::<Vec<_>>(),
        "settings": board.settings,
        "running": auto::running(core, board),
        "autoNote": board.auto_note,
    })
}

/// The card as `board_list` tells an agent of it.
fn agent_view(core: &Core, board: &Board, card: &Card, full: bool) -> Value {
    let text = |text: &str| {
        if full {
            text.to_owned()
        } else {
            clipped(text, LISTED_TEXT_CHARS)
        }
    };
    let mut view = json!({
        "card": card.id,
        "title": card.title,
        "status": card.status.words(),
        "owner": card.owner_name(core),
        "createdBy": card.creator_name(core),
    });
    let fields = view.as_object_mut().expect("a card is an object");
    if !card.details.is_empty() {
        fields.insert("details".into(), text(&card.details).into());
    }
    if !card.acceptance.is_empty() {
        fields.insert("acceptance".into(), text(&card.acceptance).into());
    }
    if let Some(parent) = card.parent {
        fields.insert("parent".into(), parent.into());
    }
    let (done, under) = board.subcards(card.id);
    if under > 0 {
        fields.insert("subcards".into(), format!("{done}/{under} done").into());
    }
    if !card.depends_on.is_empty() {
        fields.insert("dependsOn".into(), json!(card.depends_on));
        let waits = board.waits_on(card);
        if !waits.is_empty() {
            fields.insert("waitsOn".into(), json!(waits));
        }
    }
    if let Some(ticket) = &card.ticket {
        let mut label = ticket.key.clone();
        if let Some(source) = &ticket.source {
            label.push_str(&format!(" ({source})"));
        }
        fields.insert("ticket".into(), label.into());
    }
    if let Some(result) = &card.result {
        fields.insert("result".into(), text(result).into());
    }
    if card
        .owner
        .as_deref()
        .is_some_and(|owner| core.harness.questions.blocks_on_person(owner))
    {
        fields.insert("waitingOnThePerson".into(), true.into());
    }
    if full {
        let under: Vec<Value> = board
            .cards
            .iter()
            .filter(|other| other.parent == Some(card.id))
            .map(|other| {
                json!({ "card": other.id, "title": other.title, "status": other.status.words(), "owner": other.owner_name(core) })
            })
            .collect();
        if !under.is_empty() {
            fields.insert("cards".into(), under.into());
        }
    }
    view
}

fn add_card(
    core: &Core,
    project: &str,
    by: &By,
    adding: Adding,
    claim: bool,
) -> Result<Value, String> {
    let boards = &core.boards;
    let mut state = boards.lock();
    let board = state
        .boards
        .entry(project.to_owned())
        .or_insert_with(|| Board::empty(project));
    if board.cards.len() >= MAX_CARDS {
        return Err(format!(
            "This project's board already holds {MAX_CARDS} cards, the most it may; ask the person to clear the done ones"
        ));
    }
    if let Some(ticket) = &adding.ticket {
        if let Some(card) = board.cards.iter().find(|card| {
            card.ticket
                .as_ref()
                .is_some_and(|other| other.key.eq_ignore_ascii_case(&ticket.key))
        }) {
            return Err(format!(
                "{} is on the board already, as {}",
                ticket.key,
                card.label()
            ));
        }
    }
    if matches!(by, By::Agent(_)) && !adding.acceptance.is_empty() {
        return Err(ONLY_PERSON_ACCEPTS.into());
    }
    board.check_parent(None, adding.parent)?;
    board.check_depends(None, &adding.depends_on)?;
    let now = unix_ms();
    let id = board.next.max(1);
    board.next = id + 1;
    let maker = match by {
        By::Person | By::Auto(_) => None,
        By::Agent(agent) => Some(agent.clone()),
    };
    let mut card = Card {
        id,
        title: adding.title,
        details: adding.details,
        acceptance: adding.acceptance,
        status: Status::Open,
        owner: None,
        owner_name: None,
        owner_cli: None,
        parent: adding.parent,
        depends_on: adding.depends_on,
        created_by_name: maker.as_deref().map(|agent| name_of(core, agent)),
        created_by: maker,
        result: None,
        ticket: adding.ticket,
        auto: None,
        recordings: Vec::new(),
        at: now,
        updated_at: now,
    };
    let mut note = None;
    if let (true, By::Agent(agent)) = (claim, by) {
        if board.waits_on(&card).is_empty() {
            take(core, &mut card, agent);
        } else {
            note =
                Some("It waits on cards not done yet, so it stays open; claim it once they are.");
        }
    }
    board.cards.push(card);
    let card = board.card(id)?;
    let mut view = agent_view(core, board, card, true);
    if let By::Agent(agent) = by {
        state.woken.remove(agent);
    }
    boards.save(&mut state, project);
    if let (Some(note), Some(fields)) = (note, view.as_object_mut()) {
        fields.insert("note".into(), note.into());
    }
    Ok(view)
}

fn take(core: &Core, card: &mut Card, agent: &str) {
    card.owner = Some(agent.to_owned());
    card.owner_name = Some(name_of(core, agent));
    card.owner_cli = cli_of(core, agent);
    card.status = Status::Progress;
    card.updated_at = unix_ms();
}

fn claim_card(core: &Core, project: &str, agent: &str, id: u64) -> Result<Value, String> {
    let boards = &core.boards;
    let mut state = boards.lock();
    let board = state
        .boards
        .get_mut(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let card = board.card(id)?;
    if card.owner.as_deref() == Some(agent) && card.status == Status::Progress {
        return Ok(agent_view(core, board, card, true));
    }
    if card.status == Status::Done {
        return Err(format!("#{id} is done already"));
    }
    if let Some(owner) = card.owner.as_deref().filter(|owner| *owner != agent) {
        return Err(format!(
            "{} has #{id} already; pick another card, or message {} about it",
            name_of(core, owner),
            name_of(core, owner)
        ));
    }
    let waits = board.waits_on(card);
    if !waits.is_empty() {
        let list: Vec<String> = waits.iter().map(|id| format!("#{id}")).collect();
        return Err(format!(
            "#{id} waits on {} not done yet; claim it once they are",
            list.join(", ")
        ));
    }
    let card = board.card_mut(id)?;
    take(core, card, agent);
    let card = board.card(id)?;
    let view = agent_view(core, board, card, true);
    state.woken.remove(agent);
    boards.save(&mut state, project);
    Ok(view)
}

/// Changes a card and says who should hear of it.
fn change_card(
    core: &Core,
    project: &str,
    by: &By,
    id: u64,
    changes: Changes,
) -> Result<(Value, Vec<Notice>), String> {
    if changes.is_empty() {
        return Err("Say what changes: title, details, status, result, dependsOn or parent".into());
    }
    if matches!(by, By::Agent(_)) && changes.acceptance.is_some() {
        return Err(ONLY_PERSON_ACCEPTS.into());
    }
    let boards = &core.boards;
    let mut state = boards.lock();
    let board = state
        .boards
        .get_mut(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let before = board.card(id)?.clone();
    if let By::Agent(agent) = by {
        if !may_change(core, agent, before.owner.as_deref()) {
            let owner = name_of(core, before.owner.as_deref().unwrap_or_default());
            return Err(format!(
                "#{id} is {owner}'s; message {owner} with agent_send to change it"
            ));
        }
        if changes.status == Some(Status::Progress) && before.status != Status::Progress {
            return Err(format!("Claim #{id} with board_claim to start on it"));
        }
    }
    if let Some(depends_on) = &changes.depends_on {
        board.check_depends(Some(id), depends_on)?;
    }
    if let Some(parent) = changes.parent {
        board.check_parent(Some(id), parent)?;
    }
    let blocked_before = board.blocked();
    let dropping = before
        .auto
        .clone()
        .filter(|run| !run.stage.is_over())
        .and_then(|run| match changes.status {
            Some(Status::Open) => Some((run, true)),
            Some(Status::Done) => Some((run, false)),
            _ => None,
        });
    let card = board.card_mut(id)?;
    if let Some(title) = changes.title {
        card.title = title;
    }
    if let Some(details) = changes.details {
        card.details = details;
    }
    if let Some(acceptance) = changes.acceptance {
        card.acceptance = acceptance;
    }
    if let Some(result) = changes.result {
        card.result = (!result.is_empty()).then_some(result);
    }
    if let Some(depends_on) = changes.depends_on {
        card.depends_on = depends_on;
    }
    if let Some(parent) = changes.parent {
        card.parent = parent;
    }
    match changes.status {
        Some(Status::Open) => {
            card.status = Status::Open;
            card.owner = None;
            card.owner_name = None;
            card.owner_cli = None;
            card.auto = None;
        }
        Some(Status::Done) => {
            card.status = Status::Done;
            if let Some(run) = card.auto.as_mut().filter(|run| !run.stage.is_over()) {
                run.stage = Stage::Closed;
            }
            if let (None, By::Agent(agent)) = (&card.owner, by) {
                card.owner = Some(agent.clone());
                card.owner_name = Some(name_of(core, agent));
                card.owner_cli = cli_of(core, agent);
            }
        }
        Some(Status::Progress) => card.status = Status::Progress,
        None => {}
    }
    card.updated_at = unix_ms();
    let after = card.clone();
    let mut notices = Vec::new();
    let changer = match by {
        By::Person => Some("The person".to_owned()),
        By::Auto(who) => Some((*who).to_owned()),
        By::Agent(agent) if before.owner.as_deref() != Some(agent.as_str()) => {
            Some(name_of(core, agent))
        }
        By::Agent(_) => None,
    };
    if let (Some(changer), Some(owner), Status::Progress, None) =
        (changer, before.owner.as_deref(), before.status, &dropping)
    {
        let text = if after.owner.is_none() {
            format!(
                "{changer} moved your card {} back to Open, so it is no longer yours. Stop working on it unless they tell you otherwise.",
                before.label()
            )
        } else if after.status == Status::Done {
            format!("{changer} marked your card {} done.", after.label())
        } else {
            format!(
                "{changer} changed your card {}; board_list with card {id} shows it as it is now.",
                after.label()
            )
        };
        notices.push(Notice {
            to: owner.to_owned(),
            text,
        });
    }
    notices.extend(unblocked(board, &blocked_before, by));
    if after.status == Status::Open && before.status != Status::Open {
        state
            .offered
            .retain(|(other, card, _)| !(other == project && *card == id));
    }
    if let Some((run, discard)) = dropping {
        state.dropped.push(finish::Dropped {
            project: project.to_owned(),
            id,
            run,
            discard,
        });
    }
    let board = state
        .boards
        .get(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let view = agent_view(core, board, board.card(id)?, true);
    if let By::Agent(agent) = by {
        state.woken.remove(agent);
    }
    boards.save(&mut state, project);
    Ok((view, notices))
}

fn remove_card(core: &Core, project: &str, id: u64) -> Result<Vec<Notice>, String> {
    let boards = &core.boards;
    let mut state = boards.lock();
    let board = state
        .boards
        .get_mut(project)
        .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
    let removed = board.card(id)?.clone();
    let blocked_before = board.blocked();
    board.cards.retain(|card| card.id != id);
    for card in &mut board.cards {
        card.depends_on.retain(|other| *other != id);
        if card.parent == Some(id) {
            card.parent = removed.parent;
        }
    }
    let mut notices = unblocked(board, &blocked_before, &By::Person);
    let run = removed.auto.clone().filter(|run| !run.stage.is_over());
    let dropping = run.is_some();
    if let Some(run) = run {
        state.dropped.push(finish::Dropped {
            project: project.to_owned(),
            id,
            run,
            discard: false,
        });
    }
    boards.save(&mut state, project);
    if let (Some(owner), Status::Progress, false) =
        (removed.owner.clone(), removed.status, dropping)
    {
        notices.push(Notice {
            to: owner,
            text: format!(
                "The person took your card {} off the board. Stop working on it unless they tell you otherwise.",
                removed.label()
            ),
        });
    }
    Ok(notices)
}

/// Tells the owner of each card in progress that waited on others before a
/// change and no longer does.
fn unblocked(board: &Board, blocked_before: &[u64], by: &By) -> Vec<Notice> {
    board
        .cards
        .iter()
        .filter(|card| {
            card.status == Status::Progress
                && blocked_before.contains(&card.id)
                && board.waits_on(card).is_empty()
        })
        .filter_map(|card| {
            let owner = card.owner.as_deref()?;
            if matches!(by, By::Agent(agent) if agent == owner) {
                return None;
            }
            Some(Notice {
                to: owner.to_owned(),
                text: format!(
                    "Every card your card {} waits on is done, so it can go ahead.",
                    card.label()
                ),
            })
        })
        .collect()
}

fn send_notices(core: &Arc<Core>, notices: Vec<Notice>) {
    if notices.is_empty() {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        for notice in notices {
            let _ = messages::from_board(
                &core,
                notice.to,
                notice.text,
                "Sent by Sikemux's board about a card you hold, not by another agent.".to_owned(),
            )
            .await;
        }
    });
}

fn caller(request: &HarnessRequest, tool: &str) -> Result<String, String> {
    request
        .agent_id
        .clone()
        .ok_or_else(|| format!("{tool} needs the calling agent's id"))
}

pub(crate) async fn add(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = caller(request, "board_add")?;
    let params = &request.params;
    if ["ticket", "key", "url"]
        .iter()
        .any(|key| params.get(*key).is_some_and(|value| !value.is_null()))
    {
        return Err(
            "Only the person puts a Jira or Linear ticket on the board; add the card without one"
                .into(),
        );
    }
    let adding = Adding::from_params(params)?;
    let claim = match params.get("claim") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(claim)) => *claim,
        Some(_) => return Err("claim must be a boolean".into()),
    };
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = caller_project(&core, &request);
        add_card(&core, &project, &By::Agent(agent), adding, claim).map_err(Into::into)
    })
    .await
    .map_err(|error| error.to_string())
}

pub(crate) async fn claim(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = caller(request, "board_claim")?;
    let id = card_id(request.params.get("card").unwrap_or(&Value::Null), "card")?;
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = caller_project(&core, &request);
        claim_card(&core, &project, &agent, id).map_err(Into::into)
    })
    .await
    .map_err(|error| error.to_string())
}

pub(crate) async fn update(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = caller(request, "board_update")?;
    let id = card_id(request.params.get("card").unwrap_or(&Value::Null), "card")?;
    let mut changes = Changes::from_params(&request.params)?;
    let (mut view, notices, shipping) = {
        let (core, request) = (core.clone(), request.clone());
        blocking(move || {
            let project = caller_project(&core, &request);
            let own_run = {
                let state = core.boards.lock();
                state
                    .boards
                    .get(&project)
                    .and_then(|board| board.card(id).ok())
                    .is_some_and(|card| auto::own_run(card, &agent))
            };
            if own_run && changes.status == Some(Status::Open) {
                return Err("Auto runs this card, so it does not go back to Open. If you cannot do it, say why with ask and blocked true, and the card waits for the person.".into());
            }
            let shipping = own_run && changes.status == Some(Status::Done);
            if shipping {
                changes.status = None;
                if changes.is_empty() {
                    let state = core.boards.lock();
                    let board = state.boards.get(&project).ok_or("the board is gone")?;
                    return Ok((
                        agent_view(&core, board, board.card(id)?, true),
                        Vec::new(),
                        true,
                    ));
                }
            }
            let (view, notices) = change_card(&core, &project, &By::Agent(agent), id, changes)?;
            Ok((view, notices, shipping))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    send_notices(core, notices);
    if let (true, Some(fields)) = (shipping, view.as_object_mut()) {
        fields.insert(
            "note".into(),
            "Auto ships this card's work when your turn ends and marks it done once it lands, so it stays in progress until then.".into(),
        );
    }
    Ok(view)
}

pub(crate) async fn list(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = caller(request, "board_list")?;
    let one = match request.params.get("card") {
        None | Some(Value::Null) => None,
        Some(value) => Some(card_id(value, "card")?),
    };
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = caller_project(&core, &request);
        let state = core.boards.lock();
        let Some(board) = state.boards.get(&project) else {
            if let Some(id) = one {
                return Err(format!("There is no card #{id} on this project's board").into());
            }
            return Ok(json!({ "project": project, "cards": [], "note": "The board is empty; add cards with board_add." }));
        };
        if let Some(id) = one {
            return Ok(agent_view(&core, board, board.card(id)?, true));
        }
        let team = core.names.team_of(&agent);
        let rank = |card: &Card| {
            let mine = card.owner.as_deref() == Some(agent.as_str());
            let ours = team_of(&core, board, card).as_deref() == Some(team.as_str());
            let place = if mine {
                0
            } else if ours {
                1
            } else {
                2
            };
            let status = match card.status {
                Status::Progress => 0,
                Status::Open => 1,
                Status::Done => 2,
            };
            (place, status, card.id)
        };
        let mut cards: Vec<&Card> = board.cards.iter().collect();
        cards.sort_by_key(|card| rank(card));
        let listed: Vec<Value> = cards
            .iter()
            .map(|card| agent_view(&core, board, card, false))
            .collect();
        Ok(json!({
            "project": project,
            "cards": listed,
            "note": "Your cards come first, then your team's. Pass card for one card in full.",
        }))
    })
    .await
    .map_err(|error| error.to_string())
}

pub(crate) fn auto_turn_ended(core: &Arc<Core>, agent_id: &str, ending: &super::spawn::Ending) {
    auto::turn_ended(core, agent_id, ending);
}

pub(crate) fn auto_has_work(core: &Core) -> bool {
    auto::has_work(core)
}

/// The card the agent works, else the one it touched last.
fn held_card<'a>(state: &'a mut State, agent_id: &str) -> Option<(&'a String, &'a mut Card)> {
    state
        .boards
        .iter_mut()
        .flat_map(|(project, board)| board.cards.iter_mut().map(move |card| (project, card)))
        .filter(|(_, card)| card.owner.as_deref() == Some(agent_id))
        .max_by_key(|(_, card)| (card.status == Status::Progress, card.updated_at))
}

/// A line naming the agent's card, for the report its lead gets when its
/// turn ends.
pub(crate) fn card_line(core: &Core, agent_id: &str) -> Option<String> {
    let mut state = core.boards.lock();
    let (_, card) = held_card(&mut state, agent_id)?;
    Some(format!(
        "Its card on the board: {}, {}.",
        card.label(),
        card.status.words()
    ))
}

/// The ticket of the card an agent holds, or of the card it sits under.
pub(crate) fn ticket_of_agent(core: &Core, agent_id: &str) -> Option<String> {
    let state = core.boards.lock();
    state.boards.values().find_map(|board| {
        let card = board
            .cards
            .iter()
            .filter(|card| card.owner.as_deref() == Some(agent_id) && card.status != Status::Done)
            .max_by_key(|card| card.updated_at)?;
        let mut current = Some(card);
        let mut steps = 0;
        while let Some(card) = current {
            if let Some(ticket) = &card.ticket {
                return Some(ticket.key.clone());
            }
            steps += 1;
            if steps > board.cards.len() {
                break;
            }
            current = card.parent.and_then(|parent| board.card(parent).ok());
        }
        None
    })
}

/// What a card Auto handed back waits on the person for.
pub(crate) enum Waits {
    /// Its pull request is ready for review.
    Review { url: Option<String> },
    /// It came back, for the reason in `note`.
    Held { note: Option<String> },
}

/// A card as the inbox lists it.
pub(crate) struct Summary {
    pub project: String,
    pub id: u64,
    pub title: String,
    pub done: bool,
    pub owner: Option<String>,
    pub owner_name: Option<String>,
    pub result: Option<String>,
    pub recordings: Vec<Recording>,
    pub waits: Option<Waits>,
    pub updated_at: u64,
}

impl Card {
    fn summary(&self, core: &Core, project: &str) -> Summary {
        let waits = self.auto.as_ref().and_then(|run| match run.stage {
            Stage::Review => Some(Waits::Review {
                url: run.pull.as_ref().map(|pull| pull.url.clone()),
            }),
            Stage::Held => Some(Waits::Held {
                note: run.note.clone(),
            }),
            _ => None,
        });
        Summary {
            project: project.to_owned(),
            id: self.id,
            title: self.title.clone(),
            done: self.status == Status::Done,
            owner: self.owner.clone(),
            owner_name: self.owner_name(core),
            result: self.result.clone(),
            recordings: self.recordings.clone(),
            waits,
            updated_at: self.updated_at,
        }
    }
}

/// The card the agent works, or the one it reviews for Auto.
pub(crate) fn agent_card(core: &Core, agent_id: &str) -> Option<Summary> {
    let mut state = core.boards.lock();
    if let Some((project, card)) = held_card(&mut state, agent_id) {
        return Some(card.summary(core, project));
    }
    let (project, card) = blind::reviewed_card(&state, agent_id)?;
    Some(card.summary(core, project))
}

/// A reviewer Auto started gives its verdict on the change it reviews.
pub(crate) async fn verdict(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = caller(request, "review_verdict")?;
    let params = request.params.clone();
    let core = core.clone();
    blocking(move || blind::record(&core, &agent, &params).map_err(Into::into))
        .await
        .map_err(|error| error.to_string())
}

/// The cards Auto handed back to the person, and those done since
/// `done_since`.
pub(crate) fn cards_for_person(core: &Core, done_since: Option<u64>) -> Vec<Summary> {
    let state = core.boards.lock();
    state
        .boards
        .iter()
        .flat_map(|(project, board)| board.cards.iter().map(move |card| (project, card)))
        .filter(|(_, card)| {
            card.auto
                .as_ref()
                .is_some_and(|run| matches!(run.stage, Stage::Review | Stage::Held))
                || done_since
                    .is_some_and(|since| card.status == Status::Done && card.updated_at >= since)
        })
        .map(|(project, card)| card.summary(core, project))
        .collect()
}

/// Keeps a browser recording on the card the agent works.
pub(crate) fn attach_recording(core: &Core, agent_id: &str, recording: Recording) {
    let mut state = core.boards.lock();
    let Some((project, card)) = held_card(&mut state, agent_id) else {
        return;
    };
    if card.recordings.len() >= MAX_RECORDINGS {
        card.recordings.remove(0);
    }
    card.recordings.push(recording);
    let project = project.clone();
    core.boards.save(&mut state, &project);
}

/// Every project's board, as the app draws them.
pub(crate) fn boards(core: &Core) -> Vec<Value> {
    let state = core.boards.lock();
    state
        .boards
        .values()
        .map(|board| board_view(core, board))
        .collect()
}

fn project_view(core: &Core, project: &str) -> Value {
    let state = core.boards.lock();
    match state.boards.get(project) {
        Some(board) => board_view(core, board),
        None => board_view(core, &Board::empty(project)),
    }
}

fn project_param(core: &Core, params: &Value) -> Result<String, String> {
    match params.get("project") {
        Some(Value::String(path)) if Path::new(path).is_absolute() && path.len() <= 4096 => {
            Ok(board_project(core, path))
        }
        _ => Err("project must be the project's absolute path".into()),
    }
}

/// One project's board, empty when it has no cards yet.
pub(crate) async fn board(core: &Arc<Core>, params: Value) -> Result<Value, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_param(&core, &params)?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// The person adds a card, which may carry a ticket. Answers with the board.
pub(crate) async fn person_add(core: &Arc<Core>, params: Value) -> Result<Value, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_param(&core, &params)?;
        let mut adding = Adding::from_params(&params)?;
        adding.ticket = ticket_of(params.get("ticket").unwrap_or(&Value::Null))?;
        add_card(&core, &project, &By::Person, adding, false)?;
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// The person changes or moves a card. Answers with the board.
pub(crate) async fn person_update(core: &Arc<Core>, params: Value) -> Result<Value, String> {
    let (view, notices) = {
        let core = core.clone();
        blocking(move || {
            let project = project_param(&core, &params)?;
            let id = card_id(params.get("id").unwrap_or(&Value::Null), "id")?;
            let changes = Changes::from_params(&params)?;
            let (_, notices) = change_card(&core, &project, &By::Person, id, changes)?;
            Ok((project_view(&core, &project), notices))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    send_notices(core, notices);
    Ok(view)
}

/// The person changes how the project's board runs on its own. Answers
/// with the board.
pub(crate) async fn person_settings(core: &Arc<Core>, params: Value) -> Result<Value, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_param(&core, &params)?;
        let current = {
            let state = core.boards.lock();
            state
                .boards
                .get(&project)
                .map(|board| board.settings.clone())
                .unwrap_or_default()
        };
        let settings = current.changed(&params)?;
        if let (Some(builder), Some(reviewer)) = (&settings.launcher, &settings.reviewer) {
            let cli = |id: &str| core.workspaces.launcher_cli(id);
            if cli(builder).is_some() && cli(builder) == cli(&reviewer.launcher) {
                return Err("The reviewer must run another CLI than Auto's agents, so it does not share their blind spots".into());
            }
        }
        {
            let mut state = core.boards.lock();
            let board = state
                .boards
                .entry(project.clone())
                .or_insert_with(|| Board::empty(&project));
            board.settings = settings;
            core.boards.save(&mut state, &project);
        }
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// The command that proves a change sound in `project`, when the person set
/// one in the board's settings.
pub(crate) fn check_command(core: &Core, project: &str) -> Option<String> {
    core.boards
        .lock()
        .boards
        .get(project)
        .and_then(|board| board.settings.check.clone())
}

/// Runs the project's check command in `dir`: whether it passed, and the end
/// of what it wrote.
pub(crate) fn run_check(dir: &Path, command: &str) -> Result<(bool, String), String> {
    git::check(dir, command)
}

/// The person has Auto try a card it gave back to them again. Answers with
/// the board.
pub(crate) async fn person_retry(core: &Arc<Core>, params: Value) -> Result<Value, String> {
    let (project, id) = {
        let core = core.clone();
        blocking(move || {
            let project = project_param(&core, &params)?;
            let id = card_id(params.get("id").unwrap_or(&Value::Null), "id")?;
            Ok((project, id))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    finish::retry(core, &project, id)?;
    Ok(project_view(core, &project))
}

/// The person takes a card off the board. Answers with the board.
pub(crate) async fn person_remove(core: &Arc<Core>, params: Value) -> Result<Value, String> {
    let (view, notices) = {
        let core = core.clone();
        blocking(move || {
            let project = project_param(&core, &params)?;
            let id = card_id(params.get("id").unwrap_or(&Value::Null), "id")?;
            let notices = remove_card(&core, &project, id)?;
            Ok((project_view(&core, &project), notices))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    send_notices(core, notices);
    Ok(view)
}

/// From when the agent has sat idle a while with nothing waiting for it, or
/// none while it is at work or something waits for it.
fn idle_from(core: &Core, agent_id: &str) -> Option<u64> {
    if core.harness.mailbox.has_waiting(agent_id)
        || core.harness.questions.waits_on_answer(agent_id)
    {
        return None;
    }
    let at_rest = match core.chats.get(agent_id) {
        Some(chat) => chat.is_ready() && !chat.turn_running(),
        None => core
            .agent_session(agent_id)
            .and_then(|session| session.agent.as_ref().map(|agent| agent.idle()))
            .unwrap_or(false),
    };
    at_rest.then(|| {
        core.seen
            .active_at(agent_id)
            .map_or(0, |at| at + IDLE_BEFORE_OFFER_MS)
    })
}

/// Tells each idle agent another agent started of the open cards of its team
/// that nobody holds and nothing holds up, once per card. Answers when an
/// agent that has such cards will have been idle long enough.
fn offer(core: &Arc<Core>) -> Option<u64> {
    forget_stale_offers(core);
    let now = unix_ms();
    let running = core.running_agents();
    let workers: Vec<(String, String)> = running
        .iter()
        .filter_map(|agent| {
            let named = core.names.get(agent)?;
            named.parent.as_ref()?;
            Some((agent.clone(), named.project))
        })
        .collect();
    if workers.is_empty() {
        return None;
    }
    let mut next = None;
    let mut offers: Vec<(String, Vec<String>, String)> = Vec::new();
    {
        let mut state = core.boards.lock();
        let State {
            boards,
            offered,
            woken,
            ..
        } = &mut *state;
        for (agent, project) in workers {
            let Some(board) = boards.get(&project) else {
                continue;
            };
            let busy = board.cards.iter().any(|card| {
                card.owner.as_deref() == Some(agent.as_str()) && card.status == Status::Progress
            });
            if busy {
                continue;
            }
            let team = core.names.team_of(&agent);
            let free: Vec<&Card> = board
                .cards
                .iter()
                .filter(|card| {
                    card.status == Status::Open
                        && card.owner.is_none()
                        && board.waits_on(card).is_empty()
                        && !board
                            .cards
                            .iter()
                            .any(|other| other.parent == Some(card.id))
                        && !offered.contains(&(project.clone(), card.id, agent.clone()))
                        && team_of(core, board, card).as_deref() == Some(team.as_str())
                })
                .take(MAX_OFFERED)
                .collect();
            if free.is_empty() {
                continue;
            }
            match idle_from(core, &agent) {
                Some(from) if from <= now => {}
                Some(from) => {
                    next = sooner(next, Some(from));
                    continue;
                }
                None => continue,
            }
            for card in &free {
                offered.insert((project.clone(), card.id, agent.clone()));
            }
            woken.insert(agent.clone());
            offers.push((
                agent,
                free.iter().map(|card| card.label()).collect(),
                name_of(core, &team),
            ));
        }
    }
    for (agent, cards, lead) in offers {
        let text = format!(
            "Open cards on your team's board that nobody has taken:\n\n{}\n\nIf one is yours to do, claim it with board_claim and do it; board_list with card shows one in full. If none is, end your turn without doing anything.",
            cards.join("\n")
        );
        let note = format!(
            "Sent by Sikemux's board, not by another agent, because you are idle and these cards are {lead}'s team's."
        );
        let core = core.clone();
        tokio::spawn(async move {
            let sent = messages::from_board(&core, agent.clone(), text, note).await;
            let new_turn = sent.is_ok_and(|sent| sent["delivery"] == "newTurn");
            if !new_turn {
                core.boards.lock().woken.remove(&agent);
            }
        });
    }
    next
}

/// Forgets offers of cards no longer open, or gone, and offers to agents
/// since closed.
fn forget_stale_offers(core: &Core) {
    let mut state = core.boards.lock();
    if state.offered.is_empty() {
        return;
    }
    let offered_to: HashSet<String> = state
        .offered
        .iter()
        .map(|(_, _, agent)| agent.clone())
        .collect();
    let named: HashSet<String> = core
        .names
        .of(&offered_to)
        .into_iter()
        .map(|named| named.agent_id)
        .collect();
    let State {
        boards, offered, ..
    } = &mut *state;
    offered.retain(|(project, id, agent)| {
        named.contains(agent)
            && boards
                .get(project)
                .and_then(|board| board.card(*id).ok())
                .is_some_and(|card| card.status == Status::Open)
    });
}

/// Whether the agent's turn was one the board started to offer it cards, in
/// which it took none and changed nothing on the board. Ends that turn's mark.
pub(crate) fn offer_turn_ignored(core: &Core, agent_id: &str) -> bool {
    core.boards.lock().woken.remove(agent_id)
}

/// Something besides the board's offer reached the agent in the turn the
/// offer started, so that turn's end is worth reporting.
pub(crate) fn reached_by_more_than_offer(core: &Core, agent_id: &str) {
    core.boards.lock().woken.remove(agent_id);
}

/// Offers idle workers their team's free cards, moves Auto's runs and the
/// cards' tickets on, and sends the window the boards that changed. Runs when
/// a board or the agents change, and again when a run or ticket is next due.
pub(crate) fn settle(core: &Arc<Core>) {
    let mut next = offer(core);
    finish::clear_dropped(core);
    auto::start_free(core);
    next = sooner(next, finish::due(core));
    next = sooner(next, tickets::due(core));
    if let Some(next) = next {
        core.boards.wake.at(next);
    }
    if !core.boards.lock().dirty.is_empty() {
        core.harness.questions.boards_changed();
    }
    tell_window(core);
}

fn tell_window(core: &Arc<Core>) {
    let window = core.window.client_id();
    let views = {
        let mut state = core.boards.lock();
        let fresh = state.told_window != window;
        if state.dirty.is_empty() && !fresh {
            return;
        }
        state.told_window = window;
        if window.is_none() {
            return;
        }
        let projects: Vec<String> = if fresh {
            state.boards.keys().cloned().collect()
        } else {
            state.dirty.iter().cloned().collect()
        };
        state.dirty.clear();
        projects
            .iter()
            .map(|project| match state.boards.get(project) {
                Some(board) => board_view(core, board),
                None => board_view(core, &Board::empty(project)),
            })
            .collect::<Vec<_>>()
    };
    if views.is_empty() {
        return;
    }
    let request = json!({
        "id": "agent-board",
        "project": "/",
        "agentId": null,
        "method": "agent.board",
        "params": { "boards": views },
    });
    let Ok(call) = core
        .window
        .call(WindowCall::Harness { request }, "show the board", false)
    else {
        return;
    };
    let id = call.id;
    let core = core.clone();
    tokio::spawn(async move {
        if tokio::time::timeout(WINDOW_ANSWER_WAIT, call.answer)
            .await
            .is_err()
        {
            core.window.forget(id);
        }
    });
}
