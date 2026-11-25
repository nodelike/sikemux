//! What waits on the person, ranked once here so the rail, the notch and
//! phones list it the same way: whatever needs the person first, then changes
//! to review, then finished work, then work under way. Built from what the
//! core already keeps; only how each agent's latest turn went is kept here.

pub(crate) use super::chat::Fault;
use crate::server::changed::{ChangedCore, What};
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use serde::{Deserialize, Serialize};
pub use sikemux_calls::inbox::DigestParams;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::git::Git;
use crate::text::{clipped, clipped_line};
use sikemux_view::inbox::{
    AgentRollup, AgentShown, Inbox, InboxCard, InboxDigest, InboxHandoff, InboxItem, InboxKind,
    InboxNeed, InboxRecording, InboxTier, RemoteAgent, ShipHold, ShipKind,
};
use sikemux_view::workspace::WorkspaceAgent;
use sikemux_wire::protocol::{
    AgentGroup, AgentState, AgentStatus, ChatInfo, SessionId, SessionInfo,
};

use super::board::{self, Summary};
use super::connection::blocking;
use super::display;
use super::hosts::team::{Foreign, Role};
use super::kept::Kept;
use super::notify::{provider_name, Feat};
use super::roster;
use super::turn_state;
use super::{Core, CoreResult};
use sikemux_view::backdrop::Moments;
use sikemux_view::chat::FailureKind;
use sikemux_view::host_stats::HostAlertKind;
use sikemux_view::inbox::Celebration;
use sikemux_view::push::ActivityGlance;
use sikemux_view::turn::{TurnState, WorkAt};

/// How each agent's turn stands, by agent, as one view worked it out.
type Turned = HashMap<String, TurnState>;

/// Agents whose latest turn is kept; the longest-finished go first.
const MAX_TURNS: usize = 512;
const TURNS_FILE: &str = "turns.json";
const MAX_TURN_RECORDINGS: usize = 8;
const MAX_FILES: usize = 20;
const MAX_ANSWER_CHARS: usize = 600;
const MAX_LINE_CHARS: usize = 200;
const DONE_UNCOMMITTED: &str = "Done, uncommitted";
/// The files of an agent's whose state git is asked for at once.
const MAX_STATUS_FILES: usize = 256;

/// What an item tells of, which alone decides its kind, what it needs the person for, and
/// so where it ranks.
#[derive(Clone)]
enum Case {
    Permission,
    /// `secret` when the answer is something only the person has, such as a password.
    Question {
        secret: bool,
    },
    Input,
    /// Its chat holds a card with the connectors the person signs in to.
    SignIn,
    /// An agent's action that leaves this machine waits for the person to allow it.
    Ship(ShipHold),
    /// An agent proposed the project's environments, for the person to accept.
    ProposedEnvironments,
    /// Something about a host needs the person.
    HostAlert,
    /// The account vault needs the person.
    Vault,
    /// Two agents keep messaging each other and nothing else.
    Looping,
    /// An agent took turn after turn on its own, so what else would wake it waits for the person.
    Stopped,
    /// An agent at work has called no tool for a long while.
    NoProgress,
    /// Two agents keep changing the same lines.
    Colliding,
    /// The project's check keeps failing after agents' commits.
    CheckFailing,
    /// Something Sikemux keeps for the project's agents no longer holds.
    StaleMemory,
    /// The turn ran into a problem, of this kind when the agent's driver knew it.
    Failed(Option<Fault>),
    Changes,
    Finished,
    Working,
}

/// Who an item waits on: the person, for something, or nobody, so it shows quietly.
enum Wants {
    Person(InboxNeed),
    Nobody(Quiet),
}

/// Where an item that waits on nobody shows.
enum Quiet {
    Review,
    Finished,
    Progress,
}

impl Case {
    fn placed(&self) -> (InboxKind, Wants) {
        use InboxNeed::{Decide, Fix, Unblock};
        match self {
            Case::Permission => (InboxKind::Permission, Wants::Person(Decide)),
            Case::Question { secret: false } => (InboxKind::Question, Wants::Person(Decide)),
            Case::Question { secret: true } => (InboxKind::Question, Wants::Person(Unblock)),
            Case::Input => (InboxKind::Input, Wants::Person(Unblock)),
            Case::SignIn => (InboxKind::SignIn, Wants::Person(Unblock)),
            Case::Ship(_) => (InboxKind::Ship, Wants::Person(Decide)),
            Case::Stopped | Case::NoProgress => (InboxKind::Held, Wants::Person(Unblock)),
            Case::Looping | Case::CheckFailing | Case::StaleMemory => {
                (InboxKind::Held, Wants::Person(Fix))
            }
            Case::Colliding => (InboxKind::Held, Wants::Person(Decide)),
            Case::ProposedEnvironments => (InboxKind::ProposedEnvironments, Wants::Person(Decide)),
            Case::HostAlert => (InboxKind::HostAlert, Wants::Person(Fix)),
            Case::Vault => (InboxKind::Vault, Wants::Person(Unblock)),
            Case::Failed(Some(Fault::Account)) => (InboxKind::Failed, Wants::Person(Unblock)),
            Case::Failed(Some(Fault::Broke)) => (InboxKind::Failed, Wants::Person(Fix)),
            Case::Failed(Some(Fault::Retrying) | None) => {
                (InboxKind::Failed, Wants::Nobody(Quiet::Finished))
            }
            Case::Changes => (InboxKind::Changes, Wants::Nobody(Quiet::Review)),
            Case::Finished => (InboxKind::Finished, Wants::Nobody(Quiet::Finished)),
            Case::Working => (InboxKind::Working, Wants::Nobody(Quiet::Progress)),
        }
    }
}

/// Kinds whose own signal tells the person as they happen, with what answers them: a
/// permission, a question, a terminal waiting for input, a turn's problem. A sign-in card shows
/// in its chat alone. The inbox tells of every other item when it first needs the person.
fn signalled(kind: InboxKind) -> bool {
    matches!(
        kind,
        InboxKind::Permission
            | InboxKind::Question
            | InboxKind::Input
            | InboxKind::SignIn
            | InboxKind::Failed
    )
}

/// Stops an agent until the person acts, so it goes before the older items that need them.
fn pressing(kind: InboxKind) -> bool {
    matches!(
        kind,
        InboxKind::Permission | InboxKind::Question | InboxKind::Input | InboxKind::SignIn
    )
}

fn agent_shown(agent_id: String, cli: String, title: String, status: AgentStatus) -> AgentShown {
    let group = status.group();
    AgentShown {
        agent_id,
        chief: false,
        name: None,
        cli,
        title,
        titled: false,
        session: None,
        status,
        group,
        label: display::group_label(group).to_owned(),
        turn: None,
        unread: false,
        subagents: 0,
        tasks: 0,
        busy: matches!(group, AgentGroup::Blocked | AgentGroup::Working),
        failed: false,
        background: false,
        active_at: None,
        started_at: None,
        next_trigger: None,
        triggers: 0,
        line: None,
        called_at: None,
        face: None,
        lead: None,
        project: None,
        project_name: None,
        reconnecting: None,
        snoozed_until: None,
        settled_at: None,
        tags: Vec::new(),
    }
}

fn rollup<'a>(agents: impl IntoIterator<Item = &'a AgentShown>) -> Option<AgentRollup> {
    agents
        .into_iter()
        .fold(None, |rolled: Option<AgentRollup>, agent| {
            let group = rolled
                .as_ref()
                .map_or(agent.group, |rolled| rolled.group.min(agent.group));
            Some(AgentRollup {
                group,
                label: display::group_label(group).to_owned(),
                subagents: rolled.as_ref().map_or(0, |rolled| rolled.subagents) + agent.subagents,
                tasks: rolled.as_ref().map_or(0, |rolled| rolled.tasks) + agent.tasks,
            })
        })
}

/// A teammate on a linked host as the rows here show it.
pub(crate) fn foreign_shown(core: &Core, foreign: &Foreign) -> AgentShown {
    let project = (core.workspaces().project_names().remove(&foreign.project)).or_else(|| {
        let folder = Path::new(&foreign.project).file_name()?;
        Some(folder.to_string_lossy().into_owned())
    });
    shown_here(foreign, project, || provider_name(core, &foreign.cli))
}

