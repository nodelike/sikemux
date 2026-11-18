//! A project's one list of tasks: the calls waiting on the person and the work on its board, in
//! the sections the core sorts them into.

use serde::{Deserialize, Serialize};

use crate::calls::CallKind;
use crate::teammates::Teammate;
use crate::workspace::PullChecks;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTasks {
    /// The project's folder.
    pub project: String,
    /// In section order, then newest first within each.
    pub rows: Vec<TaskRow>,
    /// The tasks on teammates' boards for the same repository, read-only, in the same order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teammates: Vec<TeammateTask>,
    /// None while no teammate has asked and nobody is stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<TeammateAsks>,
}

/// A task on the board a teammate shares, as their own list shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeammateTask {
    pub person: Teammate,
    /// When the teammate's host last sent its board, in unix milliseconds.
    pub heard: u64,
    #[serde(flatten)]
    pub row: TaskRow,
}

/// What teammates' agents asked about this project's tasks, drawn after Dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeammateAsks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<AskingOff>,
    /// The people the person stopped one by one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stopped: Vec<Teammate>,
    /// Newest first.
    pub asks: Vec<TeammateAsk>,
}

/// Why no teammate can ask about this project now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AskingOff {
    /// The person stopped everyone; they can turn it back on.
    Stopped,
    /// The org's Team plan is unpaid.
    Unpaid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeammateAsk {
    pub at: u64,
    pub by: Teammate,
    /// Their agent's name.
    pub agent: String,
    /// The program their agent runs on, which picks its mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_cli: Option<String>,
    pub task: u64,
    pub task_title: String,
    pub question: String,
    pub answer: String,
    /// The name of the agent of this person's whose work answered; none when nobody held the
    /// task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_work_of: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskSection {
    /// The calls open for the person.
    ForYou,
    /// Done, and not yet opened by the person.
    Finished,
    Moving,
    /// Nobody is on it.
    Queued,
    Dropped,
    /// Done, and the person opened it.
    Opened,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TaskOf {
    Task { id: u64 },
    Call { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    pub of: TaskOf,
    pub section: TaskSection,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<TaskFact>,
    /// The agent on it, or that asks the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<TaskHolder>,
    /// When it entered its section, in Unix milliseconds.
    pub since: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped: Option<TaskDropped>,
    /// The person asked for it, so it opens what they asked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskHolder {
    pub agent_id: String,
    pub name: String,
    /// Which CLI the agent runs, given on a teammate's task, where the agent is not one of yours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<String>,
}

/// What the core checked of a task, for the one muted line beside its title.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TaskFact {
    /// Its newest commit is on the project's branch.
    Landed {
        sha: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checks: Option<PullChecks>,
    },
    /// Its open pull request.
    Pull {
        number: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checks: Option<PullChecks>,
    },
    /// Commits not on the project's branch yet.
    Commits {
        count: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checks: Option<PullChecks>,
    },
    NoCommits,
    NobodySince {
        at: u64,
    },
    Call {
        call: CallKind,
        choices: u32,
        /// It comes with a page, and this many pictures among its choices.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        page: bool,
        #[serde(default, skip_serializing_if = "is_zero")]
        pictures: u32,
    },
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDropped {
    /// Who dropped it, by name; none when the person did, and none with `by_board` when nobody
    /// touched it for long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_board: bool,
    pub why: String,
    /// The title of the task that took its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instead: Option<String>,
}
