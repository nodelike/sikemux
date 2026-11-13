//! What `agent_list` and `agent_send` answer: the agents running here, and
//! messages between them. A message reaches an agent as text marked as coming
//! from the agent that sent it, so it can never answer a permission request.
//! A busy chat takes it into its turn when it can; an agent that cannot gets
//! the messages waiting for it together once its turn ends.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::messages::{AgentMessageRows, AgentMessagesParams, MessageKind};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::cli::protocol::HarnessRequest;

use self::log::{MessageEntry, MessageLog, MessageOutcome};
use super::chat::{self, Delivery};
use super::connection::blocking;
use super::kept::Kept;
use super::roster::{listed, resolve, Listed, Reach};
use super::session::{self, Session};
use super::wake::{sooner, Wake};
use super::{agent, launching, Core, CoreResult};

pub mod log;

const FILE: &str = "agent-mailbox.json";
/// The mark on the task an agent in a terminal waits to take.
pub(crate) const TASK: &str = "task";
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_WHY_BYTES: usize = 8 * 1024;
const MAX_WAITING: usize = 10;
const HOSTS_NOTE: &str =
    "Sent by Sikemux about a teammate on another host, not by the person or another agent.";
/// The same text to the same agent again within this long is dropped.
const REPEAT_WINDOW_MS: u64 = 2 * 60 * 1000;
/// Two agents answering each other forever stop here.
const PAIR_WINDOW_MS: u64 = 10 * 60 * 1000;
const PAIR_LIMIT: usize = 30;
/// Two agents that have exchanged this many messages, both ways, with
/// neither doing anything else in between, have their messages to each other
/// held for the person.
const LOOP_MESSAGES: usize = 4;
/// The messages held between one pair of agents.
const MAX_HELD: usize = 10;
/// The tools that only talk to other agents, so calling them is no other work.
const MESSAGING_TOOLS: [&str; 4] = ["agent_send", "agent_list", "agent_read", "agent_wait"];
/// How long an agent rests after a turn before a message starts another, so a
/// message the person queued in the app goes first.
const SETTLE_MS: u64 = 1500;
/// How long an agent rests after a turn before messages nobody waits on, such as a trigger's or a
/// teammate's report, start another, so those that come close together are read in one turn.
const AUTO_SETTLE_MS: u64 = 30_000;
/// Turns in a row an agent may start on messages nobody waits on before the rest wait for the
/// person to write to it.
const MAX_AUTO_TURNS: u32 = 20;
/// How long after waking an asleep chat failed before it is tried again.
const WAKE_RETRY_MS: u64 = 60_000;
/// A terminal agent takes the Enter that sends a pasted message only once it
/// has taken in the paste.
const ENTER_AFTER_PASTE: Duration = Duration::from_millis(150);
/// A terminal agent that keeps printing this long after the Enter that sends
/// a paste has started a turn on it.
const TURN_OUTPUT_MS: u64 = 1000;
/// Pastes a terminal agent may leave untaken in a row before Sikemux stops
/// typing into it until it starts a turn of its own.
const MAX_MISSED_PASTES: u8 = 3;

struct Letter {
    kind: MessageKind,
    from_id: String,
    from_name: String,
    cli: String,
    text: String,
    at: u64,
    /// What the agent reading it is told about it, when not the usual.
    note: Option<String>,
    /// The question this answers, when the person answered one.
    answers: Option<Answered>,
    /// The question it puts, when it puts one.
    puts: Option<String>,
    /// Files the person attached, which a chat takes as it takes the composer's.
    paths: Vec<String>,
    /// It waits for an agent that has stopped, as an answer does, rather than being refused.
    waits: bool,
}

/// A question the person answered, and the agent that asked it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Answered {
    pub question: String,
    pub asker: String,
    pub asker_id: String,
    pub cli: String,
    /// The answer is a secret the agent reads from a file, so the app shows
    /// that it was answered and never the text.
    pub secret: bool,
}

/// A letter as it crosses to the host its agent runs on, which hands it on.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Carried {
    pub to: String,
    pub kind: MessageKind,
    pub from_id: String,
    pub from_name: String,
    pub cli: String,
    pub text: String,
    pub note: Option<String>,
    /// The person's answer to a question, copied to the lead it went to first.
    pub answered: Option<Answered>,
    /// The question of its own that a lead answers.
    pub question: Option<Answering>,
}

/// A lead's answer to the question `id`, and why it decided so.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Answering {
    pub id: String,
    pub why: Option<String>,
}

/// Who a message comes from when no agent sent it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sender {
    Board,
    Trigger,
    Auto,
    Sikemux,
    /// Notes about teammates on linked hosts come from here.
    Hosts,
    Person,
}

impl Sender {
    const ALL: [Sender; 6] = [
        Sender::Board,
        Sender::Trigger,
        Sender::Auto,
        Sender::Sikemux,
        Sender::Hosts,
        Sender::Person,
    ];

    pub(crate) fn of(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sender| sender.id() == id)
    }

    fn id(self) -> &'static str {
        match self {
            Sender::Board => "board",
            Sender::Trigger => "trigger",
            Sender::Auto => "auto",
            Sender::Sikemux => "sikemux",
            Sender::Hosts => "hosts",
            Sender::Person => "person",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Sender::Board => "Board",
            Sender::Trigger => "Trigger",
            Sender::Auto => "Auto",
            Sender::Sikemux | Sender::Hosts => "Sikemux",
            Sender::Person => "the person",
        }
    }

    /// A part of Sikemux that carries work on: what it sends is no repeat, and does not count
    /// as reaching an agent the board offered a card to.
    fn carries_work_on(self) -> bool {
        matches!(self, Sender::Board | Sender::Trigger | Sender::Sikemux)
    }
}

impl Letter {
    fn by_sikemux(&self) -> bool {
        Sender::of(&self.from_id).is_some_and(Sender::carries_work_on)
    }

    fn from_sender(sender: Sender, kind: MessageKind, text: String, note: Option<String>) -> Self {
        Self {
            kind,
            from_id: sender.id().to_owned(),
            from_name: sender.name().to_owned(),
            cli: "sikemux".to_owned(),
            text,
            at: epoch_ms(),
            note,
            answers: None,
            puts: None,
            paths: Vec::new(),
            waits: false,
        }
    }

    fn carried(&self, to: &str, question: Option<Answering>) -> Carried {
        Carried {
            to: to.to_owned(),
            kind: self.kind,
            from_id: self.from_id.clone(),
            from_name: self.from_name.clone(),
            cli: self.cli.clone(),
            text: self.text.clone(),
            note: self.note.clone(),
            answered: self.answers.clone(),
            question,
        }
    }
}

/// The messages still waiting, as each agent reads them, kept in the data
/// folder.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailboxRecord {
    waiting: HashMap<String, Vec<Queued>>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Origin {
    from: String,
    kind: MessageKind,
}

/// A message waiting for its agent, as the agent reads it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Queued {
    text: String,
    /// Unknown for a message that came back unread.
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<Origin>,
    /// What [`withdraw`] and [`is_waiting`] know it by: the question it puts, or [`TASK`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mark: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    paths: Vec<String>,
}

/// Whether every one of `queued` came from the person, so what they say is the person's words.
fn by_person<'a>(queued: impl IntoIterator<Item = &'a Queued>) -> bool {
    let person = Sender::Person.id();
    queued
        .into_iter()
        .all(|queued| (queued.origin.as_ref()).is_some_and(|origin| origin.from == person))
}

/// Whether anyone waits on one of `queued`: the person's words, or an answer the agent asked for.
/// Those are read soon after a turn, and never wait for the person.
fn awaited<'a>(queued: impl IntoIterator<Item = &'a Queued>) -> bool {
    let person = Sender::Person.id();
    queued.into_iter().any(|queued| {
        queued
            .origin
            .as_ref()
            .is_none_or(|origin| origin.from == person || origin.kind == MessageKind::Answer)
    })
}

/// Whether `queued` counts toward [`MAX_AUTO_TURNS`] and waits once the agent reaches it: nobody
/// waits on it, and it is no report from one of the agent's own workers.
fn capped(core: &Core, agent_id: &str, queued: &Queued) -> bool {
    let own_report = queued.origin.as_ref().is_some_and(|origin| {
        origin.kind == MessageKind::Report
            && core
                .names()
                .get(&origin.from)
                .and_then(|named| named.parent)
                .as_deref()
                == Some(agent_id)
    });
    !own_report && !awaited([queued])
}

fn settle_ms<'a>(queued: impl IntoIterator<Item = &'a Queued>) -> u64 {
    match awaited(queued) {
        true => SETTLE_MS,
        false => AUTO_SETTLE_MS,
    }
}