/// A teammate's own host's row of it, in the project here its team works in, called
/// `project`. What that row says only of its host is left behind: its terminal there and its
/// place on a team there. Until its host sends a row, what this host knows of it stands in,
/// titled `untitled`.
pub(crate) fn shown_here(
    foreign: &Foreign,
    project: Option<String>,
    untitled: impl FnOnce() -> String,
) -> AgentShown {
    let said = foreign.said.as_ref().map(|said| said.shown.clone());
    let mut shown = said.unwrap_or_else(|| {
        let mut shown = agent_shown(
            foreign.agent_id.clone(),
            foreign.cli.clone(),
            untitled(),
            foreign.status,
        );
        shown.active_at = foreign.active_at;
        shown
    });
    shown.name.get_or_insert_with(|| foreign.name.clone());
    shown.project = Some(foreign.project.clone());
    shown.project_name = project;
    shown.session = None;
    shown.lead = foreign.parent.clone();
    shown
}

/// The workers agents here started on linked hosts.
fn remote_agents(core: &Core) -> Vec<RemoteAgent> {
    core.hosts()
        .team
        .all()
        .into_iter()
        .filter(|foreign| foreign.role == Role::Worker)
        .map(|foreign| RemoteAgent {
            shown: foreign_shown(core, &foreign),
            host: super::hosts::name_of(core, &foreign.host),
            reachable: core.hosts().reachable(&foreign.host),
            host_id: foreign.host,
        })
        .collect()
}

/// What went wrong in a turn, and how, when the agent's driver knew.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Problem {
    said: String,
    fault: Option<Fault>,
    /// What kind of failure the agent's driver saw, when it saw one.
    kind: Option<FailureKind>,
}

/// An agent's latest turn, kept through a restart so how it stands is still known.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Turn {
    started_at: u64,
    /// Unset while it runs, and when the person stopped it.
    ended_at: Option<u64>,
    problem: Option<Problem>,
    recordings: Vec<InboxRecording>,
    /// Worked out the first time it is asked for once the turn ended.
    #[serde(skip)]
    handoff: Option<InboxHandoff>,
    /// The agent committed during it.
    committed: bool,
    /// What it left open, from the agent's plan as it ended.
    left: Option<String>,
    /// The commit a deploy of the agent's to a place before production passed at, whichever
    /// turn deployed it.
    staged: Option<String>,
    /// Where git said the chat's work was, read when this turn ended and on each commit since.
    work: Option<turn_state::Work>,
    /// When the ask it works on began: its latest turn the person or its own lead started, which
    /// every other turn carries on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asked_at: Option<u64>,
    /// How many times what it left was let go, so a handoff worked out before that is not kept.
    #[serde(skip)]
    let_go: u64,
    /// Where its work is will be read again, for a file it wrote that came to light late.
    #[serde(skip)]
    rereading: bool,
    /// Whoever hears of the agent was told of the work this turn left behind.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    told_left: bool,
    /// The agent that stopped it, until the next turn begins or one that may start it writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_by: Option<String>,
}

pub(crate) struct Turns {
    kept: Kept,
    turns: Mutex<HashMap<String, Turn>>,
    handoff_refreshing: Mutex<HashSet<String>>,
}

/// The agent's latest turn, making room for it among the kept ones.
fn turn<'a>(turns: &'a mut HashMap<String, Turn>, agent_id: &str) -> &'a mut Turn {
    if turns.len() >= MAX_TURNS && !turns.contains_key(agent_id) {
        let oldest = turns
            .iter()
            .min_by_key(|(_, turn)| turn.ended_at.unwrap_or(turn.started_at))
            .map(|(agent_id, _)| agent_id.clone());
        if let Some(oldest) = oldest {
            turns.remove(&oldest);
        }
    }
    turns.entry(agent_id.to_owned()).or_default()
}

impl super::part::Part for Turns {
    fn flush(&self) {
        self.flush();
    }
}

impl Turns {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, TURNS_FILE);
        let turns = Mutex::new(kept.load());
        Self {
            kept,
            turns,
            handoff_refreshing: Mutex::default(),
        }
    }

    /// Changes the turns kept, saving them.
    fn change<T>(&self, change: impl FnOnce(&mut HashMap<String, Turn>) -> T) -> T {
        let mut turns = self.turns.locked();
        let out = change(&mut turns);
        self.kept.save_soon(&*turns);
        out
    }

    pub(crate) fn started(&self, agent_id: &str) {
        self.change(|turns| {
            let earlier = turns.remove(agent_id).unwrap_or_default();
            *turn(turns, agent_id) = Turn {
                started_at: epoch_ms(),
                staged: earlier.staged,
                asked_at: earlier.asked_at,
                ..Turn::default()
            };
        });
    }

    /// The person or the agent's own lead started its latest turn, so a new ask began with it.
    pub(crate) fn asked(&self, agent_id: &str) {
        self.change(|turns| {
            if let Some(turn) = turns.get_mut(agent_id) {
                turn.asked_at = Some(turn.started_at);
            }
        });
    }

    /// Where the agent's work is, as git last said, once a turn of its that started at or after
    /// `since` has ended.
    pub(crate) fn worked_since(&self, agent_id: &str, since: u64) -> Option<WorkAt> {
        let turns = self.turns.locked();
        let turn = turns.get(agent_id)?;
        (turn.started_at >= since && turn.ended_at.is_some())
            .then(|| turn.work.as_ref().map(|work| work.changes))
            .flatten()
    }

    /// Whether whoever hears of the agent was told of the work its latest turn left behind.
    pub(crate) fn told_left(&self, agent_id: &str) -> bool {
        (self.turns.locked().get(agent_id)).is_some_and(|turn| turn.told_left)
    }

    pub(crate) fn tell_left(&self, agent_id: &str) {
        self.change(|turns| {
            if let Some(turn) = turns.get_mut(agent_id) {
                turn.told_left = true;
            }
        });
    }

    pub(crate) fn flush(&self) {
        self.kept.flush();
    }

    /// The turn ended by itself; `problem` says what went wrong, if anything.
    pub(crate) fn ended(&self, agent_id: &str, problem: Option<&str>, left: Option<String>) {
        self.change(|turns| {
            let turn = turn(turns, agent_id);
            turn.ended_at = Some(epoch_ms());
            turn.left = left;
            if problem.is_none() {
                turn.stop_by = None;
            }
            if let Some(said) = problem {
                match &mut turn.problem {
                    Some(problem) => said.clone_into(&mut problem.said),
                    None => {
                        turn.problem = Some(Problem {
                            said: said.to_owned(),
                            fault: None,
                            kind: None,
                        })
                    }
                }
            }
        });
    }

    /// The chat cannot send its failed turn's prompt again after all, so the turn waits on
    /// the person's account.
    pub(crate) fn not_retrying(&self, agent_id: &str) {
        self.change(|turns| {
            let problem = turns
                .get_mut(agent_id)
                .and_then(|turn| turn.problem.as_mut());
            if let Some(problem) = problem.filter(|problem| problem.fault == Some(Fault::Retrying))
            {
                problem.fault = Some(Fault::Account);
            }
        });
    }

    /// The agent's driver saw its turn fail, and how.
    pub(crate) fn failed(
        &self,
        agent_id: &str,
        said: &str,
        fault: Fault,
        kind: Option<FailureKind>,
    ) {
        self.change(|turns| {
            turn(turns, agent_id).problem = Some(Problem {
                said: said.to_owned(),
                fault: Some(fault),
                kind,
            });
        });
    }

    /// The agent committed, so what its last turn left is worked out again.
    pub(crate) fn committed(&self, agent_id: &str) {
        self.change(|turns| {
            if let Some(turn) = turns.get_mut(agent_id) {
                turn.handoff = None;
                turn.let_go += 1;
                turn.committed = true;
            }
        });
    }

    /// A file the agent wrote came to light after its latest turn ended by itself, so what that
    /// turn left is worked out again. Whether where its work is should be read again: not while a
    /// read is waiting to start already, nor once it reads uncommitted, which more files keep.
    pub(crate) fn wrote_after(&self, agent_id: &str) -> bool {
        self.change(|turns| {
            let Some(turn) = turns
                .get_mut(agent_id)
                .filter(|turn| turn.ended_at.is_some())
            else {
                return false;
            };
            turn.handoff = None;
            turn.let_go += 1;
            let uncommitted =
                (turn.work.as_ref()).is_some_and(|work| work.changes == WorkAt::Uncommitted);
            let read = !turn.rereading && !uncommitted;
            turn.rereading |= read;
            read
        })
    }

    /// The read [`Self::wrote_after`] asked for starts, so a file that comes to light from now on
    /// asks for another.
    pub(crate) fn rereading(&self, agent_id: &str) {
        if let Some(turn) = self.turns.locked().get_mut(agent_id) {
            turn.rereading = false;
        }
    }

    /// How the agent's latest turn ended, as its turn state reads it; none while it runs and
    /// before its first.
    pub(crate) fn ending(&self, agent_id: &str) -> Option<turn_state::Ended> {
        let turns = self.turns.locked();
        let turn = turns.get(agent_id)?;
        Some(turn_state::Ended {
            at: turn.ended_at,
            started_at: turn.started_at,
            asked_at: turn.asked_at,
            left: turn.left.clone(),
            staged: turn.staged.clone(),
            problem: (turn.problem.as_ref()).map(|problem| turn_state::Problem {
                said: problem.said.clone(),
                fault: problem.fault,
                kind: problem.kind,
            }),
            work: turn.work.clone(),
            stop_by: turn.stop_by.clone(),
        })
    }

    /// A deploy of the agent's work at `head` went to a place before production, and passed.
    pub(crate) fn staged(&self, agent_id: &str, head: String) {
        self.change(|turns| {
            if let Some(turn) = turns.get_mut(agent_id) {
                turn.staged = Some(head);
            }
        });
    }

    /// The agents whose latest turn changed a page of `project` and whose work is at one of
    /// `covered`, with when that turn began and that commit.
    pub(crate) fn showing(&self, project: &str, covered: &[String]) -> Vec<(String, u64)> {
        (self.turns.locked().iter())
            .filter(|(_, turn)| {
                turn.work.as_ref().is_some_and(|work| {
                    work.can_try
                        && work.project == project
                        && work
                            .head
                            .as_ref()
                            .is_some_and(|head| covered.contains(head))
                })
            })
            .map(|(agent, turn)| (agent.clone(), turn.started_at))
            .collect()
    }

    /// Whether the agent changed anything in its latest turn, as far as the core heard.
    pub(crate) fn changed_in_turn(&self, agent_id: &str) -> bool {
        (self.turns.locked().get(agent_id)).is_some_and(|turn| turn.committed)
    }

    /// How many times what the agent's latest turn left was let go, which a read of its work
    /// started now hands back to [`Self::keep_read_work`].
    pub(crate) fn let_go_count(&self, agent_id: &str) -> u64 {
        (self.turns.locked().get(agent_id)).map_or(0, |turn| turn.let_go)
    }

    /// Keeps where git said the agent's work is, read from when what its turn left was let go
    /// `let_go` times, answering whether that moved. A read that started before a file
    /// came to light or a commit is dropped, since the newer read that follows them must not be
    /// overwritten by it.
    pub(crate) fn keep_read_work(
        &self,
        agent_id: &str,
        work: turn_state::Work,
        let_go: u64,
    ) -> bool {
        self.change(|turns| match turns.get_mut(agent_id) {
            Some(turn) if turn.let_go == let_go => {
                let moved = turn.work.as_ref() != Some(&work);
                turn.work = Some(work);
                moved
            }
            _ => false,
        })
    }

    #[cfg(test)]
    pub(crate) fn keep_work(&self, agent_id: &str, work: turn_state::Work) -> bool {
        self.keep_read_work(agent_id, work, self.let_go_count(agent_id))
    }

    /// Whether the agent's latest turn ended by itself, with no problem.
    fn ended_well(&self, agent_id: &str) -> bool {
        (self.turns.locked().get(agent_id))
            .is_some_and(|turn| turn.ended_at.is_some() && turn.problem.is_none())
    }

    pub(crate) fn stopped(&self, agent_id: &str) {
        self.change(|turns| {
            if let Some(turn) = turns.get_mut(agent_id) {
                turn.ended_at = None;
            }
        });
    }

    /// The agent `by` stops the agent, which then waits until one that may start it writes.
    pub(crate) fn stop_by(&self, agent_id: &str, by: &str) {
        self.change(|turns| turn(turns, agent_id).stop_by = Some(by.to_owned()));
    }

    /// The agent that stopped the agent, which waits since.
    pub(crate) fn stopped_by(&self, agent_id: &str) -> Option<String> {
        (self.turns.locked().get(agent_id)).and_then(|turn| turn.stop_by.clone())
    }

    /// The agent may start again. Answers whether an agent had stopped it.
    pub(crate) fn may_start(&self, agent_id: &str) -> bool {
        let held = self.stopped_by(agent_id).is_some();
        if held {
            self.change(|turns| turn(turns, agent_id).stop_by = None);
        }
        held
    }

    fn snapshot(
        &self,
        agent_id: &str,
    ) -> Option<(Option<u64>, Option<Problem>, Vec<InboxRecording>)> {
        self.turns
            .locked()
            .get(agent_id)
            .map(|turn| (turn.ended_at, turn.problem.clone(), turn.recordings.clone()))
    }

    fn ended_between(&self, since: u64, until: Option<u64>) -> Vec<String> {
        let within = |at: u64| at >= since && until.is_none_or(|until| at <= until);
        self.turns
            .locked()
            .iter()
            .filter(|(_, turn)| turn.ended_at.is_some_and(within))
            .map(|(agent_id, _)| agent_id.clone())
            .collect()
    }
}

