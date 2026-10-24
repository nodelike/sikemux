//! What waits on the person, ranked once here so the rail, the notch and
//! phones list it the same way: agents blocked or asking first, then changes
//! to review, then finished work, then work under way. Built from what the
//! core already keeps; only how each agent's latest turn went is kept here.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::protocol::{
    AgentGroup, AgentStatus, ChatInfo, PermissionChoices, SessionId, SessionInfo,
};
use crate::text::{clipped, clipped_line};

use super::board::{self, Summary, Waits};
use super::display;
use super::notify::provider_name;
use super::Core;

/// Agents whose latest turn is kept; the longest-finished go first.
const MAX_TURNS: usize = 512;
const MAX_TURN_RECORDINGS: usize = 8;
const MAX_FILES: usize = 20;
const MAX_ANSWER_CHARS: usize = 600;
const MAX_LINE_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Tier {
    Needs,
    Review,
    Finished,
    Progress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Kind {
    /// Answer with the chat's permission reply, naming `requestId`.
    Permission,
    /// Answer with `answerQuestion`, naming `questionId`.
    Question,
    /// An agent in a terminal waits for the person to type.
    Input,
    /// Auto handed the card back, Sikemux holds the messages two agents
    /// keep sending each other, two agents keep changing the same lines, or
    /// the project's check keeps failing, for the reason in `detail`.
    Held,
    /// The agent went on with an assumption the person may correct.
    Assumption,
    /// The card's pull request at `url` is ready for review.
    PullRequest,
    /// A turn or card that changed files.
    Changes,
    Failed,
    Finished,
    Working,
}

impl Kind {
    fn tier(self) -> Tier {
        match self {
            Kind::Permission | Kind::Question | Kind::Input | Kind::Held | Kind::Assumption => {
                Tier::Needs
            }
            Kind::PullRequest | Kind::Changes => Tier::Review,
            Kind::Failed | Kind::Finished => Tier::Finished,
            Kind::Working => Tier::Progress,
        }
    }

    /// Goes before the rest of its tier.
    fn pressing(self) -> bool {
        matches!(
            self,
            Kind::Permission | Kind::Question | Kind::Input | Kind::Failed
        )
    }
}

/// A browser recording an agent made.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Recording {
    pub path: String,
    pub seconds: f64,
    pub at: u64,
    /// The chat run and turn it was made in.
    pub feed: Option<String>,
    pub turn: Option<u64>,
}

/// What an agent left when its turn ended, for whoever picks it up.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Handoff {
    /// What it wrote last.
    pub answer: Option<String>,
    /// What it put on its card as the result.
    pub result: Option<String>,
    /// Files it changed in the turn, the first [`MAX_FILES`].
    pub files: Vec<String>,
    pub edited: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CardLink {
    pub project: String,
    pub id: u64,
    pub title: String,
}

/// One thing in the inbox: who, what they need or did, and where to open it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Item {
    /// The same for as long as the item is listed.
    pub id: String,
    pub tier: Tier,
    pub kind: Kind,
    pub agent_id: Option<String>,
    pub name: Option<String>,
    pub cli: Option<String>,
    pub project: Option<String>,
    /// The chat's or terminal's title.
    pub title: Option<String>,
    /// What is needed or what happened, in a line.
    pub what: String,
    pub detail: Option<String>,
    pub at: u64,
    /// The agent's terminal; none for a chat.
    pub session: Option<SessionId>,
    pub feed: Option<String>,
    pub turn: Option<u64>,
    pub card: Option<CardLink>,
    pub request_id: Option<String>,
    /// The options a permission is answered with.
    pub choices: Option<PermissionChoices>,
    pub question_id: Option<String>,
    pub url: Option<String>,
    pub handoff: Option<Handoff>,
    pub recordings: Vec<Recording>,
    /// The linked host the agent runs on, when it is not this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    pub items: Vec<Item>,
    /// Agents at work now.
    pub working: usize,
    /// The person is away from this host, so a client can tell when they return.
    pub away: bool,
    /// When each project last logged a message between its agents, so a client
    /// showing that log reads it again.
    pub agent_messages: BTreeMap<String, u64>,
    /// Agents on linked hosts working with agents here, each shown under its
    /// lead.
    pub remote_agents: Vec<RemoteAgent>,
    /// Every open agent here as devices show it, in the order they list them.
    pub agents: Vec<Shown>,
    /// How each project's open agents show together, by project id.
    pub projects: BTreeMap<String, Rollup>,
    /// How every open agent here shows together; none while none is open.
    pub rollup: Option<Rollup>,
}

