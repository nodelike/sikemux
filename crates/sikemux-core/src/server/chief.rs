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
use crate::server::workspace_state::{WorkspaceCore, WorkspaceParts};
pub use sikemux_calls::chief::{OpenChiefParams, OpenedChief};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::agent_accounts::status::AgentStatus;
use crate::launch::providers::Kind as ProviderKind;
use crate::server::activity_log::Prices;
use crate::sessions::asked::may_hold_secret;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallOpened, CallState, PersonCall};
use sikemux_view::chat::{
    ChiefWeek, ClosedCard, DecidedLine, OverruledDecision, RuleChange, WorkerPage,
};
use sikemux_view::slice::Decision;
use sikemux_view::turn::TurnPhase;
use sikemux_view::workspace::ProjectInfo;
use sikemux_wire::protocol::ChatContext;

use super::chat::feed::Feed;
use super::chat::reset::{calendar, WallClock, Zone};
use super::chat::{Chat, ChatCommand, Whose};
use super::kept::Kept;
use super::launching::{self, Launch};
use super::{Core, CoreResult};

const FILE: &str = "chiefs.json";
/// Being with the chief again this soon after the last time is not written down again.
const WITH_STEP_MS: u64 = 60_000;
/// What a turn's digest holds at most, so it stays a few hundred tokens with twenty agents.
const DIGEST_CHARS: usize = 4_000;
/// What of one agent's or the person's words a digest line quotes.
const LONGEST_QUOTE: usize = 200;
/// Sikemux's own notes kept for a chief at most, newest kept.
const MOST_NOTES: usize = 20;
/// Chats noted as their agents went to sleep, and lines kept for the chief's chat, at most.
const MOST_FEEDS: usize = 40;
const MOST_ROWS: usize = 20;
const MOST_OPENED: usize = 4;

/// How the chief works, ahead of the project's rules in its instructions.
pub(crate) const SEAT: &str = "You are @Chief, this project's chief in Sikemux. The person talks to you; every other agent in the project works under you. Your job is to decide and route, not to do the work yourself.

- Decide and record, don't ask. Anything inside the machine is yours: who works on what, on which model, in what order, how to check it, landing by the project's rule, deploys to places that are not production. Record each decision with decide, with what it rests on, so the person can overrule it.
- Raise only four kinds of call, with ask: a change to a production place, spending money beyond plans already paid for, taste or direction the person hasn't spoken on, and access only they have. Word each in what the person asked for, give your pick and the strongest point against it, and name where to look as an @pill or a link, never a commit id, a host name as a verb or a workaround as a step. If it cannot be worded that way, it is not theirs: decide it.
- Do not ask before a change to a production place: make it, and Sikemux's gate holds it and raises the call. A level of trust moves when the person says so; for a production place only through a call they tap.
- Send work in a standing role's area to that hire with agent_send instead of spawning a new worker; hire one with agent_hire when an area will need someone for weeks.
- Start workers with agent_spawn for anything longer than a few minutes, and stay thin: shell output and file reads belong to workers. Their reports reach you; read more with agent_read only when you need it.
- A worker's turn that ends waiting on something set to wake it, such as a trigger, a run or a land in line, sends you nothing, and agent_list says what it waits on. A turn that fails, asks something, stops, or a land sent back reaches you at once. A turn that finished calmly, or a land that went in, is one line with your next turn, or wakes you after 5 minutes, at once when no other worker of yours is open. When a turn a worker's message started changes nothing for the person, end it without writing anything.
- A bug the person reports starts its fix in the same turn, without a go. A design question gets your one-line take and a Fable consult started in that turn.
- Send a bug to the worker that already holds that area with a built checkout, when one does, rather than starting a new one.
- A bug the person reports a second time is fixed with a check that reproduces their report, and the fix names each earlier one and why it missed.
- End every failure report with its cause in Sikemux and who has it, or \"agent slip, no change\".
- Pick for each worker the cheapest agent and model that will do its job well, a connected provider's included, and keep work off an account near its limit.
- When a smaller change or a deletion solves the problem, choose it.
- Sikemux offers on the Playbook to move a rule between the person's own and the project's; never move one yourself or ask about it.
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
    /// Lines the core keeps in agents' own chats, by agent: each decision it recorded itself,
    /// as decided or as overruled.
    #[serde(default)]
    lines: HashMap<String, Vec<Row>>,
    /// The last page each agent showed, and when, which a call about its work links.
    #[serde(default)]
    last_pages: HashMap<String, (WorkerPage, u64)>,
    /// The files and diffs each agent opened lately, newest last, with when.
    #[serde(default)]
    last_opened: HashMap<String, Vec<(CallOpened, u64)>>,
}