/// Notes a browser recording an agent finished, on its turn and its card.
pub(crate) fn recorded(core: &Arc<Core>, agent_id: &str, answer: &Value) {
    let (Some(path), Some(seconds)) = (
        answer.get("path").and_then(Value::as_str),
        answer.get("seconds").and_then(Value::as_f64),
    ) else {
        return;
    };
    let chat = core.chats().get(agent_id);
    let recording = InboxRecording {
        path: path.to_owned(),
        seconds,
        at: epoch_ms(),
        feed: chat.as_ref().map(|chat| chat.feed.id().to_owned()),
        turn: chat.as_ref().and_then(|chat| chat.feed.turn_start()),
    };
    core.inbox().change(|turns| {
        let turn = turn(turns, agent_id);
        if turn.recordings.len() >= MAX_TURN_RECORDINGS {
            turn.recordings.remove(0);
        }
        turn.recordings.push(recording.clone());
    });
    board::attach_recording(core, agent_id, recording);
}

/// What the agent left when its last turn ended, worked out once.
pub(crate) fn handoff(core: &Core, agent_id: &str) -> InboxHandoff {
    let kept = |turn: Option<&Turn>| turn.map(|turn| (turn.ended_at, turn.let_go));
    let before = {
        let turns = core.inbox().turns.locked();
        let turn = turns.get(agent_id);
        if let Some(handoff) = turn.and_then(|turn| turn.handoff.clone()) {
            return handoff;
        }
        kept(turn)
    };
    let mut handoff = InboxHandoff::default();
    let mut check_git: Option<(String, PathBuf)> = None;
    if let Some(chat) = core.chats().get(agent_id) {
        let reply = chat.feed.last_answer();
        handoff.offer = reply.as_deref().and_then(super::questions::offer_in);
        handoff.answer = reply.map(|answer| clipped(&answer, MAX_ANSWER_CHARS));
        let project = core.names().get(agent_id).map(|named| named.project);
        let filter = serde_json::from_value(json!({
            "project": project.clone().unwrap_or_default(),
            "agentId": agent_id,
            "feed": chat.feed.id(),
            "turn": chat.feed.turn_start(),
        }));
        if let (Some(project), Ok(filter)) = (project, filter) {
            let mut seen = HashSet::new();
            for edit in core.edits().read(&project, &filter) {
                if seen.insert(edit.path.clone()) && handoff.files.len() < MAX_FILES {
                    handoff.files.push(edit.path);
                }
            }
            handoff.edited = seen.len();
            if ever_edited(core, &project, agent_id) {
                check_git = Some((project, chat.launch.cwd.clone()));
            }
        }
    }
    if let Some((project, cwd)) = check_git {
        if let (Some(arc), Ok(handle)) = (core.arc(), tokio::runtime::Handle::try_current()) {
            {
                let mut turns = core.inbox().turns.locked();
                if kept(turns.get(agent_id)) == before {
                    turn(&mut turns, agent_id).handoff = Some(handoff.clone());
                }
            }
            if core
                .inbox()
                .handoff_refreshing
                .locked()
                .insert(agent_id.to_owned())
            {
                let agent = agent_id.to_owned();
                handle.spawn(async move {
                    let is_uncommitted = {
                        let (arc, agent, project, cwd) =
                            (arc.clone(), agent.clone(), project.clone(), cwd.clone());
                        tokio::task::spawn_blocking(move || {
                            uncommitted(&arc, &project, &agent, &cwd)
                        })
                        .await
                        .unwrap_or(false)
                    };
                    let changed = {
                        let mut turns = arc.inbox().turns.locked();
                        let updated = if kept(turns.get(&agent)) == before {
                            if let Some(cached) =
                                turns.get_mut(&agent).and_then(|t| t.handoff.as_mut())
                            {
                                let prev = cached.uncommitted;
                                cached.uncommitted = is_uncommitted;
                                prev != is_uncommitted
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        arc.inbox().handoff_refreshing.locked().remove(&agent);
                        updated
                    };
                    if changed {
                        arc.changed(What::View);
                    }
                });
            }
            return handoff;
        }
        handoff.uncommitted = uncommitted(core, &project, agent_id, &cwd);
    }
    let mut turns = core.inbox().turns.locked();
    if kept(turns.get(agent_id)) == before {
        turn(&mut turns, agent_id).handoff = Some(handoff.clone());
    }
    handoff
}

/// Whether the agent changed any file in `project`, in any turn of its chat.
pub(crate) fn ever_edited(core: &Core, project: &str, agent_id: &str) -> bool {
    let filter = serde_json::from_value(json!({ "project": project, "agentId": agent_id }));
    filter.is_ok_and(|filter| !core.edits().read(project, &filter).is_empty())
}

/// Whether a file the agent changed in any turn of its chat holds changes git has not committed,
/// so a later turn that changed nothing does not hide them.
pub(crate) fn uncommitted(core: &Core, project: &str, agent_id: &str, cwd: &Path) -> bool {
    !uncommitted_files(core, project, agent_id, cwd).is_empty()
}

/// The files the agent changed in `project` that are not committed yet.
pub(crate) fn uncommitted_files(
    core: &Core,
    project: &str,
    agent_id: &str,
    cwd: &Path,
) -> Vec<String> {
    let files = edited_files(core, project, agent_id);
    (checkouts_holding(&files, cwd).iter())
        .filter_map(|(checkout, files)| {
            let listed = uncommitted_in(checkout, files.get(..MAX_STATUS_FILES).unwrap_or(files))?;
            Some(listed.into_iter().map(|file| {
                let path = checkout.join(file);
                (path.strip_prefix(cwd).unwrap_or(&path))
                    .to_string_lossy()
                    .into_owned()
            }))
        })
        .flatten()
        .collect()
}

/// The git checkouts around or under `cwd` that hold files the agent changed in `project`,
/// for an agent whose folder is not a checkout itself, such as a folder of several repositories.
pub(crate) fn edited_checkouts(
    core: &Core,
    project: &str,
    agent_id: &str,
    cwd: &Path,
) -> Vec<PathBuf> {
    checkouts_holding(&edited_files(core, project, agent_id), cwd)
        .into_keys()
        .collect()
}

fn edited_files(core: &Core, project: &str, agent_id: &str) -> Vec<String> {
    let filter = serde_json::from_value(json!({ "project": project, "agentId": agent_id }));
    let Ok(filter) = filter else {
        return Vec::new();
    };
    let mut files: Vec<String> = (core.edits().read(project, &filter).into_iter())
        .map(|edit| edit.path)
        .collect();
    files.sort_unstable();
    files.dedup();
    files
}

/// Which of `files` git at `checkout` lists as changed, relative to the checkout; None when
/// git there cannot say.
fn uncommitted_in(checkout: &Path, files: &[String]) -> Option<Vec<String>> {
    let mut status = vec!["status", "--porcelain", "-z", "--"];
    status.extend(files.iter().map(String::as_str));
    let listed = Git::at(checkout).text(&status).ok()?;
    let mut out = Vec::new();
    let mut entries = listed.split('\0');
    while let Some(entry) = entries.next() {
        let Some(path) = entry.get(3..) else {
            continue;
        };
        let bytes = entry.as_bytes();
        let renamed_or_copied =
            matches!(bytes.first(), Some(b'R' | b'C')) || matches!(bytes.get(1), Some(b'R' | b'C'));
        if renamed_or_copied {
            let _ = entries.next();
        }
        out.push(path.to_owned());
    }
    Some(out)
}

/// `files` by the git checkout each is in, keeping the checkouts around or under `cwd`: a file
/// the agent changed in a checkout elsewhere, such as its notes, is not this agent's work here.
fn checkouts_holding(files: &[String], cwd: &Path) -> BTreeMap<PathBuf, Vec<String>> {
    let mut held: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for file in files {
        let checkout = (Path::new(file).parent())
            .and_then(sikemux_process::files::repo_root)
            .filter(|checkout| checkout.starts_with(cwd) || cwd.starts_with(checkout));
        if let Some(checkout) = checkout {
            held.entry(checkout.to_path_buf())
                .or_default()
                .push(file.clone());
        }
    }
    held
}

/// How an agent's latest turn left things, as a row about the agent says it.
#[derive(Default)]
pub(crate) struct Left {
    /// The turn ended by itself, with no problem.
    pub ended_well: bool,
    pub words: Option<String>,
    pub offer: Option<String>,
}

pub(crate) fn left(core: &Core, agent_id: &str) -> Left {
    if !core.inbox().ended_well(agent_id) {
        return Left::default();
    }
    let handoff = handoff(core, agent_id);
    let words = match (handoff.uncommitted, &handoff.offer) {
        (true, _) => Some(DONE_UNCOMMITTED.to_owned()),
        (false, Some(_)) => Some("Done".to_owned()),
        (false, None) => None,
    };
    Left {
        ended_well: true,
        words,
        offer: handoff.offer,
    }
}

/// An agent as the inbox names it.
#[derive(Clone)]
struct Agent {
    id: String,
    name: Option<String>,
    cli: Option<String>,
    project: Option<String>,
    title: Option<String>,
    session: Option<SessionId>,
    group: AgentGroup,
    at: u64,
    /// The linked host it runs on, when it is not this one.
    host: Option<String>,
}

fn agents(core: &Core, chats: &[ChatInfo], sessions: &[SessionInfo]) -> Vec<Agent> {
    let project = |agent_id: &str| core.names().get(agent_id).map(|named| named.project);
    let mut agents: Vec<Agent> = chats
        .iter()
        .map(|chat| Agent {
            id: chat.agent_id.clone(),
            name: chat.name.clone(),
            cli: Some(chat.provider.clone()),
            project: project(&chat.agent_id)
                .or_else(|| Some(core.workspaces().project_of(&chat.cwd))),
            title: chat.title.clone(),
            session: None,
            group: chat.group,
            at: chat.active_at.unwrap_or(0),
            host: None,
        })
        .collect();
    for (session, agent_id) in roster::terminals(chats, sessions) {
        let Some(group) = session.group.filter(|_| session.running) else {
            continue;
        };
        agents.push(Agent {
            id: agent_id.to_owned(),
            name: session.agent_name.clone(),
            cli: session.agent_type.clone(),
            project: project(agent_id).or_else(|| {
                let folder = roster::terminal_folder(core, session)?;
                Some(core.workspaces().project_of(&folder))
            }),
            title: session.title.clone(),
            session: Some(session.id),
            group,
            at: session.active_at.unwrap_or(0),
            host: None,
        });
    }
    for agent in &mut agents {
        if agent.group == AgentGroup::Working {
            agent.at = core.seen().started_at(&agent.id).unwrap_or(agent.at);
        }
    }
    agents.extend(core.hosts().team.all().into_iter().map(|foreign| {
        let shown = foreign_shown(core, &foreign);
        Agent {
            host: Some(super::hosts::name_of(core, &foreign.host)),
            id: foreign.agent_id,
            name: shown.name,
            cli: Some(foreign.cli),
            project: Some(foreign.project),
            title: shown.titled.then_some(shown.title),
            session: None,
            group: match foreign.role == Role::Worker && core.hosts().reachable(&foreign.host) {
                true => shown.group,
                false => AgentGroup::Idle,
            },
            at: shown.active_at.unwrap_or(0),
        }
    }));
    agents
}

/// An agent not running now, as one asleep or closed, as the names remember it.
fn gone_agent(core: &Core, agent_id: &str) -> Agent {
    let named = core.names().get(agent_id);
    Agent {
        id: agent_id.to_owned(),
        name: named.as_ref().map(|named| named.name.clone()),
        cli: None,
        project: named.map(|named| named.project).or_else(|| {
            let agent = core.workspaces().agent(agent_id)?;
            Some(core.workspaces().project_of(&agent.cwd))
        }),
        title: None,
        session: None,
        group: AgentGroup::Idle,
        at: core.seen().active_at(agent_id).unwrap_or(0),
        host: None,
    }
}

fn item(id: String, case: Case, what: String, at: u64) -> InboxItem {
    let (kind, wants) = case.placed();
    let (tier, need) = match wants {
        Wants::Person(need) => (InboxTier::Needs, Some(need)),
        Wants::Nobody(Quiet::Review) => (InboxTier::Review, None),
        Wants::Nobody(Quiet::Finished) => (InboxTier::Finished, None),
        Wants::Nobody(Quiet::Progress) => (InboxTier::Progress, None),
    };
    InboxItem {
        heading: String::new(),
        about: None,
        id,
        tier,
        kind,
        need,
        agent_id: None,
        name: None,
        cli: None,
        project: None,
        project_name: None,
        title: None,
        what,
        detail: None,
        asked: None,
        at,
        session: None,
        feed: None,
        turn: None,
        card: None,
        gate: None,
        environment: match &case {
            Case::Ship(hold) => hold.environment.clone(),
            _ => None,
        },
        evidence: None,
        hold: match case {
            Case::Ship(hold) => Some(hold),
            _ => None,
        },
        request_id: None,
        choices: None,
        question_id: None,
        answers: Vec::new(),
        pictures: Vec::new(),
        url: None,
        handoff: None,
        recordings: Vec::new(),
        host: None,
        host_alert: None,
    }
}

fn rank(item: &InboxItem) -> (InboxTier, bool, i128, &str) {
    let at = i128::from(item.at);
    let at = if item.tier == InboxTier::Needs {
        at
    } else {
        -at
    };
    (item.tier, !pressing(item.kind), at, &item.id)
}

/// Fills in who an item is about.
trait About: Sized {
    fn by(self, agent_id: &str, agent: Option<&Agent>) -> Self;
    fn on_card(self, card: Option<&Summary>) -> Self;
}

impl About for InboxItem {
    fn by(mut self, agent_id: &str, agent: Option<&Agent>) -> Self {
        self.agent_id = Some(agent_id.to_owned());
        if let Some(agent) = agent {
            self.host = agent.host.clone();
            self.name = agent.name.clone();
            self.cli = agent.cli.clone();
            self.project = agent.project.clone();
            self.title = agent.title.clone();
            self.session = agent.session;
        }
        self
    }

    fn on_card(mut self, card: Option<&Summary>) -> Self {
        let Some(card) = card else {
            return self;
        };
        self.card = Some(InboxCard {
            project: card.project.clone(),
            id: card.id,
            title: card.title.clone(),
            goal: card.goal,
        });
        self.project.get_or_insert_with(|| card.project.clone());
        if self.agent_id.is_none() {
            self.agent_id = card.owner.clone();
            self.name = card.owner_name.clone();
        }
        for recording in &card.recordings {
            if !self
                .recordings
                .iter()
                .any(|kept| kept.path == recording.path)
            {
                self.recordings.push(recording.clone());
            }
        }
        self
    }
}

/// Two agents' ids in the same order whichever of them a pair names first, so the
/// pair's item keeps its id.
fn pair_id(one: &str, other: &str) -> String {
    if one <= other {
        format!("{one}:{other}")
    } else {
        format!("{other}:{one}")
    }
}

/// How an agent's turn went, or that it is still at work.
fn agent_item(core: &Core, agent: &Agent, working: bool, turns: &Turned) -> InboxItem {
    let (ended_at, problem, recordings) = core.inbox().snapshot(&agent.id).unwrap_or_default();
    let card = board::agent_card(core, &agent.id);
    let mut item = if working {
        item(
            format!("agent:{}", agent.id),
            Case::Working,
            "Working".into(),
            agent.at,
        )
    } else {
        let mut handoff = handoff(core, &agent.id);
        handoff.result = card.as_ref().and_then(|card| card.result.clone());
        let done = turns.get(&agent.id).and_then(|turn| turn.done.as_ref());
        let uncommitted = done.map_or(handoff.uncommitted, |done| {
            done.changes == WorkAt::Uncommitted
        });
        let (case, what) = match &problem {
            Some(problem) => (Case::Failed(problem.fault), "Ran into a problem".to_owned()),
            _ if uncommitted => (Case::Changes, DONE_UNCOMMITTED.to_owned()),
            _ if card.as_ref().is_some_and(|card| card.done) => (Case::Finished, "Done".to_owned()),
            _ if handoff.edited > 0 => (
                Case::Changes,
                format!(
                    "Finished, changed {} {}",
                    handoff.edited,
                    if handoff.edited == 1 { "file" } else { "files" }
                ),
            ),
            _ => (Case::Finished, "Finished".to_owned()),
        };
        let mut item = item(
            format!("agent:{}", agent.id),
            case,
            what,
            ended_at.unwrap_or(agent.at),
        );
        item.detail = (problem.map(|problem| clipped_line(&problem.said, MAX_LINE_CHARS)))
            .or_else(|| handoff.offer.clone());
        item.handoff = Some(handoff);
        item
    };
    if let Some(chat) = core.chats().get(&agent.id) {
        item.feed = Some(chat.feed.id().to_owned());
        item.turn = chat.feed.turn_start();
    }
    item.recordings = recordings;
    item.by(&agent.id, Some(agent)).on_card(card.as_ref())
}

/// The items that wait on the person to answer or decide.
fn needs(core: &Core, agents: &[Agent], sessions: &[SessionInfo]) -> Vec<InboxItem> {
    let find = |agent_id: &str| agents.iter().find(|agent| agent.id == agent_id);
    let mut items = Vec::new();
    for attention in core.chats().attentions() {
        let mut item = item(
            format!("permission:{}:{}", attention.agent_id, attention.id),
            Case::Permission,
            "Needs your permission".into(),
            attention.at,
        )
        .by(&attention.agent_id, find(&attention.agent_id));
        item.detail = attention
            .choices
            .command
            .clone()
            .or_else(|| attention.choices.title.clone())
            .map(|text| clipped_line(&text, MAX_LINE_CHARS));
        item.request_id = Some(attention.id);
        item.choices = Some(attention.choices);
        items.push(item);
    }
    let questions = core.harness().questions.for_person();
    for question in &questions {
        let case = Case::Question {
            secret: question.secret,
        };
        let what = "Asks you".to_owned();
        let asker = find(&question.agent_id)
            .cloned()
            .unwrap_or_else(|| gone_agent(core, &question.agent_id));
        let mut item = item(format!("question:{}", question.id), case, what, question.at)
            .by(&question.agent_id, Some(&asker));
        let line = clipped_line(&question.text, MAX_LINE_CHARS);
        let asked = question.text.trim();
        item.asked = (asked != line).then(|| asked.to_owned());
        item.detail = Some(line);
        item.question_id = Some(question.id.clone());
        item.answers.clone_from(&question.answers);
        item.pictures.clone_from(&question.pictures);
        items.push(item);
    }
    for agent in agents {
        let Some((card, at)) = super::sign_in_asks::waiting(core, &agent.id) else {
            continue;
        };
        let mut item = item(
            format!("sign-in:{}", card.id),
            Case::SignIn,
            super::sign_in_asks::words(&card),
            at,
        )
        .by(&agent.id, Some(agent));
        item.detail = Some(card.why);
        items.push(item);
    }
    for waiting in core.ship().waiting() {
        let what = match waiting.hold.kind {
            _ if waiting.writes_to.is_some() => "Wants to write",
            ShipKind::Push => "Wants to push",
            ShipKind::Merge => "Wants to merge",
            ShipKind::Deploy => "Wants to deploy",
            ShipKind::Publish => "Wants to publish",
            ShipKind::ProductionWrite => "Wants to write to a production database",
        };
        let what = match (&waiting.writes_to, &waiting.hold.environment) {
            (Some(place), _) | (None, Some(place)) => format!("{what} to {place}"),
            (None, None) => what.to_owned(),
        };
        let detail = match &waiting.hold.why {
            Some(why) => clipped_line(&format!("{why} `{}`", waiting.hold.what), MAX_LINE_CHARS),
            None => clipped_line(&waiting.hold.what, MAX_LINE_CHARS),
        };
        let mut item = item(
            format!("ship:{}", waiting.hold.id),
            Case::Ship(waiting.hold),
            what,
            waiting.at,
        )
        .by(&waiting.agent_id, find(&waiting.agent_id));
        item.project.get_or_insert(waiting.project);
        item.detail = Some(detail);
        items.push(item);
    }
    for (project, proposal) in board::proposals(core) {
        let mut item = item(
            format!("environments:{project}"),
            Case::ProposedEnvironments,
            format!("{} proposes places for this project", proposal.by),
            proposal.at,
        );
        let steps: Vec<String> = (proposal.places.iter())
            .map(super::places::proposed_words)
            .collect();
        item.project = Some(project);
        item.detail = Some(clipped_line(
            &format!("{} {}.", proposal.why, steps.join("; ")),
            MAX_LINE_CHARS,
        ));
        items.push(item);
    }
    for looped in core.harness().mailbox.loops() {
        let (agent, other) = &looped.agents;
        let other_name = core.names().name_or_id(other);
        let mut item = item(
            format!("loop:{}", pair_id(agent, other)),
            Case::Looping,
            format!("Keeps messaging {other_name} and nothing else"),
            looped.since,
        )
        .by(agent, find(agent));
        item.detail = Some(
            "Sikemux holds their messages to each other until one of them does other work or you write to one of them.".to_owned(),
        );
        items.push(item);
    }
    for stopped in core.harness().mailbox.stopped_agents() {
        let agent = &stopped.agent_id;
        let mut item = item(
            format!("stopped:{agent}"),
            Case::Stopped,
            format!(
                "Took {} turns in a row with nobody writing to it",
                stopped.turns
            ),
            stopped.since,
        )
        .by(agent, find(agent));
        item.detail = Some(
            "Triggers and other agents' messages wait until you, its lead or its chief write to it; anything written lets it go on.".to_owned(),
        );
        items.push(item);
    }
    for stall in core.stalls().for_person() {
        let agent = &stall.agent_id;
        let (title, detail) = match stall.kind {
            super::stalls::StallKind::NoTool => (
                format!(
                    "Working, but no tool call for {}",
                    super::stalls::quiet_for(stall.since)
                ),
                "It may be stuck, such as on a queued build. Open it to see where, or stop it.",
            ),
            super::stalls::StallKind::Silent => (
                format!(
                    "Working, but no output for {}",
                    super::stalls::quiet_for(stall.since)
                ),
                "It has streamed no output or tool calls and may be stuck waiting on the model. Open it to check, or stop it.",
            ),
        };
        let mut item = item(
            format!("no-progress:{agent}"),
            Case::NoProgress,
            title,
            stall.since,
        )
        .by(agent, find(agent));
        item.detail = Some(detail.to_owned());
        items.push(item);
    }
    for pair in core.collisions().pairs() {
        let (agent, other) = &pair.agents;
        let other_name = core.names().name_or_id(other);
        let mut item = item(
            format!("collide:{}", pair_id(agent, other)),
            Case::Colliding,
            format!("Keeps changing the same lines as {other_name}"),
            pair.since,
        )
        .by(agent, find(agent));
        item.detail = Some(format!(
            "They collided {} times in ten minutes, last in {}. Have one of them take the file, or tell them how to split it.",
            pair.times, pair.path
        ));
        items.push(item);
    }
    for stale in super::memory::stale(core) {
        let (what, why) = match &stale.said {
            Some((text, why)) => (
                format!("No longer holds: {}", clipped_line(text, MAX_LINE_CHARS)),
                format!("{} ", clipped_line(why, MAX_LINE_CHARS)),
            ),
            None => (
                "Sensitive knowledge no longer holds".to_owned(),
                String::new(),
            ),
        };
        let mut item = item(
            format!("memory:{}", stale.id),
            Case::StaleMemory,
            what,
            stale.at,
        );
        if let Some(agent) = &stale.agent {
            item = item.by(agent, find(agent));
        }
        item.project = (stale.project).map(|folder| folder.to_string_lossy().into_owned());
        item.detail = Some(format!("{why}Correct #{} or retire it.", stale.id));
        items.push(item);
    }
    for failing in core.checks().failing() {
        let names: Vec<String> = failing
            .agents
            .iter()
            .map(|agent| core.names().name_or_id(agent))
            .collect();
        let mut item = item(
            format!("check:{}", failing.project),
            Case::CheckFailing,
            match names.is_empty() {
                true => "The project's check keeps failing".to_owned(),
                false => format!(
                    "The project's check keeps failing after {}'s commits",
                    names.join(", ")
                ),
            },
            failing.since,
        );
        if let Some(agent) = failing.agents.last() {
            item = item.by(agent, find(agent));
        }
        item.project = Some(failing.project.clone());
        item.detail = failing
            .output
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| clipped_line(line, MAX_LINE_CHARS));
        items.push(item);
    }
    for session in sessions {
        let Some(agent_id) = &session.agent_id else {
            continue;
        };
        if !session.running
            || session.agent_state != Some(AgentState::Blocked)
            || questions
                .iter()
                .any(|question| &question.agent_id == agent_id)
        {
            continue;
        }
        items.push(
            item(
                format!("input:{agent_id}"),
                Case::Input,
                "Waits for you to type".into(),
                session.active_at.unwrap_or(0),
            )
            .by(agent_id, find(agent_id)),
        );
    }
    items
}

fn ranked(mut items: Vec<InboxItem>) -> Vec<InboxItem> {
    items.sort_by(|a, b| rank(a).cmp(&rank(b)));
    items
}

/// The inbox, from the chats and terminals the device view lists.
pub(crate) fn view(core: &Core, chats: &[ChatInfo], sessions: &[SessionInfo]) -> Inbox {
    let workspace_agents = core.workspace_agents();
    let own_titles: HashMap<&str, bool> = (workspace_agents.iter())
        .map(|agent| (agent.id.as_str(), agent.titled))
        .collect();
    let places = core.workspaces().places();
    let now = epoch_ms();
    let mut shown = shown(core, chats, sessions, &own_titles);
    let seen: Vec<turn_state::Seen> = (shown.iter())
        .map(|agent| turn_state::Seen {
            id: agent.agent_id.clone(),
            status: agent.status,
            name: agent.name.clone().unwrap_or_else(|| agent.title.clone()),
        })
        .collect();
    let gathered = turn_state::Gathered::of(core, &seen);
    let chiefs = super::chief::all(core);
    for agent in &mut shown {
        agent.chief = chiefs.contains(&agent.agent_id);
        if agent.chief {
            agent.title = sikemux_view::chat::CHIEF_ROLE.to_owned();
            agent.titled = true;
        }
        agent.turn = turn_state::of(core, &gathered, &agent.agent_id, agent.status);
    }
    for agent in shown.iter_mut().filter(|agent| !agent.chief) {
        if let Some((place, _)) = places.get(&agent.agent_id) {
            super::settling::show(agent, place, now);
        }
    }
    let leads = {
        let known: HashSet<&str> = (shown.iter().map(|agent| agent.agent_id.as_str()))
            .chain(workspace_agents.iter().map(|agent| agent.id.as_str()))
            .collect();
        nearest_leads(&core.workspaces().agent_parents(), &known)
    };
    let turns: Turned = (shown.iter())
        .filter_map(|agent| Some((agent.agent_id.clone(), agent.turn.clone()?)))
        .collect();
    let (main, snoozed, settled) = super::settling::sections(shown);
    let mut listed = in_teams(main, &leads);
    listed.extend(in_teams(snoozed, &leads));
    listed.extend(in_teams(settled, &leads));
    let project_names = core.workspaces().project_names();
    let called = |place: &str| {
        (project_names.get(place).cloned()).or_else(|| {
            let folder = Path::new(place).file_name()?;
            Some(folder.to_string_lossy().into_owned())
        })
    };
    let records: HashMap<&str, &WorkspaceAgent> = (workspace_agents.iter())
        .map(|agent| (agent.id.as_str(), agent))
        .collect();
    for agent in &mut listed {
        if let Some(record) = records.get(agent.agent_id.as_str()) {
            agent.face = record.face.clone();
            agent.tags = record.tags.clone();
            agent.started_at = Some(record.created_at).filter(|&at| at > 0);
        }
    }
    let mut kept = kept_agents(core, &workspace_agents, &listed, called);
    for agent in listed.iter_mut().chain(kept.iter_mut()) {
        agent.lead = leads.get(&agent.agent_id).cloned();
    }
    for agent in &mut kept {
        agent.chief = chiefs.contains(&agent.agent_id);
        if agent.chief {
            agent.title = sikemux_view::chat::CHIEF_ROLE.to_owned();
            agent.titled = true;
        } else if let Some((place, _)) = places.get(&agent.agent_id) {
            super::settling::show(agent, place, now);
        }
    }
    let mut by_project: BTreeMap<String, Vec<&AgentShown>> = BTreeMap::new();
    let project_of = core.workspaces().agent_projects();
    for agent in &mut listed {
        agent.project = project_of.get(&agent.agent_id).cloned();
        let place = agent.project.clone().or(agent.project_name.take());
        agent.project_name = place.and_then(|place| called(&place));
    }
    let in_main = |agent: &&AgentShown| agent.snoozed_until.is_none() && agent.settled_at.is_none();
    let remote = remote_agents(core);
    let in_reach = (remote.iter())
        .filter(|remote| remote.reachable)
        .map(|remote| &remote.shown);
    for agent in listed.iter().filter(in_main).chain(in_reach.clone()) {
        if let Some(project) = &agent.project {
            by_project.entry(project.clone()).or_default().push(agent);
        }
    }
    let rolled = rollup(listed.iter().filter(in_main).chain(in_reach));
    let team_needs_you: Vec<String> = by_project
        .iter()
        .filter(|(_, agents)| {
            agents
                .iter()
                .any(|agent| !agent.chief && agent.group == AgentGroup::Blocked)
        })
        .map(|(project, _)| project.clone())
        .collect();
    let projects = by_project
        .into_iter()
        .filter_map(|(project, agents)| Some((project, rollup(agents)?)))
        .collect();
    let mut view = view_of(core, &agents(core, chats, sessions), sessions, &turns);
    view.items.retain(|item| {
        let Some(agent) =
            (listed.iter()).find(|agent| item.agent_id.as_ref() == Some(&agent.agent_id))
        else {
            return true;
        };
        agent.snoozed_until.is_none() && (agent.settled_at.is_none() || item.need.is_some())
    });
    for agent in &mut listed {
        agent.failed = (view.items.iter()).any(|item| {
            item.kind == InboxKind::Failed && item.agent_id.as_ref() == Some(&agent.agent_id)
        });
    }
    for item in &mut view.items {
        let remote = view.remote_agents.iter().map(|remote| &remote.shown);
        let agent = (listed.iter().chain(remote))
            .find(|agent| item.agent_id.as_ref() == Some(&agent.agent_id));
        let about = agent
            .map(|agent| agent.title.as_str())
            .or(item.title.as_deref())
            .or(item.card.as_ref().map(|card| card.title.as_str()));
        item.about = about
            .filter(|about| !item.what.contains(about))
            .map(str::to_owned);
        item.heading = item.about.clone().unwrap_or_else(|| item.what.clone());
        item.project_name = item.project.as_deref().and_then(called);
    }
    let mut needs: BTreeMap<String, usize> = BTreeMap::new();
    let mut needed = 0;
    for item in view.items.iter().filter(|item| item.need.is_some()) {
        needed += 1;
        let project = (item.agent_id.as_ref())
            .and_then(|agent_id| project_of.get(agent_id))
            .or(item.project.as_ref());
        if let Some(project) = project {
            *needs.entry(project.clone()).or_default() += 1;
        }
    }
    let mut inbox = Inbox {
        rollup: rolled,
        projects,
        needs,
        needed,
        agents: listed,
        kept,
        team_needs_you,
        ..view
    };
    inbox.glance = ActivityGlance::of(&inbox);
    inbox
}

/// The agents the workspace keeps that run nothing now, as a client lists them.
fn kept_agents(
    core: &Core,
    agents: &[WorkspaceAgent],
    listed: &[AgentShown],
    called: impl Fn(&str) -> Option<String>,
) -> Vec<AgentShown> {
    let open: HashSet<&str> = listed.iter().map(|agent| agent.agent_id.as_str()).collect();
    (agents.iter())
        .filter(|agent| !open.contains(agent.id.as_str()))
        .map(|agent| {
            let status = match agent.asleep {
                true => AgentStatus::Asleep,
                false => AgentStatus::Idle,
            };
            let title = match agent.titled {
                true => agent.title.clone(),
                false => provider_name(core, &agent.provider),
            };
            let mut shown = agent_shown(agent.id.clone(), agent.provider.clone(), title, status);
            shown.name = agent.name.clone();
            shown.titled = agent.titled;
            shown.face = agent.face.clone();
            shown.tags = agent.tags.clone();
            shown.project = Some(agent.project.clone());
            shown.project_name = called(&agent.project);
            shown
        })
        .collect()
}

/// The agents in the order given, but each lead's workers right after it, in the same
/// order, and theirs after them. A worker whose lead is not listed stands on its own.
fn in_teams(agents: Vec<AgentShown>, leads: &HashMap<String, String>) -> Vec<AgentShown> {
    let lead_of: Vec<Option<usize>> = {
        let at: HashMap<&str, usize> = agents
            .iter()
            .enumerate()
            .map(|(ix, agent)| (agent.agent_id.as_str(), ix))
            .collect();
        agents
            .iter()
            .enumerate()
            .map(|(ix, agent)| {
                let lead = leads.get(&agent.agent_id)?;
                at.get(lead.as_str()).copied().filter(|&lead| lead != ix)
            })
            .collect()
    };
    let mut workers = vec![Vec::new(); agents.len()];
    for (ix, lead) in lead_of.iter().enumerate() {
        if let Some(team) = lead.and_then(|lead| workers.get_mut(lead)) {
            team.push(ix);
        }
    }
    fn gather(lead: usize, workers: &[Vec<usize>], placed: &mut [bool], team: &mut Vec<usize>) {
        if let Some(seen) = placed.get_mut(lead) {
            *seen = true;
        }
        team.push(lead);
        for &worker in workers.get(lead).into_iter().flatten() {
            if placed.get(worker) == Some(&false) {
                gather(worker, workers, placed, team);
            }
        }
    }
    let mut placed = vec![false; agents.len()];
    let mut order = Vec::with_capacity(agents.len());
    for (ix, lead) in lead_of.iter().enumerate() {
        if lead.is_some() || placed.get(ix) != Some(&false) {
            continue;
        }
        gather(ix, &workers, &mut placed, &mut order);
    }
    order.extend((placed.iter().enumerate()).filter_map(|(ix, &placed)| (!placed).then_some(ix)));
    let mut agents: Vec<Option<AgentShown>> = agents.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|ix| agents.get_mut(ix).and_then(Option::take))
        .collect()
}

