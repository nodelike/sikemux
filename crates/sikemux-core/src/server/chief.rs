//! Each project's chief, @Chief: one persistent chat per project that every other agent there
//! works under. It is a chat like any other, on the agent and model a new chat in the project
//! would use, with a seat: a brief from the core and a session kept small. Clients draw its row
//! for every project from the project itself; its chat is made the first time the person opens
//! that row, and its first message is asked for then.
//!
//! The core keeps, for each chief, what its next turn hears and what the person is shown in its
//! chat: never what the person typed, which is read back from the chats' own records.

pub(crate) use super::board::goal::Overrule;
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::chief::{OpenChiefParams, OpenedChief};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::agent_accounts::status::AgentStatus;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallState, PersonCall};
use sikemux_view::chat::{
    AwayAgent, AwayDecision, AwayLanded, ChiefAway, ChiefWeek, LinesChanged, OverruledDecision,
    RuleChange,
};
use sikemux_view::turn::{TurnPhase, TurnStep};
use sikemux_view::workspace::ProjectInfo;
use sikemux_wire::protocol::ChatContext;

use super::chat::feed::Feed;
use super::chat::reset::{calendar, WallClock, Zone};
use super::chat::{rotation, Chat, ChatCommand, Whose};
use super::kept::Kept;
use super::launching::{self, Launch};
use super::{Core, CoreResult};

const FILE: &str = "chiefs.json";
/// Away this long from the chief, the person is told what happened when they come back.
const AWAY_MS: u64 = 60 * 60_000;
/// Being with the chief again this soon after the last time is not written down again.
const WITH_STEP_MS: u64 = 60_000;
const WEEK_MS: u64 = 7 * 24 * 60 * 60_000;
/// What a turn's digest holds at most, so it stays a few hundred tokens with twenty agents.
const DIGEST_CHARS: usize = 4_000;
/// What of one agent's or the person's words a digest line quotes.
const LONGEST_QUOTE: usize = 200;
/// Sikemux's own notes and landings kept for a chief at most, newest kept.
const MOST_NOTES: usize = 20;
const MOST_LANDED: usize = 40;
/// Chats noted as their agents went to sleep, and lines kept for the chief's chat, at most.
const MOST_FEEDS: usize = 40;
const MOST_ROWS: usize = 20;

/// How the chief works, ahead of the project's rules in its instructions.
pub(crate) const SEAT: &str = "You are @Chief, this project's chief in Sikemux. The person talks to you; every other agent in the project works under you. Your job is to decide and route, not to do the work yourself.

- Decide and record, don't ask. Anything inside the machine is yours: who works on what, on which model, in what order, how to check it, landing by the project's rule, deploys to places that are not production. Record each decision with board_decide, with what it rests on, so the person can overrule it.
- Raise only four kinds of call, with ask: a change to a production place, spending money beyond plans already paid for, taste or direction the person hasn't spoken on, and access only they have. Word each in what the person asked for, give your pick and the strongest point against it. If it cannot be worded that way, it is not theirs: decide it.
- Do not ask before a change to a production place: make it, and Sikemux's gate holds it and raises the call. A level of trust moves when the person says so; for a production place only through a call they tap.
- Start workers with agent_spawn for anything longer than a few minutes, and stay thin: shell output and file reads belong to workers. Their reports reach you; read more with agent_read only when you need it.
- Pick for each worker the cheapest agent and model that will do its job well, and keep work off an account near its limit.
- When a smaller change or a deletion solves the problem, choose it.
- When the person repeats a correction a rule of theirs already makes, give memory_update that rule's id with wrong \"said again\". If a check can express it, have a worker land one, then wrong \"now a check: <commit>\"; never for an Overrule's rule.
- When you report to the person, lead with the outcome, then only what needs them. Report a result as what changed, where it is and what to try. Never say something is done unless Sikemux's state line for that work says so.
- Come back to the person at five moments only: a call; done, with where the work is and what to try; slipped, with the new time and why; stuck on something only they have; and when they ask.";

/// What the chief is handed as it starts: how it works, then the project's rules and knowledge.
pub(crate) fn brief(core: &Core, cwd: &std::path::Path) -> String {
    match super::memory::for_chat(core, cwd) {
        Some(kept) => format!("{SEAT}\n\n{kept}"),
        None => SEAT.to_owned(),
    }
}

/// What the core keeps for the project's chiefs, saved, by chief: see [`Seat`]; and each
/// Overrule the person made, by the decision it overruled.
pub(crate) struct Chiefs {
    kept: Kept,
    state: Mutex<Saved>,
    /// The rule lines put in an agent's own chat, where a project has no chief, by their key.
    lines_in_chats: Mutex<HashMap<String, (String, RuleChange)>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    seats: HashMap<String, Seat>,
    overrules: HashMap<String, Overrule>,
}

/// What the core keeps for one chief.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Seat {
    /// The project, by id.
    project: String,
    /// When the person was last with it, with their window in front.
    with: u64,
    /// The weeks, by when each began, in which the person was with it, the latest two.
    with_weeks: Vec<u64>,
    /// It was asked for its first message.
    greeted: bool,
    /// When its last turn took the digest.
    digested: u64,
    /// The chats, as agent and feed, that its project's agents went to sleep on since its last
    /// digest: where to read what the person typed to them. Ids only, never words.
    feeds: Vec<(String, String)>,
    /// The agents of its project the person wrote to since its last digest: only their chats
    /// are read for it.
    wrote: Vec<String>,
    /// What Sikemux has to tell it with its next turn, and when.
    notes: Vec<(u64, String)>,
    /// The work landed in its project, newest last.
    landed: Vec<Landing>,
    /// What happened while the person was away, shown until it is done with.
    away: Option<Away>,
    /// Lines of its chat the core keeps until the person has seen them or they are done with,
    /// put in its chat again when it opens on another feed.
    rows: Vec<Row>,
    /// When the last week it counted began.
    counted: u64,
    /// Where its session stands, so a sleep or a new core goes on from it.
    session: Option<rotation::KeptSession>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Landing {
    agent: String,
    at: u64,
    /// What its commit says it changed.
    change: String,
    #[serde(default)]
    lines: Option<LinesChanged>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    /// What it is about, so a later one replaces it or Undo lets it go.
    key: String,
    update: Value,
    /// When it was first put in the chat, which it keeps when it is put there again.
    at: u64,
    /// The feed it was last put on.
    feed: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Away {
    shown: ChiefAway,
    /// Built with calls in it, it stays until they are answered; else until the person writes.
    until_answered: bool,
}

impl Chiefs {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let state = Mutex::new(kept.load());
        Self {
            kept,
            state,
            lines_in_chats: Mutex::default(),
        }
    }

    fn change<T>(&self, change: impl FnOnce(&mut Saved) -> T) -> T {
        let mut state = self.state.locked();
        let changed = change(&mut state);
        self.kept.save_soon(&*state);
        changed
    }

    fn seat<T>(&self, chief: &str, change: impl FnOnce(&mut Seat) -> T) -> Option<T> {
        self.change(|saved| saved.seats.get_mut(chief).map(change))
    }

    fn read<T>(&self, chief: &str, read: impl FnOnce(&Seat) -> T) -> Option<T> {
        self.state.locked().seats.get(chief).map(read)
    }
}