/// How devices show an open agent here.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Shown {
    pub agent_id: String,
    /// The CLI it runs, such as `claude`.
    pub cli: String,
    /// What its row is titled: the chat's or terminal's own title, else what
    /// it is, such as "New Claude chat".
    pub title: String,
    pub status: AgentStatus,
    pub group: AgentGroup,
    /// The word shown for its group, such as "Needs input".
    pub label: &'static str,
    pub unread: bool,
    pub subagents: u32,
    pub tasks: u32,
    /// Mid-turn or waiting on an answer, so stopping it now loses work.
    pub busy: bool,
    /// It still has subagents or shells running.
    pub background: bool,
    pub active_at: Option<u64>,
    /// The face it wears, as the workspace's agent does; none while it shows
    /// its CLI's logo.
    pub face: Option<crate::protocol::AgentFace>,
    /// Its place on its lead's team, which the agents listed after a lead
    /// belong to; none for an agent on its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamLink>,
}

/// Where an agent sits on its team: the lead heads it, the last worker ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TeamLink {
    Head,
    Member,
    Last,
}

impl Shown {
    fn new(
        agent_id: String,
        cli: String,
        title: String,
        status: AgentStatus,
        group: AgentGroup,
    ) -> Self {
        Self {
            agent_id,
            cli,
            title,
            status,
            group,
            label: display::group_label(group),
            unread: false,
            subagents: 0,
            tasks: 0,
            busy: matches!(group, AgentGroup::Blocked | AgentGroup::Working),
            background: false,
            active_at: None,
            face: None,
            team: None,
        }
    }

    fn running(mut self, subagents: u32, tasks: u32) -> Self {
        self.subagents = subagents;
        self.tasks = tasks;
        self.background = subagents + tasks > 0;
        self
    }
}

/// Several agents shown as one: the group of the one that wants the person
/// most, with every subagent and shell they still have running.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Rollup {
    pub group: AgentGroup,
    pub label: &'static str,
    pub subagents: u32,
    pub tasks: u32,
}

fn rollup<'a>(agents: impl IntoIterator<Item = &'a Shown>) -> Option<Rollup> {
    agents
        .into_iter()
        .fold(None, |rolled: Option<Rollup>, agent| {
            let group = rolled
                .as_ref()
                .map_or(agent.group, |rolled| rolled.group.min(agent.group));
            Some(Rollup {
                group,
                label: display::group_label(group),
                subagents: rolled.as_ref().map_or(0, |rolled| rolled.subagents) + agent.subagents,
                tasks: rolled.as_ref().map_or(0, |rolled| rolled.tasks) + agent.tasks,
            })
        })
}

/// An agent on a linked host that works with agents here.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteAgent {
    pub agent_id: String,
    pub name: String,
    /// What this host calls the host it runs on.
    pub host: String,
    pub host_id: String,
    pub cli: String,
    pub status: AgentStatus,
    pub group: AgentGroup,
    /// The word shown for its status, such as "Sleeping".
    pub label: &'static str,
    /// The agent here that started it, for a worker.
    pub lead: Option<String>,
    pub project: String,
    /// Its host answers now.
    pub reachable: bool,
    /// Its place on its lead's team, shown after the last of that team's agents here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamLink>,
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
            group: display::group_of_status(foreign.status),
            label: display::status_label(foreign.status),
            lead: foreign.parent,
            project: foreign.project,
            team: None,
        })
        .collect()
}

