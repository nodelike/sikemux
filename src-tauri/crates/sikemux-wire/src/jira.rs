//! What the Jira plugin's views hand a pane: lists of issues, the person's
//! spaces, a board, and one issue with its activity.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub account_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueSummary {
    pub key: String,
    pub summary: String,
    pub status: String,
    /// Jira's three buckets every status belongs to: `new`, `indeterminate` or `done`.
    pub status_category: String,
    pub priority: Option<String>,
    pub assignee: Option<Person>,
    pub issue_type: Option<String>,
    pub sprint: Option<String>,
    pub updated: Option<String>,
    /// The day it is due, as `2026-10-31`.
    pub due: Option<String>,
    /// The epic or other issue it sits under.
    pub parent: Option<Parent>,
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Parent {
    pub key: String,
    pub summary: String,
    /// Jira's name for the epic's colour, `color_1` to `color_14`, when a board says it.
    pub color: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub author_avatar_url: Option<String>,
    pub created: String,
    /// Markdown.
    pub body: String,
}

/// One edit to an issue: who made it, when, and the fields it changed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    pub author: String,
    pub author_avatar_url: Option<String>,
    pub created: String,
    pub fields: Vec<FieldChange>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub field: String,
    /// Absent when the field was empty, or is long text such as the description.
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub id: String,
    pub name: String,
    /// The status the issue is in after taking it.
    pub to: String,
    pub to_category: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub id: u64,
    pub name: String,
    /// `scrum` or `kanban`.
    pub kind: String,
    pub project: Option<String>,
    /// The project's key when the board belongs to one rather than to a person.
    pub project_key: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sprint {
    pub id: u64,
    pub name: String,
    pub end: Option<String>,
    pub goal: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub status_ids: Vec<String>,
    pub issues: Vec<IssueSummary>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BoardView {
    pub id: u64,
    pub name: String,
    pub kind: String,
    /// The scrum board's sprint in progress; its issues are the board's.
    pub sprint: Option<Sprint>,
    pub columns: Vec<Column>,
    /// More issues matched than the board shows.
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    pub id: String,
    pub name: String,
    pub jql: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub key: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

/// Which issues a list shows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum IssueListKind {
    /// Everything open that is the person's, and what they finished in the
    /// last two weeks so a done ticket does not just vanish.
    Mine,
    Reported,
    Watching,
    Recent,
    Sprint,
    /// A project's open issues.
    Project {
        key: String,
    },
    /// A project's open issues with nobody on them.
    Unassigned {
        key: String,
    },
    Filter {
        jql: String,
    },
    /// What the person typed: JQL when it reads as JQL, otherwise words to find
    /// in any issue's text.
    Search {
        text: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueList {
    pub issues: Vec<IssueSummary>,
    /// Each status with how many of the issues sit in it, work in progress
    /// first and finished work last.
    pub statuses: Vec<StatusCount>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusCount {
    pub status: String,
    pub category: String,
    pub count: u32,
}

/// Jira's spaces: each project the person can browse with its boards, then
/// the boards no listed project holds, and their starred filters.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Spaces {
    pub spaces: Vec<Space>,
    pub other_boards: Vec<Board>,
    pub filters: Vec<Filter>,
    /// Why Jira would not list the boards, when it would not.
    pub boards_failed: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub project: Project,
    pub boards: Vec<Board>,
}

/// A board, narrowed to the cards of the people picked, with everyone who has
/// a card on the whole board.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BoardPage {
    pub board: BoardView,
    /// Busiest first.
    pub people: Vec<BoardPerson>,
    /// How many cards on the whole board nobody has.
    pub unassigned: u32,
    /// The narrowed board split into one row of columns per assignee, as
    /// Jira's "Group: Assignee" does: busiest first, nobody last.
    pub lanes: Vec<Swimlane>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BoardPerson {
    pub account_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Swimlane {
    /// The assignee's account id, or empty for nobody.
    pub id: String,
    pub person: Option<Person>,
    pub count: u32,
    pub columns: Vec<Column>,
}

/// One issue as its page shows it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueView {
    pub issue: IssueSummary,
    pub project: Option<String>,
    pub reporter: Option<Person>,
    pub labels: Vec<String>,
    pub created: Option<String>,
    /// Markdown.
    pub description: String,
    pub transitions: Vec<Transition>,
    pub subtasks: Vec<IssueSummary>,
    /// The keys of the issues this one is blocked by.
    pub blocked_by: Vec<String>,
    /// The keys of the issues this one blocks.
    pub blocks: Vec<String>,
    /// How many comments there are, which can be more than `activity` holds.
    pub comment_count: u64,
    /// Its comments and edits, newest first.
    pub activity: Vec<Activity>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Activity {
    Comment { comment: Comment },
    Change { change: Change },
}
