//! What the Linear pane shows: the person's open issues or a search, and one issue with the
//! states it can move to and what was said on it. The core keeps each and reads it again on
//! the pace the manifest gives.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::linear::{Comment, IssueList, IssueRow, IssueView, Label, State};

use crate::error::LinearResult;
use crate::issues::{self, IssueRef, Listing};

/// How many issues a list shows.
const LIST_LIMIT: u64 = 50;

#[derive(Deserialize)]
pub struct ListAsk {
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub query: String,
}

pub async fn list_view(data_dir: &Path, ask: ListAsk) -> LinearResult<IssueList> {
    let query = ask.query.trim().to_owned();
    let listing = Listing {
        query: Some(query.clone()),
        limit: Some(LIST_LIMIT),
        workspace: ask.workspace,
    };
    let issues = issues::search(data_dir, listing).await?;
    let (heading, empty) = match query.is_empty() {
        true => (
            "My open issues".to_owned(),
            "Nothing assigned to you is open.".to_owned(),
        ),
        false => (
            format!("Matching “{query}”"),
            "No issue matches.".to_owned(),
        ),
    };
    Ok(IssueList {
        heading,
        empty,
        issues: issues.iter().map(row).collect(),
    })
}

pub async fn issue_view(data_dir: &Path, ask: IssueRef) -> LinearResult<IssueView> {
    let issue = issues::issue(data_dir, ask).await?;
    let mut states: Vec<(f64, State)> = list(&issue, "/team/states")
        .map(|each| {
            (
                each.pointer("/position")
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                state(each),
            )
        })
        .collect();
    states.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut view = IssueView {
        id: text(&issue, "/id"),
        identifier: text(&issue, "/identifier"),
        title: text(&issue, "/title"),
        url: text(&issue, "/url"),
        branch_name: text(&issue, "/branchName"),
        state: state(field(&issue, "/state")),
        states: states.into_iter().map(|(_, state)| state).collect(),
        assignee: person(field(&issue, "/assignee")),
        priority: (issue
            .pointer("/priority")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            > 0)
        .then(|| text(&issue, "/priorityLabel"))
        .filter(|label| !label.is_empty()),
        labels: list(&issue, "/labels")
            .map(|label| Label {
                name: text(label, "/name"),
                color: text(label, "/color"),
            })
            .collect(),
        description: issue
            .pointer("/description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
        comments: list(&issue, "/comments")
            .map(|comment| Comment {
                id: text(comment, "/id"),
                author: person(field(comment, "/user")).unwrap_or_else(|| "Someone".into()),
                created_at: text(comment, "/createdAt"),
                body: text(comment, "/body"),
            })
            .collect(),
        for_agent: String::new(),
    };
    view.for_agent = for_agent(&view);
    Ok(view)
}

fn row(issue: &Value) -> IssueRow {
    IssueRow {
        id: text(issue, "/id"),
        identifier: text(issue, "/identifier"),
        title: text(issue, "/title"),
        state: state(field(issue, "/state")),
    }
}

fn state(state: &Value) -> State {
    State {
        id: text(state, "/id"),
        name: text(state, "/name"),
        kind: text(state, "/type"),
        color: text(state, "/color"),
    }
}

/// The name Linear shows for a person: what they go by, else their full name.
fn person(person: &Value) -> Option<String> {
    ["/displayName", "/name"]
        .iter()
        .map(|pointer| text(person, pointer))
        .find(|name| !name.is_empty())
}

/// The value at `pointer`, or null where there is none.
fn field<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    value.pointer(pointer).unwrap_or(&Value::Null)
}

fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn list<'a>(value: &'a Value, pointer: &str) -> impl Iterator<Item = &'a Value> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// An issue as an agent reads it: what it is, where it stands, what it asks for and what was said.
fn for_agent(issue: &IssueView) -> String {
    let labels = issue
        .labels
        .iter()
        .map(|label| label.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let facts = [
        Some(format!("State: {}", issue.state.name)),
        Some(match &issue.assignee {
            Some(name) => format!("Assignee: {name}"),
            None => "Unassigned".to_owned(),
        }),
        issue
            .priority
            .as_ref()
            .map(|priority| format!("Priority: {priority}")),
        (!labels.is_empty()).then(|| format!("Labels: {labels}")),
        Some(format!("Branch: {}", issue.branch_name)),
    ];
    let facts: Vec<String> = facts.into_iter().flatten().collect();
    let comments: Vec<String> = issue
        .comments
        .iter()
        .map(|comment| format!("**{}**: {}", comment.author, comment.body))
        .collect();
    let parts = [
        Some(format!(
            "Linear issue {}: {} — {}",
            issue.identifier, issue.title, issue.url
        )),
        Some(facts.join("\n")),
        Some(
            issue
                .description
                .clone()
                .unwrap_or_else(|| "(No description.)".into()),
        ),
        (!comments.is_empty()).then(|| format!("Comments:\n\n{}", comments.join("\n\n"))),
        Some(format!(
            "To update it, use linear_update or linear_comment with id {}.",
            issue.identifier
        )),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_an_agent_the_issue_with_what_it_needs_to_update_it() {
        let state = State {
            id: "s1".into(),
            name: "Todo".into(),
            kind: "unstarted".into(),
            color: "#e2e2e2".into(),
        };
        let issue = IssueView {
            id: "i1".into(),
            identifier: "ENG-12".into(),
            title: "Fix the login flake".into(),
            url: "https://linear.app/acme/issue/ENG-12".into(),
            branch_name: "ankit/eng-12-fix-the-login-flake".into(),
            state: state.clone(),
            states: vec![state],
            assignee: None,
            priority: Some("High".into()),
            labels: Vec::new(),
            description: Some("It fails one run in ten.".into()),
            comments: vec![Comment {
                id: "c1".into(),
                author: "irwan".into(),
                created_at: "2026-10-15T10:00:00Z".into(),
                body: "Seen it on CI too.".into(),
            }],
            for_agent: String::new(),
        };
        let text = for_agent(&issue);
        for said in [
            "Linear issue ENG-12: Fix the login flake — https://linear.app/acme/issue/ENG-12",
            "State: Todo",
            "Branch: ankit/eng-12-fix-the-login-flake",
            "**irwan**: Seen it on CI too.",
            "linear_update or linear_comment with id ENG-12",
        ] {
            assert!(text.contains(said), "{said} is missing from:\n{text}");
        }
    }
}