/// What happened while the person was away.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Digest {
    /// When they left. None when they never have, which lists nothing as
    /// finished or failed.
    pub since: Option<u64>,
    pub away: bool,
    pub finished: Vec<Item>,
    pub failed: Vec<Item>,
    pub waiting: Vec<Item>,
}

#[derive(Default)]
struct Turn {
    started_at: u64,
    /// Unset while it runs, and when the person stopped it.
    ended_at: Option<u64>,
    problem: Option<String>,
    recordings: Vec<Recording>,
    /// Worked out the first time it is asked for once the turn ended.
    handoff: Option<Handoff>,
}

#[derive(Default)]
pub(crate) struct Inbox {
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

impl Inbox {
    pub(crate) fn started(&self, agent_id: &str) {
        *turn(&mut self.turns.locked(), agent_id) = Turn {
            started_at: epoch_ms(),
            ..Turn::default()
        };
    }

    /// The turn ended by itself; `problem` says what went wrong, if anything.
    pub(crate) fn ended(&self, agent_id: &str, problem: Option<&str>) {
        let mut turns = self.turns.locked();
        let turn = turn(&mut turns, agent_id);
        turn.ended_at = Some(epoch_ms());
        if let Some(problem) = problem {
            turn.problem = Some(problem.to_owned());
        }
    }

    pub(crate) fn stopped(&self, agent_id: &str) {
        if let Some(turn) = self.turns.locked().get_mut(agent_id) {
            turn.ended_at = None;
        }
    }

    fn snapshot(&self, agent_id: &str) -> Option<(Option<u64>, Option<String>, Vec<Recording>)> {
        self.turns
            .locked()
            .get(agent_id)
            .map(|turn| (turn.ended_at, turn.problem.clone(), turn.recordings.clone()))
    }

