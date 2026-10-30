//! What `agent_list` and `agent_send` answer: the agents running here, and
//! messages between them. A message reaches an agent as text marked as coming
//! from the agent that sent it, so it can never answer a permission request.
//! A busy chat takes it into its turn when it can; an agent that cannot gets
//! the messages waiting for it together once its turn ends.

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
use crate::protocol::{AgentStatus, ChatState, WorkspaceAgent};

use self::log::{Entry, MessageLog, Outcome};
use super::chat::{self, Delivery};
use super::connection::blocking;
use super::kept::Kept;
use super::session::{self, Session};
use super::wake::{sooner, Wake};
use super::workspace::project_of;
use super::{agent, launching, Core, CoreResult};

pub(crate) mod log;

const FILE: &str = "agent-mailbox.json";
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_WAITING: usize = 10;
const BOARD: &str = "board";
const TRIGGER: &str = "trigger";
const AUTO: &str = "auto";
/// Notes about teammates on linked hosts come from here.
const HOSTS: &str = "hosts";
const HOSTS_NOTE: &str =
    "Sent by Sikemux about a teammate on another host, not by the person or another agent.";
const SIKEMUX: &str = "sikemux";
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

/// What a message between agents is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Kind {
    Message,
    /// How a worker's turn ended, sent to its lead.
    Report,
    Question,
    Answer,
    /// A note from Sikemux itself, such as two agents changing the same lines.
    Notice,
}

struct Letter {
    kind: Kind,
    from_id: String,
    from_name: String,
    cli: String,
    text: String,
    at: u64,
    /// What the agent reading it is told about it, when not the usual.
    note: Option<String>,
    /// The question this answers, when the person answered one.
    answers: Option<Answered>,
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
    pub kind: Kind,
    pub from_id: String,
    pub from_name: String,
    pub cli: String,
    pub text: String,
    pub note: Option<String>,
    /// The person's answer to a question, copied to the lead it went to first.
    pub answered: Option<Answered>,
    /// The id of the question of its own that a lead answers.
    pub question: Option<String>,
}

impl Letter {
    /// Sent by a part of Sikemux that carries work on, not by an agent.
    fn by_sikemux(&self) -> bool {
        self.from_id == BOARD || self.from_id == TRIGGER
    }

    fn carried(&self, to: &str, question: Option<String>) -> Carried {
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

/// Who sent a message and what it is.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Origin {
    from: String,
    kind: Kind,
}

/// A message waiting for its agent, as the agent reads it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Queued {
    text: String,
    /// Unknown for a message that came back unread.
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<Origin>,
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
    kind: Kind,
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
                    && sent.kind != Kind::Question
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
        self.kept.save(&state.record());
        drop(state);
        self.wake.now();
        changed
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
        // however alike they read, and so do a board note and a trigger's firing.
        if let Some(repeat) =
            repeat.filter(|_| letter.kind == Kind::Message && !letter.by_sikemux())
        {
            return Ok(Admitted::Repeat {
                seconds_ago: now.saturating_sub(repeat.at) / 1000,
            });
        }
        if letter.kind == Kind::Message && state.looping(&letter.from_id, to) {
            return Ok(Admitted::Held);
        }
        let report = letter.kind == Kind::Report;
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

