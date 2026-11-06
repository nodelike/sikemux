//! Every goal and every agent on one screen, as the core works them out: which agents work each
//! goal, what is on their desks, what comes next and by when. Clients only filter, rank and draw.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::inbox::InboxNeed;
use crate::slice::{NextGate, SliceState};
use crate::workspace::AgentFace;
use sikemux_wire::protocol::{AgentGroup, AgentStatus, SessionId};

/// What the `goals` call answers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalsView {
    /// What needs the person first, then what moves, what waits and what is done, newest first.
    pub goals: Vec<GoalRow>,
    /// How many goals finished before the oldest listed.
    pub done_earlier: usize,
    /// Every open agent, whether or not it works a goal.
    pub agents: Vec<GoalAgent>,
    /// The open agents working no goal still open, by id.
    pub no_goal: Vec<String>,
    /// What waits on the person about no goal, in the inbox's order.
    pub loose_needs: Vec<LooseNeed>,
}

/// A row for something that waits on the person about no goal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LooseNeed {
    /// The inbox item's id, which says what can be done about it.
    pub id: String,
    pub need: InboxNeed,
    /// What an agent asked, else what the core says it is.
    pub what: String,
    pub agent: Option<String>,
    pub name: Option<String>,
    /// The folder or project id the item names.
    pub project: Option<String>,
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalRow {
    /// Its project's folder, as boards name it.
    pub project: String,
    pub card: u64,
    pub title: String,
    /// `NeedsYou` also when only something an agent asked holds it up.
    pub state: SliceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<GoalNext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<GoalClock>,
    pub slices: Vec<GoalSlice>,
    /// The agents working it, by id, its lead first.
    pub agents: Vec<String>,
    /// What waits on the person about it, as ids of the inbox's items, in its order.
    pub needs: Vec<String>,
    pub desk: DeskCounts,
    /// The agent to open for a goal one agent works alone, unless that agent closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSlice {
    pub card: u64,
    pub title: String,
    pub state: SliceState,
}

/// The one thing a goal's row says comes next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GoalNext {
    /// What the first of its needs is for.
    Need {
        need: InboxNeed,
        what: String,
    },
    /// A slice's check failed, with nothing in the inbox about it yet.
    Failed {
        slice: String,
    },
    /// The gate its slices reach next.
    Gate {
        gate: NextGate,
        slice: String,
        owner: Option<String>,
    },
    /// The agent on a goal with no gates, by name, whose state is the goal's.
    Agent {
        agent: String,
        idle: bool,
    },
    Done,
}

/// The time a goal's row shows, in Unix milliseconds. Clients count from it themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GoalClock {
    /// How long it has waited on the person.
    Since { at: u64 },
    /// When it should be through.
    Until { at: u64 },
    /// When it was due, now it has no estimate left.
    Late { at: u64 },
    /// When it finished.
    Finished { at: u64 },
}

/// How much is on the desks of a goal's agents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskCounts {
    /// Those still running.
    pub terminals: usize,
    pub pages: usize,
    pub simulators: usize,
    pub files: usize,
}

impl DeskCounts {
    pub fn add(&mut self, desk: &AgentDesk) {
        self.terminals += (desk.terminals.iter())
            .filter(|terminal| terminal.running)
            .count();
        self.pages += desk.pages.len();
        self.simulators += usize::from(desk.simulator.is_some());
        self.files += desk.files.len();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalAgent {
    pub id: String,
    pub name: Option<String>,
    /// The CLI it runs, such as `claude`.
    pub cli: String,
    pub face: Option<AgentFace>,
    pub status: AgentStatus,
    pub group: AgentGroup,
    /// The slice it owns, else its chat's title.
    pub task: String,
    /// Its project, by id.
    pub project: Option<String>,
    pub desk: AgentDesk,
    /// How its last turn left things, such as "Done, uncommitted".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    /// More work its last reply offered, which the person takes up in its chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDesk {
    pub pages: Vec<DeskPage>,
    pub files: Vec<String>,
    pub terminals: Vec<DeskTerminal>,
    /// The device its simulator shows, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simulator: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskPage {
    pub id: String,
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskTerminal {
    pub session: SessionId,
    pub title: String,
    /// Its session still runs. A finished one stays on the desk until its agent's turn ends. The
    /// core fills this in; the window leaves it out.
    #[serde(default)]
    pub running: bool,
}

/// What the `setDesks` call takes: every desk the window holds, by agent. The window owns the
/// browser and the editors, so it tells the core what they hold.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesksParams {
    pub desks: BTreeMap<String, AgentDesk>,
}
