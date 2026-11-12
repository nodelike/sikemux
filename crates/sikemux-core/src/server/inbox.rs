//! What waits on the person, ranked once here so the rail, the notch and
//! phones list it the same way: whatever needs the person first, then changes
//! to review, then finished work, then work under way. Built from what the
//! core already keeps; only how each agent's latest turn went is kept here.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::git::Git;
use crate::text::{clipped, clipped_line};
use sikemux_view::inbox::{
    AgentRollup, AgentShown, Inbox, InboxCard, InboxDigest, InboxHandTo, InboxHandoff, InboxItem,
    InboxKind, InboxLook, InboxNeed, InboxRecording, InboxStalled, InboxTier, RemoteAgent,
    ShipHold, ShipKind, TeamLink,
};
use sikemux_view::workspace::{AgentFace, WorkspaceAgent};
use sikemux_wire::protocol::{
    AgentGroup, AgentState, AgentStatus, ChatInfo, ChatState, SessionId, SessionInfo,
};

use super::board::{self, RunHold, Summary, Waits};
use super::connection::blocking;
use super::display;
use super::kept::Kept;
use super::notify::provider_name;
use super::roster;
use super::turn_state;
use super::{Core, CoreResult};
use sikemux_fold::AgentNeed;
use sikemux_view::chat::{FailureKind, TeamNeedKind};
use sikemux_view::host_stats::HostAlertKind;
use sikemux_view::slice::{Gate, Proof};
use sikemux_view::turn::{TurnState, WorkAt};

use super::board::steps::Step;

/// How each agent's turn stands, by agent, as one view worked it out.
type Turned = HashMap<String, TurnState>;

/// Agents whose latest turn is kept; the longest-finished go first.
const MAX_TURNS: usize = 512;
const TURNS_FILE: &str = "turns.json";
const MAX_TURN_RECORDINGS: usize = 8;
const MAX_FILES: usize = 20;
const MAX_ANSWER_CHARS: usize = 600;
const MAX_LINE_CHARS: usize = 200;
/// The agents a stalled slice's item offers to hand it to.
const MAX_HAND_TO: usize = 12;
const DONE_UNCOMMITTED: &str = "Done, uncommitted";
/// The files of an agent's whose state git is asked for at once.
const MAX_STATUS_FILES: usize = 256;

/// How a turn failed, as far as the person is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Fault {
    /// Its account is signed out, or out of usage with nothing to send again once that resets.
    Account,
    /// The chat sends the prompt again by itself, once the plan resets or the network is back.
    Retrying,
    /// Anything else that stopped the turn.
    Broke,
}

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
    /// The slice waits for the person to pass this step.
    Gate(Step),
    /// An agent's action that leaves this machine waits for the person to allow it.
    Ship(ShipHold),
    /// An agent proposed the project's environments, for the person to accept.
    ProposedEnvironments,
    /// Something about a host needs the person.
    HostAlert,
    /// The account vault needs the person.
    Vault,
    /// An agent wrote a command as an assertion's evidence, for the person to accept.
    ProposedCommand,
    PullRequest,
    /// Auto handed the card back, for the person to fix something or unblock it.
    HandedBack(RunHold),
    /// The slice's ticket was closed in its tracker with nothing of it
    /// landed, where the person lands slices.
    TicketDone,
    /// The slice lost its owner, which stalled.
    Stalled,
    /// Two agents keep messaging each other and nothing else.
    Looping,
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
            Case::Gate(_) => (InboxKind::Gate, Wants::Person(Decide)),
            Case::Ship(_) => (InboxKind::Ship, Wants::Person(Decide)),
            Case::PullRequest => (InboxKind::PullRequest, Wants::Person(Decide)),
            Case::HandedBack(RunHold::Unblock) | Case::Stalled => {
                (InboxKind::Held, Wants::Person(Unblock))
            }
            Case::HandedBack(RunHold::Fix)
            | Case::Looping
            | Case::CheckFailing
            | Case::StaleMemory => (InboxKind::Held, Wants::Person(Fix)),
            Case::Colliding | Case::TicketDone | Case::ProposedCommand => {
                (InboxKind::Held, Wants::Person(Decide))
            }
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
/// permission, a question, a terminal waiting for input, a turn's problem. The inbox tells
/// of every other item when it first needs the person.
fn signalled(kind: InboxKind) -> bool {
    matches!(
        kind,
        InboxKind::Permission | InboxKind::Question | InboxKind::Input | InboxKind::Failed
    )
}

