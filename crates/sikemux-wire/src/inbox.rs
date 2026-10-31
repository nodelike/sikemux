//! What waits on the person, as the core ranks it, so the rail, the notch and
//! phones list it the same way: agents blocked or asking first, then changes
//! to review, then finished work, then work under way.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::protocol::{AgentFace, AgentGroup, AgentStatus, PermissionChoices, SessionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboxTier {
    Needs,
    Review,
    Finished,
    Progress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboxKind {
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

/// A browser recording an agent made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxRecording {
    pub path: String,
    pub seconds: f64,
    pub at: u64,
    /// The chat run and turn it was made in.
    pub feed: Option<String>,
    pub turn: Option<u64>,
}

/// What an agent left when its turn ended, for whoever picks it up.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxHandoff {
    /// What it wrote last.
    pub answer: Option<String>,
    /// What it put on its card as the result.
    pub result: Option<String>,
    /// Files it changed in the turn, the first few.
    pub files: Vec<String>,
    pub edited: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxCard {
    pub project: String,
    pub id: u64,
    pub title: String,
}

/// One thing in the inbox: who, what they need or did, and where to open it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    /// The same for as long as the item is listed.
    pub id: String,
    pub tier: InboxTier,
    pub kind: InboxKind,
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
    pub card: Option<InboxCard>,
    pub request_id: Option<String>,
    /// The options a permission is answered with.
    pub choices: Option<PermissionChoices>,
    pub question_id: Option<String>,
    pub url: Option<String>,
    pub handoff: Option<InboxHandoff>,
    pub recordings: Vec<InboxRecording>,
    /// The linked host the agent runs on, when it is not this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbox {
    pub items: Vec<InboxItem>,
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
    pub agents: Vec<AgentShown>,
    /// How each project's open agents show together, by project id.
    pub projects: BTreeMap<String, AgentRollup>,
    /// How many items wait on the person in each project, by the id of the project their
    /// agent is in, else by the project the item names.
    pub needs: BTreeMap<String, usize>,
    /// How every open agent here shows together; none while none is open.
    pub rollup: Option<AgentRollup>,
}

/// How devices show an open agent here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentShown {
    pub agent_id: String,
    /// The CLI it runs, such as `claude`.
    pub cli: String,
    /// What its row is titled: the chat's or terminal's own title, else what
    /// it is, such as "New Claude chat".
    pub title: String,
    pub status: AgentStatus,
    pub group: AgentGroup,
    /// The word shown for its group, such as "Needs input".
    pub label: String,
    pub unread: bool,
    pub subagents: u32,
    pub tasks: u32,
    /// Mid-turn or waiting on an answer, so stopping it now loses work.
    pub busy: bool,
    /// It still has subagents or shells running.
    pub background: bool,
    pub active_at: Option<u64>,
    /// When the soonest trigger set on it sends it a prompt, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_trigger: Option<u64>,
    /// When the agent was last named with `@` or messaged by another agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub called_at: Option<u64>,
    /// The face it wears, as the workspace's agent does; none while it shows
    /// its CLI's logo.
    pub face: Option<AgentFace>,
    /// Its place on its lead's team, which the agents listed after a lead
    /// belong to; none for an agent on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamLink>,
    /// The workspace project it is in, by id; none for one outside every project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// Where an agent sits on its team: the lead heads it, the last worker ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamLink {
    Head,
    Member,
    Last,
}

/// Several agents shown as one: the group of the one that wants the person
/// most, with every subagent and shell they still have running.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRollup {
    pub group: AgentGroup,
    pub label: String,
    pub subagents: u32,
    pub tasks: u32,
}

/// An agent on a linked host that works with agents here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAgent {
    pub agent_id: String,
    pub name: String,
    /// What this host calls the host it runs on.
    pub host: String,
    pub host_id: String,
    pub cli: String,
    pub status: AgentStatus,
    pub group: AgentGroup,
    /// The word shown for its status, such as "Sleeping".
    pub label: String,
    /// The agent here that started it, for a worker.
    pub lead: Option<String>,
    pub project: String,
    /// Its host answers now.
    pub reachable: bool,
    /// Its place on its lead's team, shown after the last of that team's agents here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamLink>,
}

/// What happened while the person was away.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxDigest {
    /// When they left. None when they never have, which lists nothing as
    /// finished or failed.
    pub since: Option<u64>,
    pub away: bool,
    pub finished: Vec<InboxItem>,
    pub failed: Vec<InboxItem>,
    pub waiting: Vec<InboxItem>,
}