/// Each agent's lead among `known`: the agent that spawned it, or while that one is gone, the
/// nearest agent above it that is still known.
fn nearest_leads(
    parents: &HashMap<String, String>,
    known: &HashSet<&str>,
) -> HashMap<String, String> {
    parents
        .keys()
        .filter_map(|agent| {
            let mut lead = parents.get(agent)?;
            for _ in 0..parents.len() {
                if lead == agent {
                    return None;
                }
                if known.contains(lead.as_str()) {
                    return Some((agent.clone(), lead.clone()));
                }
                lead = parents.get(lead)?;
            }
            None
        })
        .collect()
}

fn shown(
    core: &Core,
    chats: &[ChatInfo],
    sessions: &[SessionInfo],
    own_titles: &HashMap<&str, bool>,
) -> Vec<AgentShown> {
    let own = |agent_id: &str, title: &Option<String>| {
        (title.clone())
            .filter(|title| !title.is_empty())
            .filter(|_| own_titles.get(agent_id) != Some(&false))
    };
    let mut ranked: Vec<(u32, AgentShown)> = chats
        .iter()
        .map(|chat| {
            let title = own(&chat.agent_id, &chat.title);
            let titled = title.is_some();
            let title = title
                .unwrap_or_else(|| format!("New {} chat", provider_name(core, &chat.provider)));
            let mut shown = agent_shown(
                chat.agent_id.clone(),
                chat.provider.clone(),
                title,
                chat.status,
            );
            shown.titled = titled;
            shown.name = chat.name.clone();
            shown.project_name = Some(chat.cwd.to_string_lossy().into_owned());
            shown.reconnecting = chat.reconnecting;
            shown.subagents = chat.subagents;
            shown.tasks = chat.tasks;
            shown.background = chat.subagents + chat.tasks > 0;
            shown.unread = chat.unread;
            shown.active_at = chat.active_at;
            shown.next_trigger = chat.next_trigger;
            shown.triggers = core.triggers().count(&chat.agent_id);
            shown.line = core.line().waiting(core, &chat.agent_id);
            shown.called_at = chat.called_at;
            (chat.attention_rank, shown)
        })
        .collect();
    ranked.extend(
        roster::terminals(chats, sessions).filter_map(|(session, agent_id)| {
            let cli = session.agent_type.clone().unwrap_or_else(|| "agent".into());
            let title = own(agent_id, &session.title);
            let titled = title.is_some();
            let title = title.unwrap_or_else(|| provider_name(core, &cli));
            let mut shown = agent_shown(agent_id.to_owned(), cli, title, session.status?);
            shown.titled = titled;
            shown.name = session.agent_name.clone();
            shown.session = Some(session.id);
            shown.project_name = session.project.clone();
            shown.unread = session.unread;
            shown.active_at = session.active_at;
            shown.next_trigger = session.next_trigger;
            shown.triggers = core.triggers().count(agent_id);
            shown.line = core.line().waiting(core, agent_id);
            shown.called_at = session.called_at;
            Some((session.attention_rank.unwrap_or(u32::MAX), shown))
        }),
    );
    ranked.sort_by(|a, b| (a.0, &a.1.agent_id).cmp(&(b.0, &b.1.agent_id)));
    ranked.into_iter().map(|(_, shown)| shown).collect()
}

