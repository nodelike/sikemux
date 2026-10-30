//! What the Linear plugin's views hand a pane: a list of issues, and one issue with what
//! can be done to it and what was said on it.

use serde::{Deserialize, Serialize};

/// Where an issue stands in its team's workflow.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub id: String,
    pub name: String,
    /// Linear's kind of state: `backlog`, `unstarted`, `started`, `completed`, `canceled` or `triage`.
    pub kind: String,
    pub color: String,
}

/// One issue as the list shows it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueRow {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub state: State,
}

/// The issues a list shows, what it says above them, and what it says when it has none.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueList {
    pub heading: String,
    pub empty: String,
    pub issues: Vec<IssueRow>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    pub color: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub author: String,
    /// When it was written, as Linear gives it; each client says it in its viewer's own words.
    pub created_at: String,
    pub body: String,
}

/// One issue: where it stands and the states it can move to, who has it, and what was said.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueView {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub url: String,
    pub branch_name: String,
    pub state: State,
    /// Its team's states, in the order the team keeps them.
    pub states: Vec<State>,
    /// Who has it, by the name Linear shows; none while it is unassigned.
    pub assignee: Option<String>,
    /// Its priority, when it has one.
    pub priority: Option<String>,
    pub labels: Vec<Label>,
    pub description: Option<String>,
    pub comments: Vec<Comment>,
    /// The issue as an agent is handed it.
    pub for_agent: String,
}
