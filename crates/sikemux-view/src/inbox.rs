//! What waits on the person, as the core ranks it, so the rail, the notch and
//! phones list it the same way: whatever needs the person first, then changes
//! to review, then finished work, then work under way.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::slice::{Evidence, Gate};
use crate::workspace::AgentFace;
use sikemux_wire::protocol::{AgentGroup, AgentStatus, PermissionChoices, SessionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboxTier {
    /// Everything with a `need`, and nothing else.
    Needs,
    Review,
    Finished,
    Progress,
}

/// What the person is needed for, for an item that waits on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboxNeed {
    /// A gate to pass, a question to answer, a permission to give, a pull
    /// request to merge or an action leaving this machine to allow.
    Decide,
    /// Something only the person can do before work goes on, such as signing in,
    /// giving a secret or typing in a terminal.
    Unblock,
    /// A failed check or turn, or something held that needs mending.
    Fix,
}

/// What an agent's action does once it leaves this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShipKind {
    /// Sends commits to a remote.
    Push,
    /// Merges a pull request on the code host.
    Merge,
    /// Puts work live, such as a deploy or a release of infrastructure.
    Deploy,
    /// Releases a package or an image for others to install, or writes to a website or a
    /// connector's service as the person, which released clients read the same way.
    Publish,
    /// Changes data in a database marked as production.
    ProductionWrite,
}

/// An action an agent wanted to take that leaves this machine, held until the
/// person allows it once or refuses it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShipHold {
    pub id: String,
    pub kind: ShipKind,
    /// The exact command or tool call, which allowing it lets run once.
    pub what: String,
    /// The project's environment it lands in, when the core recognised one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Why it waits, in the core's words, in a project with environments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// What the person can always allow from now on, such as `dev/*`, in place of allowing
    /// this once. Only what reads as no production offers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub let_go: Option<String>,
    /// The name of the folder whose every project it covers, when it covers more than this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub let_go_in: Option<String>,
}

/// What the id of an item begins with when it asks about environments an agent proposed.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboxKind {
    /// Answer with the chat's permission reply, naming `requestId`.
    Permission,
    /// What the agent waits on the person to answer, asked with a tool or in
    /// the reply its turn ended with, as `detail` says: one at most for each
    /// agent, the latest. Answer with `answerQuestion`, naming `questionId`.
    Question,
    /// An agent in a terminal waits for the person to type.
    Input,
    /// Auto handed the card back, Sikemux holds the messages two agents
    /// keep sending each other, two agents keep changing the same lines, the
    /// project's check keeps failing, a rule or knowledge Sikemux keeps no
    /// longer holds, or a slice's ticket was closed with nothing of it landed,
    /// for the reason in `detail`.
    Held,
    /// An assumption an agent went on with, as a host of an earlier release lists it. This
    /// release's core keeps assumptions in the decision log instead.
    Assumption,
    /// The slice on `card` waits for the person to pass `gate`.
    Gate,
    /// An agent's action that leaves this machine, held as `hold` until the
    /// person allows it once with `allowShip` or refuses it with `refuseShip`.
    Ship,
    /// The card's pull request at `url` is ready for review.
    PullRequest,
    /// Rules the person kept saying in past sessions in `project`, offered to
    /// keep; its memory view lists them, for `keepSeededRules`.
    SeededRules,
    /// An agent proposes the environments of `project`, for the person to accept or change.
    ProposedEnvironments,
    /// A turn or card that changed files.
    Changes,
    Failed,
    Finished,
    Working,
    /// Something about a host needs the person, as `hostAlert` says: this host's, or the
    /// linked one `host` names. Open its page on the Hosts tab.
    HostAlert,
    /// The account vault needs the person, as `what` says: a host that may use none of the
    /// account's sign-ins until they allow it, or a recovery key to save. Open Settings,
    /// Connectors.
    Vault,
    /// A kind from a later release.
    #[serde(other)]
    Unknown,
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
    /// What it changed in the turn is still not committed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uncommitted: bool,
    /// More work its last reply offered, for the person to take up in its chat if they want it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxCard {
    pub project: String,
    pub id: u64,
    pub title: String,
    /// The goal card above it, the one at the top; none when it is at the top.
    #[serde(default)]
    pub goal: Option<u64>,
}