/// The items the inbox tells the person of: those that need them, which no signal of
/// their own tells.
pub(crate) fn told(core: &Core) -> Vec<InboxItem> {
    let (chats, sessions) = core.ranked_infos();
    let agents = agents(core, &chats, &sessions);
    waiting(core, &agents, &sessions)
        .into_iter()
        .filter(|item| item.need.is_some() && !signalled(item.kind))
        .collect()
}

/// Everything that waits on the person besides how agents' turns went.
fn waiting(core: &Core, agents: &[Agent], sessions: &[SessionInfo]) -> Vec<InboxItem> {
    let mut items = needs(core, agents, sessions);
    items.extend(host_alert_items(core));
    items.extend(vault_items(core));
    items
}

/// What about this host and the linked ones needs the person, each named by its host.
fn host_alert_items(core: &Core) -> Vec<InboxItem> {
    let alerts = super::host_stats::inbox_alerts(core);
    (alerts.into_iter())
        .map(|(host, name, alert)| {
            let (word, what) = match alert.kind {
                HostAlertKind::Memory if alert.pressure => {
                    ("memory", format!("Memory short, {}% used", alert.percent))
                }
                HostAlertKind::Memory => ("memory", format!("Memory {}% used", alert.percent)),
                HostAlertKind::Disk => ("disk", format!("Disk {}% full", alert.percent)),
                HostAlertKind::Cpu => ("cpu", format!("CPU {}% busy", alert.percent)),
                HostAlertKind::Offline => ("offline", "Out of reach".to_owned()),
                HostAlertKind::Unknown => ("unknown", String::new()),
            };
            let id = format!("host-alert:{}:{word}", host.as_deref().unwrap_or("here"));
            let mut item = item(id, Case::HostAlert, what, alert.since);
            item.title = Some(name);
            item.host = host;
            item.host_alert = Some(alert);
            item
        })
        .collect()
}