/// What the core keeps for one chief.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Seat {
    /// The project, by id.
    project: String,
    /// When the person was last with it, with their window in front.
    with: u64,
    /// When the person last wrote to it.
    #[serde(default)]
    wrote_at: u64,
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
    /// Lines of its chat the core keeps until the person has seen them or they are done with,
    /// put in its chat again when it opens on another feed.
    rows: Vec<Row>,
    /// When the last week it counted began.
    counted: u64,
    /// What it was last told it can start workers on, and when.
    #[serde(default)]
    can_start: Option<(String, u64)>,
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

/// When the person last wrote to the chief, if ever.
pub(crate) fn person_wrote_at(core: &Core, chief: &str) -> Option<u64> {
    (core.chiefs().read(chief, |seat| seat.wrote_at)).filter(|at| *at > 0)
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
pub(crate) fn chief_over(core: &Core, agent_id: &str) -> Option<String> {
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
    if let Some(home) = &project.chief_home {
        return Err(format!(
            "@Chief of this project lives on {}; open it there",
            home.host_name
        )
        .into());
    }
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

/// Opens the project's chief here after its home host was lost, with `standing`, what its home
/// last sent of where it stood, as its first message in place of the greeting.
pub(crate) async fn take_over(
    core: &Arc<Core>,
    project: &ProjectInfo,
    standing: Vec<ChatContext>,
) -> CoreResult<String> {
    let chief = project
        .chief
        .clone()
        .ok_or("the project has no chief here")?;
    ensure_seat(core, &chief, &project.id);
    core.chiefs().seat(&chief, |seat| seat.greeted = true);
    let agent_id = seat(core, project).await?;
    let ask = ChatCommand::Prompt {
        message_id: None,
        text: String::new(),
        paths: Vec::new(),
        context: standing,
        shown: true,
        whose: Whose::NOT_PERSON,
    };
    core.chats()
        .running(&agent_id)
        .and_then(|chat| chat.send(ask))?;
    Ok(agent_id)
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
            let heard = (project.identity.as_deref())
                .map_or(0, |identity| core.hosts().copies.chief_count(identity));
            let count = project.chief_count.max(heard) + 1;
            core.workspaces().set_chief(&project.id, &chief, count)?;
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
        if open.is_empty() {
            "- none".to_owned()
        } else {
            open.join("\n")
        }
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

/// The person wrote to `agent_id` themselves. Written to a chief, they are with it; written to
/// another agent, its chief's next digest reads that chat.
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
    core.chiefs().seat(agent_id, |seat| seat.wrote_at = now);
    core.hosts().copies.wake.now();
    let moved = core.chiefs().read(agent_id, |seat| {
        now.saturating_sub(seat.with) >= WITH_STEP_MS
    });
    if moved == Some(true) {
        core.chiefs().seat(agent_id, |seat| be_with(seat, now));
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
        let Some(seat) = saved.seats.get_mut(chief) else {
            return;
        };
        seat.rows.retain(|kept| kept.key != row.key);
        seat.rows.push(row);
        while seat.rows.len() > MOST_ROWS {
            let first = |going: &dyn Fn(&Row) -> bool| seat.rows.iter().position(going);
            let gone = first(&|row| row.key == "week").unwrap_or(0);
            seat.rows.remove(gone);
        }
    });
    if let Some(feed) = feed {
        emit_row(&feed, chief, update, at);
    }
}

/// The chief's chat is ready and its saved history shown: the lines it keeps that this feed has
/// not had go at its end, each under the time it was first made.
pub(crate) fn chat_ready(core: &Core, chat: &Chat) {
    if *chat.saved_reading.borrow() {
        return;
    }
    rows_missing(core, chat.agent_id(), &chat.feed);
}

/// The lines kept for `agent` that `feed` has not had go at its end, each under the time it was
/// first made.
pub(crate) fn rows_missing(core: &Core, agent: &str, feed: &Arc<Feed>) {
    let id = feed.id().to_owned();
    let missing: Vec<(Value, u64)> = {
        let mut saved = core.chiefs().state.locked();
        let saved = &mut *saved;
        let seat_rows = (saved.seats.get_mut(agent)).map(|seat| &mut seat.rows);
        let lines = saved.lines.get_mut(agent);
        let missing: Vec<(Value, u64)> = (seat_rows.into_iter().chain(lines))
            .flat_map(|rows| rows.iter_mut())
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
        emit_row(feed, agent, update, at);
    }
}

/// The lines kept for `agent` that were put on feed `from` count as on `to`, which holds them.
pub(crate) fn feed_moved(core: &Core, agent: &str, from: &str, to: &str) {
    core.chiefs().change(|saved| {
        let seat_rows = (saved.seats.get_mut(agent)).map(|seat| &mut seat.rows);
        let lines = saved.lines.get_mut(agent);
        for row in (seat_rows.into_iter().chain(lines)).flat_map(|rows| rows.iter_mut()) {
            if row.feed.as_deref() == Some(from) {
                row.feed = Some(to.to_owned());
            }
        }
    });
}

fn emit_row(feed: &Arc<Feed>, agent: &str, mut update: Value, at: u64) {
    sikemux_fold::update::stamp_replayed(&mut update, Some(at), false);
    let session = (feed.start()).map_or_else(|| agent.to_owned(), |start| start.session_id);
    feed.emit(
        sikemux_fold::ChatEventKind::SessionUpdate,
        json!({ "sessionId": session, "update": update }),
    );
}

/// What the chief's turn starts with, for the agent only: what it can start workers on, when its
/// session has not been told that yet, then the facts since its last digest as one compact
/// block, left out when nothing happened. The person's words are read back from the chats'
/// records and sit apart from what agents wrote, which is always quoted and named as theirs.
/// The reading runs off the threads that serve chats.
pub(crate) async fn digest(core: &Arc<Core>, chief: &str) -> Vec<ChatContext> {
    if !is_chief(core, chief) {
        return Vec::new();
    }
    let (core, chief) = (core.clone(), chief.to_owned());
    super::connection::blocking(move || {
        let digest = digest_now(&core, &chief);
        Ok((can_start_untold(&core, &chief).into_iter().flatten())
            .chain(standing_roles(&core, &chief))
            .chain(digest)
            .collect())
    })
    .await
    .unwrap_or_default()
}

/// [`can_start`], unless the chief's session was told the same since it last lost what it was
/// told, to a compaction or a fresh session.
fn can_start_untold(core: &Core, chief: &str) -> Option<Vec<ChatContext>> {
    let contexts = can_start(core);
    let text: String = contexts
        .iter()
        .map(|context| context.text.as_str())
        .collect();
    let forgot = (core.chats().get(chief)).map_or(0, |chat| chat.meter.forgot_at());
    let now = epoch_ms();
    let told = core.chiefs().seat(chief, |seat| {
        let told = (seat.can_start.as_ref()).is_some_and(|(was, at)| *was == text && *at > forgot);
        seat.can_start = Some((text, now));
        told
    });
    (told != Some(true)).then_some(contexts)
}

/// What the chief can start workers on here: the model providers connected, each with the
/// agents that run on it and its models, then the agents signed in and the models each last
/// offered, with what each model costs where that is known. Read from memory, with no model or
/// provider asked. Accounts, keys and limits stay out: the core picks the account, moves chats
/// off a full one and resumes them by itself.
pub(crate) fn can_start(core: &Core) -> Vec<ChatContext> {
    let mut kinds: Vec<String> = Vec::new();
    for launcher in core.workspaces().chat_launchers() {
        if matches!(launcher.status, Some(AgentStatus::Ready { .. }))
            && !kinds.contains(&launcher.agent.provider)
        {
            kinds.push(launcher.agent.provider.clone());
        }
    }
    if kinds.is_empty() {
        return Vec::new();
    }
    let prices = core.activity_log().prices();
    let priced = |model: &str| match prices.per_million(model) {
        Some((input, output)) => format!("{model} ${}/${}", dollars(input), dollars(output)),
        None => model.to_owned(),
    };
    let connected: Vec<(String, Vec<String>)> = (super::model_providers::on_hand(core).into_iter())
        .filter(|provider| provider.agents.iter().any(|agent| kinds.contains(agent)))
        .map(|provider| {
            let models = (provider.models.iter())
                .map(|model| match provider.kind {
                    ProviderKind::Api => priced(model),
                    ProviderKind::Plan | ProviderKind::Local => model.clone(),
                })
                .collect();
            let agents: Vec<&str> = (provider.agents.iter())
                .filter(|agent| kinds.contains(agent))
                .map(String::as_str)
                .collect();
            let paid = match provider.kind {
                ProviderKind::Api => "",
                ProviderKind::Plan => ", a plan with nothing more per token",
                ProviderKind::Local => ", on this computer for free",
            };
            (
                format!("{} (for {}{paid})", provider.id, agents.join(", ")),
                models,
            )
        })
        .collect();
    let agents: Vec<(String, Vec<String>)> = (kinds.into_iter())
        .map(|kind| {
            let on_plan = crate::agent_catalog::agent(&kind).is_some_and(|agent| agent.accounts);
            let models = offered_models(core, &kind, on_plan, &prices);
            match on_plan {
                true => (
                    format!("{kind} (on a plan, nothing more per token)"),
                    models,
                ),
                false => (kind, models),
            }
        })
        .collect();
    let providers = fitted(
        "Model providers connected here, ahead of the agents' own catalogs; start a worker on one \
         with agent_spawn's cli, provider and model. Prices are USD per million tokens in/out: ",
        &connected,
        "models",
    )
    .map(|text| {
        crate::sessions::for_agent::sikemux_context(
            "model-providers",
            "Model providers you can pay workers with",
            &text,
        )
    });
    let agents = fitted("Agents you can start workers on here: ", &agents, "agents").map(|text| {
        crate::sessions::for_agent::sikemux_context(
            "can-start",
            "What you can start workers on",
            &text,
        )
    });
    providers.into_iter().chain(agents).collect()
}

fn dollars(per_million: f64) -> String {
    let cents = (per_million * 100.0).round() / 100.0;
    format!("{cents}")
}

/// The agents of the chief's project hired for a standing role, each with what it covers.
fn standing_roles(core: &Core, chief: &str) -> Option<ChatContext> {
    let project = core.names().get(chief)?.project;
    let open = core.open_agents();
    let roles: Vec<String> = (core.names().roles_in(&project).into_iter())
        .filter(|(agent, _, _)| open.contains(agent))
        .map(|(_, name, role)| format!("@{name}: {role}"))
        .collect();
    if roles.is_empty() {
        return None;
    }
    Some(crate::sessions::for_agent::sikemux_context(
        "standing-roles",
        "Standing roles",
        &crate::text::clipped(&roles.join("; "), CAN_START_CHARS),
    ))
}

const CAN_START_CHARS: usize = 600;
const CAN_START_SHOWN: usize = 3;

/// The models `kind` offers. An agent on its own plan keeps its picker's order; one paid per
/// token puts its default first, then the cheapest, each with its price.
fn offered_models(core: &Core, kind: &str, on_plan: bool, prices: &Prices) -> Vec<String> {
    let mut choices: Vec<_> = (super::chat::models_of(core, kind).into_iter())
        .flat_map(|picker| picker.choices)
        .filter(|choice| {
            !choice.superseded && !crate::launch::providers::routed_model(&choice.value)
        })
        .map(|choice| {
            let price = (!on_plan)
                .then(|| prices.per_million(&choice.value))
                .flatten();
            (choice, price)
        })
        .collect();
    if !on_plan {
        let recent = core.workspaces().recent_models(kind);
        let recency = |value: &str| {
            (recent.iter().position(|used| used.model == value)).unwrap_or(recent.len())
        };
        choices.sort_by(|(a, a_price), (b, b_price)| {
            let cost = |price: &Option<(f64, f64)>| price.map(|(input, output)| input + output);
            (recency(&a.value).cmp(&recency(&b.value)))
                .then_with(|| b.is_default.cmp(&a.is_default))
                .then_with(|| match (cost(a_price), cost(b_price)) {
                    (Some(a), Some(b)) => a.total_cmp(&b),
                    (a, b) => b.is_some().cmp(&a.is_some()),
                })
        });
    }
    (choices.into_iter())
        .map(|(choice, price)| {
            let priced = match price {
                Some((input, output)) => {
                    format!("{} ${}/${}", choice.value, dollars(input), dollars(output))
                }
                None => choice.value.clone(),
            };
            match choice.label == choice.value {
                true => priced,
                false => format!("{priced} ({})", choice.label),
            }
        })
        .collect()
}

/// `intro` then each of `entries` with its models, within [`CAN_START_CHARS`]: models past the
/// first few go first, then the last entries, counted as `more`.
fn fitted(intro: &str, entries: &[(String, Vec<String>)], more: &str) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let line = |models: usize, kept: usize| {
        let mut parts: Vec<String> = (entries.iter().take(kept))
            .map(|(name, offered)| {
                if offered.is_empty() {
                    return name.clone();
                }
                let mut named: Vec<String> = offered.iter().take(models).cloned().collect();
                if offered.len() > models {
                    named.push(format!("+{} more", offered.len() - models));
                }
                format!("{name}; models {}", named.join(", "))
            })
            .collect();
        if entries.len() > kept {
            parts.push(format!("+{} more {more}", entries.len() - kept));
        }
        format!("{intro}{}", parts.join(" · "))
    };
    let fits = |text: &String| text.chars().count() <= CAN_START_CHARS;
    let mut text = line(usize::MAX, entries.len());
    if !fits(&text) {
        text = line(CAN_START_SHOWN, entries.len());
    }
    let mut kept = entries.len();
    while !fits(&text) && kept > 1 {
        kept -= 1;
        text = line(CAN_START_SHOWN, kept);
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
        let reported =
            super::messages::reported_at(core, agent, chief).is_some_and(|at| at >= turn.since);
        if turn.state == TurnPhase::Working || reported {
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
    person_lines.extend(typed_since(
        core,
        chief,
        &wrote,
        feeds,
        since,
        &quoted_answers,
    ));
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
    chief: &str,
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
        for said in super::why::persons_said_in(core, feed) {
            let Some(at) = said.at.filter(|at| *at >= since) else {
                continue;
            };
            heard.insert(agent.clone());
            if said.command {
                continue;
            }
            let line = match said.words {
                Some(words) if quoted_answers.contains(&(agent.clone(), words.clone())) => format!(
                    "- wrote to {}, the words that answered its call",
                    called(core, agent)
                ),
                Some(words) => format!(
                    "- wrote to {}: {}{}",
                    called(core, agent),
                    quoted(&words),
                    mentioned(core, chief, &said.mentions)
                ),
                None => format!("- wrote to {}", called(core, agent)),
            };
            lines.push((at, line));
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

/// Who each `@` mention in the person's words links to, as `chief` should read it: itself, or
/// another agent, named with its project when that is not the chief's.
fn mentioned(core: &Core, chief: &str, mentions: &[(String, String)]) -> String {
    if mentions.is_empty() {
        return String::new();
    }
    let project = |agent: &str| core.workspaces().agent(agent).map(|agent| agent.project);
    let links: Vec<String> = (mentions.iter())
        .map(|(title, agent)| match agent == chief {
            true => format!("{title} is you"),
            false if project(agent) != project(chief) => {
                let elsewhere = (project(agent).and_then(|id| core.workspaces().project_info(&id)))
                    .map_or_else(|| "another project".to_owned(), |info| info.name);
                format!("{title} is {agent} of {elsewhere}, not you")
            }
            false => format!("{title} is {agent}"),
        })
        .collect();
    format!(" ({})", links.join("; "))
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

/// A client put chats on screen: the person is with a chief brought to the front, and the lines
/// kept for its chat until seen are done with.
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
        let moved = core
            .chiefs()
            .read(chief, |seat| now.saturating_sub(seat.with) >= WITH_STEP_MS);
        if moved == Some(true) {
            core.chiefs().seat(chief, |seat| be_with(seat, now));
        }
    }
}

/// `text` an agent wrote, as Sikemux quotes it in a rule or a line: one sentence of it, on one
/// line, cut, in quotes.
fn first_sentence(text: &str) -> String {
    quoted(&sentence(text))
}

/// The first sentence of `text`, on one line, cut.
fn sentence(text: &str) -> String {
    let line = crate::text::one_line(text);
    let end = line
        .char_indices()
        .find(|(at, c)| {
            matches!(c, '.' | '?' | '!')
                && line.get(at + 1..).is_some_and(|rest| rest.starts_with(' '))
        })
        .map_or(line.len(), |(at, _)| at + 1);
    crate::text::clipped(line.get(..end).unwrap_or(&line), LONGEST_QUOTE)
}

/// The person overruled decision `decision` of `project` (a path), which `told` heard of: a
/// rule now brings decisions like it to them, the chief's chat says so on one line with Undo,
/// and the chief hears it with its next turn.
pub(crate) fn overruled(
    core: &Core,
    project: &str,
    goal: Option<u64>,
    decision: (u64, &str),
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
        let overrule = Overrule { rule, told };
        saved
            .overrules
            .insert(overrule_key(project, goal, id), overrule);
    });
    let title = match rule {
        Some(_) => "Overruled. Decisions like this come to you from now on",
        None => "Overruled",
    };
    let (org, waiting) =
        (rule.map(|rule| super::memory::kept_for_org(core, project, rule))).unwrap_or_default();
    let shown = OverruledDecision {
        project: project.to_owned(),
        goal,
        decision: id,
        text: sentence(text),
        rule,
        org,
        waiting,
        undone: false,
        stopped_auto: false,
    };
    let update = sikemux_fold::update::overruled_notice(title, shown);
    decided_line_moved(core, project, goal, id, &update);
    let Some(chief) = chief_at(core, project) else {
        return;
    };
    put_row(
        core,
        &chief,
        format!("overrule|{}", overrule_key(project, goal, id)),
        update,
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

/// The board closed card `card` of `project` (a path) by itself, as the project's check passed on
/// `commit`: the chief's chat says so on one line with Undo.
pub(crate) fn card_closed(core: &Core, project: &str, card: u64, title: &str, commit: &str) {
    let Some(chief) = chief_at(core, project) else {
        return;
    };
    let line = ClosedCard {
        project: project.to_owned(),
        card,
        title: title.to_owned(),
        commit: commit.to_owned(),
        reopened: false,
    };
    let update = sikemux_fold::update::closed_card_notice("Closed by itself", line);
    put_row(core, &chief, closed_key(project, card), update);
}

/// The person reopened a card the board closed by itself: its line says so and offers no Undo.
pub(crate) fn card_reopened(core: &Core, project: &str, card: u64) {
    let Some(chief) = chief_at(core, project) else {
        return;
    };
    let key = closed_key(project, card);
    let line = core.chiefs().read(&chief, |seat| {
        let row = seat.rows.iter().find(|row| row.key == key)?;
        serde_json::from_value::<ClosedCard>(row.update.get("closedCard")?.clone()).ok()
    });
    let Some(mut line) = line.flatten() else {
        return;
    };
    line.reopened = true;
    let update = sikemux_fold::update::closed_card_notice("Closed by itself, then reopened", line);
    put_row(core, &chief, key, update);
}

fn closed_key(project: &str, card: u64) -> String {
    format!("closed|{project}|{card}")
}

/// An agent recorded `decision` of `project` (a path) itself: a line under its reply in its own
/// chat says so, with Overrule, kept until the line is pushed out by later ones. A decision whose
/// words may hold a secret gets no line.
pub(crate) fn decided(core: &Core, project: &str, goal: Option<u64>, decision: &Decision) {
    let Some(agent) = decision.learned.agent.as_deref() else {
        return;
    };
    let said = std::iter::once(&decision.text).chain(&decision.why);
    if decision.holder.is_none() || said.into_iter().any(|words| may_hold_secret(words)) {
        return;
    }
    let line = DecidedLine {
        project: project.to_owned(),
        goal,
        decision: decision.id,
        text: sentence(&decision.text),
        why: decision.why.as_deref().map(sentence),
    };
    let feed = core.chats().heard(agent);
    let row = Row {
        key: decided_key(project, goal, decision.id),
        update: sikemux_fold::update::decided_notice(line),
        at: epoch_ms(),
        feed: feed.as_ref().map(|feed| feed.id().to_owned()),
    };
    core.chiefs().change(|saved| {
        let lines = saved.lines.entry(agent.to_owned()).or_default();
        lines.push(row.clone());
        if lines.len() > MOST_ROWS {
            lines.remove(0);
        }
    });
    if let Some(feed) = feed {
        emit_row(&feed, agent, row.update, row.at);
    }
}

/// The decided line of decision `id` of `project`, in the chat of the agent that recorded it,
/// says what `update` does now: overruled, or that Undo put it back.
fn decided_line_moved(core: &Core, project: &str, goal: Option<u64>, id: u64, update: &Value) {
    let key = decided_key(project, goal, id);
    let moved: Vec<(String, u64, Option<Arc<Feed>>)> = core.chiefs().change(|saved| {
        (saved.lines.iter_mut())
            .filter_map(|(agent, lines)| {
                let line = lines.iter_mut().find(|line| line.key == key)?;
                let feed = core.chats().heard(agent);
                line.update = update.clone();
                line.feed = feed.as_ref().map(|feed| feed.id().to_owned());
                Some((agent.clone(), line.at, feed))
            })
            .collect()
    });
    for (agent, at, feed) in moved {
        if let Some(feed) = feed {
            emit_row(&feed, &agent, update.clone(), at);
        }
    }
}

/// `page` was shown: a row in the chat of the agent that started its worker names it, kept with
/// that chat's other lines, in place of the row for a page it showed before under the same title.
pub(crate) fn page_shown(core: &Core, lead: Option<&str>, page: WorkerPage) {
    let at = epoch_ms();
    let feed = lead.and_then(|lead| core.chats().heard(lead));
    let key = format!("page|{}|{}", page.agent_id, page.title);
    let update = sikemux_fold::update::worker_page_notice(page.clone());
    core.chiefs().change(|saved| {
        (saved.last_pages).insert(page.agent_id.clone(), (page, at));
        let Some(lead) = lead else {
            return;
        };
        let lines = saved.lines.entry(lead.to_owned()).or_default();
        let first = (lines.iter().position(|row| row.key == key)).map(|at| lines.remove(at).at);
        lines.push(Row {
            key,
            update: update.clone(),
            at: first.unwrap_or(at),
            feed: feed.as_ref().map(|feed| feed.id().to_owned()),
        });
        if lines.len() > MOST_ROWS {
            lines.remove(0);
        }
    });
    if let Some((feed, lead)) = feed.zip(lead) {
        emit_row(&feed, lead, update, at);
    }
}

/// The page `agent` showed last, if it did in the last `within` ms.
pub(crate) fn last_page(core: &Core, agent: &str, within: u64) -> Option<WorkerPage> {
    let now = epoch_ms();
    let state = core.chiefs().state.locked();
    let (page, at) = state.last_pages.get(agent)?;
    (now.saturating_sub(*at) <= within).then(|| page.clone())
}

/// `agent` opened `opened` for the person: kept, in place of the same file or diff opened before.
pub(crate) fn opened(core: &Core, agent: &str, opened: CallOpened) {
    let at = epoch_ms();
    core.chiefs().change(|saved| {
        let kept = saved.last_opened.entry(agent.to_owned()).or_default();
        kept.retain(|(was, _)| !same_place(was, &opened));
        kept.push((opened, at));
        if kept.len() > MOST_OPENED {
            kept.remove(0);
        }
    });
}

fn same_place(a: &CallOpened, b: &CallOpened) -> bool {
    match (a, b) {
        (CallOpened::File { path: a, .. }, CallOpened::File { path: b, .. }) => a == b,
        (CallOpened::Diff { checkout: a, .. }, CallOpened::Diff { checkout: b, .. }) => a == b,
        _ => false,
    }
}

/// What `agent` opened in the last `within` ms, newest first.
pub(crate) fn opened_lately(core: &Core, agent: &str, within: u64) -> Vec<CallOpened> {
    let now = epoch_ms();
    let state = core.chiefs().state.locked();
    (state.last_opened.get(agent).into_iter().flatten().rev())
        .filter(|(_, at)| now.saturating_sub(*at) <= within)
        .map(|(opened, _)| opened.clone())
        .collect()
}

fn decided_key(project: &str, goal: Option<u64>, decision: u64) -> String {
    format!("decided|{}", overrule_key(project, goal, decision))
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

/// The person moved a rule between their own store and the project's: one line with Undo in the
/// chief's chat says so.
pub(crate) fn rule_moved(core: &Core, line: RuleChange) {
    if let Some(chief) = chief_at(core, &line.project) {
        let key = rule_key(&line);
        put_row(
            core,
            &chief,
            key,
            sikemux_fold::update::rule_change_notice("Rule moved", line),
        );
    }
}

fn rule_line(core: &Core, line: RuleChange, title: &str, agent: &str) {
    let key = rule_key(&line);
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
                .filter_map(|row| {
                    serde_json::from_value(row.update.get("ruleChange")?.clone()).ok()
                })
                .filter(|line| picked(line))
                .collect::<Vec<RuleChange>>()
        });
        for mut line in lines.unwrap_or_default() {
            line.restored = true;
            let key = rule_key(&line);
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

fn rule_key(line: &RuleChange) -> String {
    let moved = if line.moved { "|moved" } else { "" };
    format!("rule|{}|{}{moved}", line.project, line.rule)
}

/// The Overrule of a decision, when the person made one that Undo can put back.
pub(crate) fn undoable(
    core: &Core,
    project: &str,
    goal: Option<u64>,
    decision: u64,
) -> Result<Overrule, String> {
    let key = overrule_key(project, goal, decision);
    (core.chiefs().state.locked().overrules.get(&key).cloned())
        .ok_or_else(|| "Only a decision you overruled can be put back".to_owned())
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
    let undone = OverruledDecision {
        project: project.to_owned(),
        goal,
        decision: id,
        text: sentence(text),
        rule: overrule.rule,
        org: None,
        waiting: false,
        undone: true,
        stopped_auto: false,
    };
    let update = sikemux_fold::update::overruled_notice("Overrule undone", undone);
    decided_line_moved(core, project, goal, id, &update);
    if let Some(chief) = chief_at(core, project) {
        put_row(
            core,
            &chief,
            format!("overrule|{}", overrule_key(project, goal, id)),
            update,
        );
        let named = match goal {
            Some(goal) => format!("decision {id} of goal #{goal}"),
            None => format!("decision {id} of the project"),
        };
        note(
            core,
            &chief,
            format!(
                "The person undid their overrule of {named}, which an agent wrote as {said}. It stands again."
            ),
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

/// When the week before the one that began at `began` began, which is not seven days earlier
/// when the clocks changed in it.
fn week_before(began: u64) -> Option<u64> {
    week_began(began.checked_sub(1)?)
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
        let Some((before, began)) =
            week_began(epoch_ms()).and_then(|began| Some((week_before(began)?, began)))
        else {
            continue;
        };
        let due: Vec<(String, String, bool)> = (core.chiefs().state.locked().seats.iter())
            .filter(|(_, seat)| seat.counted < began)
            .map(|(chief, seat)| {
                let was_with = seat.with_weeks.contains(&before);
                (chief.clone(), seat.project.clone(), was_with)
            })
            .collect();
        for (chief, project, was_with) in due {
            core.chiefs().seat(&chief, |seat| seat.counted = began);
            if was_with {
                count_week(&core, &chief, &project, before, began);
            }
        }
    }
}

fn count_week(core: &Core, chief: &str, project: &str, from: u64, began: u64) {
    let Some(project) = (core.workspaces().projects().into_iter()).find(|info| info.id == project)
    else {
        return;
    };
    let path = project.path.to_string_lossy().into_owned();
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

/// Lets go of what is kept for a chief that is no longer one, of Overrules in projects gone, and of
/// the lines kept for agents the workspace no longer keeps.
fn prune(core: &Core) {
    let chiefs = core.workspaces().chiefs();
    let agents = core.workspaces().agent_ids();
    core.chats().forget_gone(|agent| agents.contains(agent));
    let reopened = |agent: &String| agents.contains(agent) || core.chats().shelf.holds(agent);
    let paths: HashSet<String> = (core.workspaces().projects().into_iter())
        .map(|project| project.path.to_string_lossy().into_owned())
        .collect();
    let in_project = |key: &String| {
        key.split('|')
            .next()
            .is_some_and(|path| paths.contains(path))
    };
    let stale = {
        let saved = core.chiefs().state.locked();
        saved.seats.keys().any(|chief| !chiefs.contains(chief))
            || saved.overrules.keys().any(|key| !in_project(key))
            || saved.lines.keys().any(|agent| !reopened(agent))
    };
    if stale {
        core.chiefs().change(|saved| {
            saved.seats.retain(|chief, _| chiefs.contains(chief));
            saved.overrules.retain(|key, _| in_project(key));
            saved.lines.retain(|agent, _| reopened(agent));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::week_before;

    #[test]
    fn the_week_before_a_clock_change_begins_at_nine_on_its_monday() {
        const ZONE: &str = if cfg!(windows) {
            "PST8PDT"
        } else {
            "America/Los_Angeles"
        };
        const NAME: &str = "the_week_before_a_clock_change_begins_at_nine_on_its_monday";
        if std::env::var("TZ").as_deref() != Ok(ZONE) {
            let module = module_path!().split_once("::").expect("in a crate").1;
            let ran = sikemux_process::user_environment::command(
                std::env::current_exe().expect("this test"),
            )
            .args([
                &format!("{module}::{NAME}"),
                "--exact",
                "--list",
                "--format=terse",
            ])
            .output()
            .expect("the tests list");
            assert!(
                String::from_utf8_lossy(&ran.stdout).contains(NAME),
                "the test cannot find itself"
            );
            let ran = sikemux_process::user_environment::command(
                std::env::current_exe().expect("this test"),
            )
            .args([&format!("{module}::{NAME}"), "--exact"])
            .env("TZ", ZONE)
            .status()
            .expect("the test runs in Los Angeles");
            assert!(ran.success());
            return;
        }
        let monday_nine_pdt = 1_793_030_400_000;
        let next_monday_nine_pst = 1_793_638_800_000;
        assert_eq!(week_before(next_monday_nine_pst), Some(monday_nine_pdt));
    }
}