    fn ended_since(&self, since: u64) -> Vec<String> {
        self.turns
            .locked()
            .iter()
            .filter(|(_, turn)| turn.ended_at.is_some_and(|at| at >= since))
            .map(|(agent_id, _)| agent_id.clone())
            .collect()
    }
}

/// Notes a browser recording an agent finished, on its turn and its card.
pub(crate) fn recorded(core: &Core, agent_id: &str, answer: &Value) {
    let (Some(path), Some(seconds)) = (
        answer.get("path").and_then(Value::as_str),
        answer.get("seconds").and_then(Value::as_f64),
    ) else {
        return;
    };
    let chat = core.chats.get(agent_id);
    let recording = Recording {
        path: path.to_owned(),
        seconds,
        at: epoch_ms(),
        feed: chat.as_ref().map(|chat| chat.feed.id().to_owned()),
        turn: chat.as_ref().and_then(|chat| chat.feed.turn_start()),
    };
    {
        let mut turns = core.inbox.turns.locked();
        let turn = turn(&mut turns, agent_id);
        if turn.recordings.len() >= MAX_TURN_RECORDINGS {
            turn.recordings.remove(0);
        }
        turn.recordings.push(recording.clone());
    }
    board::attach_recording(core, agent_id, recording);
}

/// What the agent left when its last turn ended, worked out once.
fn handoff(core: &Core, agent_id: &str) -> Handoff {
    if let Some(handoff) = core
        .inbox
        .turns
        .locked()
        .get(agent_id)
        .and_then(|turn| turn.handoff.clone())
    {
        return handoff;
    }
    let mut handoff = Handoff::default();
    if let Some(chat) = core.chats.get(agent_id) {
        handoff.answer = chat
            .feed
            .last_answer()
            .map(|answer| clipped(&answer, MAX_ANSWER_CHARS));
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
        }
    }
    turn(&mut core.inbox.turns.locked(), agent_id).handoff = Some(handoff.clone());
    handoff
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
                .or_else(|| Some(chat.cwd.to_string_lossy().into_owned())),
            title: chat.title.clone(),
            session: None,
            group: chat.group,
            at: chat.active_at.unwrap_or(0),
            host: None,
        })
        .collect();
    for session in sessions {
        let (Some(agent_id), Some(group)) = (&session.agent_id, session.group) else {
            continue;
        };
        if !session.running || chats.iter().any(|chat| &chat.agent_id == agent_id) {
            continue;
        }
        agents.push(Agent {
            id: agent_id.clone(),
            name: session.agent_name.clone(),
            cli: session.agent_type.clone(),
            project: project(agent_id).or_else(|| {
                let id = session.project.as_deref()?;
                let folder = core.workspaces.project_info(id)?.path;
                Some(folder.to_string_lossy().into_owned())
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

/// An agent no longer open, as the names remember it.
fn gone_agent(core: &Core, agent_id: &str) -> Agent {
    let named = core.names.get(agent_id);
    Agent {
        id: agent_id.to_owned(),
        name: named.as_ref().map(|named| named.name.clone()),
        cli: None,
        project: named.map(|named| named.project),
        title: None,
        session: None,
        group: AgentGroup::Idle,
        at: core.seen.active_at(agent_id).unwrap_or(0),
        host: None,
    }
}

impl Item {
    fn new(id: String, kind: Kind, what: String, at: u64) -> Self {
        Self {
            id,
            tier: kind.tier(),
            kind,
            agent_id: None,
            name: None,
            cli: None,
            project: None,
            title: None,
            what,
            detail: None,
            at,
            session: None,
            feed: None,
            turn: None,
            card: None,
            request_id: None,
            choices: None,
            question_id: None,
            url: None,
            handoff: None,
            recordings: Vec::new(),
            host: None,
        }
    }

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
        self.card = Some(CardLink {
            project: card.project.clone(),
            id: card.id,
            title: card.title.clone(),
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

    fn rank(&self) -> (Tier, bool, i128, &str) {
        let at = i128::from(self.at);
        let at = if self.tier == Tier::Needs { at } else { -at };
        (self.tier, !self.kind.pressing(), at, &self.id)
    }
}

fn card_item(card: &Summary, waits: &Waits) -> Item {
    let (kind, what, detail, url) = match waits {
        Waits::Review { url } => (
            Kind::PullRequest,
            "Its pull request is ready for review",
            None,
            url.clone(),
        ),
        Waits::Held { note } => (Kind::Held, "Its card came back to you", note.clone(), None),
    };
    let mut item = Item::new(
        format!("card:{}:{}", card.project, card.id),
        kind,
        what.into(),
        card.updated_at,
    )
    .on_card(Some(card));
    item.detail = detail;
    item.url = url;
    item
}

/// How an agent's turn went, or that it is still at work.
fn agent_item(core: &Core, agent: &Agent, working: bool) -> Item {
    let (ended_at, problem, recordings) = core.inbox.snapshot(&agent.id).unwrap_or_default();
    let card = board::agent_card(core, &agent.id);
    let mut item = if working {
        Item::new(
            format!("agent:{}", agent.id),
            Kind::Working,
            "Working".into(),
            agent.at,
        )
    } else {
        let mut handoff = handoff(core, &agent.id);
        handoff.result = card.as_ref().and_then(|card| card.result.clone());
        let (kind, what) = match &problem {
            Some(_) => (Kind::Failed, "Ran into a problem"),
            None if handoff.edited > 0 || card.as_ref().is_some_and(|card| card.done) => {
                (Kind::Changes, "Finished, with changes to review")
            }
            None => (Kind::Finished, "Finished"),
        };
        let mut item = Item::new(
            format!("agent:{}", agent.id),
            kind,
            what.into(),
            ended_at.unwrap_or(agent.at),
        );
        item.detail = problem.map(|problem| clipped_line(&problem, MAX_LINE_CHARS));
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
fn needs(core: &Core, agents: &[Agent], sessions: &[SessionInfo]) -> Vec<Item> {
    let find = |agent_id: &str| agents.iter().find(|agent| agent.id == agent_id);
    let mut items = Vec::new();
    for attention in core.chats.attentions() {
        let mut item = Item::new(
            format!("permission:{}:{}", attention.agent_id, attention.id),
            Kind::Permission,
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
        let (kind, what) = match (&question.assumption, question.blocked) {
            (Some(assumption), false) => (
                Kind::Assumption,
                format!(
                    "Went on assuming {}",
                    clipped_line(assumption, MAX_LINE_CHARS)
                ),
            ),
            _ => (Kind::Question, "Asks you".to_owned()),
        };
        let mut item = Item::new(format!("question:{}", question.id), kind, what, question.at)
            .by(&question.agent_id, find(&question.agent_id));
        item.detail = Some(clipped_line(&question.text, MAX_LINE_CHARS));
        item.question_id = Some(question.id.clone());
        items.push(item);
    }
    for looped in core.harness.mailbox.loops() {
        let (agent, other) = &looped.agents;
        let other_name = core.names.name(other).unwrap_or_else(|| other.clone());
        let mut item = Item::new(
            format!("loop:{agent}:{other}"),
            Kind::Held,
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
        let other_name = core.names.name(other).unwrap_or_else(|| other.clone());
        let mut item = Item::new(
            format!("collide:{agent}:{other}"),
            Kind::Held,
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
    for failing in core.collisions.failing() {
        let names: Vec<String> = failing
            .agents
            .iter()
            .map(|agent| core.names.name(agent).unwrap_or_else(|| agent.clone()))
            .collect();
        let mut item = Item::new(
            format!("check:{}", failing.project),
            Kind::Held,
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
            || session.agent_state.as_deref() != Some("blocked")
            || questions
                .iter()
                .any(|question| &question.agent_id == agent_id)
        {
            continue;
        }
        items.push(
            Item::new(
                format!("input:{agent_id}"),
                Kind::Input,
                "Waits for you to type".into(),
                session.active_at.unwrap_or(0),
            )
            .by(agent_id, find(agent_id)),
        );
    }
    items
}

fn ranked(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by(|a, b| a.rank().cmp(&b.rank()));
    items
}

/// The inbox, from the chats and terminals the device view lists.
pub(crate) fn view(core: &Core, chats: &[ChatInfo], sessions: &[SessionInfo]) -> View {
    let mut listed = in_teams(shown(core, chats, sessions), &core.workspaces.agent_leads());
    let mut faces = core.agent_faces();
    for agent in &mut listed {
        agent.face = faces.remove(&agent.agent_id);
    }
    let mut by_project: BTreeMap<String, Vec<&Shown>> = BTreeMap::new();
    let mut project_of = core.workspaces.agent_projects();
    for agent in &listed {
        if let Some(project) = project_of.remove(&agent.agent_id) {
            by_project.entry(project).or_default().push(agent);
        }
    }
    let projects = by_project
        .into_iter()
        .filter_map(|(project, agents)| Some((project, rollup(agents)?)))
        .collect();
    let mut view = view_of(core, &agents(core, chats, sessions), sessions);
    with_remote_workers(&mut listed, &mut view.remote_agents);
    View {
        rollup: rollup(&listed),
        projects,
        agents: listed,
        ..view
    }
}

/// Adds the workers agents here started on linked hosts to their leads' teams: each team
/// with such workers ends with them, in the order they come, so the team's last agent here
/// no longer ends it. Workers whose lead is not listed stand on their own.
fn with_remote_workers(listed: &mut [Shown], remote: &mut Vec<RemoteAgent>) {
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
fn in_teams(agents: Vec<Shown>, leads: &HashMap<String, String>) -> Vec<Shown> {
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
    let mut agents: Vec<Option<Shown>> = agents.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|(ix, link)| {
            let mut agent = agents[ix].take()?;
            agent.team = link;
            Some(agent)
        })
        .collect()
}

fn shown(core: &Core, chats: &[ChatInfo], sessions: &[SessionInfo]) -> Vec<Shown> {
    let mut ranked: Vec<(u32, Shown)> = chats
        .iter()
        .map(|chat| {
            let title = chat
                .title
                .clone()
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| format!("New {} chat", provider_name(core, &chat.provider)));
            let mut shown = Shown::new(
                chat.agent_id.clone(),
                chat.provider.clone(),
                title,
                chat.status,
                chat.group,
            )
            .running(chat.subagents, chat.tasks);
            shown.unread = chat.unread;
            shown.active_at = chat.active_at;
            (chat.attention_rank, shown)
        })
        .collect();
    ranked.extend(sessions.iter().filter_map(|session| {
        let agent_id = session.agent_id.as_ref()?;
        if chats.iter().any(|chat| &chat.agent_id == agent_id) {
            return None;
        }
        let cli = session.agent_type.clone().unwrap_or_else(|| "agent".into());
        let title = session
            .title
            .clone()
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| provider_name(core, &cli));
        let mut shown = Shown::new(
            agent_id.clone(),
            cli,
            title,
            session.status?,
            session.group?,
        );
        shown.unread = session.unread;
        shown.active_at = session.active_at;
        Some((session.attention_rank.unwrap_or(u32::MAX), shown))
    }));
    ranked.sort_by(|a, b| (a.0, &a.1.agent_id).cmp(&(b.0, &b.1.agent_id)));
    ranked.into_iter().map(|(_, shown)| shown).collect()
}

fn view_of(core: &Core, agents: &[Agent], sessions: &[SessionInfo]) -> View {
    let mut items = needs(core, agents, sessions);
    for card in board::cards_for_person(core, None) {
        if let Some(waits) = &card.waits {
            items.push(card_item(&card, waits));
        }
    }
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
        items.push(agent_item(core, agent, is_working));
    }
    View {
        items: ranked(items),
        working,
        away: core.notify.absence().1,
        agent_messages: core.harness.mailbox.log.latest(),
        remote_agents: remote_agents(core),
        agents: Vec::new(),
        projects: BTreeMap::new(),
        rollup: None,
    }
}

/// What finished, failed and waits since the person last left, or since
/// `since` when given.
pub(crate) fn digest(core: &Core, since: Option<u64>) -> Digest {
    let (left_at, away) = core.notify.absence();
    let since = since.or(left_at);
    let (chats, sessions) = core.ranked_infos();
    let agents = agents(core, &chats, &sessions);
    let waiting = view_of(core, &agents, &sessions)
        .items
        .into_iter()
        .filter(|item| item.tier == Tier::Needs)
        .collect();
    let (mut finished, mut failed) = (Vec::new(), Vec::new());
    if let Some(since) = since {
        let mut listed = HashSet::new();
        for agent_id in core.inbox.ended_since(since) {
            let agent = agents
                .iter()
                .find(|agent| agent.id == agent_id)
                .cloned()
                .unwrap_or_else(|| gone_agent(core, &agent_id));
            let item = agent_item(core, &agent, false);
            listed.insert(agent_id);
            match item.kind {
                Kind::Failed => failed.push(item),
                _ => finished.push(item),
            }
        }
        for card in board::cards_for_person(core, Some(since)) {
            let owner_listed = card
                .owner
                .as_ref()
                .is_some_and(|owner| listed.contains(owner));
            if !card.done || owner_listed {
                continue;
            }
            let mut item = Item::new(
                format!("card:{}:{}", card.project, card.id),
                Kind::Finished,
                "Its card is done".into(),
                card.updated_at,
            );
            item.handoff = Some(Handoff {
                result: card.result.clone(),
                ..Handoff::default()
            });
            finished.push(item.on_card(Some(&card)));
        }
    }
    Digest {
        since,
        away,
        finished: ranked(finished),
        failed: ranked(failed),
        waiting,
    }
}