/// What the account vault waits on the person for: a recovery key not yet saved.
fn vault_items(core: &Core) -> Vec<InboxItem> {
    super::vault::waits(core)
        .into_iter()
        .map(|wait| {
            let mut item = item(wait.id, Case::Vault, wait.what, wait.since);
            item.title = Some(wait.title);
            item
        })
        .collect()
}

/// What the hosts workers of agents here run on hold in their inboxes about them, as items
/// here in the project their team works in.
fn foreign_items(core: &Core) -> Vec<InboxItem> {
    let mut items = Vec::new();
    for foreign in core.hosts().team.all() {
        let Some(said) = foreign.said.filter(|_| foreign.role == Role::Worker) else {
            continue;
        };
        if !core.hosts().reachable(&foreign.host) {
            continue;
        }
        let host = super::hosts::name_of(core, &foreign.host);
        items.extend(said.items.into_iter().map(|mut item| {
            item.host = Some(host.clone());
            item.project = Some(foreign.project.clone());
            item.session = None;
            item
        }));
    }
    items
}

/// The permission requests of workers on linked hosts, which the person answers here.
pub(crate) fn foreign_attentions(core: &Core) -> Vec<sikemux_wire::protocol::Attention> {
    (core.hosts().team.all().into_iter())
        .filter(|foreign| foreign.role == Role::Worker && core.hosts().reachable(&foreign.host))
        .filter_map(|foreign| foreign.said)
        .flat_map(|said| said.attentions)
        .collect()
}