impl super::part::Part for Chiefs {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![tokio::spawn(count_weeks(core.clone()))]
    }

    fn flush(&self) {
        self.kept.flush();
    }
}

/// Every project's chief, by agent id.
pub(crate) fn all(core: &Core) -> HashSet<String> {
    core.workspaces().chiefs()
}

/// Whether `agent_id` is a project's chief.
pub(crate) fn is_chief(core: &Core, agent_id: &str) -> bool {
    core.workspaces().is_chief(agent_id)
}

/// Each project's chief whose chat is open, by the project's id. A chief closed hears nothing.
pub(crate) fn seated(core: &Core) -> HashMap<String, String> {
    (core.workspaces().projects().into_iter())
        .filter_map(|project| Some((project.id, project.chief?)))
        .filter(|(_, chief)| core.workspaces().agent(chief).is_some())
        .collect()
}

/// The chief over `agent_id`, when its project has one open other than the agent itself.
fn chief_over(core: &Core, agent_id: &str) -> Option<String> {
    let project = core.workspaces().agent(agent_id)?.project;
    seated(core)
        .remove(&project)
        .filter(|chief| chief != agent_id)
}

fn project_of_chief(core: &Core, chief: &str) -> Option<ProjectInfo> {
    (core.workspaces().projects().into_iter())
        .find(|project| project.chief.as_deref() == Some(chief))
}

pub(crate) fn chief_at(core: &Core, path: &str) -> Option<String> {
    (core.workspaces().projects().into_iter())
        .find(|project| project.path.to_string_lossy() == path)
        .and_then(|project| project.chief)
}

/// The chief's seat, made the first time.
fn ensure_seat(core: &Core, chief: &str, project: &str) {
    if core.chiefs().read(chief, |_| ()).is_some() {
        return;
    }
    let now = epoch_ms();
    let mut seat = Seat {
        project: project.to_owned(),
        digested: now,
        counted: week_began(now).unwrap_or_default(),
        ..Seat::default()
    };
    be_with(&mut seat, now);
    core.chiefs().change(|saved| {
        saved.seats.entry(chief.to_owned()).or_insert(seat);
    });
}

/// Opens a project's chief, as the person opening its row or writing to it does: made the
/// first time, waiting for the person, and brought back as itself after a close.
pub(crate) async fn open(core: &Arc<Core>, params: OpenChiefParams) -> CoreResult<OpenedChief> {
    static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one_at_a_time = OPENING.lock().await;
    let project = (core.workspace_view().projects.into_iter())
        .find(|project| project.id == params.project)
        .ok_or("that project is not on this host")?;
    let agent_id = seat(core, &project).await?;
    if let Some(text) = greeting(core, &project, &agent_id) {
        let ask = ChatCommand::Prompt {
            message_id: None,
            text: String::new(),
            paths: Vec::new(),
            context: vec![crate::sessions::for_agent::sikemux_context(
                "first-message",
                "Your first message",
                &text,
            )],
            shown: true,
            whose: Whose::NOT_PERSON,
        };
        let sent = core
            .chats()
            .running(&agent_id)
            .and_then(|chat| chat.send(ask));
        if let Err(error) = sent {
            eprintln!("sikemux core: the chief was not asked for its first message: {error}");
        }
    }
    Ok(OpenedChief { agent_id })
}

/// The project's chief, open: itself while it is, taken up again as itself after a close,
/// else made, waiting for the person.
async fn seat(core: &Arc<Core>, project: &ProjectInfo) -> CoreResult<String> {
    match &project.chief {
        Some(chief) if core.workspaces().agent(chief).is_some() => Ok(chief.clone()),
        Some(chief) if core.names().ran_session(chief).is_some() => {
            launching::reopen_agent(core, chief.clone())
                .await
                .map(|taken| taken.agent_id)
        }
        Some(chief) => make(core, project, chief.clone()).await,
        None => {
            let chief = super::names::new_agent_id();
            core.workspaces().set_chief(&project.id, &chief)?;
            make(core, project, chief).await
        }
    }
}

/// Starts the chief's chat on the launcher a new chat in the project would use, waiting for
/// the person.
async fn make(core: &Arc<Core>, project: &ProjectInfo, chief: String) -> CoreResult<String> {
    let launcher = (project.launcher.clone()).ok_or("no agent here can run it")?;
    let kind = super::workspace::LauncherId::parse(&launcher)
        .kind
        .to_owned();
    let pick = core.workspaces().model_pick(&kind).unwrap_or_default();
    core.names()
        .name_chief(&chief, &core.workspaces().project_of(&project.path));
    let chosen = Launch {
        agent_id: chief,
        launcher,
        cwd: project.path.clone(),
        permission_mode: None,
        model: pick.model,
        model_provider: None,
        effort: pick.effort,
        resume_id: None,
        account: None,
        fallbacks: Vec::new(),
        program: None,
        waits_for_person: true,
    };
    let agent_id = chosen.agent_id.clone();
    launching::start(core, None, chosen).await?;
    Ok(agent_id)
}

/// Its first message, the first time the person opens a chief whose chat has had no turn.
/// Tested and marked under the lock [`open`] holds, so two opens at once greet once.
fn greeting(core: &Core, project: &ProjectInfo, chief: &str) -> Option<String> {
    ensure_seat(core, chief, &project.id);
    let fresh = core
        .chats()
        .get(chief)
        .is_some_and(|chat| !chat.has_answered() && !chat.working());
    let greet = core.chiefs().seat(chief, |seat| {
        let greet = fresh && !seat.greeted;
        seat.greeted = true;
        if greet {
            seat.with = epoch_ms();
        }
        greet
    })?;
    if !greet {
        return None;
    }
    let path = project.path.to_string_lossy().into_owned();
    let now = epoch_ms();
    let turns = super::turn_state::all(core);
    let calls = super::person_calls::shown(core);
    let open: Vec<String> = (core.workspaces().agent_projects().into_iter())
        .filter(|(agent, id)| *id == project.id && agent != chief)
        .map(|(agent, _)| {
            let title = core.workspaces().title(&agent).unwrap_or_default();
            let stands = turns.get(&agent).map_or_else(
                || "not started".to_owned(),
                |turn| super::chat::cap::standing(turn, now),
            );
            let asks: Vec<String> = (calls.iter())
                .filter(|call| call.project == path && call.state == CallState::Open)
                .filter(|call| call.asker.agent_id == agent)
                .map(|call| format!("asks the person {}", quoted(&call.question)))
                .collect();
            let asks = match asks.is_empty() {
                true => String::new(),
                false => format!("; {}", asks.join("; ")),
            };
            format!(
                "- {}, titled {}: {}{asks}",
                called(core, &agent),
                quoted(&title),
                quoted(&stands)
            )
        })
        .collect();
    Some(format!(
        "The person has opened you, @Chief of {}, for the first time. Write them your first message: \
         say who you are, how many chats are open here and how they stand, what waits on them \
         first in the words of what was asked, then one line for each of the rest. Ask nothing you \
         could decide, close nobody's chat, and say their chats stay where they are and that they \
         can still talk to any of them. The open chats, as Sikemux has them, each chat's title and \
         state quoted as its agent wrote them:\n{}",
        crate::text::one_line(&project.name),
        if open.is_empty() { "- none".to_owned() } else { open.join("\n") }
    ))
}