/// One thing in the inbox: who, what they need or did, and where to open it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    /// The same for as long as the item is listed.
    pub id: String,
    pub tier: InboxTier,
    pub kind: InboxKind,
    /// What its row leads with: what it is about, or what it says when that already names it.
    pub heading: String,
    /// What it is about, while `what` does not already name that. `heading` then says it,
    /// and `what` goes beneath.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// What the person is needed for; none while nothing waits on them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need: Option<InboxNeed>,
    pub agent_id: Option<String>,
    pub name: Option<String>,
    pub cli: Option<String>,
    pub project: Option<String>,
    /// What to call `project`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// The chat's or terminal's title.
    pub title: Option<String>,
    /// What is needed or what happened, in a line.
    pub what: String,
    pub detail: Option<String>,
    /// All an agent asked, when `detail` is only its first line or cut short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<String>,
    pub at: u64,
    /// The agent's terminal; none for a chat.
    pub session: Option<SessionId>,
    pub feed: Option<String>,
    pub turn: Option<u64>,
    pub card: Option<InboxCard>,
    /// The gate the slice on `card` waits at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
    /// The project's environment `gate` stands for, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// The evidence the gate of a Gate item leads with, without what a run printed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
    /// What the person looks at, for a Gate item at Looks right on a goal's card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<InboxLook>,
    /// The action a Ship item holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<ShipHold>,
    pub request_id: Option<String>,
    /// The options a permission is answered with.
    pub choices: Option<PermissionChoices>,
    pub question_id: Option<String>,
    /// The answers a question can be given in one tap, in order: `answerQuestion`
    /// takes the index as `choice`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<String>,
    /// The picture each of `answers` shows, in the same order, when the agent that asked gave
    /// them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<Option<String>>,
    pub url: Option<String>,
    pub handoff: Option<InboxHandoff>,
    pub recordings: Vec<InboxRecording>,
    /// The linked host the agent runs on, or a HostAlert item is about, when it is not this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// What a HostAlert item is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alert: Option<crate::host_stats::HostAlert>,
    /// What the person can do about the slice on `card` losing `agentId`: `boardResumeOwner`
    /// or `boardHandOn`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled: Option<InboxStalled>,
    /// What a lead says about the decision it raised to the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised: Option<InboxRaised>,
    /// The one of `answers` the agent that asked would take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<String>,
    /// What the agent that asked goes on assuming until the person answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumption: Option<String>,
}

/// The one look the person takes at a goal whose work changes what people see, once every slice
/// that shows something is Built.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxLook {
    /// What it asks the person, naming the goal once it has a short name.
    pub question: String,
    /// What the goal goes by while it waits for its short name, which `question` leaves out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unnamed: Option<String>,
    /// The slices whose work it shows, in the board's order.
    pub slices: Vec<LookSlice>,
    /// Every picture and recording those slices hold, from before each one's first commit and
    /// on its latest, in the order they were taken.
    pub pictures: Vec<LookPicture>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookSlice {
    pub card: u64,
    pub title: String,
    pub promise: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookPicture {
    /// The slice that holds it, which a note on it goes to with `evidence`'s id.
    pub card: u64,
    /// Taken before the slice's first commit, so it shows how things were.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub before: bool,
    /// A picture: its width is set for the phone's and tablet's takes, and its length for a
    /// recording.
    pub evidence: Evidence,
}

/// A decision a lead raised to the person, which stands alone: `detail` says what is being
/// decided and `answers` the options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxRaised {
    pub reason: RaisedFor,
    /// The lead that raised it, when it was not the asker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The option it would take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<String>,
    /// What breaks if that is wrong, or what stays stuck without the access it asks for.
    pub breaks: String,
}