fn view_of(core: &Core, agents: &[Agent], sessions: &[SessionInfo], turns: &Turned) -> Inbox {
    let mut items = waiting(core, agents, sessions);
    items.extend(foreign_items(core));
    let listed: HashSet<String> = items
        .iter()
        .filter_map(|item| item.agent_id.clone())
        .collect();
    let mut working = 0;
    for agent in agents {
        let is_working = agent.group == AgentGroup::Working;
        working += usize::from(is_working);
        if listed.contains(&agent.id) || !(is_working || agent.group == AgentGroup::Done) {
            continue;
        }
        items.push(agent_item(core, agent, is_working, turns));
    }
    Inbox {
        items: ranked(items),
        kept: Vec::new(),
        working,
        away: core.notify().absence().2,
        agent_messages: core.harness().mailbox.log.latest(),
        remote_agents: remote_agents(core),
        agents: Vec::new(),
        projects: BTreeMap::new(),
        needs: BTreeMap::new(),
        needed: 0,
        rollup: None,
        glance: ActivityGlance::default(),
        moment: moment(core),
        team_needs_you: Vec::new(),
    }
}

/// The latest feat here when the theme celebrates it.
fn moment(core: &Core) -> Option<Celebration> {
    let achieved = core.notify().achieved()?;
    let show = match (core.look().palette().backdrop.look.moments, achieved.feat) {
        (Moments::Off, _) | (Moments::Quiet, Feat::Landed | Feat::ChecksPassed) => return None,
        (Moments::Full, Feat::Landed | Feat::ChecksPassed) | (Moments::Quiet, Feat::Merged) => {
            false
        }
        (Moments::Full, Feat::Merged) => achieved.roomy,
    };
    Some(Celebration {
        agent_id: achieved.agent_id,
        at: achieved.at,
        count: achieved.count,
        show,
    })
}