    /// The pairs of agents whose messages to each other are held.
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
                kind: Kind::Report,
            }) = &queued.origin
            {
                let (older, kept) = std::mem::take(waiting).into_iter().partition(|waiting| {
                    waiting
                        .origin
                        .as_ref()
                        .is_some_and(|origin| origin.kind == Kind::Report && &origin.from == from)
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
        self.put_back(agent_id, vec![Queued { text, origin: None }]);
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
        format!("Sent by {name}, another agent in Sikemux, not by the person. Reply with agent_send only with an answer, a decision or a blocker, never just to acknowledge.")
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

/// The task a lead gives an agent it started, as that agent reads it.
pub(crate) fn task(
    lead_id: &str,
    lead_name: &str,
    lead_cli: &str,
    reports: bool,
    text: String,
) -> String {
    let answer = if reports {
        format!("When your turn ends, your final answer goes to {lead_name} by itself. Message {lead_name} with agent_send if you need to ask it something first.")
    } else {
        format!("Your answers do not go to {lead_name} by themselves: send {lead_name} what it needs with agent_send when you are done.")
    };
    wrapped(&Letter {
        kind: Kind::Message,
        from_id: lead_id.to_owned(),
        from_name: lead_name.to_owned(),
        cli: lead_cli.to_owned(),
        text,
        at: epoch_ms(),
        answers: None,
        note: Some(format!(
            "{lead_name}, another agent in Sikemux, started you to do this; it is your lead, not the person. {answer}"
        )),
    })
}

/// Hands a new agent its task: into a turn, or ahead of whatever waits for
/// it when it is already busy with something else.
pub(crate) async fn hand_task(
    core: &Arc<Core>,
    agent_id: &str,
    task: String,
) -> Result<(), String> {
    let mailbox = &core.harness.mailbox;
    let from = core
        .names
        .get(agent_id)
        .and_then(|named| named.parent)
        .unwrap_or_else(|| AUTO.to_owned());
    let queued = Queued {
        text: task.clone(),
        origin: Some(Origin {
            from,
            kind: Kind::Message,
        }),
    };
    if mailbox.has_waiting(agent_id) {
        log(core, agent_id, [&queued], Outcome::Waiting);
        mailbox.put_back(agent_id, vec![queued]);
        return Ok(());
    }
    match chat::letter(core, agent_id, task).await {
        Ok(delivered @ (Delivery::Steered | Delivery::NewTurn)) => {
            let outcome = match delivered {
                Delivery::Steered => Outcome::Steered,
                _ => Outcome::NewTurn,
            };
            log(core, agent_id, [&queued], outcome);
            Ok(())
        }
        Ok(Delivery::TurnEnded | Delivery::Refused) => {
            log(core, agent_id, [&queued], Outcome::Waiting);
            mailbox.put_back(agent_id, vec![queued]);
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// The latest messages between agents in `project`, newest first.
pub(crate) fn logged(core: &Core, project: &str, limit: Option<usize>) -> Vec<Entry> {
    let limit = limit.unwrap_or(log::MAX_READ).min(log::MAX_READ);
    core.harness.mailbox.log.read(project, limit)
}

/// The project's messages between agents, one row each, newest first.
pub(crate) fn logged_rows(core: &Core, project: &str) -> Vec<log::Row> {
    log::rows(&logged(core, project, None), |id| core.names.name(id))
}

/// Notes in the log of the project of agent `to` what became of messages
/// for it.
fn log<'a>(core: &Core, to: &str, queued: impl IntoIterator<Item = &'a Queued>, outcome: Outcome) {
    let queued: Vec<&Queued> = queued.into_iter().collect();
    let answers = queued.iter().any(|queued| {
        queued
            .origin
            .as_ref()
            .is_some_and(|origin| origin.kind == Kind::Answer)
    });
    if answers && !matches!(outcome, Outcome::Waiting | Outcome::Returned) {
        core.harness.questions.delivered(to);
    }
    let Some(project) = core.names.get(to).map(|named| named.project) else {
        return;
    };
    let at = epoch_ms();
    let to_name = core.names.name(to);
    let mut logged = false;
    for queued in queued {
        let from = queued.origin.as_ref().map(|origin| origin.from.clone());
        core.harness.mailbox.log.append(
            &project,
            &Entry {
                at,
                from_name: from.as_deref().and_then(|from| core.names.name(from)),
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
        core.view_changed();
    }
}

/// How an agent can be reached.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    Chat,
    Asleep,
    Terminal,
    Stopped,
    /// Through the linked host it runs on.
    Remote,
}

#[derive(Clone)]
pub(crate) struct Listed {
    pub agent_id: String,
    pub name: Option<String>,
    pub project: String,
    pub cli: String,
    pub title: Option<String>,
    pub status: AgentStatus,
    parent: Option<String>,
    active_at: Option<u64>,
    rank: u32,
    pub reach: Reach,
}

impl Listed {
    /// Its status, with an agent at rest that has a task or messages waiting
    /// for it counted as about to start its next turn.
    pub(crate) fn standing(&self, core: &Core) -> AgentStatus {
        let resting = matches!(
            self.status,
            AgentStatus::Idle | AgentStatus::Done | AgentStatus::Asleep
        );
        if resting && core.harness.mailbox.has_waiting(&self.agent_id) {
            AgentStatus::Starting
        } else {
            self.status
        }
    }
}

/// Every agent the core runs or the app holds, in the order the device view
/// puts them. Reads the disk to place agents in projects.
pub(crate) fn listed(core: &Core) -> Vec<Listed> {
    let (chats, sessions) = core.ranked_infos();
    let projects = core.workspaces.project_paths();
    let place = |agent_id: &str, cwd: Option<&std::path::Path>| {
        core.names
            .get(agent_id)
            .map(|named| (named.project, named.parent))
            .unwrap_or_else(|| {
                let project = cwd.map_or_else(String::new, |cwd| project_of(cwd, &projects));
                (project, None)
            })
    };
    let mut agents: Vec<Listed> = chats
        .into_iter()
        .map(|chat| {
            let (project, parent) = place(&chat.agent_id, Some(&chat.cwd));
            let reach = match chat.state {
                ChatState::Asleep => Reach::Asleep,
                ChatState::Stopped => Reach::Stopped,
                ChatState::Starting | ChatState::Ready => Reach::Chat,
            };
            Listed {
                name: chat.name,
                project,
                cli: chat.provider,
                title: chat.title,
                status: chat.status,
                parent,
                active_at: chat.active_at,
                rank: chat.attention_rank,
                reach,
                agent_id: chat.agent_id,
            }
        })
        .collect();
    for session in sessions {
        let (Some(agent_id), Some(rank), Some(status)) =
            (session.agent_id, session.attention_rank, session.status)
        else {
            continue;
        };
        let folder = session
            .project
            .as_deref()
            .and_then(|id| core.workspaces.project_info(id))
            .map(|project| project.path);
        let (project, parent) = place(&agent_id, folder.as_deref());
        agents.push(Listed {
            name: session.agent_name,
            project,
            cli: session.agent_type.unwrap_or_else(|| "terminal".into()),
            title: session.title,
            status,
            parent,
            active_at: session.active_at,
            rank,
            reach: Reach::Terminal,
            agent_id,
        });
    }
    let away: Vec<WorkspaceAgent> = core
        .workspaces
        .agent_ids()
        .into_iter()
        .filter(|agent_id| !agents.iter().any(|listed| &listed.agent_id == agent_id))
        .filter_map(|agent_id| core.workspaces.agent(&agent_id))
        .filter(|agent| agent.terminal)
        .collect();
    for agent in away {
        let (project, parent) = place(&agent.id, Some(&agent.cwd));
        agents.push(Listed {
            name: core.names.get(&agent.id).map(|named| named.name),
            project,
            title: core.workspaces.title(&agent.id),
            cli: agent.provider,
            status: AgentStatus::Asleep,
            parent,
            active_at: None,
            rank: u32::MAX,
            reach: Reach::Asleep,
            agent_id: agent.id,
        });
    }
    agents.sort_by_key(|agent| agent.rank);
    agents.extend(core.hosts.team.all().into_iter().map(|foreign| Listed {
        name: Some(foreign.name),
        project: foreign.project,
        cli: foreign.cli,
        title: None,
        status: foreign.status,
        parent: foreign.parent,
        active_at: foreign.active_at,
        rank: u32::MAX,
        reach: Reach::Remote,
        agent_id: foreign.agent_id,
    }));
    agents
}

/// The host an agent on a linked host runs on, as this host calls it.
fn host_of(core: &Core, agent: &Listed) -> Option<String> {
    (agent.reach == Reach::Remote)
        .then(|| core.hosts.team.get(&agent.agent_id))
        .flatten()
        .map(|foreign| super::hosts::name_of(core, &foreign.host))
}

fn since(at: Option<u64>, now: u64) -> Option<String> {
    let seconds = now.saturating_sub(at?) / 1000;
    Some(match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    })
}

pub(crate) fn status_word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Blocked => "waiting on the person",
        AgentStatus::Working => "working",
        AgentStatus::Done => "done",
        AgentStatus::Idle => "idle",
        AgentStatus::Asleep => "asleep",
        AgentStatus::Stopped => "stopped",
        AgentStatus::Starting => "starting",
    }
}

/// The project the calling agent belongs to.
pub(crate) fn caller_project(core: &Core, request: &HarnessRequest) -> String {
    request
        .agent_id
        .as_deref()
        .and_then(|agent_id| core.names.get(agent_id))
        .map(|named| named.project)
        .unwrap_or_else(|| {
            core.workspaces
                .project_of(std::path::Path::new(&request.project))
        })
}

pub(crate) async fn list(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let all = match request.params.get("allProjects") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(all)) => *all,
        Some(_) => return Err("allProjects must be a boolean".into()),
    };
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = caller_project(&core, &request);
        let caller = request.agent_id.clone().unwrap_or_default();
        let agents = listed(&core);
        let names: HashMap<String, String> = agents
            .iter()
            .filter_map(|agent| Some((agent.agent_id.clone(), agent.name.clone()?)))
            .collect();
        let now = epoch_ms();
        let rows: Vec<Value> = agents
            .iter()
            .filter(|agent| all || agent.project == project)
            .map(|agent| {
                let host = host_of(&core, agent);
                let reachable = core
                    .hosts
                    .team
                    .get(&agent.agent_id)
                    .is_none_or(|foreign| core.hosts.reachable(&foreign.host));
                let status = if reachable {
                    status_word(agent.standing(&core))
                } else {
                    "out of reach"
                };
                let mut row = json!({
                    "name": agent.name,
                    "agentId": agent.agent_id,
                    "cli": agent.cli,
                    "title": agent.title,
                    "status": status,
                    "lastActive": since(agent.active_at, now),
                    "canMessage": agent.reach != Reach::Stopped
                        && agent.agent_id != caller
                        && blind_between(&core, &caller, &agent.agent_id, "").is_ok(),
                });
                let fields = row.as_object_mut().expect("a row is an object");
                if let Some(parent) = &agent.parent {
                    fields.insert(
                        "startedBy".into(),
                        names
                            .get(parent)
                            .cloned()
                            .unwrap_or_else(|| parent.clone())
                            .into(),
                    );
                }
                if agent.agent_id == caller {
                    fields.insert("you".into(), true.into());
                }
                if let Some(host) = host {
                    fields.insert("host".into(), host.into());
                }
                if all {
                    fields.insert("project".into(), agent.project.clone().into());
                }
                row
            })
            .collect();
        Ok(json!({ "project": project, "agents": rows }))
    })
    .await
    .map_err(|error| error.to_string())
}