/// Stops an agent until the person acts, so it goes before the older items that need them.
fn pressing(kind: InboxKind) -> bool {
    matches!(
        kind,
        InboxKind::Permission | InboxKind::Question | InboxKind::Input
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
        next_trigger: None,
        triggers: 0,
        build_queued_at: None,
        called_at: None,
        face: None,
        team: None,
        project: None,
        project_name: None,
        reconnecting: None,
        snoozed_until: None,
        settled_at: None,
        auto_settled: false,
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

/// The workers agents here started on linked hosts.
fn remote_agents(core: &Core) -> Vec<RemoteAgent> {
    core.hosts
        .team
        .all()
        .into_iter()
        .filter(|foreign| foreign.role == super::hosts::team::Role::Worker)
        .map(|foreign| RemoteAgent {
            host: super::hosts::name_of(core, &foreign.host),
            reachable: core.hosts.reachable(&foreign.host),
            host_id: foreign.host,
            agent_id: foreign.agent_id,
            name: foreign.name,
            cli: foreign.cli,
            status: foreign.status,
            group: foreign.status.group(),
            label: display::status_label(foreign.status).to_owned(),
            turn: foreign.turn,
            lead: foreign.parent,
            project: foreign.project,
            team: None,
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
}

pub(crate) struct Turns {
    kept: Kept,
    turns: Mutex<HashMap<String, Turn>>,
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

impl Turns {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, TURNS_FILE);
        let turns = Mutex::new(kept.load());
        Self { kept, turns }
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
                ..Turn::default()
            };
        });
    }

    /// The turn ended by itself; `problem` says what went wrong, if anything.
    pub(crate) fn ended(&self, agent_id: &str, problem: Option<&str>, left: Option<String>) {
        self.change(|turns| {
            let turn = turn(turns, agent_id);
            turn.ended_at = Some(epoch_ms());
            turn.left = left;
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

    /// How the agent's driver saw its latest turn fail, if it did.
    pub(crate) fn fault(&self, agent_id: &str) -> Option<Fault> {
        (self.turns.locked().get(agent_id))
            .and_then(|turn| turn.problem.as_ref())
            .and_then(|problem| problem.fault)
    }

    /// Whether the agent's driver saw its latest turn fail, which the person hears of by itself.
    pub(crate) fn failing(&self, agent_id: &str) -> bool {
        (self.turns.locked().get(agent_id))
            .and_then(|turn| turn.problem.as_ref())
            .is_some_and(|problem| problem.fault.is_some())
    }

    /// What went wrong in the agent's latest turn, while that waits on the person.
    fn failure(&self, agent_id: &str) -> Option<String> {
        (self.turns.locked().get(agent_id))
            .and_then(|turn| turn.problem.as_ref())
            .filter(|problem| matches!(problem.fault, Some(Fault::Account | Fault::Broke)))
            .map(|problem| problem.said.clone())
    }

    /// The agent committed, so what its last turn left is worked out again.
    pub(crate) fn committed(&self, agent_id: &str) {
        self.change(|turns| {
            if let Some(turn) = turns.get_mut(agent_id) {
                turn.handoff = None;
                turn.committed = true;
            }
        });
    }

    /// How the agent's latest turn ended, as its turn state reads it; none while it runs and
    /// before its first.
    pub(crate) fn ending(&self, agent_id: &str) -> Option<turn_state::Ended> {
        let turns = self.turns.locked();
        let turn = turns.get(agent_id)?;
        Some(turn_state::Ended {
            at: turn.ended_at,
            started_at: turn.started_at,
            left: turn.left.clone(),
            staged: turn.staged.clone(),
            problem: (turn.problem.as_ref()).map(|problem| turn_state::Problem {
                said: problem.said.clone(),
                fault: problem.fault,
                kind: problem.kind,
            }),
            work: turn.work.clone(),
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
    pub(crate) fn showing(
        &self,
        project: &str,
        covered: &[String],
    ) -> Vec<(String, u64, Option<String>)> {
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
            .map(|(agent, turn)| {
                let head = turn.work.as_ref().and_then(|work| work.head.clone());
                (agent.clone(), turn.started_at, head)
            })
            .collect()
    }

    /// Whether the agent changed anything in its latest turn, as far as the core heard.
    pub(crate) fn changed_in_turn(&self, agent_id: &str) -> bool {
        (self.turns.locked().get(agent_id)).is_some_and(|turn| turn.committed)
    }

    /// Keeps where git said the agent's work is, answering whether that moved.
    pub(crate) fn keep_work(&self, agent_id: &str, work: turn_state::Work) -> bool {
        self.change(|turns| {
            let Some(turn) = turns.get_mut(agent_id) else {
                return false;
            };
            let moved = turn.work.as_ref() != Some(&work);
            turn.work = Some(work);
            moved
        })
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
    let chat = core.chats.get(agent_id);
    let recording = InboxRecording {
        path: path.to_owned(),
        seconds,
        at: epoch_ms(),
        feed: chat.as_ref().map(|chat| chat.feed.id().to_owned()),
        turn: chat.as_ref().and_then(|chat| chat.feed.turn_start()),
    };
    core.inbox.change(|turns| {
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
    if let Some(handoff) = core
        .inbox
        .turns
        .locked()
        .get(agent_id)
        .and_then(|turn| turn.handoff.clone())
    {
        return handoff;
    }
    let mut handoff = InboxHandoff::default();
    if let Some(chat) = core.chats.get(agent_id) {
        let reply = chat.feed.last_answer();
        handoff.offer = reply.as_deref().and_then(super::questions::offer_in);
        handoff.answer = reply.map(|answer| clipped(&answer, MAX_ANSWER_CHARS));
        let project = core.names.get(agent_id).map(|named| named.project);
        let filter = serde_json::from_value(json!({
            "project": project.clone().unwrap_or_default(),
            "agentId": agent_id,
            "feed": chat.feed.id(),
            "turn": chat.feed.turn_start(),
        }));
        if let (Some(project), Ok(filter)) = (project, filter) {
            let mut seen = HashSet::new();
            for edit in core.edits.read(&project, &filter) {
                if seen.insert(edit.path.clone()) && handoff.files.len() < MAX_FILES {
                    handoff.files.push(edit.path);
                }
            }
            handoff.edited = seen.len();
            handoff.uncommitted = uncommitted(core, &project, agent_id, &chat.launch.cwd);
        }
    }
    turn(&mut core.inbox.turns.locked(), agent_id).handoff = Some(handoff.clone());
    handoff
}

/// Whether the agent changed any file in `project`, in any turn of its chat.
pub(crate) fn ever_edited(core: &Core, project: &str, agent_id: &str) -> bool {
    let filter = serde_json::from_value(json!({ "project": project, "agentId": agent_id }));
    filter.is_ok_and(|filter| !core.edits.read(project, &filter).is_empty())
}

/// Whether a file the agent changed in any turn of its chat holds changes git has not committed,
/// so a later turn that changed nothing does not hide them.
pub(crate) fn uncommitted(core: &Core, project: &str, agent_id: &str, cwd: &Path) -> bool {
    let filter = serde_json::from_value(json!({ "project": project, "agentId": agent_id }));
    let Ok(filter) = filter else {
        return false;
    };
    let mut files: Vec<String> = (core.edits.read(project, &filter).into_iter())
        .map(|edit| edit.path)
        .collect();
    files.sort_unstable();
    files.dedup();
    files.truncate(MAX_STATUS_FILES);
    if files.is_empty() {
        return false;
    }
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend(files.iter().map(String::as_str));
    Git::at(cwd)
        .text(&status)
        .is_ok_and(|listed| !listed.trim().is_empty())
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
    if !core.inbox.ended_well(agent_id) {
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
    let project = |agent_id: &str| core.names.get(agent_id).map(|named| named.project);
    let mut agents: Vec<Agent> = chats
        .iter()
        .map(|chat| Agent {
            id: chat.agent_id.clone(),
            name: chat.name.clone(),
            cli: Some(chat.provider.clone()),
            project: project(&chat.agent_id)
                .or_else(|| Some(core.workspaces.project_of(&chat.cwd))),
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
                Some(core.workspaces.project_of(&folder))
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
            agent.at = core.seen.started_at(&agent.id).unwrap_or(agent.at);
        }
    }
    agents.extend(core.hosts.team.all().into_iter().map(|foreign| Agent {
        host: Some(super::hosts::name_of(core, &foreign.host)),
        id: foreign.agent_id,
        name: Some(foreign.name),
        cli: Some(foreign.cli),
        project: Some(foreign.project),
        title: None,
        session: None,
        group: AgentGroup::Idle,
        at: foreign.active_at.unwrap_or(0),
    }));
    agents
}

/// An agent not running now, as one asleep or closed, as the names remember it.
fn gone_agent(core: &Core, agent_id: &str) -> Agent {
    let named = core.names.get(agent_id);
    Agent {
        id: agent_id.to_owned(),
        name: named.as_ref().map(|named| named.name.clone()),
        cli: None,
        project: named.map(|named| named.project).or_else(|| {
            let agent = core.workspaces.agent(agent_id)?;
            Some(core.workspaces.project_of(&agent.cwd))
        }),
        title: None,
        session: None,
        group: AgentGroup::Idle,
        at: core.seen.active_at(agent_id).unwrap_or(0),
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
        gate: match &case {
            Case::Gate(gate) => Some(gate.gate()),
            _ => None,
        },
        environment: match &case {
            Case::Gate(gate) => gate.environment().map(str::to_owned),
            Case::Ship(hold) => hold.environment.clone(),
            _ => None,
        },
        evidence: None,
        look: None,
        hold: match case {
            Case::Ship(hold) => Some(hold),
            _ => None,
        },
        request_id: None,
        stalled: None,
        raised: None,
        pick: None,
        assumption: None,
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

/// A deploy held for the environment `card` waits at already asks the person,
/// in place of its gate.
fn deploy_asks_instead(core: &Core, card: &Summary, waits: &Waits) -> bool {
    let Waits::Gate {
        gate: Step::Environment(environment),
        ..
    } = waits
    else {
        return false;
    };
    core.ship.waiting().iter().any(|waiting| {
        waiting.project == card.project
            && Some(&waiting.agent_id) == card.owner.as_ref()
            && waiting.hold.environment.as_ref() == Some(environment)
    })
}

/// The item for a card that waits on the person, unless what it waits for is
/// an answer, which its question's item asks for.
fn card_item(card: &Summary, waits: &Waits, agents: &[Agent]) -> Option<InboxItem> {
    let card_id = |state: &str| format!("{state}:{}:{}", card.project, card.id);
    let mut item = match waits {
        Waits::Review { url } => {
            let mut item = item(
                card_id("review"),
                Case::PullRequest,
                "Its pull request is ready for review".into(),
                card.updated_at,
            );
            item.url = url.clone();
            item
        }
        Waits::Held { need, note } => {
            let mut item = item(
                card_id("held"),
                Case::HandedBack(*need),
                "Its card came back to you".into(),
                card.updated_at,
            );
            item.detail = note.clone();
            item
        }
        Waits::Gate {
            gate,
            promise,
            url,
            evidence,
        } => {
            let mut item = gate_item(card, gate, promise);
            item.url = url.clone();
            item.evidence = evidence.as_deref().cloned();
            item
        }
        Waits::TicketDone { key, gate } => {
            let mut item = item(
                card_id("ticket"),
                Case::TicketDone,
                format!("{key} says done, but nothing of it landed"),
                card.updated_at,
            );
            item.detail = Some(match gate {
                Some(Step::Gate(Gate::Landed)) => format!(
                    "{key} was closed in its tracker. Pass Landed once its work is in, mark the slice done to close it as it stands, or reopen the ticket"
                ),
                Some(gate) => format!(
                    "{key} was closed in its tracker while the slice waits at {}. Mark the slice done to close it as it stands, or reopen the ticket",
                    board::gate_words(gate)
                ),
                None => format!(
                    "{key} was closed in its tracker. Mark the slice done to close it as it stands, or reopen the ticket"
                ),
            });
            item
        }
        Waits::Stalled {
            agent,
            name,
            why,
            can_resume,
        } => {
            let mut item = item(
                card_id("stalled"),
                Case::Stalled,
                format!("Stalled on {}", card.title),
                card.updated_at,
            );
            item.detail = Some(format!("{} Nobody holds it now.", why.words()));
            item.agent_id = Some(agent.clone());
            item.name = Some(name.clone());
            item.stalled = Some(InboxStalled {
                can_resume: *can_resume,
                hand_to: (agents.iter())
                    .filter(|other| {
                        other.id != *agent
                            && other.host.is_none()
                            && other.project.as_deref() == Some(card.project.as_str())
                    })
                    .filter_map(|other| {
                        Some(InboxHandTo {
                            agent_id: other.id.clone(),
                            name: other.name.clone()?,
                        })
                    })
                    .take(MAX_HAND_TO)
                    .collect(),
            });
            item
        }
        Waits::Look(look) => {
            let mut item = gate_item(card, &Step::Gate(Gate::LooksRight), &look_detail(look));
            item.evidence = (look.pictures.iter().rev())
                .find(|picture| !picture.before)
                .or(look.pictures.last())
                .map(|picture| picture.evidence.clone());
            item.recordings = (look.pictures.iter())
                .map(|picture| &picture.evidence)
                .filter_map(|evidence| match &evidence.proof {
                    Proof::Picture {
                        path,
                        seconds: Some(seconds),
                        feed,
                        turn,
                        ..
                    } => Some(InboxRecording {
                        path: path.clone(),
                        seconds: *seconds,
                        at: evidence.at,
                        feed: feed.clone(),
                        turn: *turn,
                    }),
                    _ => None,
                })
                .collect();
            item.look = Some((**look).clone());
            item
        }
        Waits::Answer => return None,
    };
    item = item.on_card(Some(card));
    Some(item)
}

/// What a goal's look is about: its one slice's promise, else how many slices it covers.
fn look_detail(look: &InboxLook) -> String {
    match look.slices.as_slice() {
        [one] => one.promise.clone(),
        many => format!("What its {} slices change, to look at once", many.len()),
    }
}

/// A slice waiting for the person to pass `gate`: the one way a Gate item is made.
fn gate_item(card: &Summary, gate: &Step, promise: &str) -> InboxItem {
    let mut item = item(
        format!(
            "gate:{}:{}:{}",
            card.project,
            card.id,
            board::gate_key(gate)
        ),
        Case::Gate(gate.clone()),
        format!("Waits for you at {}", board::gate_words(gate)),
        card.updated_at,
    );
    item.detail = Some(promise.to_owned());
    item
}

/// How an agent's turn went, or that it is still at work.
fn agent_item(core: &Core, agent: &Agent, working: bool, turns: &Turned) -> InboxItem {
    let (ended_at, problem, recordings) = core.inbox.snapshot(&agent.id).unwrap_or_default();
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
    if let Some(chat) = core.chats.get(&agent.id) {
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
    for attention in core.chats.attentions() {
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
    let questions = core.harness.questions.for_person();
    for question in &questions {
        let case = Case::Question {
            secret: question.access,
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
        item.raised.clone_from(&question.raised);
        item.pick.clone_from(&question.pick);
        item.assumption.clone_from(&question.assumption);
        items.push(item);
    }
    for waiting in core.ship.waiting() {
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
    for proposed in board::proposed_commands(core) {
        let assertion = &proposed.assertion;
        let changed = &assertion.files.changed;
        let (what, at, seen) = match changed.first() {
            Some(first) if assertion.awaits_approval().is_none() => (
                "A file its command names changed",
                proposed.at,
                format!(":{}", first.head),
            ),
            _ => (
                "Proposes a command as a gate",
                assertion.learned.at,
                String::new(),
            ),
        };
        let mut item = item(
            format!(
                "command:{}:{}:{}{seen}",
                proposed.project, proposed.goal, assertion.id
            ),
            Case::ProposedCommand,
            what.to_owned(),
            at,
        );
        let detail = if seen.is_empty() {
            if let Some(agent) = assertion.learned.agent.as_deref() {
                item = item.by(agent, find(agent));
            }
            format!(
                "`{}` for \"{}\". It does not count until you accept it.",
                assertion.awaits_approval().unwrap_or_default(),
                assertion.text
            )
        } else {
            let files: Vec<&str> = (changed.iter())
                .map(|changed| changed.file.path.as_str())
                .collect();
            format!(
                "{} for `{}`. It does not run on that commit until you accept it.",
                files.join(", "),
                assertion.command().unwrap_or_default()
            )
        };
        item.detail = Some(clipped_line(&detail, MAX_LINE_CHARS));
        item.project = Some(proposed.project.clone());
        item.card = Some(InboxCard {
            project: proposed.project,
            id: proposed.goal,
            title: proposed.title,
            goal: Some(proposed.goal),
        });
        items.push(item);
    }
    for looped in core.harness.mailbox.loops() {
        let (agent, other) = &looped.agents;
        let other_name = core.names.name_or_id(other);
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
    for pair in core.collisions.pairs() {
        let (agent, other) = &pair.agents;
        let other_name = core.names.name_or_id(other);
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
        item.project = Some(stale.folder);
        item.detail = Some(format!("{why}Correct #{} or retire it.", stale.id));
        items.push(item);
    }
    for failing in core.checks.failing() {
        let names: Vec<String> = failing
            .agents
            .iter()
            .map(|agent| core.names.name_or_id(agent))
            .collect();
        let mut item = item(
            format!("check:{}", failing.project),
            Case::CheckFailing,
            format!(
                "The project's check keeps failing after {}'s commits",
                names.join(", ")
            ),
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
    let places = core.workspaces.places();
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
        agent.turn = turn_state::of(core, &gathered, &agent.agent_id, agent.status);
    }
    for agent in shown.iter_mut().filter(|agent| !agent.chief) {
        if let Some((place, _)) = places.get(&agent.agent_id) {
            super::settling::show(agent, place, now);
        }
    }
    let leads = core.workspaces.agent_leads();
    let turns: Turned = (shown.iter())
        .filter_map(|agent| Some((agent.agent_id.clone(), agent.turn.clone()?)))
        .collect();
    let (main, snoozed, settled) = super::settling::sections(shown);
    let mut listed = in_teams(main, &leads);
    listed.extend(in_teams(snoozed, &leads));
    listed.extend(in_teams(settled, &leads));
    let project_names = core.workspaces.project_names();
    let called = |place: &str| {
        (project_names.get(place).cloned()).or_else(|| {
            let folder = Path::new(place).file_name()?;
            Some(folder.to_string_lossy().into_owned())
        })
    };
    let faces: HashMap<&str, &AgentFace> = (workspace_agents.iter())
        .filter_map(|agent| Some((agent.id.as_str(), agent.face.as_ref()?)))
        .collect();
    for agent in &mut listed {
        agent.face = faces.get(agent.agent_id.as_str()).map(|&face| face.clone());
    }
    let mut kept = kept_agents(core, &workspace_agents, &listed, called);
    for agent in kept
        .iter_mut()
        .filter(|agent| !chiefs.contains(&agent.agent_id))
    {
        if let Some((place, _)) = places.get(&agent.agent_id) {
            super::settling::show(agent, place, now);
        }
    }
    let mut by_project: BTreeMap<String, Vec<&AgentShown>> = BTreeMap::new();
    let project_of = core.workspaces.agent_projects();
    for agent in &mut listed {
        agent.project = project_of.get(&agent.agent_id).cloned();
        let place = agent.project.clone().or(agent.project_name.take());
        agent.project_name = place.and_then(|place| called(&place));
    }
    let in_main = |agent: &&AgentShown| agent.snoozed_until.is_none() && agent.settled_at.is_none();
    for agent in listed.iter().filter(in_main) {
        if let Some(project) = &agent.project {
            by_project.entry(project.clone()).or_default().push(agent);
        }
    }
    let projects = by_project
        .into_iter()
        .filter_map(|(project, agents)| Some((project, rollup(agents)?)))
        .collect();
    let mut view = view_of(core, &agents(core, chats, sessions), sessions, &turns);
    with_remote_workers(&mut listed, &mut view.remote_agents);
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
        let agent = (listed.iter()).find(|agent| item.agent_id.as_ref() == Some(&agent.agent_id));
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
    Inbox {
        rollup: rollup(listed.iter().filter(in_main)),
        projects,
        needs,
        needed,
        agents: listed,
        kept,
        ..view
    }
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
            shown.project = Some(agent.project.clone());
            shown.project_name = called(&agent.project);
            shown
        })
        .collect()
}

/// Adds the workers agents here started on linked hosts to their leads' teams: each team
/// with such workers ends with them, in the order they come, so the team's last agent here
/// no longer ends it. Workers whose lead is not listed stand on their own.
fn with_remote_workers(listed: &mut [AgentShown], remote: &mut Vec<RemoteAgent>) {
    let mut teams: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < listed.len() {
        let mut end = at;
        if listed[at].team == Some(TeamLink::Head) {
            while end + 1 < listed.len() && listed[end].team != Some(TeamLink::Last) {
                end += 1;
            }
        }
        teams.push((at, end));
        at = end + 1;
    }
    let mut placed = vec![false; remote.len()];
    let mut order = Vec::with_capacity(remote.len());
    for (start, end) in teams {
        let ids: Vec<&str> = listed[start..=end]
            .iter()
            .map(|agent| agent.agent_id.as_str())
            .collect();
        let workers: Vec<usize> = (0..remote.len())
            .filter(|&ix| {
                !placed[ix]
                    && remote[ix]
                        .lead
                        .as_deref()
                        .is_some_and(|lead| ids.contains(&lead))
            })
            .collect();
        let Some(&last) = workers.last() else {
            continue;
        };
        listed[end].team = Some(if start == end {
            TeamLink::Head
        } else {
            TeamLink::Member
        });
        for &worker in &workers {
            placed[worker] = true;
            remote[worker].team = Some(if worker == last {
                TeamLink::Last
            } else {
                TeamLink::Member
            });
            order.push(worker);
        }
    }
    order.extend((0..remote.len()).filter(|&ix| !placed[ix]));
    let mut taken: Vec<Option<RemoteAgent>> = remote.drain(..).map(Some).collect();
    remote.extend(order.into_iter().filter_map(|ix| taken[ix].take()));
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
        if let Some(lead) = lead {
            workers[*lead].push(ix);
        }
    }
    fn gather(lead: usize, workers: &[Vec<usize>], placed: &mut [bool], team: &mut Vec<usize>) {
        placed[lead] = true;
        team.push(lead);
        for &worker in &workers[lead] {
            if !placed[worker] {
                gather(worker, workers, placed, team);
            }
        }
    }
    let mut placed = vec![false; agents.len()];
    let mut order = Vec::with_capacity(agents.len());
    for ix in 0..agents.len() {
        if placed[ix] || lead_of[ix].is_some() {
            continue;
        }
        let mut team = Vec::new();
        gather(ix, &workers, &mut placed, &mut team);
        let last = team.len() - 1;
        order.extend(team.into_iter().enumerate().map(|(at, member)| {
            let link = match at {
                _ if last == 0 => None,
                0 => Some(TeamLink::Head),
                at if at == last => Some(TeamLink::Last),
                _ => Some(TeamLink::Member),
            };
            (member, link)
        }));
    }
    order.extend(
        (0..agents.len())
            .filter(|&ix| !placed[ix])
            .map(|ix| (ix, None)),
    );
    let mut agents: Vec<Option<AgentShown>> = agents.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|(ix, link)| {
            let mut agent = agents[ix].take()?;
            agent.team = link;
            Some(agent)
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
            shown.triggers = core.triggers.count(&chat.agent_id);
            shown.build_queued_at = core.builds.waiting_since(&chat.agent_id);
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
            shown.triggers = core.triggers.count(agent_id);
            shown.build_queued_at = core.builds.waiting_since(agent_id);
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

/// What each agent needs of the person, for the runs of its messages in other chats to say:
/// the same questions, holds and failures the inbox lists, and an agent that ended. A failure
/// is only a turn its driver saw fail; what an agent says went wrong in its work is not read.
pub(crate) fn agent_needs(core: &Core) -> Vec<AgentNeed> {
    let (chats, sessions) = core.ranked_infos();
    let agents = agents(core, &chats, &sessions);
    let mut needs: Vec<AgentNeed> = Vec::new();
    let mut add = |agent_id: &str, kind, text: Option<String>| {
        if !needs.iter().any(|need| need.agent_id == agent_id) {
            needs.push(AgentNeed {
                agent_id: agent_id.to_owned(),
                kind,
                text,
            });
        }
    };
    let waits = waiting(core, &agents, &sessions);
    let held = (waits.iter()).filter(|item| item.need.is_some());
    let (asked, others): (Vec<_>, Vec<_>) = held.partition(|item| item.kind == InboxKind::Question);
    for item in asked {
        if let Some(agent_id) = &item.agent_id {
            add(agent_id, TeamNeedKind::Asks, item.detail.clone());
        }
    }
    for item in others {
        if let Some(agent_id) = &item.agent_id {
            add(agent_id, TeamNeedKind::Waits, Some(item.what.clone()));
        }
    }
    for agent in &agents {
        if let Some(said) = core.inbox.failure(&agent.id) {
            let said = clipped_line(&said, MAX_LINE_CHARS);
            add(&agent.id, TeamNeedKind::Failed, Some(said));
        }
    }
    for chat in chats.iter().filter(|chat| chat.state == ChatState::Stopped) {
        add(&chat.agent_id, TeamNeedKind::Stopped, None);
    }
    for session in &sessions {
        if let Some(agent_id) = session
            .agent_id
            .as_ref()
            .filter(|_| session.status == Some(AgentStatus::Stopped))
        {
            add(agent_id, TeamNeedKind::Stopped, None);
        }
    }
    needs.sort_by(|one, other| one.agent_id.cmp(&other.agent_id));
    needs
}

/// Everything that waits on the person besides how agents' turns went.
fn waiting(core: &Core, agents: &[Agent], sessions: &[SessionInfo]) -> Vec<InboxItem> {
    let mut items = needs(core, agents, sessions);
    items.extend(host_alert_items(core));
    items.extend(vault_items(core));
    for card in board::cards_for_person(core, None) {
        if let Some(item) = card
            .waits
            .as_ref()
            .filter(|waits| !deploy_asks_instead(core, &card, waits))
            .and_then(|waits| card_item(&card, waits, agents))
        {
            items.push(item);
        }
    }
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

/// What the account vault waits on the person for: a host that may use none of the
/// account's sign-ins until they allow it, and a recovery key not yet saved.
fn vault_items(core: &Core) -> Vec<InboxItem> {
    super::vault::waits(core)
        .into_iter()
        .map(|wait| {
            let mut item = item(wait.id, Case::Vault, wait.what, wait.since);
            item.title = Some(wait.title);
            item.host = wait.host;
            item
        })
        .collect()
}

fn view_of(core: &Core, agents: &[Agent], sessions: &[SessionInfo], turns: &Turned) -> Inbox {
    let mut items = waiting(core, agents, sessions);
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
        away: core.notify.absence().2,
        agent_messages: core.harness.mailbox.log.latest(),
        remote_agents: remote_agents(core),
        agents: Vec::new(),
        projects: BTreeMap::new(),
        needs: BTreeMap::new(),
        needed: 0,
        rollup: None,
    }
}

/// What finished, failed and waits since the person last left, or since
/// `since` when given.
pub(crate) fn digest(core: &Core, since: Option<u64>, project: Option<&str>) -> InboxDigest {
    let (left_at, back_at, away) = core.notify.absence();
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
        for agent_id in core.inbox.ended_between(since, until) {
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
        for card in board::cards_for_person(core, Some(since)) {
            let owner_listed = card
                .owner
                .as_ref()
                .is_some_and(|owner| listed.contains(owner));
            if !card.done || owner_listed || !before_until(card.updated_at) {
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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestParams {
    pub since: Option<u64>,
    /// Only what is about this project, by its path.
    pub project: Option<String>,
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