/// Why a decision is the person's to make rather than a lead's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RaisedFor {
    Irreversible,
    Money,
    Taste,
    Access,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxStalled {
    /// Whether the agent or its session is there to start again.
    pub can_resume: bool,
    /// The agents open in the project that could take the slice.
    pub hand_to: Vec<InboxHandTo>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxHandTo {
    pub agent_id: String,
    pub name: String,
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
    /// The agents the workspace keeps that run nothing now, asleep or ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<AgentShown>,
    /// How each project's open agents show together, by project id.
    pub projects: BTreeMap<String, AgentRollup>,
    /// How many items wait on the person in each project, by the id of the project their
    /// agent is in, else by the project the item names.
    pub needs: BTreeMap<String, usize>,
    /// How many items wait on the person, in a project or not.
    #[serde(default)]
    pub needed: usize,
    /// How every open agent here shows together; none while none is open.
    pub rollup: Option<AgentRollup>,
    /// The agents here at a glance, as a phone's Live Activity and widgets show them.
    #[serde(default)]
    pub glance: crate::push::ActivityGlance,
}

/// How devices show an open agent here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentShown {
    pub agent_id: String,
    /// It is its project's chief, @Chief: the one chat of the project every other agent there
    /// works under.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chief: bool,
    /// What the core calls it, such as Otter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The CLI it runs, such as `claude`.
    pub cli: String,
    /// What its row is titled: the chat's or terminal's own title, else what
    /// it is, such as "New Claude chat".
    pub title: String,
    /// `title` is its own, not only what it is. A client may show its name instead.
    #[serde(default)]
    pub titled: bool,
    /// Its terminal; none for a chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    pub status: AgentStatus,
    pub group: AgentGroup,
    /// The word shown for its group, such as "Needs input".
    pub label: String,
    /// How its turn stands; none before its first turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<crate::turn::TurnState>,
    pub unread: bool,
    pub subagents: u32,
    pub tasks: u32,
    /// Mid-turn or waiting on an answer, so stopping it now loses work.
    pub busy: bool,
    /// Its last turn ended in a failure.
    pub failed: bool,
    /// It still has subagents or shells running.
    pub background: bool,
    pub active_at: Option<u64>,
    /// When the soonest trigger set on it sends it a prompt, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_trigger: Option<u64>,
    /// How many triggers it set on its own chat, which pause when it closes.
    #[serde(default)]
    pub triggers: u32,
    /// When a build it started began to wait for its turn on the host, while it waits, in
    /// Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_queued_at: Option<u64>,
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
    /// What to call where it works: its project, else its folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// The attempt it is retrying a failed request on, 0 when it did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnecting: Option<u32>,
    /// It is snoozed out of the main list until then, in Unix milliseconds. None once it
    /// wakes, at that time or earlier when it asks the person something.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<u64>,
    /// When it was settled out of the main list; none while it is in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<u64>,
    /// The core settled it by itself rather than the person, so a client offers Undo.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_settled: bool,
    /// The person's labels for it, lower case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
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
    /// Its row as the host it runs on lists it among its own agents, so it shows all an
    /// agent here does. Its project is the one here its team works in, and its `team` its
    /// place after the last of that team's agents here.
    #[serde(flatten)]
    pub shown: AgentShown,
    /// What this host calls the host it runs on.
    pub host: String,
    pub host_id: String,
    /// The agent here that started it, for a worker.
    pub lead: Option<String>,
    /// Its host answers now.
    pub reachable: bool,
}

/// What happened while the person was away.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxDigest {
    /// When they left. None when they never have, which lists nothing as
    /// finished or failed.
    pub since: Option<u64>,
    /// When they came back, so what happened after is not listed; none while they are away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    pub away: bool,
    pub finished: Vec<InboxItem>,
    pub failed: Vec<InboxItem>,
    pub waiting: Vec<InboxItem>,
    /// What agents decided and nobody has overruled, oldest first: on a goal in that time, and on
    /// a project's own log whenever.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decided: Vec<SwarmDecision>,
}

/// A decision agents made without the person, which `overruleDecision` overrules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmDecision {
    pub project: String,
    /// The goal whose log keeps it; none when its agent works on no goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<u64>,
    pub decision: crate::slice::Decision,
}