/// Closes agents the caller started, or that one of its workers started, as
/// the person closing their tabs would. Their chats stay in Recent.
pub(crate) async fn close(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let caller = request
        .agent_id
        .clone()
        .ok_or("agent_close needs the calling agent's id")?;
    let wanted: Vec<String> = match request.params.get("agents") {
        Some(Value::Array(agents)) if !agents.is_empty() => agents
            .iter()
            .map(|agent| agent.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or("agents must be names or ids")?,
        _ => return Err("agents must list the agents to close".into()),
    };
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let project = caller_project(&core, &request);
        let agents = listed(&core);
        let mut closing = Vec::new();
        for to in &wanted {
            let target = resolve(&core, &agents, to, &project, &caller)?;
            let name = target.name.clone().unwrap_or(target.agent_id.clone());
            if target.reach == Reach::Remote {
                return Err(format!(
                    "{name} runs on another host; ask it to finish, and the person closes its tab"
                )
                .into());
            }
            if !core.names.leads(&caller, &target.agent_id) {
                return Err(format!(
                    "You did not start {name}, so closing it is the person's or its lead's to do"
                )
                .into());
            }
            closing.push((target.agent_id, name));
        }
        let ids: Vec<String> = closing.iter().map(|(id, _)| id.clone()).collect();
        super::workspace::close(&core, &ids)?;
        let closed: Vec<String> = closing.into_iter().map(|(_, name)| name).collect();
        Ok(json!({ "closed": closed }))
    })
    .await
    .map_err(|error| error.to_string())
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