/// `text` an agent or the person wrote, on one line, cut, in quotes: never a line of its own.
fn quoted(text: &str) -> String {
    let line = crate::text::one_line(text).replace('"', "'");
    format!("\"{}\"", crate::text::clipped(&line, LONGEST_QUOTE))
}

fn called(core: &Core, agent_id: &str) -> String {
    format!(
        "@{}",
        crate::text::one_line(&core.names().name_or_id(agent_id))
    )
}

/// Keeps where a chief's session stands when it changed, so a sleep or a new core goes on from it.
pub(crate) fn keep_session(core: &Core, chat: &Chat) {
    if !chat.capped() {
        return;
    }
    let kept = rotation::kept(chat);
    let unchanged = core
        .chiefs()
        .read(chat.agent_id(), |seat| seat.session.as_ref() == Some(&kept));
    if unchanged == Some(false) {
        core.chiefs()
            .seat(chat.agent_id(), |seat| seat.session = Some(kept));
    }
}

pub(crate) fn kept_session(core: &Core, agent_id: &str) -> Option<rotation::KeptSession> {
    core.chiefs()
        .read(agent_id, |seat| seat.session.clone())
        .flatten()
}

/// A closed chief's chat lets go of where its session stood.
pub(crate) fn closed(core: &Core, agent_id: &str) {
    if core.chiefs().read(agent_id, |seat| seat.session.is_some()) == Some(true) {
        core.chiefs().seat(agent_id, |seat| seat.session = None);
    }
}

/// The person wrote to `agent_id` themselves. Written to a chief, they are with it, and an away
/// block with no call in it is done with; written to another agent, its chief's next digest
/// reads that chat.
pub(crate) fn person_wrote(core: &Core, agent_id: &str) {
    if !is_chief(core, agent_id) {
        let Some(chief) = chief_over(core, agent_id) else {
            return;
        };
        let known = core
            .chiefs()
            .read(&chief, |seat| {
                seat.wrote.iter().any(|wrote| wrote == agent_id)
            })
            .unwrap_or(true);
        if !known {
            core.chiefs()
                .seat(&chief, |seat| seat.wrote.push(agent_id.to_owned()));
        }
        return;
    }
    let now = epoch_ms();
    let stands = core.chiefs().read(agent_id, |seat| {
        let done = seat.away.as_ref().is_some_and(|away| !away.until_answered);
        (done, now.saturating_sub(seat.with) >= WITH_STEP_MS)
    });
    let Some((away_done, moved)) = stands else {
        return;
    };
    if away_done || moved {
        core.chiefs().seat(agent_id, |seat| {
            be_with(seat, now);
            if away_done {
                seat.away = None;
            }
        });
    }
    if away_done {
        if let Some(feed) = core.chats().heard(agent_id) {
            feed.set_away(None);
        }
    }
}

fn be_with(seat: &mut Seat, now: u64) {
    seat.with = now;
    if let Some(week) = week_began(now).filter(|week| !seat.with_weeks.contains(week)) {
        seat.with_weeks.push(week);
        if seat.with_weeks.len() > 2 {
            seat.with_weeks.remove(0);
        }
    }
}

/// A chat went to sleep: where its record is, for its chief's next digest.
pub(crate) fn chat_rested(core: &Core, agent_id: &str, feed: &str) {
    let Some(chief) = chief_over(core, agent_id) else {
        return;
    };
    let noted = (agent_id.to_owned(), feed.to_owned());
    let known = core
        .chiefs()
        .read(&chief, |seat| seat.feeds.contains(&noted))
        .unwrap_or(true);
    if !known {
        core.chiefs().seat(&chief, |seat| {
            seat.feeds.push(noted);
            if seat.feeds.len() > MOST_FEEDS {
                seat.feeds.remove(0);
            }
        });
    }
}

/// `agent` landed work in the project at `path` with a commit whose message is `message`.
pub(crate) fn landed(
    core: &Core,
    path: &str,
    agent: &str,
    message: &str,
    lines: Option<LinesChanged>,
) {
    let Some(chief) = chief_at(core, path) else {
        return;
    };
    let landing = Landing {
        agent: agent.to_owned(),
        at: epoch_ms(),
        change: change_of(message),
        lines,
    };
    core.chiefs().seat(&chief, |seat| {
        seat.landed.push(landing);
        if seat.landed.len() > MOST_LANDED {
            seat.landed.remove(0);
        }
    });
}

/// Sikemux tells the chief `text` with its next turn.
fn note(core: &Core, chief: &str, text: String) {
    core.chiefs().seat(chief, |seat| {
        seat.notes.push((epoch_ms(), text));
        if seat.notes.len() > MOST_NOTES {
            seat.notes.remove(0);
        }
    });
}

/// Puts `update` in the chief's chat, open or asleep, and keeps it, under `key`, until it is done
/// with: a chat that opens on another feed, after a restart, gets it again.
fn put_row(core: &Core, chief: &str, key: String, update: Value) {
    let feed = core.chats().heard(chief);
    let at = epoch_ms();
    let row = Row {
        key,
        update: update.clone(),
        at,
        feed: feed.as_ref().map(|feed| feed.id().to_owned()),
    };
    core.chiefs().change(|saved| {
        let refused: HashSet<String> = (saved.overrules.iter())
            .filter(|(_, overrule)| overrule.stopped_auto)
            .map(|(key, _)| format!("overrule|{key}"))
            .collect();
        let Some(seat) = saved.seats.get_mut(chief) else {
            return;
        };
        seat.rows.retain(|kept| kept.key != row.key);
        seat.rows.push(row);
        while seat.rows.len() > MOST_ROWS {
            let first = |going: &dyn Fn(&Row) -> bool| seat.rows.iter().position(going);
            let gone = first(&|row| row.key == "week")
                .or_else(|| first(&|row| refused.contains(&row.key)))
                .unwrap_or(0);
            seat.rows.remove(gone);
        }
    });
    if let Some(feed) = feed {
        emit_row(&feed, chief, update, at);
    }
}

/// The chief's line kept under `key` is done with.
fn drop_row(core: &Core, chief: &str, key: &str) {
    core.chiefs()
        .seat(chief, |seat| seat.rows.retain(|row| row.key != key));
}

/// The chief's chat is ready and its saved history shown: the lines it keeps that this feed has
/// not had go at its end, each under the time it was first made.
pub(crate) fn chat_ready(core: &Core, chat: &Chat) {
    if *chat.saved_reading.borrow() {
        return;
    }
    let id = chat.feed.id().to_owned();
    let missing: Vec<(Value, u64)> = {
        let mut saved = core.chiefs().state.locked();
        let Some(seat) = saved.seats.get_mut(chat.agent_id()) else {
            return;
        };
        let missing: Vec<(Value, u64)> = (seat.rows.iter_mut())
            .filter(|row| row.feed.as_deref() != Some(id.as_str()))
            .map(|row| {
                row.feed = Some(id.clone());
                (row.update.clone(), row.at)
            })
            .collect();
        if !missing.is_empty() {
            core.chiefs().kept.save_soon(&*saved);
        }
        missing
    };
    for (update, at) in missing {
        emit_row(&chat.feed, chat.agent_id(), update, at);
    }
}