/// What finished, failed and waits since the person last left, or since
/// `since` when given.
pub(crate) fn digest(core: &Core, since: Option<u64>, project: Option<&str>) -> InboxDigest {
    let (left_at, back_at, away) = core.notify().absence();
    let until = since.is_none().then_some(back_at).flatten();
    let since = since.or(left_at);
    let before_until = |at: u64| until.is_none_or(|until| at <= until);
    let (chats, sessions) = core.ranked_infos();
    let agents = agents(core, &chats, &sessions);
    let turns = turn_state::all_from(core, &chats, &sessions);
    let waiting = ranked(waiting(core, &agents, &sessions));
    let (mut finished, mut failed) = (Vec::new(), Vec::new());
    if let Some(since) = since {
        let mut listed = HashSet::new();
        for agent_id in core.inbox().ended_between(since, until) {
            let agent = agents
                .iter()
                .find(|agent| agent.id == agent_id)
                .cloned()
                .unwrap_or_else(|| gone_agent(core, &agent_id));
            let item = agent_item(core, &agent, false, &turns);
            listed.insert(agent_id);
            match item.kind {
                InboxKind::Failed => failed.push(item),
                _ => finished.push(item),
            }
        }
        for card in board::cards_done_since(core, since) {
            let owner_listed = card
                .owner
                .as_ref()
                .is_some_and(|owner| listed.contains(owner));
            if owner_listed || !before_until(card.updated_at) {
                continue;
            }
            let mut item = item(
                format!("card:{}:{}", card.project, card.id),
                Case::Finished,
                "Its card is done".into(),
                card.updated_at,
            );
            item.handoff = Some(InboxHandoff {
                result: card.result.clone(),
                ..InboxHandoff::default()
            });
            finished.push(item.on_card(Some(&card)));
        }
    }
    let here = |items: Vec<InboxItem>| -> Vec<InboxItem> {
        match project {
            Some(project) => items
                .into_iter()
                .filter(|item| item.project.as_deref() == Some(project))
                .collect(),
            None => items,
        }
    };
    let mut decided = board::goal::decided_since(core, since, project);
    decided.retain(|swarm| swarm.goal.is_none() || before_until(swarm.decision.learned.at));
    InboxDigest {
        since,
        until,
        away,
        finished: here(ranked(finished)),
        failed: here(ranked(failed)),
        waiting: here(waiting),
        decided,
    }
}

/// What waits on the person, ranked as devices' views list it.
pub(super) async fn read(core: &Arc<Core>) -> CoreResult<sikemux_view::inbox::Inbox> {
    let core = core.clone();
    blocking(move || {
        let (chats, sessions) = core.ranked_infos();
        Ok(view(&core, &chats, &sessions))
    })
    .await
}

/// What finished, failed and waits since the person last left, or `since`.
pub(crate) async fn read_digest(
    core: &Arc<Core>,
    params: DigestParams,
) -> CoreResult<sikemux_view::inbox::InboxDigest> {
    let project = (params.project)
        .map(|project| board::project_at(core, &project))
        .transpose()?;
    let core = core.clone();
    blocking(move || Ok(digest(&core, params.since, project.as_deref()))).await
}