/// The agent `to` names: an agent id, or a name, preferring the caller's
/// project.
pub(crate) fn resolve(
    core: &Core,
    agents: &[Listed],
    to: &str,
    project: &str,
    caller: &str,
) -> Result<Listed, String> {
    let wanted = to.trim().trim_start_matches('@');
    if let Some(agent) = agents.iter().find(|agent| agent.agent_id == wanted) {
        return Ok(agent.clone());
    }
    let (wanted, on) = on_host(wanted);
    let named = |agent: &&Listed| {
        agent
            .name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
            && on.is_none_or(|on| {
                host_of(core, agent).is_some_and(|host| host.eq_ignore_ascii_case(on))
            })
    };
    let (yours, called): (Vec<&Listed>, Vec<&Listed>) = agents
        .iter()
        .filter(named)
        .partition(|agent| agent.agent_id == caller);
    if called.is_empty() && !yours.is_empty() {
        return Err("That agent is you".into());
    }
    let here: Vec<&Listed> = called
        .iter()
        .copied()
        .filter(|agent| agent.project == project)
        .collect();
    match (here.as_slice(), called.as_slice()) {
        ([agent], _) | ([], [agent]) => Ok((*agent).clone()),
        ([], []) => Err(format!(
            "No open agent is called {wanted}; agent_list shows the agents you can message. One that closed is found by agent_sessions, or who_wrote for its commit, and reopened by agent_spawn with resume set to its sessionId"
        )),
        (_, many) => Err(format!(
            "More than one agent is called {wanted}: {}. Pass the agent id instead",
            many.iter()
                .map(|agent| match host_of(core, agent) {
                    Some(host) => format!("{} on {host}", agent.agent_id),
                    None => format!("{} in {}", agent.agent_id, agent.project),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// An agent's name and the host it is said to run on, as in "Otter on
/// studio" or "Otter@studio".
fn on_host(wanted: &str) -> (&str, Option<&str>) {
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

/// Sends one message to every other agent of `from`'s team: the agent that
/// leads it and every agent it started, each delivered as a message of its own.
async fn send_team(
    core: &Arc<Core>,
    request: &HarnessRequest,
    from: &str,
) -> Result<Value, String> {
    let top = core.names.team_of(from);
    let members: Vec<String> = core
        .names
        .team(&top)
        .into_iter()
        .filter(|member| member != from)
        .collect();
    if members.is_empty() {
        return Err("You have no team: no agent leads you and you started none".into());
    }
    let mut sent = Vec::new();
    let mut failed = Vec::new();
    for member in members {
        let mut one = request.clone();
        one.params["to"] = Value::String(member.clone());
        let name = core.names.name(&member).unwrap_or_else(|| member.clone());
        match send(core, &one).await {
            Ok(answer) => {
                sent.push(serde_json::json!({ "to": name, "delivery": answer["delivery"] }))
            }
            Err(why) => failed.push(serde_json::json!({ "to": name, "error": why })),
        }
    }
    Ok(serde_json::json!({ "sent": sent, "failed": failed }))
}

pub(crate) async fn send(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let from = request
        .agent_id
        .clone()
        .ok_or("agent_send needs the calling agent's id")?;
    let to = match request.params.get("to") {
        Some(Value::String(to)) if !to.trim().is_empty() && to.len() <= 200 => to.clone(),
        _ => return Err("to must be the agent's name or id".into()),
    };
    let text = message_text(&request.params)?;
    let answers = match request.params.get("answers") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if !id.trim().is_empty() => Some(id.trim().to_owned()),
        Some(_) => return Err("answers must be the id of the question this answers".into()),
    };
    if to.trim().eq_ignore_ascii_case(TEAM) {
        if answers.is_some() {
            return Err("answers goes to one agent, not the team".into());
        }
        return Box::pin(send_team(core, request, &from)).await;
    }
    let (target, letter) = {
        let (core, request, answers) = (core.clone(), request.clone(), answers.clone());
        blocking(move || {
            let project = caller_project(&core, &request);
            let agents = listed(&core);
            let target = resolve(&core, &agents, &to, &project, &from)?;
            if target.agent_id == from {
                return Err("That agent is you".into());
            }
            let name = target.name.as_deref().unwrap_or(&target.agent_id);
            if let Err(why) = blind_between(&core, &from, &target.agent_id, name) {
                let refused = Queued {
                    text: String::new(),
                    origin: Some(Origin {
                        from: from.clone(),
                        kind: Kind::Message,
                    }),
                };
                log(&core, &target.agent_id, [&refused], Outcome::Refused);
                return Err(why.into());
            }
            let mut letter = letter_from(&agents, Kind::Message, from, text, None);
            if let Some(id) = answers.as_ref().filter(|_| target.reach != Reach::Remote) {
                let name = target.name.as_deref().unwrap_or(&target.agent_id);
                let asked = core
                    .harness
                    .questions
                    .asked_of(&letter.from_id, &target.agent_id, id)
                    .ok_or_else(|| format!("{name} has no open question {id} that it asked you; it was answered already, or leave out answers to send this as a plain message"))?;
                letter.kind = Kind::Answer;
                letter.note = Some(format!(
                    "Sent by {}, your lead in Sikemux, not by the person, answering the question you asked: {asked}",
                    letter.from_name
                ));
            }
            Ok((target, letter))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let from = letter.from_id.clone();
    if target.reach == Reach::Remote {
        return carry(core, &target, letter, answers).await;
    }
    let sent = deliver(core, &target, letter).await?;
    core.seen.called(&target.agent_id);
    core.agents_changed();
    if let Some(id) = answers {
        core.harness
            .questions
            .lead_answered(&from, &target.agent_id, &id);
    }
    Ok(sent)
}

/// Sends a letter to its agent on a linked host, noting it here as sent so
/// two agents that keep messaging each other across hosts are held too.
async fn carry(
    core: &Arc<Core>,
    target: &Listed,
    letter: Letter,
    question: Option<String>,
) -> Result<Value, String> {
    core.harness.mailbox.note_sent(&letter, &target.agent_id);
    let queued = Queued {
        text: String::new(),
        origin: Some(Origin {
            from: letter.from_id.clone(),
            kind: letter.kind,
        }),
    };
    let sent = super::hosts::route::deliver(
        core,
        &target.agent_id,
        letter.carried(&target.agent_id, question),
    )
    .await;
    let outcome = match &sent {
        Ok(answer) => match answer["delivery"].as_str() {
            Some("steered") => Outcome::Steered,
            Some("newTurn") => Outcome::NewTurn,
            Some("repeat") => Outcome::Repeat,
            _ => Outcome::Waiting,
        },
        Err(_) => Outcome::Refused,
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
    };
    if let Some((branch, count)) = landed {
        letter.text.push_str(&format!(
            "\n\nIts commits came back to the branch {branch} in your checkout: {count} on top of the commit you started it from, the first holding your uncommitted changes as they were then. Merge what you want of them."
        ));
    }
    if let Some(id) = &carried.question {
        let asked = core
            .harness
            .questions
            .asked_of(&letter.from_id, &target.agent_id, id)
            .ok_or_else(|| format!("{name} has no open question {id} that it asked you; it was answered already, or leave out answers to send this as a plain message"))?;
        letter.kind = Kind::Answer;
        letter.note = Some(format!(
            "Sent by {}, your lead in Sikemux, not by the person, answering the question you asked: {asked}",
            letter.from_name
        ));
    }
    let from = letter.from_id.clone();
    let sent = deliver(core, &target, letter).await?;
    if let Some(id) = carried.question {
        core.harness
            .questions
            .lead_answered(&from, &target.agent_id, &id);
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
    let letter = Letter {
        kind: Kind::Message,
        from_id: HOSTS.to_owned(),
        from_name: "Sikemux".to_owned(),
        cli: "sikemux".to_owned(),
        text,
        at: epoch_ms(),
        note: Some(HOSTS_NOTE.to_owned()),
        answers: None,
    };
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
    if core.names.is_blind(from) {
        return Err("You review a card blind for Auto, so you may not reach other agents; ask the person with ask if you must".into());
    }
    if core.names.is_blind(to) {
        return Err(format!(
            "{to_name} reviews a card blind for Auto, so no agent may reach it"
        ));
    }
    Ok(())
}

/// Puts the question `text` from one agent to its lead, with `note` in
/// place of the usual one. Fails when the lead has stopped or is gone.
pub(crate) async fn question(
    core: &Arc<Core>,
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
            Ok((
                target,
                letter_from(&agents, Kind::Question, from, text, Some(note)),
            ))
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
    answers: Vec<(Answered, String, String)>,
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
    let mut letters = answers.into_iter().map(|(answered, text, note)| Letter {
        kind: Kind::Answer,
        from_id: "person".to_owned(),
        from_name: "the person".to_owned(),
        cli: "person".to_owned(),
        text,
        at: epoch_ms(),
        note: Some(note),
        answers: Some(answered),
    });
    let letter = letters.next().ok_or("there is no answer to send")?;
    let rest: Vec<Letter> = letters.collect();
    if rest.is_empty() {
        return deliver(core, &target, letter).await;
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
    deliver_as(core, &target, letter, text).await
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

/// Hands an agent a note from Sikemux's board about its cards. Fails when the
/// agent has stopped or is gone.
pub(crate) async fn from_board(
    core: &Arc<Core>,
    to: String,
    text: String,
    note: String,
) -> Result<Value, String> {
    let target = running(core, to).await?;
    let letter = Letter {
        kind: Kind::Message,
        from_id: BOARD.to_owned(),
        from_name: "Board".to_owned(),
        cli: "sikemux".to_owned(),
        text,
        at: epoch_ms(),
        note: Some(note),
        answers: None,
    };
    deliver(core, &target, letter).await
}

/// Hands an agent the prompt of a trigger it set, waking it first when it sleeps. Fails when
/// the agent has stopped or is gone.
pub(crate) async fn from_trigger(
    core: &Arc<Core>,
    to: String,
    text: String,
    note: String,
) -> Result<Value, String> {
    let mut target = running(core, to.clone()).await?;
    if target.reach == Reach::Asleep {
        launching::wake(core, to.clone())
            .await
            .map_err(|error| format!("it is asleep and did not wake: {error}"))?;
        target = running(core, to).await?;
    }
    let letter = Letter {
        kind: Kind::Message,
        from_id: TRIGGER.to_owned(),
        from_name: "Trigger".to_owned(),
        cli: "sikemux".to_owned(),
        text,
        at: epoch_ms(),
        note: Some(note),
        answers: None,
    };
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
    let letter = Letter {
        kind: Kind::Notice,
        from_id: SIKEMUX.to_owned(),
        from_name: "Sikemux".to_owned(),
        cli: "sikemux".to_owned(),
        text,
        at: epoch_ms(),
        note: Some("Sent by Sikemux itself, not by the person or another agent.".to_owned()),
        answers: None,
    };
    deliver(core, &target, letter).await
}

const AUTO_NOTE: &str = "Sent by Sikemux's Auto, which runs this project's board for the person, not by the person or another agent.";

fn auto_letter(text: String) -> Letter {
    Letter {
        kind: Kind::Message,
        from_id: AUTO.to_owned(),
        from_name: "Auto".to_owned(),
        cli: "sikemux".to_owned(),
        text,
        at: epoch_ms(),
        note: Some(AUTO_NOTE.to_owned()),
        answers: None,
    }
}

/// The task Auto gives an agent it started, as that agent reads it.
pub(crate) fn auto_task(text: String) -> String {
    wrapped(&auto_letter(text))
}

/// Auto tells an agent of its own what its card needs next.
pub(crate) async fn from_auto(core: &Arc<Core>, to: String, text: String) -> Result<Value, String> {
    let target = running(core, to).await?;
    deliver(core, &target, auto_letter(text)).await
}

/// Drops the messages holding `mark` that still wait for the agent.
pub(crate) fn withdraw(core: &Core, agent_id: &str, mark: &str) {
    if !is_waiting(core, agent_id, mark) {
        return;
    }
    let withdrawn = core.harness.mailbox.change(|state| {
        let Some(waiting) = state.waiting.get_mut(agent_id) else {
            return Vec::new();
        };
        let (withdrawn, kept) = std::mem::take(waiting)
            .into_iter()
            .partition(|queued| queued.text.contains(mark));
        *waiting = kept;
        withdrawn
    });
    log(core, agent_id, &withdrawn, Outcome::Dropped);
}

/// Whether a message holding `mark` still waits for the agent's turn to end.
pub(crate) fn is_waiting(core: &Core, agent_id: &str, mark: &str) -> bool {
    core.harness
        .mailbox
        .state
        .locked()
        .waiting
        .get(agent_id)
        .is_some_and(|waiting| waiting.iter().any(|queued| queued.text.contains(mark)))
}

fn letter_from(
    agents: &[Listed],
    kind: Kind,
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
    }
}

/// Hands `text` from `worker` to the agent that started it, as a message
/// about how its turn ended. A lead that has stopped gets it once it is
/// opened again; only one the person closed goes without.
pub(crate) async fn report(core: &Arc<Core>, worker: String, lead: String, text: String) {
    let name = core.names.name(&worker);
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
            let note = format!(
                "{name} is an agent you started, and this is how its turn ended, sent by Sikemux. Reply with agent_send to {name} if it needs more."
            );
            let mut letter = letter_from(&agents, Kind::Report, worker, text, Some(note));
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
    deliver_as(core, target, letter, text).await
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
    };
    let handed = handed(core, target, letter, &queued).await;
    let outcome = handed
        .as_ref()
        .map_or(Outcome::Refused, |(_, outcome)| *outcome);
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
) -> Result<(Value, Outcome), String> {
    let name = target
        .name
        .clone()
        .unwrap_or_else(|| target.agent_id.clone());
    let answer = |delivery: &str, note: String| json!({ "to": name, "agentId": target.agent_id, "delivery": delivery, "note": note });
    let mailbox = &core.harness.mailbox;
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
                Outcome::Repeat,
            ))
        }
        Admitted::Held => {
            let dropped = mailbox.hold(&letter.from_id, &target.agent_id, queued.clone());
            log(core, &target.agent_id, &dropped, Outcome::Dropped);
            return Ok((
                answer(
                    "held",
                    format!("You and {name} keep messaging each other without either doing other work, so Sikemux holds this and has asked the person. It goes to {name} once one of you does other work or the person writes to one of you; carry on with your own work."),
                ),
                Outcome::Waiting,
            ));
        }
    }
    let mut woke = "";
    let mut held = None;
    let mut reach = target.reach;
    let notice = letter.kind == Kind::Notice;
    match target.reach {
        Reach::Chat | Reach::Terminal => {}
        Reach::Asleep if notice => held = Some("is asleep"),
        Reach::Asleep if !core.window.is_open() => held = Some("is asleep while Sikemux is closed"),
        Reach::Asleep => {
            launching::wake(core, target.agent_id.clone())
                .await
                .map_err(|error| format!("{name} is asleep and did not wake: {error}"))?;
            mailbox.state.locked().wake_failed.remove(&target.agent_id);
            woke = " Sikemux woke it first.";
            if core.agent_session(&target.agent_id).is_some() {
                reach = Reach::Terminal;
            }
        }
        Reach::Stopped if letter.answers.is_some() || letter.kind == Kind::Report => {
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
            super::board::reached_by_more_than_offer(core, &target.agent_id);
        }
    }
    if held.is_none() && mailbox.has_waiting(&target.agent_id) {
        held = Some("already has messages waiting");
    }
    if held.is_none() {
        let delivered =
            match reach {
                Reach::Terminal => ready_terminal(core, &target.agent_id).map(|terminal| {
                    paste(core, &target.agent_id, terminal, vec![queued.clone()]);
                    Outcome::Typed
                }),
                _ => hand(core, &target.agent_id, &queued.text)
                    .await
                    .map(|delivered| match delivered {
                        Delivery::Steered => Outcome::Steered,
                        _ => Outcome::NewTurn,
                    }),
            };
        match delivered {
            Ok(Outcome::Steered) => {
                return Ok((
                    answer(
                        "steered",
                        format!("{name} is working and reads this at its next step.{woke}"),
                    ),
                    Outcome::Steered,
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
            Outcome::Dropped,
        ));
    }
    let (waiting, superseded) = mailbox.wait(&target.agent_id, queued.clone());
    log(core, &target.agent_id, &superseded, Outcome::Dropped);
    let busy = held.unwrap_or_default();
    Ok((
        answer(
            "waiting",
            format!("{name} {busy}, so it gets this once it is free, with any others waiting ({waiting} now).{woke}"),
        ),
        Outcome::Waiting,
    ))
}

/// When the agent will have rested since its last turn, unless it has.
fn rests_until(core: &Core, agent_id: &str) -> Option<u64> {
    core.seen
        .active_at(agent_id)
        .map(|at| at + SETTLE_MS)
        .filter(|until| *until > epoch_ms())
}

fn rested(core: &Core, agent_id: &str) -> bool {
    rests_until(core, agent_id).is_none()
}

/// Hands `text` to the chat as one message, or says why it cannot take it
/// now.
async fn hand(core: &Core, agent_id: &str, text: &str) -> Result<Delivery, &'static str> {
    let chat = core.chats.get(agent_id).ok_or("is not running")?;
    if !chat.is_ready() {
        chat.start_agent();
        return Err("is still starting");
    }
    if !chat.turn_running() && !rested(core, agent_id) {
        return Err("has just finished a turn");
    }
    match chat::letter(core, agent_id, text.to_owned()).await {
        Ok(delivered @ (Delivery::Steered | Delivery::NewTurn)) => Ok(delivered),
        Ok(Delivery::TurnEnded) => Err("has just finished a turn"),
        Ok(Delivery::Refused) | Err(_) => Err("is working and cannot read messages mid-turn"),
    }
}

/// The terminal of an agent that waits for a prompt, or why it cannot take
/// one now.
fn ready_terminal(core: &Core, agent_id: &str) -> Result<Arc<Session>, &'static str> {
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
    if !rested(core, agent_id) {
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
    core.harness.mailbox.change(|state| {
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
            .harness
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
    let returned = core.harness.mailbox.change(|state| {
        state.delivering.remove(agent_id);
        let pasted = state.pasted.remove(agent_id)?;
        put_back(state, agent_id, pasted.texts.clone());
        *state.missed_pastes.entry(agent_id.to_owned()).or_default() += 1;
        Some(pasted.texts)
    });
    log(core, agent_id, returned.iter().flatten(), Outcome::Returned);
}

/// Types what the person wrote to a terminal agent into its terminal once its CLI takes a
/// prompt, waking it on its saved session first when its terminal has ended.
pub(crate) fn from_person(core: &Arc<Core>, agent_id: &str, text: String) -> CoreResult<()> {
    let text = session::typable(&text).ok_or("there is nothing to send")?;
    worked(core, agent_id);
    let mailbox = &core.harness.mailbox;
    mailbox.wait(agent_id, Queued { text, origin: None });
    mailbox.wake.now();
    Ok(())
}

/// The agent did something besides messaging other agents, which ends any
/// loop it was in: the messages held between it and another agent go on.
pub(crate) fn worked(core: &Core, agent_id: &str) {
    let mailbox = &core.harness.mailbox;
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
    let mailbox = &core.harness.mailbox;
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
        mailbox.kept.save(&state.record());
        taken
    };
    mailbox.wake.now();
    log(
        core,
        agent_id,
        taken.iter().flat_map(|pasted| &pasted.texts),
        Outcome::Delivered,
    );
}

/// The terminal agent came to rest, or stopped, after a turn. When the turn
/// followed a paste, it took the paste if it went on printing a while after
/// Enter, as a turn does; a swallowed Enter only redraws the prompt box. A
/// turn of its own makes it worth typing into again.
pub(crate) fn prompt_settled(core: &Core, agent_id: &str, last_output_ms: u64) {
    let entered_at = {
        let mut state = core.harness.mailbox.state.locked();
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
    let mailbox = &core.harness.mailbox;
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
        if let Some(chat) = core.chats.get(&agent_id) {
            if !chat.is_ready() {
                chat.start_agent();
                continue;
            }
            if chat.turn_running() {
                continue;
            }
            match rests_until(core, &agent_id) {
                Some(until) => next = sooner(next, Some(until)),
                None => deliver_later(core, agent_id, Pending::Chat),
            }
        } else if let Some(terminal) = core.agent_session(&agent_id) {
            if mailbox.missed_pastes(&agent_id) >= MAX_MISSED_PASTES
                || !agent::takes_prompt(core, &terminal)
            {
                continue;
            }
            match rests_until(core, &agent_id) {
                Some(until) => next = sooner(next, Some(until)),
                None => {
                    let texts = mailbox.take(&agent_id);
                    if !texts.is_empty() {
                        paste(core, &agent_id, terminal, texts);
                    }
                }
            }
        } else if core
            .workspaces
            .agent(&agent_id)
            .is_some_and(|agent| agent.asleep || agent.terminal)
        {
            if !core.window.is_open() {
                continue;
            }
            let failed = mailbox.state.locked().wake_failed.get(&agent_id).copied();
            match failed.map(|at| at + WAKE_RETRY_MS) {
                Some(retry) if retry > epoch_ms() => next = sooner(next, Some(retry)),
                _ => deliver_later(core, agent_id, Pending::Asleep),
            }
        } else if core.workspaces.closed(&agent_id) {
            log(core, &agent_id, &mailbox.take(&agent_id), Outcome::Dropped);
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
    let mailbox = &core.harness.mailbox;
    if !mailbox.state.locked().delivering.insert(agent_id.clone()) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let mailbox = &core.harness.mailbox;
        match pending {
            Pending::Chat => {
                let texts = mailbox.take(&agent_id);
                if !texts.is_empty() {
                    match hand(&core, &agent_id, &joined(&texts)).await {
                        Ok(_) => log(&core, &agent_id, &texts, Outcome::Delivered),
                        Err(_) => mailbox.put_back(&agent_id, texts),
                    }
                }
            }
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