fn emit_row(feed: &Arc<Feed>, chief: &str, mut update: Value, at: u64) {
    sikemux_fold::update::stamp_replayed(&mut update, Some(at), false);
    let session = (feed.start()).map_or_else(|| chief.to_owned(), |start| start.session_id);
    feed.emit(
        sikemux_fold::ChatEventKind::SessionUpdate,
        json!({ "sessionId": session, "update": update }),
    );
}

/// The chief's chat opened on `feed`: what happened while the person was away is shown on it.
pub(crate) fn chat_opened(core: &Core, chief: &str, feed: &Arc<Feed>) {
    if let Some(away) = core.chiefs().read(chief, |seat| {
        seat.away.as_ref().map(|away| away.shown.clone())
    }) {
        feed.set_away(away);
    }
}

/// What the chief's turn starts with, for the agent only: what it can start workers on, then
/// the facts since its last digest as one compact block, left out when nothing happened. The
/// person's words are read back from the chats' records and sit apart from what agents wrote,
/// which is always quoted and named as theirs. The reading runs off the threads that serve chats.
pub(crate) async fn digest(core: &Arc<Core>, chief: &str) -> Vec<ChatContext> {
    if !is_chief(core, chief) {
        return Vec::new();
    }
    let (core, chief) = (core.clone(), chief.to_owned());
    super::connection::blocking(move || {
        Ok((can_start(&core).into_iter())
            .chain(digest_now(&core, &chief))
            .collect())
    })
    .await
    .unwrap_or_default()
}

/// What the chief can start workers on here, with each turn: the agents signed in, the models
/// each last offered, and how much each account has left. Read from memory, with no model asked.
pub(crate) fn can_start(core: &Core) -> Option<ChatContext> {
    let now = epoch_ms();
    let mut agents: Vec<StartsOn> = Vec::new();
    for launcher in core.workspaces().chat_launchers() {
        if !matches!(launcher.status, Some(AgentStatus::Ready { .. })) {
            continue;
        }
        let kind = launcher.agent.provider.clone();
        let given = (launcher.label.split_once(" · ")).map(|(_, name)| name.to_owned());
        let room = room_left(&kind, launcher.agent.config_path.as_deref(), now);
        match agents.iter_mut().find(|agent| agent.kind == kind) {
            Some(agent) => agent.accounts.push((given, room)),
            None => agents.push(StartsOn {
                models: offered_models(core, &kind),
                kind,
                accounts: vec![(given, room)],
            }),
        }
    }
    let text = can_start_text(&agents)?;
    Some(crate::sessions::for_agent::sikemux_context(
        "can-start",
        "What you can start workers on",
        &text,
    ))
}

const CAN_START_CHARS: usize = 600;
const CAN_START_SHOWN: usize = 3;

/// An agent the chief can start workers on: the models it offers, and each account's name the
/// person gave it, if any, with what it has left.
struct StartsOn {
    kind: String,
    models: Vec<String>,
    accounts: Vec<(Option<String>, String)>,
}

fn offered_models(core: &Core, kind: &str) -> Vec<String> {
    (super::chat::models_of(core, kind).into_iter())
        .flat_map(|picker| picker.choices)
        .filter(|choice| !choice.superseded)
        .map(|choice| choice.label)
        .collect()
}

/// How much the account's tightest limit has left, as its last reading said.
fn room_left(kind: &str, config_path: Option<&str>, now: u64) -> String {
    use crate::agent_accounts::usage::{last_reading, UsageResetAt};
    let Some(reading) = last_reading(kind, config_path) else {
        return "no plan reading".to_owned();
    };
    let tightest = (reading.usage.windows.iter())
        .max_by(|one, other| one.used_percent.total_cmp(&other.used_percent));
    let Some(window) = tightest else {
        return "no plan reading".to_owned();
    };
    let left = (100.0 - window.used_percent).clamp(0.0, 100.0).round();
    let over = match window.window_minutes {
        Some(minutes) if minutes >= 7 * 24 * 60 => "this week".to_owned(),
        Some(minutes) if minutes >= 60 => format!("of this {}h window", minutes / 60),
        _ => format!("of {}", window.label.to_lowercase()),
    };
    let mut room = format!("{left}% left {over}");
    if let (true, Some(UsageResetAt::Unix(at))) = (left < 15.0, &window.resets_at) {
        let hours = (at * 1000).saturating_sub(now) / 3_600_000;
        room.push_str(&match hours {
            0 => " (resets within the hour)".to_owned(),
            1..=47 => format!(" (resets in {hours}h)"),
            _ => format!(" (resets in {}d)", hours / 24),
        });
    }
    if now.saturating_sub(reading.read_at) >= 15 * 60 * 1000 {
        room.push_str(" (reading old)");
    }
    room
}

/// The line itself, within [`CAN_START_CHARS`]: models past the first few go first, then
/// accounts past the first few, then the last agents. An account is called by the name the
/// person gave it in Sikemux, or by its place among the agent's accounts, never by an address.
fn can_start_text(agents: &[StartsOn]) -> Option<String> {
    if agents.is_empty() {
        return None;
    }
    let line = |models: usize, accounts: usize, kept: usize| {
        let mut parts: Vec<String> = (agents.iter().take(kept))
            .map(|agent| {
                let called = |at: usize, given: &Option<String>| match given {
                    Some(name) if !name.contains('@') => name.clone(),
                    _ => format!("account {}", at + 1),
                };
                let mut rooms: Vec<String> = match agent.accounts.as_slice() {
                    [(None, room)] => vec![room.clone()],
                    all => (all.iter().enumerate().take(accounts))
                        .map(|(at, (given, room))| format!("{} {room}", called(at, given)))
                        .collect(),
                };
                if agent.accounts.len() > accounts {
                    rooms.push(format!("+{} more", agent.accounts.len() - accounts));
                }
                let mut said = format!("{}: {}", agent.kind, rooms.join(", "));
                if !agent.models.is_empty() {
                    let shown = agent.models.iter().take(models).cloned();
                    let mut named: Vec<String> = shown.collect();
                    if agent.models.len() > models {
                        named.push(format!("+{} more", agent.models.len() - models));
                    }
                    said.push_str(&format!("; models {}", named.join(", ")));
                }
                said
            })
            .collect();
        if agents.len() > kept {
            parts.push(format!("+{} more agents", agents.len() - kept));
        }
        format!(
            "Agents you can start workers on here: {}",
            parts.join(" · ")
        )
    };
    let most = usize::MAX;
    let fits = |text: &String| text.chars().count() <= CAN_START_CHARS;
    let mut text = line(most, most, agents.len());
    for (models, accounts) in [(CAN_START_SHOWN, most), (CAN_START_SHOWN, CAN_START_SHOWN)] {
        if fits(&text) {
            return Some(text);
        }
        text = line(models, accounts, agents.len());
    }
    let mut kept = agents.len();
    while !fits(&text) && kept > 1 {
        kept -= 1;
        text = line(CAN_START_SHOWN, CAN_START_SHOWN, kept);
    }
    Some(text.chars().take(CAN_START_CHARS).collect())
}