fn joined(queued: &[Queued]) -> String {
    queued
        .iter()
        .map(|queued| queued.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

struct Sent {
    from: String,
    to: String,
    kind: MessageKind,
    digest: u64,
    at: u64,
}

/// A message held because its sender and the agent it is for keep messaging
/// each other and nothing else.
struct Held {
    from: String,
    to: String,
    queued: Queued,
    at: u64,
}

/// Two agents whose messages to each other are held, from when.
pub(crate) struct Loop {
    pub agents: (String, String),
    pub since: u64,
}

/// Messages typed into a terminal agent's prompt that it has not yet been
/// seen to take.
struct Pasted {
    texts: Vec<Queued>,
    /// When Enter was sent, once it was.
    entered_at: Option<u64>,
}

#[derive(Default)]
struct State {
    waiting: HashMap<String, Vec<Queued>>,
    sent: VecDeque<Sent>,
    delivering: HashSet<String>,
    wake_failed: HashMap<String, u64>,
    pasted: HashMap<String, Pasted>,
    /// How many pastes in a row each terminal agent did not take.
    missed_pastes: HashMap<String, u8>,
    /// When each agent last did something besides messaging other agents,
    /// within the pair window.
    worked: HashMap<String, u64>,
    held: Vec<Held>,
    /// How many turns each agent started in a row on messages nobody waits on, since the person
    /// last wrote to it, and when it reached [`MAX_AUTO_TURNS`].
    auto_turns: HashMap<String, AutoTurns>,
    /// Messages held for each agent stopped at [`MAX_AUTO_TURNS`], until the person writes to it.
    capped: HashMap<String, Vec<Queued>>,
}

#[derive(Default)]
struct AutoTurns {
    taken: u32,
    stopped_at: Option<u64>,
}

/// An agent whose messages wait for the person, as it took [`MAX_AUTO_TURNS`] turns on its own.
pub(crate) struct Stopped {
    pub agent_id: String,
    pub turns: u32,
    pub since: u64,
}

impl State {
    /// Whether `from` and `to` have messaged each other, both ways, enough
    /// times since either last did other work to count as a loop.
    fn looping(&self, from: &str, to: &str) -> bool {
        let since = [from, to]
            .iter()
            .filter_map(|agent| self.worked.get(*agent))
            .max()
            .copied()
            .unwrap_or(0);
        let exchanged: Vec<&Sent> = self
            .sent
            .iter()
            .filter(|sent| {
                sent.at > since
                    && sent.kind != MessageKind::Question
                    && ((sent.from == from && sent.to == to)
                        || (sent.from == to && sent.to == from))
            })
            .collect();
        exchanged.len() >= LOOP_MESSAGES && exchanged.iter().any(|sent| sent.from == to)
    }
}

impl State {
    /// What is kept: a paste not yet taken waits ahead of the rest.
    fn record(&self) -> MailboxRecord {
        let mut waiting = self.waiting.clone();
        for (agent_id, pasted) in &self.pasted {
            let queued = waiting.entry(agent_id.clone()).or_default();
            let mut texts = pasted.texts.clone();
            texts.append(queued);
            *queued = texts;
        }
        for (agent_id, capped) in &self.capped {
            waiting
                .entry(agent_id.clone())
                .or_default()
                .extend(capped.iter().cloned());
        }
        MailboxRecord { waiting }
    }
}

pub(crate) struct Mailbox {
    state: Mutex<State>,
    kept: Kept,
    pub(crate) log: MessageLog,
    /// Hands waiting messages on, when an agent comes free.
    pub(crate) wake: Wake,
}

enum Admitted {
    Yes,
    Repeat {
        seconds_ago: u64,
    },
    /// It is held, as one of a loop between two agents.
    Held,
}

impl Mailbox {
    /// Takes up the messages left waiting for agents `known` says still
    /// exist.
    pub(crate) fn new(data_dir: Option<&Path>, known: impl Fn(&str) -> bool) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let mut record: MailboxRecord = kept.load();
        record.waiting.retain(|agent_id, _| known(agent_id));
        Self {
            state: Mutex::new(State {
                waiting: record.waiting,
                ..State::default()
            }),
            kept,
            log: MessageLog::new(data_dir),
            wake: Wake::default(),
        }
    }

    /// Changes the messages waiting, and keeps what they are now.
    fn change<R>(&self, change: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.state.locked();
        let changed = change(&mut state);
        state.waiting.retain(|_, waiting| !waiting.is_empty());
        self.kept.save_soon(&state.record());
        drop(state);
        self.wake.now();
        changed
    }

    pub(super) fn flush(&self) {
        self.kept.flush();
    }

    fn admit(&self, letter: &Letter, to: &str, to_name: &str) -> Result<Admitted, String> {
        let mut state = self.state.locked();
        let now = letter.at;
        state
            .sent
            .retain(|sent| now.saturating_sub(sent.at) < PAIR_WINDOW_MS);
        let digest = digest(&wrapped(letter));
        let pair: Vec<&Sent> = state
            .sent
            .iter()
            .filter(|sent| sent.from == letter.from_id && sent.to == to)
            .collect();
        let repeat = pair
            .iter()
            .rev()
            .find(|sent| sent.digest == digest && now.saturating_sub(sent.at) < REPEAT_WINDOW_MS);
        // Answers, reports and questions each stand for an event of their own,
        // however alike they read, and so do a board note, a trigger's firing
        // and Sikemux's word of what the person allowed.
        if let Some(repeat) =
            repeat.filter(|_| letter.kind == MessageKind::Message && !letter.by_sikemux())
        {
            return Ok(Admitted::Repeat {
                seconds_ago: now.saturating_sub(repeat.at) / 1000,
            });
        }
        if letter.kind == MessageKind::Message && state.looping(&letter.from_id, to) {
            return Ok(Admitted::Held);
        }
        let report = letter.kind == MessageKind::Report;
        if pair.len() >= PAIR_LIMIT && !report {
            return Err(format!(
                "You have sent {to_name} {PAIR_LIMIT} messages in the last {} minutes, so Sikemux holds the rest; two agents must not keep each other busy without end. Wait, or ask the person.",
                PAIR_WINDOW_MS / 60_000
            ));
        }
        if !report && state.waiting.get(to).map_or(0, Vec::len) >= MAX_WAITING {
            return Err(format!(
                "{to_name} already has {MAX_WAITING} messages waiting for its turn to end; send more once it has read them"
            ));
        }
        Ok(Admitted::Yes)
    }

    fn note_sent(&self, letter: &Letter, to: &str) {
        self.state.locked().sent.push_back(Sent {
            from: letter.from_id.clone(),
            to: to.to_owned(),
            kind: letter.kind,
            digest: digest(&wrapped(letter)),
            at: letter.at,
        });
    }

    /// Holds a message of a loop, keeping the newest few each pair sent.
    /// Answers those it no longer keeps.
    fn hold(&self, from: &str, to: &str, queued: Queued) -> Vec<Queued> {
        let mut state = self.state.locked();
        let pair = |held: &Held| {
            (held.from == from && held.to == to) || (held.from == to && held.to == from)
        };
        let mut dropped = Vec::new();
        while state.held.iter().filter(|held| pair(held)).count() >= MAX_HELD {
            if let Some(oldest) = state.held.iter().position(&pair) {
                dropped.push(state.held.remove(oldest).queued);
            }
        }
        state.held.push(Held {
            from: from.to_owned(),
            to: to.to_owned(),
            queued,
            at: epoch_ms(),
        });
        dropped
    }

    pub(crate) fn loops(&self) -> Vec<Loop> {
        let state = self.state.locked();
        let mut loops: Vec<Loop> = Vec::new();
        for held in &state.held {
            let known = loops.iter().any(|known| {
                let (a, b) = &known.agents;
                (*a == held.from && *b == held.to) || (*a == held.to && *b == held.from)
            });
            if !known {
                loops.push(Loop {
                    agents: (held.from.clone(), held.to.clone()),
                    since: held.at,
                });
            }
        }
        loops
    }

    /// Whether the agent took [`MAX_AUTO_TURNS`] turns on its own, so only what someone waits on
    /// reaches it until the person writes to it.
    fn stopped(&self, agent_id: &str) -> bool {
        let state = self.state.locked();
        let turns = state.auto_turns.get(agent_id);
        turns.is_some_and(|turns| turns.stopped_at.is_some())
    }

    /// How long the agent rests after a turn before what waits for it goes.
    fn rest_for(&self, agent_id: &str) -> u64 {
        let state = self.state.locked();
        state.waiting.get(agent_id).map_or(SETTLE_MS, settle_ms)
    }

    /// Holds `queued` for the agent stopped at [`MAX_AUTO_TURNS`], until the person writes to it.
    fn cap(&self, agent_id: &str, queued: Queued) -> usize {
        self.change(|state| {
            let capped = state.capped.entry(agent_id.to_owned()).or_default();
            capped.push(queued);
            capped.len()
        })
    }

    /// The agents stopped after [`MAX_AUTO_TURNS`] turns of their own that have messages held.
    pub(crate) fn stopped_agents(&self) -> Vec<Stopped> {
        let state = self.state.locked();
        (state.auto_turns.iter())
            .filter(|(agent_id, _)| state.capped.contains_key(*agent_id))
            .filter_map(|(agent_id, turns)| {
                Some(Stopped {
                    agent_id: agent_id.clone(),
                    turns: turns.taken,
                    since: turns.stopped_at?,
                })
            })
            .collect()
    }

    pub(crate) fn has_waiting(&self, agent_id: &str) -> bool {
        self.state.locked().waiting.contains_key(agent_id)
    }

    fn missed_pastes(&self, agent_id: &str) -> u8 {
        self.state
            .locked()
            .missed_pastes
            .get(agent_id)
            .copied()
            .unwrap_or_default()
    }

    /// Answers how many wait now, and the earlier reports from the same
    /// worker this one replaces, so a lead holds only each worker's newest.
    fn wait(&self, agent_id: &str, queued: Queued) -> (usize, Vec<Queued>) {
        self.change(|state| {
            let waiting = state.waiting.entry(agent_id.to_owned()).or_default();
            let mut superseded = Vec::new();
            if let Some(Origin {
                from,
                kind: MessageKind::Report,
            }) = &queued.origin
            {
                let (older, kept) = std::mem::take(waiting).into_iter().partition(|waiting| {
                    waiting.origin.as_ref().is_some_and(|origin| {
                        origin.kind == MessageKind::Report && &origin.from == from
                    })
                });
                *waiting = kept;
                superseded = older;
            }
            waiting.push(queued);
            (waiting.len(), superseded)
        })
    }

    fn take(&self, agent_id: &str) -> Vec<Queued> {
        if !self.has_waiting(agent_id) {
            return Vec::new();
        }
        self.change(|state| state.waiting.remove(agent_id).unwrap_or_default())
    }

    /// Keeps `texts` waiting ahead of any that arrived meanwhile.
    fn put_back(&self, agent_id: &str, texts: Vec<Queued>) {
        self.change(|state| put_back(state, agent_id, texts));
    }

    /// A message the agent was handed but dropped unread waits for its next
    /// turn, ahead of the rest.
    pub(crate) fn returned(&self, agent_id: &str, text: String) {
        self.put_back(
            agent_id,
            vec![Queued {
                text,
                origin: None,
                mark: None,
                paths: Vec::new(),
            }],
        );
    }

    /// The task an agent in a terminal starts on waits for its CLI to take a prompt.
    pub(crate) fn task_waits(&self, agent_id: &str, text: String) {
        self.put_back(
            agent_id,
            vec![Queued {
                text,
                origin: None,
                mark: Some(TASK.to_owned()),
                paths: Vec::new(),
            }],
        );
    }
}

fn put_back(state: &mut State, agent_id: &str, mut texts: Vec<Queued>) {
    let waiting = state.waiting.entry(agent_id.to_owned()).or_default();
    texts.append(waiting);
    *waiting = texts;
}

fn digest(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A message as the agent reads it. The app reads the same form back out of
/// a transcript to show who sent it.
fn wrapped(letter: &Letter) -> String {
    if let Some(answered) = &letter.answers {
        return format!(
            "<question-answer asker=\"{}\" asker-id=\"{}\" cli=\"{}\" question=\"{}\" note=\"{}\"{}>\n{}\n</question-answer>",
            attribute(&answered.asker),
            attribute(&answered.asker_id),
            attribute(&answered.cli),
            attribute(&answered.question),
            attribute(letter.note.as_deref().unwrap_or_default()),
            if answered.secret { " secret=\"true\"" } else { "" },
            letter
                .text
                .replace("</question-answer", "&lt;/question-answer"),
        );
    }
    let name = &letter.from_name;
    let note = letter.note.clone().unwrap_or_else(|| {
        let sender = called(name, &letter.from_id);
        format!("Sent by {sender}, another agent in Sikemux, not by the person. Reply with agent_send only with an answer, a decision or a blocker, never just to acknowledge.")
    });
    format!(
        "<agent-message from=\"{}\" agent-id=\"{}\" cli=\"{}\" note=\"{}\">\n{}\n</agent-message>",
        attribute(name),
        attribute(&letter.from_id),
        attribute(&letter.cli),
        attribute(&note),
        letter.text.replace("</agent-message", "&lt;/agent-message"),
    )
}

/// An agent as a note to another agent names it: its name with its id beside
/// it, since a name can pass to another agent once it closes.
pub(crate) fn called(name: &str, agent_id: &str) -> String {
    if name == agent_id {
        agent_id.to_owned()
    } else {
        format!("{name} ({agent_id})")
    }
}

/// [`called`] for an agent by its id, under the name it has now.
pub(crate) fn called_now(core: &Core, agent_id: &str) -> String {
    called(&core.names().name_or_id(agent_id), agent_id)
}

/// The task a lead gives an agent it started, as that agent reads it.
pub(crate) fn task(
    core: &Core,
    agent_id: &str,
    lead_id: &str,
    lead_name: &str,
    lead_cli: &str,
    reports: bool,
    text: String,
) -> String {
    let lead = called(lead_name, lead_id);
    let answer = if reports {
        format!("When your turn ends, your final answer goes to {lead} by itself. Message it with agent_send if you need to ask it something first.")
    } else {
        format!("Your answers do not go to {lead} by themselves: send it what it needs with agent_send when you are done.")
    };
    let brief = super::memory::brief(core, agent_id, Some(&text));
    wrapped(&Letter {
        kind: MessageKind::Message,
        from_id: lead_id.to_owned(),
        from_name: lead_name.to_owned(),
        cli: lead_cli.to_owned(),
        text: with_brief(text, brief),
        at: epoch_ms(),
        answers: None,
        puts: None,
        paths: Vec::new(),
        waits: false,
        note: Some(format!(
            "{lead_name}, another agent in Sikemux, started you to do this; it is your lead, not the person. {answer}"
        )),
    })
}

/// A new agent's task with what Sikemux keeps for its project after it, in
/// the same letter.
fn with_brief(task: String, brief: Option<String>) -> String {
    match brief {
        Some(brief) => format!("{task}\n\n{brief}"),
        None => task,
    }
}

/// Hands a new agent its task: into a turn, or ahead of whatever waits for
/// it when it is already busy with something else.
pub(crate) async fn hand_task(
    core: &Arc<Core>,
    agent_id: &str,
    task: String,
) -> Result<(), String> {
    let mailbox = &core.harness().mailbox;
    let from = core
        .names()
        .get(agent_id)
        .and_then(|named| named.parent)
        .unwrap_or_else(|| Sender::Auto.id().to_owned());
    let queued = Queued {
        text: task.clone(),
        origin: Some(Origin {
            from,
            kind: MessageKind::Message,
        }),
        mark: None,
        paths: Vec::new(),
    };
    if mailbox.has_waiting(agent_id) {
        log(core, agent_id, [&queued], MessageOutcome::Waiting);
        mailbox.put_back(agent_id, vec![queued]);
        return Ok(());
    }
    match chat::letter(core, agent_id, task, Vec::new(), false).await {
        Ok(delivered @ (Delivery::Steered | Delivery::NewTurn)) => {
            let outcome = match delivered {
                Delivery::Steered => MessageOutcome::Steered,
                _ => MessageOutcome::NewTurn,
            };
            log(core, agent_id, [&queued], outcome);
            Ok(())
        }
        Ok(Delivery::TurnEnded | Delivery::Refused) => {
            log(core, agent_id, [&queued], MessageOutcome::Waiting);
            mailbox.put_back(agent_id, vec![queued]);
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// The latest messages between agents in `project`, newest first.
pub(crate) fn logged(core: &Core, project: &str, limit: Option<usize>) -> Vec<MessageEntry> {
    let limit = limit.unwrap_or(log::MAX_READ).min(log::MAX_READ);
    core.harness().mailbox.log.read(project, limit)
}

/// The project's messages between agents, one row each, newest first.
pub(crate) fn logged_rows(core: &Core, project: &str) -> Vec<log::MessageRow> {
    log::rows(&logged(core, project, None), |id| core.names().name(id))
}

/// Notes in the log of the project of agent `to` what became of messages
/// for it.
fn log<'a>(
    core: &Core,
    to: &str,
    queued: impl IntoIterator<Item = &'a Queued>,
    outcome: MessageOutcome,
) {
    let queued: Vec<&Queued> = queued.into_iter().collect();
    let answers = queued.iter().any(|queued| {
        queued
            .origin
            .as_ref()
            .is_some_and(|origin| origin.kind == MessageKind::Answer)
    });
    if answers && !matches!(outcome, MessageOutcome::Waiting | MessageOutcome::Returned) {
        core.harness().questions.delivered(to);
    }
    let Some(project) = core.names().get(to).map(|named| named.project) else {
        return;
    };
    let at = epoch_ms();
    let to_name = core.names().name(to);
    let mut logged = false;
    for queued in queued {
        let from = queued.origin.as_ref().map(|origin| origin.from.clone());
        core.harness().mailbox.log.append(
            &project,
            &MessageEntry {
                at,
                from_name: from.as_deref().and_then(|from| core.names().name(from)),
                from,
                kind: queued.origin.as_ref().map(|origin| origin.kind),
                to: to.to_owned(),
                to_name: to_name.clone(),
                outcome,
            },
        );
        logged = true;
    }
    if logged {
        core.changed(What::View);
    }
}

fn message_text(params: &Value) -> Result<String, String> {
    match params.get("text") {
        Some(Value::String(text)) if !text.trim().is_empty() && text.len() <= MAX_TEXT_BYTES => {
            Ok(text.clone())
        }
        Some(Value::String(text)) if !text.trim().is_empty() => Err(format!(
            "text is {} bytes; a message holds at most {MAX_TEXT_BYTES}, so put long material in a file and send its path",
            text.len(),
        )),
        _ => Err("text must be the message, as nonempty text".into()),
    }
}

/// An agent's name and the host it is said to run on, as in "Otter on
/// studio" or "Otter@studio".
pub(super) fn on_host(wanted: &str) -> (&str, Option<&str>) {
    if let Some((name, host)) = wanted.split_once('@') {
        return (name.trim(), Some(host.trim()));
    }
    let lower = wanted.to_ascii_lowercase();
    match lower.find(" on ") {
        Some(at) => (wanted[..at].trim(), Some(wanted[at + 4..].trim())),
        None => (wanted, None),
    }
}

/// What `agent_send` takes as `to` for every other agent of the caller's team.
const TEAM: &str = "team";
const MAX_TO_BYTES: usize = 2000;

/// Sends one message to every other agent of `from`'s team: the agent that
/// leads it and every agent it started, each delivered as a message of its own.
async fn send_team(
    core: &Arc<Core>,
    request: &HarnessRequest,
    from: &str,
) -> Result<Value, String> {
    let top = core.names().team_of(from);
    let members: Vec<String> = core
        .names()
        .team(&top)
        .into_iter()
        .filter(|member| member != from)
        .collect();
    if members.is_empty() {
        return Err("You have no team: no agent leads you and you started none".into());
    }
    let named = members
        .into_iter()
        .map(|member| (core.names().name_or_id(&member), member))
        .collect();
    Ok(send_each(core, request, named).await)
}

/// Sends one message to each of `to`, its shown name and its address, all at once, each
/// delivered as a message of its own, and answers with each one's delivery in order.
async fn send_each(core: &Arc<Core>, request: &HarnessRequest, to: Vec<(String, String)>) -> Value {
    let sending = to.into_iter().map(|(name, address)| {
        let mut one = request.clone();
        one.params["to"] = Value::String(address);
        async move { (name, Box::pin(send(core, &one)).await) }
    });
    let answers = futures_util::future::join_all(sending).await;
    let (mut sent, mut failed) = (Vec::new(), Vec::new());
    for (name, answer) in answers {
        match answer {
            Ok(answer) => sent.push(json!({ "to": name, "delivery": answer["delivery"] })),
            Err(why) => failed.push(json!({ "to": name, "error": why })),
        }
    }
    json!({ "sent": sent, "failed": failed })
}

pub(crate) async fn send(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let from = request
        .agent_id
        .clone()
        .ok_or("agent_send needs the calling agent's id")?;
    let to = match request.params.get("to") {
        Some(Value::String(to)) if !to.trim().is_empty() && to.len() <= MAX_TO_BYTES => to.clone(),
        _ => {
            return Err("to must be the agent's name or id, or several separated by commas".into())
        }
    };
    let text = message_text(&request.params)?;
    let why = super::caller::text_param(&request.params, "why", MAX_WHY_BYTES)?.map(str::to_owned);
    let answers = match request.params.get("answers") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if !id.trim().is_empty() => Some(Answering {
            id: id.trim().to_owned(),
            why,
        }),
        Some(_) => return Err("answers must be the id of the question this answers".into()),
    };
    if to.trim().eq_ignore_ascii_case(TEAM) {
        if answers.is_some() {
            return Err("answers goes to one agent, not the team".into());
        }
        return Box::pin(send_team(core, request, &from)).await;
    }
    if to.contains(',') {
        let mut several: Vec<String> = Vec::new();
        for name in to.split(',').map(str::trim).filter(|name| !name.is_empty()) {
            if !several.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
                several.push(name.to_owned());
            }
        }
        if answers.is_some() {
            return Err("answers goes to one agent, not several".into());
        }
        if several.iter().any(|name| name.eq_ignore_ascii_case(TEAM)) {
            return Err("team goes alone in to, not among other names".into());
        }
        let named = several
            .into_iter()
            .map(|name| (name.clone(), name))
            .collect();
        return Ok(send_each(core, request, named).await);
    }
    let found = {
        let (core, request, answers) = (core.clone(), request.clone(), answers.clone());
        blocking(move || {
            let project = super::caller::project_of(&core, &request);
            let agents = listed(&core);
            let target = match resolve(&core, &agents, &to, &project, &from) {
                Ok(target) => target,
                Err(why) => {
                    let (wanted, on) = on_host(to.trim().trim_start_matches('@'));
                    if on.is_some() {
                        return Err(why.into());
                    }
                    let named = |agent: &Listed| {
                        agent.agent_id == wanted
                            || (agent.name.as_deref())
                                .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
                                && super::roster::reaches(&core, agent, &project, &from)
                    };
                    return match core.names().closed_worker(&from, wanted) {
                        Some(ran) if answers.is_none() && !agents.iter().any(named) => {
                            Ok(Found::Closed(wanted.to_owned(), ran))
                        }
                        _ => Err(why.into()),
                    };
                }
            };
            if target.agent_id == from {
                return Err("That agent is you".into());
            }
            let name = target.name.as_deref().unwrap_or(&target.agent_id);
            if let Err(why) = blind_between(&core, &from, &target.agent_id, name) {
                let refused = Queued {
                    text: String::new(),
                    origin: Some(Origin {
                        from: from.clone(),
                        kind: MessageKind::Message,
                    }),
                    mark: None,
                    paths: Vec::new(),
                };
                log(&core, &target.agent_id, [&refused], MessageOutcome::Refused);
                return Err(why.into());
            }
            let mut letter = letter_from(&agents, MessageKind::Message, from, text, None);
            if let Some(Answering { id, .. }) = answers.as_ref().filter(|_| target.reach != Reach::Remote) {
                let name = target.name.as_deref().unwrap_or(&target.agent_id);
                let asked = core
                    .harness()
                    .questions
                    .asked_of(&letter.from_id, &target.agent_id, id)
                    .ok_or_else(|| format!("{name} has no open question {id} that it asked you; it was answered already, or leave out answers to send this as a plain message"))?;
                letter.kind = MessageKind::Answer;
                letter.note = Some(lead_answer_note(&letter.from_name, &asked));
            }
            Ok(Found::Open(target, Box::new(letter)))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let (target, letter) = match found {
        Found::Open(target, letter) => (target, *letter),
        Found::Closed(name, ran) => return reopen(core, request, &name, ran).await,
    };
    let from = letter.from_id.clone();
    if target.reach == Reach::Remote {
        return carry(core, &target, letter, answers).await;
    }
    let answer = letter.text.clone();
    let sent = deliver(core, &target, letter).await?;
    core.seen().called(&target.agent_id);
    core.changed(What::Agents);
    if let Some(answering) = answers {
        settle_answer(core, from, target.agent_id.clone(), answering, answer).await;
    }
    Ok(sent)
}

enum Found {
    Open(Listed, Box<Letter>),
    /// A closed agent the sender led, by the name it was sent to.
    Closed(String, super::names::RanSession),
}

/// Takes up again the session of a closed agent the sender led, with the message as its task.
async fn reopen(
    core: &Arc<Core>,
    request: &HarnessRequest,
    name: &str,
    ran: super::names::RanSession,
) -> Result<Value, String> {
    let mut spawning = request.clone();
    spawning.params = json!({
        "prompt": message_text(&request.params)?,
        "resume": ran.session_id,
        "cli": ran.provider,
    });
    let opened = Box::pin(super::spawn::spawn(core, &spawning))
        .await
        .map_err(|why| format!("{name} had closed, and reopening its session failed: {why}"))?;
    let now = opened["name"].as_str().unwrap_or(name);
    let as_now = if now.eq_ignore_ascii_case(name) {
        String::new()
    } else {
        format!(" as {now}")
    };
    Ok(json!({
        "to": now,
        "agentId": opened["agentId"],
        "delivery": "reopened",
        "note": format!("{name} had closed, so Sikemux reopened its session{as_now}; it gets this as its task once it is ready."),
    }))
}

/// What the asker is told beside its lead's answer.
fn lead_answer_note(lead: &str, asked: &str) -> String {
    format!(
        "Sent by {lead}, your lead in Sikemux, not by the person, answering the question you asked: {asked}\nIt does not allow anything that stops for the person: a push, merge, deploy, publish or production write still waits for them."
    )
}

/// Closes the question a lead answered and keeps the answer as a decision.
async fn settle_answer(
    core: &Arc<Core>,
    lead: String,
    asker: String,
    answering: Answering,
    answer: String,
) {
    let core = core.clone();
    let settled = blocking(move || {
        super::questions::lead_answered(
            &core,
            &lead,
            &asker,
            &answering.id,
            &answer,
            answering.why,
        );
        Ok(())
    })
    .await;
    if let Err(error) = settled {
        eprintln!("sikemux core: a lead's answer was not kept as a decision: {error}");
    }
}

/// Sends a letter to its agent on a linked host, noting it here as sent so
/// two agents that keep messaging each other across hosts are held too.
async fn carry(
    core: &Arc<Core>,
    target: &Listed,
    letter: Letter,
    question: Option<Answering>,
) -> Result<Value, String> {
    core.harness().mailbox.note_sent(&letter, &target.agent_id);
    let queued = Queued {
        text: String::new(),
        origin: Some(Origin {
            from: letter.from_id.clone(),
            kind: letter.kind,
        }),
        mark: None,
        paths: Vec::new(),
    };
    let sent = super::hosts::route::deliver(
        core,
        &target.agent_id,
        letter.carried(&target.agent_id, question),
    )
    .await;
    let outcome = match &sent {
        Ok(answer) => match answer["delivery"].as_str() {
            Some("steered") => MessageOutcome::Steered,
            Some("newTurn") => MessageOutcome::NewTurn,
            Some("repeat") => MessageOutcome::Repeat,
            _ => MessageOutcome::Waiting,
        },
        Err(_) => MessageOutcome::Refused,
    };
    log(core, &target.agent_id, [&queued], outcome);
    sent
}

/// A letter a linked host carried here for one of this host's agents, sent
/// by an agent of a team the two hosts share, on `host`, or the person's
/// answer to a question that went to a lead here first. `landed` names the
/// branch the sender's latest commits came back on, for its report to say.
pub(crate) async fn from_host(
    core: &Arc<Core>,
    host: Option<&str>,
    carried: Carried,
    landed: Option<(String, usize)>,
) -> Result<Value, String> {
    let to = carried.to.clone();
    let target = {
        let core = core.clone();
        blocking(move || {
            listed(&core)
                .into_iter()
                .find(|agent| agent.agent_id == to && agent.reach != Reach::Remote)
                .ok_or_else(|| "that agent is not on this host".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let name = target
        .name
        .clone()
        .unwrap_or_else(|| target.agent_id.clone());
    blind_between(core, &carried.from_id, &target.agent_id, &name)?;
    let mut letter = Letter {
        kind: carried.kind,
        from_id: carried.from_id.clone(),
        from_name: match host {
            Some(host) => format!("{} on {host}", carried.from_name),
            None => carried.from_name,
        },
        cli: carried.cli,
        text: carried.text,
        at: epoch_ms(),
        note: carried.note,
        answers: carried.answered,
        puts: None,
        paths: Vec::new(),
        waits: false,
    };
    if let Some((branch, count)) = landed {
        letter.text.push_str(&format!(
            "\n\nIts commits came back to the branch {branch} in your checkout: {count} on top of the commit you started it from, the first holding your uncommitted changes as they were then. Merge what you want of them."
        ));
    }
    if let Some(Answering { id, .. }) = &carried.question {
        let asked = core
            .harness()
            .questions
            .asked_of(&letter.from_id, &target.agent_id, id)
            .ok_or_else(|| format!("{name} has no open question {id} that it asked you; it was answered already, or leave out answers to send this as a plain message"))?;
        letter.kind = MessageKind::Answer;
        letter.note = Some(lead_answer_note(&letter.from_name, &asked));
    }
    let from = letter.from_id.clone();
    let answer = letter.text.clone();
    let sent = deliver(core, &target, letter).await?;
    if let Some(answering) = carried.question {
        settle_answer(core, from, target.agent_id.clone(), answering, answer).await;
    }
    Ok(sent)
}

/// Tells an agent here something about a teammate on another host. Fails
/// when the agent has stopped or is gone.
pub(crate) async fn from_hosts(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = {
        let core = core.clone();
        blocking(move || {
            listed(&core)
                .into_iter()
                .find(|agent| {
                    agent.agent_id == to && !matches!(agent.reach, Reach::Stopped | Reach::Remote)
                })
                .ok_or_else(|| "that agent is not running".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let letter = Letter::from_sender(
        Sender::Hosts,
        MessageKind::Message,
        text,
        Some(HOSTS_NOTE.to_owned()),
    );
    deliver(core, &target, letter).await
}

/// Refuses any contact between a reviewer Auto started blind and another
/// agent, either way.
pub(crate) fn blind_between(
    core: &Core,
    from: &str,
    to: &str,
    to_name: &str,
) -> Result<(), String> {
    if core.names().is_blind(from) {
        return Err("You review a card blind for Auto, so you may not reach other agents; ask the person with ask if you must".into());
    }
    if core.names().is_blind(to) {
        return Err(format!(
            "{to_name} reviews a card blind for Auto, so no agent may reach it"
        ));
    }
    Ok(())
}

/// Puts the question `id`, worded `text`, from one agent to its lead, with `note` in
/// place of the usual one. Fails when the lead has stopped or is gone.
pub(crate) async fn question(
    core: &Arc<Core>,
    id: String,
    from: String,
    to: String,
    text: String,
    note: String,
) -> Result<Value, String> {
    let (target, letter) = {
        let core = core.clone();
        blocking(move || {
            let agents = listed(&core);
            let target = agents
                .iter()
                .find(|agent| agent.agent_id == to && agent.reach != Reach::Stopped)
                .cloned()
                .ok_or("that agent is not running")?;
            let mut letter = letter_from(&agents, MessageKind::Question, from, text, Some(note));
            letter.puts = Some(id);
            Ok((target, letter))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    deliver(core, &target, letter).await
}

/// Hands an agent the person's answer to the question it asked.
pub(crate) async fn answer(
    core: &Arc<Core>,
    to: String,
    answers: Vec<(Answered, String, String, Vec<String>)>,
) -> Result<Value, String> {
    let target = {
        let core = core.clone();
        let to = to.clone();
        blocking(move || {
            listed(&core)
                .into_iter()
                .find(|agent| agent.agent_id == to)
                .ok_or_else(|| "that agent is gone".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let chat = core.chats().get(&target.agent_id).is_some();
    let mut letters = answers.into_iter().map(|(answered, text, note, paths)| {
        let (text, paths) = match chat && !paths.is_empty() {
            true => (super::questions::without_attached(&text, &paths), paths),
            false => (text, Vec::new()),
        };
        Letter {
            kind: MessageKind::Answer,
            from_id: Sender::Person.id().to_owned(),
            from_name: Sender::Person.name().to_owned(),
            cli: "person".to_owned(),
            text,
            at: epoch_ms(),
            note: Some(note),
            answers: Some(answered),
            puts: None,
            paths,
            waits: false,
        }
    });
    let mut letter = letters.next().ok_or("there is no answer to send")?;
    let rest: Vec<Letter> = letters.collect();
    if rest.is_empty() {
        return Box::pin(deliver(core, &target, letter)).await;
    }
    if target.reach == Reach::Remote {
        let mut delivered = carry(core, &target, letter, None).await?;
        for letter in rest {
            delivered = carry(core, &target, letter, None).await?;
        }
        return Ok(delivered);
    }
    let text = std::iter::once(&letter)
        .chain(&rest)
        .map(wrapped)
        .collect::<Vec<_>>()
        .join("\n\n");
    letter
        .paths
        .extend(rest.into_iter().flat_map(|rest| rest.paths));
    Box::pin(deliver_as(core, &target, letter, text)).await
}

/// The agent `to`, unless it has stopped or is gone.
async fn running(core: &Arc<Core>, to: String) -> Result<Listed, String> {
    let core = core.clone();
    blocking(move || {
        listed(&core)
            .into_iter()
            .find(|agent| agent.agent_id == to && agent.reach != Reach::Stopped)
            .ok_or_else(|| "that agent is not running".into())
    })
    .await
    .map_err(|error| error.to_string())
}

/// Whether a message to `to` would reach a running agent.
pub(crate) async fn reachable(core: &Arc<Core>, to: &str) -> bool {
    running(core, to.to_owned()).await.is_ok()
}

/// Hands an agent a note from Sikemux's board about its cards. Fails when the
/// agent has stopped or is gone.
pub(crate) async fn from_board(
    core: &Arc<Core>,
    to: String,
    text: String,
    note: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(Sender::Board, MessageKind::Message, text, Some(note));
    deliver(core, &target, letter).await
}

/// Hands an agent the prompt of a trigger it set, waking it first when it sleeps, unless it
/// sleeps since the person stopped everything. Fails when the agent has stopped or is gone.
pub(crate) async fn from_trigger(
    core: &Arc<Core>,
    to: String,
    text: String,
    note: String,
) -> Result<Value, String> {
    let mut target = running(core, to.clone()).await?;
    if target.reach == Reach::Asleep && !core.workspaces().held(&to) {
        launching::wake(core, to.clone())
            .await
            .map_err(|error| format!("it is asleep and did not wake: {error}"))?;
        target = running(core, to).await?;
    }
    let letter = Letter::from_sender(Sender::Trigger, MessageKind::Message, text, Some(note));
    deliver(core, &target, letter).await
}

/// Hands an agent a note from Sikemux itself, about work it shares with
/// other agents. Fails when the agent has stopped or is gone.
pub(crate) async fn from_sikemux(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Notice,
        text,
        Some("Sent by Sikemux itself, not by the person or another agent.".to_owned()),
    );
    deliver(core, &target, letter).await
}

/// Gives an agent work from Sikemux itself, starting a turn when it is at rest, where a notice
/// would be dropped; `why` says to the agent what Sikemux sends it for. Fails when the agent has
/// stopped or is gone.
pub(crate) async fn work_from_sikemux(
    core: &Arc<Core>,
    to: String,
    text: String,
    why: &str,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some(format!(
            "Sent by Sikemux itself {why}, not by the person or another agent."
        )),
    );
    deliver(core, &target, letter).await
}

/// Hands an agent the step the person took on its turn's line with one click. It starts a turn
/// when the agent is at rest. The words are Sikemux's for that step, not the person's, so they
/// answer no call and are not kept as what the person said.
pub(crate) async fn step_from_person(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    person_wrote(core, &target.agent_id);
    let letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some("Sent by Sikemux because the person took this step on your turn's line with one click; the words are Sikemux's, not theirs.".to_owned()),
    );
    deliver(core, &target, letter).await
}

/// Sends an agent back to what it offered, for a rule of its project to decide. It starts a turn
/// when the agent is at rest, where a notice would be dropped. Fails when the agent has stopped or
/// is gone.
pub(crate) async fn over_rule(core: &Arc<Core>, to: String, text: String) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some("Sent by Sikemux itself over a rule of this project, not by the person.".to_owned()),
    );
    deliver(core, &target, letter).await
}

/// Tells an agent what came of something it waited on the person for, as [`for_person`] does,
/// but waits for it when it has stopped. Fails only when it is gone.
pub(crate) async fn for_person_waiting(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = {
        let core = core.clone();
        blocking(move || {
            listed(&core)
                .into_iter()
                .find(|agent| agent.agent_id == to)
                .ok_or_else(|| "that agent is gone".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let mut letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some(
            "Sent by Sikemux with the person's answer about an action it held for them.".to_owned(),
        ),
    );
    letter.waits = true;
    deliver(core, &target, letter).await
}

/// Tells an agent what the person decided about something of its that
/// Sikemux held for them, which starts a turn when it is at rest. Fails when
/// the agent has stopped or is gone.
pub(crate) async fn for_person(
    core: &Arc<Core>,
    to: String,
    text: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter::from_sender(
        Sender::Sikemux,
        MessageKind::Message,
        text,
        Some(
            "Sent by Sikemux with the person's answer about an action it held for them.".to_owned(),
        ),
    );
    deliver(core, &target, letter).await
}

const AUTO_NOTE: &str = "Sent by Sikemux's Auto, which runs this project's board for the person, not by the person or another agent.";

fn auto_letter(text: String) -> Letter {
    Letter::from_sender(
        Sender::Auto,
        MessageKind::Message,
        text,
        Some(AUTO_NOTE.to_owned()),
    )
}

/// The task Auto gives an agent it started, as that agent reads it.
pub(crate) fn auto_task(core: &Core, agent_id: &str, text: String) -> String {
    let brief = super::memory::brief(core, agent_id, None);
    wrapped(&auto_letter(with_brief(text, brief)))
}

/// Auto tells an agent of its own what its card needs next.
pub(crate) async fn from_auto(core: &Arc<Core>, to: String, text: String) -> Result<Value, String> {
    let mut letter = auto_letter(text);
    if let Some(lead) = core.hosts().team.get(&to).and_then(|worker| worker.parent) {
        letter.from_id = lead;
    }
    let target = running(core, to).await?;
    deliver(core, &target, letter).await
}

/// Drops the messages holding `mark` that still wait for the agent.
pub(crate) fn withdraw(core: &Core, agent_id: &str, mark: &str) {
    if !is_waiting(core, agent_id, mark) {
        return;
    }
    let withdrawn = core.harness().mailbox.change(|state| {
        let Some(waiting) = state.waiting.get_mut(agent_id) else {
            return Vec::new();
        };
        let (withdrawn, kept) = std::mem::take(waiting)
            .into_iter()
            .partition(|queued| queued.mark.as_deref() == Some(mark));
        *waiting = kept;
        withdrawn
    });
    log(core, agent_id, &withdrawn, MessageOutcome::Dropped);
}

/// Whether the message known by `mark` still waits for the agent's turn to end.
pub(crate) fn is_waiting(core: &Core, agent_id: &str, mark: &str) -> bool {
    core.harness()
        .mailbox
        .state
        .locked()
        .waiting
        .get(agent_id)
        .is_some_and(|waiting| {
            waiting
                .iter()
                .any(|queued| queued.mark.as_deref() == Some(mark))
        })
}

fn letter_from(
    agents: &[Listed],
    kind: MessageKind,
    from: String,
    text: String,
    note: Option<String>,
) -> Letter {
    let sender = agents.iter().find(|agent| agent.agent_id == from);
    Letter {
        kind,
        from_name: sender
            .and_then(|agent| agent.name.clone())
            .unwrap_or_else(|| from.clone()),
        cli: sender.map_or_else(|| "agent".to_owned(), |agent| agent.cli.clone()),
        from_id: from,
        text,
        at: epoch_ms(),
        note,
        answers: None,
        puts: None,
        paths: Vec::new(),
        waits: false,
    }
}

/// Hands `text` from `worker` to the agent that started it, as a message
/// about how its turn ended. A lead that has stopped gets it once it is
/// opened again; only one the person closed goes without.
pub(crate) async fn report(core: &Arc<Core>, worker: String, lead: String, text: String) {
    let name = core.names().name(&worker);
    report_as(core, worker, name, lead, text).await;
}

/// [`report`] for a worker that may be gone, under the name it had.
pub(crate) async fn report_as(
    core: &Arc<Core>,
    worker: String,
    name: Option<String>,
    lead: String,
    text: String,
) {
    let found = {
        let core = core.clone();
        blocking(move || {
            let agents = listed(&core);
            let target = agents.iter().find(|agent| agent.agent_id == lead).cloned();
            let name = name.unwrap_or_else(|| worker.clone());
            let worker_called = called(&name, &worker);
            let note = format!(
                "{worker_called} is an agent you started, and this is how its turn ended, sent by Sikemux. Reply with agent_send to it if it needs more; once its work has landed and you need nothing more, close it with agent_close."
            );
            let mut letter = letter_from(&agents, MessageKind::Report, worker, text, Some(note));
            letter.from_name = name;
            Ok(target.map(|target| (target, letter)))
        })
        .await
    };
    let Ok(Some((target, letter))) = found else {
        eprintln!("sikemux core: an agent's report has no lead left to reach");
        return;
    };
    if let Err(error) = deliver(core, &target, letter).await {
        eprintln!("sikemux core: an agent's report did not reach its lead: {error}");
    }
}

async fn deliver(core: &Arc<Core>, target: &Listed, letter: Letter) -> Result<Value, String> {
    let text = wrapped(&letter);
    Box::pin(deliver_as(core, target, letter, text)).await
}

/// Delivers `text` in place of `letter`'s own, such as several answers read
/// as one message.
async fn deliver_as(
    core: &Arc<Core>,
    target: &Listed,
    letter: Letter,
    text: String,
) -> Result<Value, String> {
    if target.reach == Reach::Remote {
        return carry(core, target, letter, None).await;
    }
    let queued = Queued {
        text,
        origin: Some(Origin {
            from: letter.from_id.clone(),
            kind: letter.kind,
        }),
        mark: letter.puts.clone(),
        paths: letter.paths.clone(),
    };
    let handed = handed(core, target, letter, &queued).await;
    let outcome = handed
        .as_ref()
        .map_or(MessageOutcome::Refused, |(_, outcome)| *outcome);
    log(core, &target.agent_id, [&queued], outcome);
    handed.map(|(answer, _)| answer)
}

/// Hands `letter` to its agent, or leaves it waiting for the agent, and says
/// what became of it.
async fn handed(
    core: &Arc<Core>,
    target: &Listed,
    letter: Letter,
    queued: &Queued,
) -> Result<(Value, MessageOutcome), String> {
    let name = target
        .name
        .clone()
        .unwrap_or_else(|| target.agent_id.clone());
    let answer = |delivery: &str, note: String| json!({ "to": name, "agentId": target.agent_id, "delivery": delivery, "note": note });
    let mailbox = &core.harness().mailbox;
    match mailbox.admit(&letter, &target.agent_id, &name)? {
        Admitted::Yes => {}
        Admitted::Repeat { seconds_ago } => {
            return Ok((
                answer(
                    "repeat",
                    format!(
                    "You sent {name} the same text {seconds_ago} s ago, so it was not sent again."
                ),
                ),
                MessageOutcome::Repeat,
            ))
        }
        Admitted::Held => {
            let dropped = mailbox.hold(&letter.from_id, &target.agent_id, queued.clone());
            log(core, &target.agent_id, &dropped, MessageOutcome::Dropped);
            return Ok((
                answer(
                    "held",
                    format!("You and {name} keep messaging each other without either doing other work, so Sikemux holds this and has asked the person. It goes to {name} once one of you does other work or the person writes to one of you; carry on with your own work."),
                ),
                MessageOutcome::Waiting,
            ));
        }
    }
    if Sender::of(&letter.from_id) == Some(Sender::Person) {
        person_wrote(core, &target.agent_id);
    }
    let stopped = capped(core, &target.agent_id, queued) && mailbox.stopped(&target.agent_id);
    let mut woke = "";
    let not_woken;
    let mut held = None;
    let mut reach = target.reach;
    let notice = letter.kind == MessageKind::Notice;
    match target.reach {
        Reach::Chat | Reach::Terminal | Reach::Asleep if stopped => held = Some(STOPPED_NOTE),
        Reach::Chat | Reach::Terminal => {}
        Reach::Asleep if notice => held = Some("is asleep"),
        Reach::Asleep
            if !core.window().is_open() && core.hosts().team.get(&letter.from_id).is_none() =>
        {
            held = Some("is asleep while Sikemux is closed")
        }
        Reach::Asleep if core.workspaces().held(&target.agent_id) && !lifts_hold(core, &letter) => {
            held = Some("was put to sleep when the person stopped everything")
        }
        Reach::Asleep => {
            let woken = match launching::wake(core, target.agent_id.clone()).await {
                Ok(()) => Ok(()),
                Err(_) => launching::wake(core, target.agent_id.clone()).await,
            };
            match woken {
                Ok(()) => {
                    mailbox.state.locked().wake_failed.remove(&target.agent_id);
                    woke = " Sikemux woke it first.";
                    if core.agent_session(&target.agent_id).is_some() {
                        reach = Reach::Terminal;
                    }
                }
                Err(error) => {
                    mailbox
                        .state
                        .locked()
                        .wake_failed
                        .insert(target.agent_id.clone(), epoch_ms());
                    not_woken = format!("is asleep and did not wake twice ({error}); Sikemux tries again within a minute");
                    held = Some(not_woken.as_str());
                }
            }
        }
        Reach::Stopped
            if letter.answers.is_some() || letter.kind == MessageKind::Report || letter.waits =>
        {
            held = Some("has stopped")
        }
        Reach::Stopped => {
            return Err(format!(
                "{name} has stopped; it gets messages again once the person opens it"
            ))
        }
        Reach::Remote => return Err(format!("{name} runs on another host")),
    }
    if !notice {
        mailbox.note_sent(&letter, &target.agent_id);
        if !letter.by_sikemux() {
            super::board::offers::reached_by_more_than_offer(core, &target.agent_id);
        }
    }
    if held.is_none() && mailbox.has_waiting(&target.agent_id) {
        held = Some("already has messages waiting");
    }
    if held.is_none() {
        let delivered = match reach {
            Reach::Terminal => {
                ready_terminal(core, &target.agent_id, settle_ms([queued])).map(|terminal| {
                    paste(core, &target.agent_id, terminal, vec![queued.clone()]);
                    MessageOutcome::Typed
                })
            }
            _ => hand(core, &target.agent_id, std::slice::from_ref(queued))
                .await
                .map(|delivered| match delivered {
                    Delivery::Steered => MessageOutcome::Steered,
                    _ => MessageOutcome::NewTurn,
                }),
        };
        match delivered {
            Ok(MessageOutcome::Steered) => {
                return Ok((
                    answer(
                        "steered",
                        format!("{name} is working and reads this at its next step.{woke}"),
                    ),
                    MessageOutcome::Steered,
                ))
            }
            Ok(outcome) => {
                return Ok((
                    answer(
                        "newTurn",
                        format!("{name} was idle and started a turn to read this.{woke}"),
                    ),
                    outcome,
                ))
            }
            Err(busy) => held = Some(busy),
        }
    }
    if notice {
        let busy = held.unwrap_or_default();
        return Ok((
            answer(
                "dropped",
                format!(
                    "{name} {busy}, and a notice is only worth reading at once, so it was dropped."
                ),
            ),
            MessageOutcome::Dropped,
        ));
    }
    let (waiting, superseded) = match stopped {
        true => (mailbox.cap(&target.agent_id, queued.clone()), Vec::new()),
        false => mailbox.wait(&target.agent_id, queued.clone()),
    };
    log(core, &target.agent_id, &superseded, MessageOutcome::Dropped);
    if stopped {
        core.changed(What::View);
    }
    let busy = held.unwrap_or_default();
    Ok((
        answer(
            "waiting",
            format!("{name} {busy}, so it gets this once it is free, with any others waiting ({waiting} now).{woke}"),
        ),
        MessageOutcome::Waiting,
    ))
}

/// Whether this letter may wake an agent held since the person stopped everything: the person
/// wrote it, or an agent they have set going again did. Mail left from the stop never may.
fn lifts_hold(core: &Core, letter: &Letter) -> bool {
    match Sender::of(&letter.from_id) {
        Some(sender) => sender == Sender::Person,
        None => {
            core.workspaces().agent(&letter.from_id).is_some()
                && !core.workspaces().held(&letter.from_id)
        }
    }
}

/// When the agent will have rested `settle_ms` since its last turn, unless it has.
fn rests_until(core: &Core, agent_id: &str, settle_ms: u64) -> Option<u64> {
    core.seen()
        .active_at(agent_id)
        .map(|at| at + settle_ms)
        .filter(|until| *until > epoch_ms())
}

fn rested(core: &Core, agent_id: &str, settle_ms: u64) -> bool {
    rests_until(core, agent_id, settle_ms).is_none()
}

/// Hands `queued` to the chat as one message, or says why it cannot take it
/// now.
async fn hand(core: &Core, agent_id: &str, queued: &[Queued]) -> Result<Delivery, &'static str> {
    let chat = core.chats().get(agent_id).ok_or("is not running")?;
    if !chat.is_ready() {
        chat.start_agent();
        return Err("is still starting");
    }
    if !chat.turn_running() && !rested(core, agent_id, settle_ms(queued)) {
        return Err("has just finished a turn");
    }
    let paths: Vec<String> = queued
        .iter()
        .flat_map(|queued| queued.paths.clone())
        .collect();
    match chat::letter(core, agent_id, joined(queued), paths, by_person(queued)).await {
        Ok(Delivery::NewTurn) => {
            took_turn(core, agent_id, queued);
            Ok(Delivery::NewTurn)
        }
        Ok(Delivery::Steered) => Ok(Delivery::Steered),
        Ok(Delivery::TurnEnded) => Err("has just finished a turn"),
        Ok(Delivery::Refused) | Err(_) => Err("is working and cannot read messages mid-turn"),
    }
}

/// The terminal of an agent that waits for a prompt, or why it cannot take
/// one now.
fn ready_terminal(
    core: &Core,
    agent_id: &str,
    settle_ms: u64,
) -> Result<Arc<Session>, &'static str> {
    let terminal = core
        .agent_session(agent_id)
        .ok_or("no longer has a terminal")?;
    if !agent::takes_prompt(core, &terminal) {
        let idle = terminal
            .agent
            .as_ref()
            .is_some_and(agent::AgentActivity::idle);
        return Err(if idle {
            "is still starting in its terminal"
        } else {
            "is working in its terminal"
        });
    }
    if !rested(core, agent_id, settle_ms) {
        return Err("has just finished a turn");
    }
    Ok(terminal)
}

/// Types `texts` into the agent's terminal as one pasted prompt. They count
/// as delivered only once the agent is seen to take them, and wait again if
/// it does not; Enter is held back from a permission prompt that came up
/// meanwhile.
fn paste(core: &Arc<Core>, agent_id: &str, terminal: Arc<Session>, texts: Vec<Queued>) {
    let typed = session::pasted(&terminal, &joined(&texts));
    took_turn(core, agent_id, &texts);
    core.harness().mailbox.change(|state| {
        state.delivering.insert(agent_id.to_owned());
        state.pasted.insert(
            agent_id.to_owned(),
            Pasted {
                texts,
                entered_at: None,
            },
        );
    });
    agent::note_input(core, &terminal, b"\r");
    session::queue_input(&terminal, typed, session::Typist::Other, Box::new(drop));
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        tokio::time::sleep(ENTER_AFTER_PASTE).await;
        if agent::asks_now(&core, &terminal) {
            unpasted(&core, &agent_id);
            return;
        }
        session::queue_input(
            &terminal,
            b"\r".to_vec(),
            session::Typist::Other,
            Box::new(drop),
        );
        if let Some(pasted) = core
            .harness()
            .mailbox
            .state
            .locked()
            .pasted
            .get_mut(&agent_id)
        {
            pasted.entered_at = Some(epoch_ms());
        }
    });
}

/// The paste the agent did not take waits again, ahead of the rest.
fn unpasted(core: &Core, agent_id: &str) {
    let returned = core.harness().mailbox.change(|state| {
        state.delivering.remove(agent_id);
        let pasted = state.pasted.remove(agent_id)?;
        put_back(state, agent_id, pasted.texts.clone());
        *state.missed_pastes.entry(agent_id.to_owned()).or_default() += 1;
        Some(pasted.texts)
    });
    log(
        core,
        agent_id,
        returned.iter().flatten(),
        MessageOutcome::Returned,
    );
}

/// Types what the person wrote to a terminal agent into its terminal once its CLI takes a
/// prompt, waking it on its saved session first when its terminal has ended.
pub(crate) fn from_person(core: &Arc<Core>, agent_id: &str, text: String) -> CoreResult<()> {
    let text = session::typable(&text).ok_or("there is nothing to send")?;
    worked(core, agent_id);
    person_wrote(core, agent_id);
    let mailbox = &core.harness().mailbox;
    mailbox.wait(
        agent_id,
        Queued {
            text,
            origin: None,
            mark: None,
            paths: Vec::new(),
        },
    );
    mailbox.wake.now();
    Ok(())
}

const STOPPED_NOTE: &str =
    "has taken turn after turn with nobody writing to it and waits for the person";

/// The agent started a turn on `queued`. One nobody waits on counts toward [`MAX_AUTO_TURNS`].
fn took_turn(core: &Core, agent_id: &str, queued: &[Queued]) {
    if awaited(queued) || !queued.iter().any(|queued| capped(core, agent_id, queued)) {
        return;
    }
    let stopped = {
        let mut state = core.harness().mailbox.state.locked();
        let turns = state.auto_turns.entry(agent_id.to_owned()).or_default();
        turns.taken += 1;
        let stops = turns.taken >= MAX_AUTO_TURNS && turns.stopped_at.is_none();
        if stops {
            turns.stopped_at = Some(epoch_ms());
        }
        stops
    };
    if stopped {
        core.changed(What::View);
    }
}

/// The person wrote to the agent, so it may take turns of its own again, and what waited for
/// them goes on.
pub(crate) fn person_wrote(core: &Core, agent_id: &str) {
    let mailbox = &core.harness().mailbox;
    let turns = mailbox.state.locked().auto_turns.remove(agent_id);
    if turns.is_some_and(|turns| turns.stopped_at.is_some()) {
        mailbox.change(|state| {
            let capped = state.capped.remove(agent_id).unwrap_or_default();
            state
                .waiting
                .entry(agent_id.to_owned())
                .or_default()
                .extend(capped);
        });
        core.changed(What::View);
    }
}

/// The agent did something besides messaging other agents, which ends any
/// loop it was in: the messages held between it and another agent go on.
pub(crate) fn worked(core: &Core, agent_id: &str) {
    let mailbox = &core.harness().mailbox;
    let now = epoch_ms();
    let looped = {
        let mut state = mailbox.state.locked();
        state
            .worked
            .retain(|_, at| now.saturating_sub(*at) < PAIR_WINDOW_MS);
        state.worked.insert(agent_id.to_owned(), now);
        state
            .held
            .iter()
            .any(|held| held.from == agent_id || held.to == agent_id)
    };
    if !looped {
        return;
    }
    mailbox.change(|state| {
        let (released, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut state.held)
            .into_iter()
            .partition(|held| held.from == agent_id || held.to == agent_id);
        state.held = kept;
        for held in released {
            state.waiting.entry(held.to).or_default().push(held.queued);
        }
    });
}

/// The agent called a tool, by the name its CLI gives it.
pub(crate) fn used_tool(core: &Core, agent_id: &str, tool: &str) {
    core.seen().used_tool(agent_id);
    core.stalls().worked(core, agent_id);
    if !MESSAGING_TOOLS
        .iter()
        .any(|messaging| tool.ends_with(messaging))
    {
        worked(core, agent_id);
        super::hosts::route::worked(core, agent_id);
    }
}

/// The terminal agent was seen at work on a turn or asking the person
/// during one, so it took what was pasted, if anything was.
pub(crate) fn prompt_taken(core: &Core, agent_id: &str) {
    let mailbox = &core.harness().mailbox;
    let taken = {
        let mut state = mailbox.state.locked();
        state.missed_pastes.remove(agent_id);
        let entered = state
            .pasted
            .get(agent_id)
            .is_some_and(|pasted| pasted.entered_at.is_some());
        if !entered {
            return;
        }
        state.delivering.remove(agent_id);
        let taken = state.pasted.remove(agent_id);
        mailbox.kept.save_soon(&state.record());
        taken
    };
    mailbox.wake.now();
    log(
        core,
        agent_id,
        taken.iter().flat_map(|pasted| &pasted.texts),
        MessageOutcome::Delivered,
    );
}

/// The terminal agent came to rest, or stopped, after a turn. When the turn
/// followed a paste, it took the paste if it went on printing a while after
/// Enter, as a turn does; a swallowed Enter only redraws the prompt box. A
/// turn of its own makes it worth typing into again.
pub(crate) fn prompt_settled(core: &Core, agent_id: &str, last_output_ms: u64) {
    let entered_at = {
        let mut state = core.harness().mailbox.state.locked();
        match state.pasted.get(agent_id) {
            Some(pasted) => pasted.entered_at,
            None => {
                state.missed_pastes.remove(agent_id);
                return;
            }
        }
    };
    match entered_at {
        Some(at) if last_output_ms >= at + TURN_OUTPUT_MS => prompt_taken(core, agent_id),
        Some(_) => unpasted(core, agent_id),
        None => {}
    }
}

/// Gives each agent that has rested since its turn ended the messages
/// waiting for it, wakes an asleep chat that has some, and drops those for
/// agents that are gone. Runs when an agent's state or its messages change,
/// and again when an agent still resting from a turn is due.
pub(crate) fn settle(core: &Arc<Core>) {
    let mailbox = &core.harness().mailbox;
    let capped: Vec<String> = mailbox.state.locked().capped.keys().cloned().collect();
    for agent_id in capped
        .into_iter()
        .filter(|agent_id| core.workspaces().closed(agent_id))
    {
        let dropped = mailbox.change(|state| {
            state.auto_turns.remove(&agent_id);
            state.capped.remove(&agent_id).unwrap_or_default()
        });
        log(core, &agent_id, &dropped, MessageOutcome::Dropped);
    }
    let pasted: Vec<String> = mailbox.state.locked().pasted.keys().cloned().collect();
    for agent_id in pasted {
        if core.agent_session(&agent_id).is_none() {
            unpasted(core, &agent_id);
        }
    }
    let due: Vec<String> = {
        let mut state = mailbox.state.locked();
        let State {
            waiting,
            wake_failed,
            missed_pastes,
            delivering,
            ..
        } = &mut *state;
        wake_failed.retain(|agent_id, _| waiting.contains_key(agent_id));
        missed_pastes.retain(|agent_id, _| waiting.contains_key(agent_id));
        waiting
            .keys()
            .filter(|agent_id| !delivering.contains(*agent_id))
            .cloned()
            .collect()
    };
    let mut next = None;
    for agent_id in due {
        let rest = mailbox.rest_for(&agent_id);
        if let Some(chat) = core.chats().get(&agent_id) {
            if !chat.is_ready() {
                chat.start_agent();
                continue;
            }
            if chat.turn_running() {
                continue;
            }
            match rests_until(core, &agent_id, rest) {
                Some(until) => next = sooner(next, Some(until)),
                None => deliver_later(core, agent_id, Pending::Chat),
            }
        } else if let Some(terminal) = core.agent_session(&agent_id) {
            if mailbox.missed_pastes(&agent_id) >= MAX_MISSED_PASTES
                || !agent::takes_prompt(core, &terminal)
            {
                continue;
            }
            match rests_until(core, &agent_id, rest) {
                Some(until) => next = sooner(next, Some(until)),
                None => {
                    let texts = mailbox.take(&agent_id);
                    if !texts.is_empty() {
                        paste(core, &agent_id, terminal, texts);
                    }
                }
            }
        } else if core
            .workspaces()
            .agent(&agent_id)
            .is_some_and(|agent| agent.asleep || agent.terminal)
        {
            if !core.window().is_open() || core.workspaces().held(&agent_id) {
                continue;
            }
            let failed = mailbox.state.locked().wake_failed.get(&agent_id).copied();
            match failed.map(|at| at + WAKE_RETRY_MS) {
                Some(retry) if retry > epoch_ms() => next = sooner(next, Some(retry)),
                _ => deliver_later(core, agent_id, Pending::Asleep),
            }
        } else if core.workspaces().closed(&agent_id) {
            log(
                core,
                &agent_id,
                &mailbox.take(&agent_id),
                MessageOutcome::Dropped,
            );
        }
    }
    if let Some(next) = next {
        mailbox.wake.at(next);
    }
}

enum Pending {
    Chat,
    Asleep,
}

fn deliver_later(core: &Arc<Core>, agent_id: String, pending: Pending) {
    let mailbox = &core.harness().mailbox;
    if !mailbox.state.locked().delivering.insert(agent_id.clone()) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let mailbox = &core.harness().mailbox;
        match pending {
            Pending::Chat => {
                let texts = mailbox.take(&agent_id);
                if !texts.is_empty() {
                    match hand(&core, &agent_id, &texts).await {
                        Ok(_) => log(&core, &agent_id, &texts, MessageOutcome::Delivered),
                        Err(_) => mailbox.put_back(&agent_id, texts),
                    }
                }
            }
            Pending::Asleep if core.workspaces().held(&agent_id) => {}
            Pending::Asleep => {
                let woke = launching::wake(&core, agent_id.clone()).await;
                let mut state = mailbox.state.locked();
                match woke {
                    Ok(_) => state.wake_failed.remove(&agent_id),
                    Err(_) => state.wake_failed.insert(agent_id.clone(), epoch_ms()),
                };
            }
        }
        mailbox.state.locked().delivering.remove(&agent_id);
        mailbox.wake.now();
    });
}

/// Runs `read` off the async threads on the project at `project`, which must be an absolute
/// path, under the project's own path.
async fn in_project<T: Send + 'static>(
    core: &Arc<Core>,
    project: &str,
    read: impl FnOnce(&Core, String) -> T + Send + 'static,
) -> CoreResult<T> {
    let path = Path::new(project);
    if !path.is_absolute() {
        return Err("project must be the project's absolute path".into());
    }
    let (core, path) = (core.clone(), path.to_owned());
    blocking(move || {
        let project = core.workspaces().project_of(&path);
        Ok(read(&core, project))
    })
    .await
}

/// The project's log of messages between agents, newest first, under the
/// project's own path, which keys it in the inbox's `agentMessages`.
pub(crate) async fn agent_messages(
    core: &Arc<Core>,
    params: AgentMessagesParams,
) -> CoreResult<Value> {
    let limit = params.limit;
    in_project(core, &params.project, move |core, project| {
        let entries = logged(core, &project, limit);
        json!({ "project": project, "entries": entries })
    })
    .await
}

/// The project's messages between agents folded into one row each, newest first.
pub(crate) async fn agent_message_rows(
    core: &Arc<Core>,
    params: AgentMessagesParams,
) -> CoreResult<AgentMessageRows> {
    in_project(core, &params.project, |core, project| {
        let rows = logged_rows(core, &project);
        AgentMessageRows { project, rows }
    })
    .await
}