fn digest_now(core: &Core, chief: &str) -> Option<ChatContext> {
    let project = project_of_chief(core, chief)?;
    ensure_seat(core, chief, &project.id);
    let now = epoch_ms();
    let (since, feeds, wrote, notes) = core.chiefs().seat(chief, |seat| {
        let since = std::mem::replace(&mut seat.digested, now);
        (
            since,
            std::mem::take(&mut seat.feeds),
            std::mem::take(&mut seat.wrote),
            std::mem::take(&mut seat.notes),
        )
    })?;
    let path = project.path.to_string_lossy().into_owned();
    let agents: Vec<String> = (core.workspaces().agent_projects().into_iter())
        .filter(|(agent, id)| *id == project.id && agent != chief)
        .map(|(agent, _)| agent)
        .collect();
    let mut agent_lines = Vec::new();
    let turns = super::turn_state::all(core);
    for agent in &agents {
        let Some(turn) = turns.get(agent).filter(|turn| turn.since >= since) else {
            continue;
        };
        if turn.state == TurnPhase::Working {
            continue;
        }
        let title = core.workspaces().title(agent).unwrap_or_default();
        agent_lines.push((
            turn.since,
            format!(
                "- {} ended a turn on {}. Its state: {}",
                called(core, agent),
                quoted(&title),
                quoted(&super::chat::cap::standing(turn, now))
            ),
        ));
    }
    let mut person_lines = Vec::new();
    let mut quoted_answers: HashSet<(String, String)> = HashSet::new();
    let calls = super::person_calls::history(core, &path).unwrap_or_default();
    let asked_by_chief = |call: &PersonCall| {
        call.asker.agent_id == chief || call.joined.iter().any(|joined| joined.agent_id == chief)
    };
    for call in calls.iter().filter(|call| !asked_by_chief(call)) {
        if call.raised_at >= since && call.state == CallState::Open {
            agent_lines.push((
                call.raised_at,
                format!(
                    "- {} raised a call to the person: {}",
                    called(core, &call.asker.agent_id),
                    quoted(&call.question)
                ),
            ));
        }
        if let Some(answer) = call.answer.as_ref().filter(|answer| answer.at >= since) {
            let asker = called(core, &call.asker.agent_id);
            let question = quoted(&call.question);
            let line = match (answer.option.as_ref(), answer.words.as_ref()) {
                _ if super::person_calls::handed_back(call) => {
                    format!("- handed {asker}'s call {question} back to be decided without them")
                }
                _ if super::person_calls::let_stand(call) => {
                    format!("- let {asker}'s assumption stand on its call {question}")
                }
                (Some(id), _) => {
                    let picked = (call.options.iter())
                        .find(|option| &option.id == id)
                        .map(|option| format!("picked {}", quoted(&option.label)))
                        .unwrap_or_else(|| "picked an option".to_owned());
                    format!("- answered {asker}'s call {question}: {picked}")
                }
                (None, Some(words))
                    if quoted_answers.insert((call.asker.agent_id.clone(), words.clone())) =>
                {
                    format!(
                        "- answered {asker}'s call {question}: in their words {}",
                        quoted(words)
                    )
                }
                (None, Some(_)) => {
                    format!("- answered {asker}'s call {question}: in the same words")
                }
                (None, None) => format!("- answered {asker}'s call {question}"),
            };
            person_lines.push((answer.at, line));
        }
    }
    let wrote: Vec<String> = (wrote.into_iter())
        .filter(|agent| agents.contains(agent))
        .collect();
    person_lines.extend(typed_since(core, &wrote, feeds, since, &quoted_answers));
    let notes: Vec<(u64, String)> = (notes.into_iter())
        .map(|(at, text)| (at, format!("- {text}")))
        .collect();
    let sections = [
        (
            "What agents of the project did, their words quoted as they wrote them:",
            agent_lines,
        ),
        ("What the person did, from Sikemux's record:", person_lines),
        ("From Sikemux:", notes),
    ];
    let text = digest_text(sections)?;
    Some(crate::sessions::for_agent::sikemux_context(
        "digest",
        "Since your last turn",
        &text,
    ))
}

/// What the person typed to `wrote`, the project's agents they wrote to since `since`, read
/// back from each chat's record: the running and resting chats, and those `feeds` noted as they
/// went to sleep. A slash command is not counted. What the record marks as typed while a call
/// whose words are never kept was open there is not passed on; nor are words the reader
/// leaves out, though the line says the person wrote.
fn typed_since(
    core: &Core,
    wrote: &[String],
    feeds: Vec<(String, String)>,
    since: u64,
    quoted_answers: &HashSet<(String, String)>,
) -> Vec<(u64, String)> {
    let mut feeds: Vec<(String, String)> = (feeds.into_iter())
        .filter(|(agent, _)| wrote.contains(agent))
        .collect();
    for agent in wrote {
        if let Some(feed) = core.chats().heard(agent) {
            feeds.push((agent.clone(), feed.id().to_owned()));
        }
    }
    feeds.sort();
    feeds.dedup();
    let places = core.workspaces().places();
    let said_at = |agent: &str| {
        (places.get(agent))
            .and_then(|(place, _)| place.said_at)
            .unwrap_or(since)
    };
    let mut lines = Vec::new();
    let mut heard: HashSet<String> = HashSet::new();
    for (agent, feed) in &feeds {
        let mut kept_back = false;
        for said in super::why::persons_said_in(core, feed) {
            let Some(at) = said.at.filter(|at| *at >= since) else {
                continue;
            };
            heard.insert(agent.clone());
            if said.command {
                continue;
            }
            if said.hidden {
                kept_back = true;
                continue;
            }
            let line = match said.words {
                Some(words) if quoted_answers.contains(&(agent.clone(), words.clone())) => format!(
                    "- wrote to {}, the words that answered its call",
                    called(core, agent)
                ),
                Some(words) => format!("- wrote to {}: {}", called(core, agent), quoted(&words)),
                None => format!("- wrote to {}", called(core, agent)),
            };
            lines.push((at, line));
        }
        if kept_back {
            lines.push((
                since,
                format!(
                    "- wrote to {} while a secret or access call was open there; Sikemux does not pass those words on",
                    called(core, agent)
                ),
            ));
        }
    }
    for agent in wrote.iter().filter(|agent| !heard.contains(*agent)) {
        let at = said_at(agent);
        let line = match core.workspaces().in_terminal(agent) {
            true => format!(
                "- wrote to {} in its terminal; Sikemux keeps no record of a terminal's words",
                called(core, agent)
            ),
            false => format!(
                "- wrote to {}; Sikemux could not read the words back",
                called(core, agent)
            ),
        };
        lines.push((at, line));
    }
    lines
}

/// The digest's sections under their headings, within [`DIGEST_CHARS`]: the oldest lines go
/// first. None when every section is empty.
fn digest_text<const N: usize>(sections: [(&str, Vec<(u64, String)>); N]) -> Option<String> {
    const HEAD: &str = "What happened in the project since your last turn, as Sikemux keeps it.";
    let mut all: Vec<(u64, usize, String)> = (sections.iter().enumerate())
        .flat_map(|(section, (_, lines))| {
            lines
                .iter()
                .map(move |(at, line)| (*at, section, line.clone()))
        })
        .collect();
    if all.is_empty() {
        return None;
    }
    all.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
    let headings: usize = (sections.iter())
        .map(|(heading, _)| heading.chars().count() + 2)
        .sum();
    let mut room = DIGEST_CHARS.saturating_sub(HEAD.chars().count() + headings + 40);
    let mut kept = Vec::new();
    let mut left_out = 0;
    for (at, section, line) in all {
        match room.checked_sub(line.chars().count() + 1) {
            Some(left) if left_out == 0 => {
                room = left;
                kept.push((at, section, line));
            }
            _ => left_out += 1,
        }
    }
    kept.sort_by_key(|(at, _, _)| *at);
    let mut text = HEAD.to_owned();
    if left_out > 0 {
        text.push_str(&format!("\n({left_out} earlier lines left out.)"));
    }
    for (index, (heading, _)) in sections.iter().enumerate() {
        let lines: Vec<&String> = (kept.iter())
            .filter(|(_, section, _)| *section == index)
            .map(|(_, _, line)| line)
            .collect();
        if lines.is_empty() {
            continue;
        }
        text.push_str(&format!("\n\n{heading}"));
        for line in lines {
            text.push('\n');
            text.push_str(line);
        }
    }
    Some(text)
}

/// A client put chats on screen. A chief brought to the front after the person was away an
/// hour or more shows what happened meanwhile; a block they have not dismissed grows from where
/// it began.
pub(crate) fn on_screen(core: &Core, agent_ids: &[String], focused: bool) {
    if !focused {
        return;
    }
    let now = epoch_ms();
    for chief in agent_ids.iter().filter(|agent| is_chief(core, agent)) {
        let on_screen = core.chats().heard(chief).map(|feed| feed.id().to_owned());
        let shown = |row: &Row| row.feed.is_some() && row.feed == on_screen;
        if core
            .chiefs()
            .read(chief, |seat| seat.rows.iter().any(shown))
            == Some(true)
        {
            core.chiefs()
                .seat(chief, |seat| seat.rows.retain(|row| !shown(row)));
        }
        let seen = core.chiefs().read(chief, |seat| {
            let pending = seat.away.as_ref();
            (
                seat.with,
                pending.map(|away| (away.shown.since, away.until_answered)),
            )
        });
        let Some((last, pending)) = seen else {
            continue;
        };
        if now.saturating_sub(last) < WITH_STEP_MS {
            continue;
        }
        let away = (now.saturating_sub(last) >= AWAY_MS)
            .then(|| {
                let (since, answered) = pending.unwrap_or((last, false));
                let mut away = away(core, chief, since)?;
                away.until_answered |= answered;
                Some(away)
            })
            .flatten();
        let shown = away.as_ref().map(|away| away.shown.clone());
        core.chiefs().seat(chief, |seat| {
            be_with(seat, now);
            if away.is_some() {
                seat.away = away;
            }
        });
        if let (Some(shown), Some(feed)) = (shown, core.chats().heard(chief)) {
            feed.set_away(Some(shown));
        }
    }
}

/// What happened in the chief's project since `since`; none when nothing did.
fn away(core: &Core, chief: &str, since: u64) -> Option<Away> {
    let project = project_of_chief(core, chief)?;
    let path = project.path.to_string_lossy().into_owned();
    let calls = open_calls(&super::person_calls::shown(core), &path);
    let decided: Vec<AwayDecision> =
        super::board::goal::decided_since(core, Some(since), Some(&path))
            .into_iter()
            .filter(|swarm| swarm.decision.learned.at >= since)
            .map(|swarm| AwayDecision {
                project: swarm.project,
                goal: swarm.goal,
                decision: swarm.decision.id,
                text: swarm.decision.text,
                why: swarm.decision.why,
                by: swarm.decision.learned.agent.map(|agent_id| AwayAgent {
                    name: core.names().name_or_id(&agent_id),
                    agent_id,
                }),
                at: swarm.decision.learned.at,
                overruled: false,
            })
            .collect();
    let landings = core
        .chiefs()
        .read(chief, |seat| seat.landed.clone())
        .unwrap_or_default();
    let mut by_agent: HashMap<String, Landing> = HashMap::new();
    for landing in landings.into_iter().filter(|landing| landing.at >= since) {
        let before = by_agent.get(&landing.agent).and_then(|before| before.lines);
        let lines = match (before, landing.lines) {
            (Some(before), Some(lines)) => Some(LinesChanged {
                adds: before.adds + lines.adds,
                dels: before.dels + lines.dels,
            }),
            (before, lines) => lines.or(before),
        };
        by_agent.insert(landing.agent.clone(), Landing { lines, ..landing });
    }
    let turns = super::turn_state::all(core);
    let mut landed: Vec<AwayLanded> = (by_agent.into_iter())
        .map(|(agent_id, landing)| AwayLanded {
            name: core.names().name(&agent_id),
            title: core.workspaces().title(&agent_id).unwrap_or_default(),
            at: landing.at,
            change: (!landing.change.is_empty()).then_some(landing.change),
            lines: landing.lines,
            try_it: (turns.get(&agent_id))
                .and_then(|turn| turn.done.as_ref())
                .is_some_and(|done| done.next.iter().any(|next| next.step == TurnStep::TryIt)),
            agent_id,
        })
        .collect();
    landed.sort_by_key(|landed| landed.at);
    let shown = ChiefAway {
        since,
        back: Some(epoch_ms()),
        calls,
        answered: Vec::new(),
        decided,
        landed,
    };
    let something =
        !shown.calls.is_empty() || !shown.decided.is_empty() || !shown.landed.is_empty();
    something.then_some(Away {
        until_answered: !shown.calls.is_empty(),
        shown,
    })
}

fn open_calls(calls: &[PersonCall], path: &str) -> Vec<String> {
    (calls.iter())
        .filter(|call| call.project == path && call.state == CallState::Open)
        .map(|call| call.id.clone())
        .collect()
}

/// Each chief's away block with its calls as they stand now: one whose calls are all answered
/// is done with.
pub(crate) fn refresh_away(core: &Core, calls: &[PersonCall]) {
    let shown: Vec<(String, String, Away)> = (core.chiefs().state.locked().seats.iter())
        .filter_map(|(chief, seat)| Some((chief.clone(), seat.project.clone(), seat.away.clone()?)))
        .collect();
    if shown.is_empty() {
        return;
    }
    let paths: HashMap<String, String> = (core.workspaces().projects().into_iter())
        .map(|project| (project.id, project.path.to_string_lossy().into_owned()))
        .collect();
    for (chief, project, mut away) in shown {
        let Some(path) = paths.get(&project) else {
            continue;
        };
        let live = open_calls(calls, path);
        if live == away.shown.calls {
            continue;
        }
        let done = away.until_answered && live.is_empty();
        let answered = (away.shown.calls.iter())
            .filter(|id| !live.contains(id) && !away.shown.answered.contains(id))
            .filter(|id| {
                (calls.iter()).any(|call| &call.id == *id && call.state == CallState::Answered)
            })
            .cloned()
            .collect::<Vec<_>>();
        away.shown.answered.extend(answered);
        away.shown.calls = live;
        let shown = (!done).then(|| away.shown.clone());
        core.chiefs()
            .seat(&chief, |seat| seat.away = (!done).then_some(away));
        if let Some(feed) = core.chats().heard(&chief) {
            feed.set_away(shown);
        }
    }
}

/// `text` an agent wrote, as Sikemux quotes it in a rule or a line: one sentence of it, on one
/// line, cut, in quotes.
fn first_sentence(text: &str) -> String {
    quoted(&sentence(text))
}

/// What a commit's message says it changed: the first sentence of its subject, without a
/// leading "type(scope): ".
fn change_of(message: &str) -> String {
    let subject = message.lines().next().unwrap_or_default();
    let said = match subject.split_once(": ") {
        Some((kind, said))
            if !kind.contains(' ')
                && kind.chars().next().is_some_and(|c| c.is_ascii_lowercase()) =>
        {
            said
        }
        _ => subject,
    };
    sentence(said)
}

/// The first sentence of `text`, on one line, cut.
fn sentence(text: &str) -> String {
    let line = crate::text::one_line(text);
    let end = line
        .char_indices()
        .find(|(at, c)| matches!(c, '.' | '?' | '!') && line[at + 1..].starts_with(' '))
        .map_or(line.len(), |(at, _)| at + 1);
    crate::text::clipped(&line[..end], LONGEST_QUOTE)
}

/// The decision `decision` in the chief's away block is struck as overruled, or stands again.
fn mark_away(core: &Core, chief: &str, goal: Option<u64>, decision: u64, overruled: bool) {
    let shown = core.chiefs().seat(chief, |seat| {
        let away = seat.away.as_mut()?;
        let found = (away.shown.decided.iter_mut())
            .find(|decided| decided.goal == goal && decided.decision == decision)?;
        if found.overruled == overruled {
            return None;
        }
        found.overruled = overruled;
        Some(away.shown.clone())
    });
    if let (Some(Some(shown)), Some(feed)) = (shown, core.chats().heard(chief)) {
        feed.set_away(Some(shown));
    }
}

/// The person overruled decision `decision` of `project` (a path), which `told` heard of: a
/// rule now brings decisions like it to them, the chief's chat says so on one line with Undo,
/// and the chief hears it with its next turn. `stopped_auto` says Auto's run on a slice stopped,
/// which Undo cannot put back.
pub(crate) fn overruled(
    core: &Core,
    project: &str,
    goal: Option<u64>,
    decision: (u64, &str),
    stopped_auto: bool,
    told: Vec<String>,
) {
    let (id, text) = decision;
    let named = match goal {
        Some(goal) => format!("decision {id} of goal #{goal}"),
        None => format!("decision {id} of the project"),
    };
    let said = first_sentence(text);
    let rule = super::memory::keep_persons_rule(
        core,
        project,
        format!(
            "Decisions like the person's overruled {named} are theirs to make: raise them as a call. It read, as an agent wrote it: {said}"
        ),
    )
    .inspect_err(|error| eprintln!("sikemux core: an Overrule kept no rule: {error}"))
    .ok();
    core.chiefs().change(|saved| {
        let overrule = Overrule {
            rule,
            stopped_auto,
            told,
        };
        saved
            .overrules
            .insert(overrule_key(project, goal, id), overrule);
    });
    let Some(chief) = chief_at(core, project) else {
        return;
    };
    let title = match rule {
        Some(_) => "Overruled. Decisions like this come to you from now on",
        None => "Overruled",
    };
    let shown = OverruledDecision {
        project: project.to_owned(),
        goal,
        decision: id,
        text: sentence(text),
        rule,
        undone: false,
        stopped_auto,
    };
    mark_away(core, &chief, goal, id, true);
    put_row(
        core,
        &chief,
        format!("overrule|{}", overrule_key(project, goal, id)),
        sikemux_fold::update::overruled_notice(title, shown),
    );
    let from_now_on = match rule {
        Some(_) => " Decisions like it are theirs from now on: raise them as a call.",
        None => "",
    };
    note(
        core,
        &chief,
        format!("The person overruled {named}, which an agent wrote as {said}.{from_now_on}"),
    );
}

fn overrule_key(project: &str, goal: Option<u64>, decision: u64) -> String {
    format!(
        "{project}|{}|{decision}",
        goal.map_or_else(String::new, |goal| goal.to_string())
    )
}

/// Whether rule `id` of `project` (a path) is one an Overrule kept.
pub(crate) fn kept_by_overrule(core: &Core, project: &str, id: u64) -> bool {
    let prefix = format!("{project}|");
    (core.chiefs().state.locked().overrules.iter())
        .any(|(key, overrule)| key.starts_with(&prefix) && overrule.rule == Some(id))
}

/// An agent took a rule of the person's off the list or reworded it: one line with Undo says so,
/// kept in the chief's chat until seen, or in `agent`'s own chat where the project has no chief.
pub(crate) fn rule_changed(core: &Core, line: RuleChange, agent: &str) {
    let title = match (&line.commit, &line.was) {
        (Some(_), _) => "Now a check",
        (None, Some(_)) => "Rule changed",
        (None, None) => "Rule removed",
    };
    rule_line(core, line, title, agent);
}

/// A rule of the person's came back, by their Undo or an agent they asked: the line says so, in
/// the same place [`rule_changed`] puts it.
pub(crate) fn rule_back(core: &Core, line: RuleChange, agent: &str) {
    rule_line(core, line, "Rule back", agent);
}

fn rule_line(core: &Core, line: RuleChange, title: &str, agent: &str) {
    let key = rule_key(&line.project, line.rule);
    let chief = chief_at(core, &line.project);
    match chief {
        Some(chief) => {
            let update = sikemux_fold::update::rule_change_notice(title, line);
            put_row(core, &chief, key, update);
        }
        None => in_chat(core, key, line, title, agent),
    }
}

fn in_chat(core: &Core, key: String, line: RuleChange, title: &str, agent: &str) {
    let update = sikemux_fold::update::rule_change_notice(title, line.clone());
    (core.chiefs().lines_in_chats.locked()).insert(key, (agent.to_owned(), line));
    if let Some(feed) = core.chats().heard(agent) {
        emit_row(&feed, agent, update, epoch_ms());
    }
}

/// Marks as back each line of `project` that `back` picks and that still offers Undo, in the
/// chief's chat or the agent's that holds it: its rule came back by another way.
pub(crate) fn rule_lines_back(core: &Core, project: &str, back: impl Fn(&RuleChange) -> bool) {
    let picked = |line: &RuleChange| line.project == project && !line.restored && back(line);
    if let Some(chief) = chief_at(core, project) {
        let lines = core.chiefs().read(&chief, |seat| {
            (seat.rows.iter())
                .filter_map(|row| serde_json::from_value(row.update["ruleChange"].clone()).ok())
                .filter(|line| picked(line))
                .collect::<Vec<RuleChange>>()
        });
        for mut line in lines.unwrap_or_default() {
            line.restored = true;
            let key = rule_key(project, line.rule);
            put_row(core, &chief, key, rule_back_notice(line));
        }
    }
    let in_chats: Vec<(String, String, RuleChange)> =
        (core.chiefs().lines_in_chats.locked().iter())
            .filter(|(_, (_, line))| picked(line))
            .map(|(key, (agent, line))| (key.clone(), agent.clone(), line.clone()))
            .collect();
    for (key, agent, mut line) in in_chats {
        line.restored = true;
        in_chat(core, key, line, "Rule back", &agent);
    }
}

fn rule_back_notice(line: RuleChange) -> Value {
    sikemux_fold::update::rule_change_notice("Rule back", line)
}

fn rule_key(project: &str, rule: u64) -> String {
    format!("rule|{project}|{rule}")
}

/// The Overrule of a decision, when the person made one that Undo can put back.
pub(crate) fn undoable(
    core: &Core,
    project: &str,
    goal: Option<u64>,
    decision: u64,
) -> Result<Overrule, String> {
    let key = overrule_key(project, goal, decision);
    let overrule = (core.chiefs().state.locked().overrules.get(&key).cloned())
        .ok_or("Only a decision you overruled can be put back")?;
    if overrule.stopped_auto {
        return Err("Overruling this stopped Auto's run on its slice and dropped what it did, so it cannot be put back".into());
    }
    Ok(overrule)
}

/// The person undid their Overrule and the decision stands again: the rule it kept is retired,
/// and the agents it told and the chief hear that it stands.
pub(crate) async fn overrule_undone(
    core: &Arc<Core>,
    project: &str,
    goal: Option<u64>,
    decision: (u64, &str),
    overrule: Overrule,
) {
    let (id, text) = decision;
    core.chiefs()
        .change(|saved| saved.overrules.remove(&overrule_key(project, goal, id)));
    if let Some(rule) = overrule.rule {
        if let Err(error) = super::memory::retire_persons_rule(core, project, rule) {
            eprintln!("sikemux core: an undone Overrule's rule was not retired: {error}");
        }
    }
    let said = first_sentence(text);
    let stands = format!(
        "The person undid their overrule of this decision, which an agent wrote as {said}. It stands again: go by it."
    );
    for agent in overrule.told {
        let why = "because the person undid their overrule of a decision you heard of";
        if let Err(error) =
            super::messages::work_from_sikemux(core, agent, stands.clone(), why).await
        {
            eprintln!("sikemux core: an agent did not hear an Overrule was undone: {error}");
        }
    }
    if let Some(chief) = chief_at(core, project) {
        drop_row(
            core,
            &chief,
            &format!("overrule|{}", overrule_key(project, goal, id)),
        );
        mark_away(core, &chief, goal, id, false);
        if let Some(feed) = core.chats().heard(&chief) {
            let undone = OverruledDecision {
                project: project.to_owned(),
                goal,
                decision: id,
                text: sentence(text),
                rule: overrule.rule,
                undone: true,
                stopped_auto: false,
            };
            let update = sikemux_fold::update::overruled_notice("Overrule undone", undone);
            emit_row(&feed, &chief, update, epoch_ms());
        }
        let named = match goal {
            Some(goal) => format!("decision {id} of goal #{goal}"),
            None => format!("decision {id} of the project"),
        };
        note(
            core,
            &chief,
            format!("The person undid their overrule of {named}, which an agent wrote as {said}. It stands again."),
        );
    }
}

/// When the week holding `at` began: Monday at 09:00 on this computer's clock.
fn week_began(at: u64) -> Option<u64> {
    let day = calendar(at, Zone::Host)?;
    let back = (day.tm_wday + 6) % 7;
    let nine = WallClock {
        zone: Zone::Host,
        date: None,
        hour: 9,
        minute: 0,
    };
    let began = nine.on_day(at, -back)?;
    match began > at {
        true => nine.on_day(at, -back - 7),
        false => Some(began),
    }
}

/// At the first tick after each Monday 09:00, every chief the person was with the week before
/// shows its count of that week in its chat, when something was decided or a call answered. It
/// starts no turn: the chief hears the numbers with its next one.
async fn count_weeks(core: Arc<Core>) {
    const TICK: std::time::Duration = std::time::Duration::from_secs(60);
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        prune(&core);
        let Some(began) = week_began(epoch_ms()) else {
            continue;
        };
        let due: Vec<(String, String, bool)> = (core.chiefs().state.locked().seats.iter())
            .filter(|(_, seat)| seat.counted < began)
            .map(|(chief, seat)| {
                let was_with = seat.with_weeks.contains(&(began - WEEK_MS));
                (chief.clone(), seat.project.clone(), was_with)
            })
            .collect();
        for (chief, project, was_with) in due {
            core.chiefs().seat(&chief, |seat| seat.counted = began);
            if was_with {
                count_week(&core, &chief, &project, began);
            }
        }
    }
}

fn count_week(core: &Core, chief: &str, project: &str, began: u64) {
    let Some(project) = (core.workspaces().projects().into_iter()).find(|info| info.id == project)
    else {
        return;
    };
    let path = project.path.to_string_lossy().into_owned();
    let from = began - WEEK_MS;
    let (decided, overruled) = super::board::goal::counted_since(core, &path, from, began);
    let answered = super::person_calls::answered_since(core, &path, from, began);
    let as_picked = answered.iter().filter(|as_picked| **as_picked).count();
    if decided == 0 && answered.is_empty() {
        return;
    }
    let week = ChiefWeek {
        from,
        to: began,
        decided: decided as u32,
        overruled: overruled as u32,
        answered: answered.len() as u32,
        as_picked: as_picked as u32,
    };
    put_row(
        core,
        chief,
        "week".to_owned(),
        sikemux_fold::update::week_notice("The week's count", week),
    );
    note(
        core,
        chief,
        format!(
            "The person was shown your count of last week: you decided {decided} things and they overruled {overruled}; they answered {} calls, {as_picked} as the asking agent picked.",
            answered.len()
        ),
    );
}

/// Lets go of what is kept for a chief that is no longer one, and of Overrules in projects gone.
fn prune(core: &Core) {
    let chiefs = core.workspaces().chiefs();
    let paths: HashSet<String> = (core.workspaces().projects().into_iter())
        .map(|project| project.path.to_string_lossy().into_owned())
        .collect();
    let stale = {
        let saved = core.chiefs().state.locked();
        saved.seats.keys().any(|chief| !chiefs.contains(chief))
            || (saved.overrules.keys()).any(|key| {
                !key.split('|')
                    .next()
                    .is_some_and(|path| paths.contains(path))
            })
    };
    if stale {
        core.chiefs().change(|saved| {
            saved.seats.retain(|chief, _| chiefs.contains(chief));
            (saved.overrules).retain(|key, _| {
                key.split('|')
                    .next()
                    .is_some_and(|path| paths.contains(path))
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{can_start_text, StartsOn};

    #[test]
    fn an_account_named_by_an_address_is_called_by_its_place() {
        let claude = StartsOn {
            kind: "claude".into(),
            models: vec!["Opus 5.5".into(), "Haiku 5.5".into()],
            accounts: vec![
                (Some("Work".into()), "62% left this week".into()),
                (Some("ada@example.com".into()), "8% left this week".into()),
            ],
        };
        let text = can_start_text(&[claude]).expect("a line");
        assert_eq!(
            text,
            "Agents you can start workers on here: claude: Work 62% left this week, account 2 8% left this week; models Opus 5.5, Haiku 5.5"
        );
    }
}
