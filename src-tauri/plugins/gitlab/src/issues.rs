// Issues and releases of a project, and the person's To-Do list, which is
// GitLab's inbox: what is waiting on them across every project.

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client;
use crate::error::{GitlabError, GitlabResult};
use crate::notes::{Thread, ThreadOf};
use crate::pulls::{label_of, Label, LabelRow};
use crate::repo::{avatar_of, login_of, RepoRef, User};

#[derive(Deserialize)]
pub struct IssueRow {
    iid: u64,
    #[serde(default)]
    title: String,
    description: Option<String>,
    #[serde(default)]
    state: String,
    author: Option<User>,
    created_at: String,
    updated_at: Option<String>,
    closed_at: Option<String>,
    #[serde(default)]
    user_notes_count: u64,
    #[serde(default)]
    labels: Vec<LabelRow>,
    #[serde(default)]
    assignees: Vec<User>,
    web_url: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: &'static str,
    pub state_reason: Option<String>,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub comments: u64,
    pub labels: Vec<Label>,
    pub assignees: Vec<String>,
    pub url: String,
}

impl Issue {
    pub fn from_row(row: IssueRow) -> Self {
        let closed = row.state == "closed";
        Issue {
            number: row.iid,
            title: row.title,
            body: row.description.unwrap_or_default(),
            state: if closed { "closed" } else { "open" },
            state_reason: closed.then(|| "completed".into()),
            author: login_of(row.author.as_ref()),
            avatar_url: avatar_of(row.author.as_ref()),
            updated_at: row.updated_at.unwrap_or_else(|| row.created_at.clone()),
            created_at: row.created_at,
            closed_at: row.closed_at,
            comments: row.user_notes_count,
            labels: row.labels.into_iter().map(label_of).collect(),
            assignees: row
                .assignees
                .iter()
                .filter_map(|user| login_of(Some(user)))
                .collect(),
            url: row.web_url,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueQuery {
    #[serde(flatten)]
    pub repo: RepoRef,
    #[serde(default)]
    pub state: String,
    #[serde(default = "first_page")]
    pub page: u32,
}

fn first_page() -> u32 {
    1
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuePage {
    pub issues: Vec<Issue>,
    pub total: u64,
    pub next_page: Option<u32>,
}

pub async fn list(data_dir: &Path, input: IssueQuery) -> GitlabResult<IssuePage> {
    let state = match input.state.as_str() {
        "closed" => "closed",
        "all" => "all",
        _ => "opened",
    };
    let found: client::Page<IssueRow> = client::get_page(
        data_dir,
        &input.repo.path("/issues")?,
        &[
            ("state", state.into()),
            ("order_by", "updated_at".into()),
            ("with_labels_details", "true".into()),
        ],
        input.page,
        30,
    )
    .await?;
    let issues: Vec<Issue> = found.items.into_iter().map(Issue::from_row).collect();
    Ok(IssuePage {
        total: found.total.unwrap_or(issues.len() as u64),
        next_page: found.next,
        issues,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
}

impl IssueRef {
    fn path(&self, rest: &str) -> GitlabResult<String> {
        Thread {
            repo: self.repo.clone(),
            number: self.number,
            of: ThreadOf::Issue,
        }
        .path(rest)
    }
}

pub async fn get(data_dir: &Path, input: IssueRef) -> GitlabResult<Issue> {
    let row: IssueRow = client::get(
        data_dir,
        &input.path("")?,
        &[("with_labels_details", "true".into())],
    )
    .await?;
    Ok(Issue::from_row(row))
}

#[derive(Deserialize)]
pub struct NewIssue {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

pub async fn create(data_dir: &Path, input: NewIssue) -> GitlabResult<Issue> {
    if input.title.trim().is_empty() {
        return Err(GitlabError::BadArg("an issue needs a title".into()));
    }
    let row: IssueRow = client::send_json(
        data_dir,
        Method::POST,
        &input.repo.path("/issues")?,
        &json!({ "title": input.title.trim(), "description": input.body }),
    )
    .await?;
    Ok(Issue::from_row(row))
}

#[derive(Deserialize)]
pub struct SetState {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub state: String,
}

pub async fn set_state(data_dir: &Path, input: SetState) -> GitlabResult<()> {
    let event = if input.state == "open" {
        "reopen"
    } else {
        "close"
    };
    client::write(
        data_dir,
        Method::PUT,
        &input.issue.path("")?,
        Some(&json!({ "state_event": event })),
    )
    .await
}

#[derive(Deserialize)]
struct ReleaseRow {
    tag_name: String,
    name: Option<String>,
    description: Option<String>,
    released_at: Option<String>,
    #[serde(default)]
    upcoming_release: bool,
    author: Option<User>,
    #[serde(rename = "_links")]
    links: Option<ReleaseLinks>,
}

#[derive(Deserialize)]
struct ReleaseLinks {
    #[serde(rename = "self")]
    page: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub id: u64,
    pub tag: String,
    pub name: String,
    pub body: String,
    pub draft: bool,
    pub prerelease: bool,
    pub published_at: Option<String>,
    pub author: Option<String>,
    pub assets: Vec<serde_json::Value>,
    pub url: String,
}

/// GitLab names releases by their tag; the list's place stands in for the number other hosts give.
pub async fn releases(data_dir: &Path, repo: RepoRef) -> GitlabResult<Vec<Release>> {
    let rows: Vec<ReleaseRow> = client::get_all(data_dir, &repo.path("/releases")?, &[], 2).await?;
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| Release {
            id: index as u64 + 1,
            name: row
                .name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| row.tag_name.clone()),
            tag: row.tag_name,
            body: row.description.unwrap_or_default(),
            draft: false,
            prerelease: row.upcoming_release,
            published_at: row.released_at,
            author: login_of(row.author.as_ref()),
            assets: Vec::new(),
            url: row.links.and_then(|links| links.page).unwrap_or_default(),
        })
        .collect())
}

#[derive(Deserialize)]
struct TodoProject {
    path_with_namespace: Option<String>,
}

#[derive(Deserialize)]
struct TodoTarget {
    iid: Option<u64>,
    title: Option<String>,
}

#[derive(Deserialize)]
pub struct TodoRow {
    id: u64,
    action_name: Option<String>,
    target_type: Option<String>,
    target: Option<TodoTarget>,
    target_url: Option<String>,
    body: Option<String>,
    project: Option<TodoProject>,
    state: Option<String>,
    updated_at: Option<String>,
    created_at: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub reason: String,
    pub repo: String,
    pub number: Option<u64>,
    pub unread: bool,
    pub updated_at: String,
    pub url: Option<String>,
}

/// A To-Do item as the inbox reads every host's notifications.
pub fn notification(row: TodoRow) -> Notification {
    let kind = match row.target_type.as_deref() {
        Some("MergeRequest") => "PullRequest",
        Some("Issue") => "Issue",
        Some("Commit") => "Commit",
        Some(other) => other,
        None => "Todo",
    };
    let title = row
        .target
        .as_ref()
        .and_then(|target| target.title.clone())
        .or(row.body)
        .unwrap_or_default();
    Notification {
        id: row.id.to_string(),
        title,
        kind: kind.to_string(),
        reason: row.action_name.unwrap_or_default().replace('_', " "),
        repo: row
            .project
            .and_then(|project| project.path_with_namespace)
            .unwrap_or_default(),
        number: row.target.and_then(|target| target.iid),
        unread: row.state.as_deref() != Some("done"),
        updated_at: row.updated_at.unwrap_or(row.created_at),
        url: row.target_url,
    }
}

#[derive(Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    pub all: bool,
}

/// GitLab lists To-Do items by one state at a time, and has no word for both.
fn todo_states(all: bool) -> &'static [&'static str] {
    if all {
        &["pending", "done"]
    } else {
        &["pending"]
    }
}

pub async fn inbox(data_dir: &Path, input: InboxQuery) -> GitlabResult<Vec<Notification>> {
    let lists =
        futures::future::try_join_all(todo_states(input.all).iter().map(|state| async move {
            client::get_all::<TodoRow>(data_dir, "/todos", &[("state", state.to_string())], 2).await
        }))
        .await?;
    let mut notes: Vec<Notification> = lists.into_iter().flatten().map(notification).collect();
    notes.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(notes)
}

#[derive(Deserialize)]
pub struct TodoRef {
    pub id: String,
}

pub async fn mark_read(data_dir: &Path, input: TodoRef) -> GitlabResult<()> {
    let id: u64 = input
        .id
        .parse()
        .map_err(|_| GitlabError::BadArg(format!("`{}` is not a To-Do item", input.id)))?;
    client::write(
        data_dir,
        Method::POST,
        &format!("/todos/{id}/mark_as_done"),
        None,
    )
    .await
}

pub async fn mark_all_read(data_dir: &Path) -> GitlabResult<()> {
    client::write(data_dir, Method::POST, "/todos/mark_as_done", None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_reads_as_every_hosts_issue() {
        let row: IssueRow = serde_json::from_value(json!({
            "iid": 31, "title": "Invoices round VAT per total", "description": "Should be per line.", "state": "closed",
            "author": { "username": "irwan" }, "created_at": "t1", "updated_at": "t2", "closed_at": "t3",
            "user_notes_count": 4, "labels": [ { "name": "bug", "color": "#d9534f" } ], "assignees": [ { "username": "ankit" } ],
            "web_url": "https://gitlab.com/acme/api/-/issues/31"
        }))
        .expect("issue parses");
        let issue = Issue::from_row(row);
        assert_eq!(
            (issue.number, issue.state, issue.state_reason.as_deref()),
            (31, "closed", Some("completed"))
        );
        assert_eq!(
            (issue.comments, issue.assignees.as_slice()),
            (4, ["ankit".to_string()].as_slice())
        );
        assert_eq!(
            issue.labels,
            [Label {
                name: "bug".into(),
                color: "d9534f".into()
            }]
        );
    }

    #[test]
    fn the_whole_inbox_asks_for_each_state_gitlab_knows() {
        assert_eq!(todo_states(false), ["pending"]);
        assert_eq!(todo_states(true), ["pending", "done"]);
    }

    #[test]
    fn a_to_do_reads_as_a_notification() {
        let row: TodoRow = serde_json::from_value(json!({
            "id": 102, "action_name": "review_requested", "target_type": "MergeRequest",
            "target": { "iid": 42, "title": "Fix the VAT rounding" },
            "target_url": "https://gitlab.com/acme/api/-/merge_requests/42",
            "project": { "path_with_namespace": "acme/api" }, "state": "pending", "created_at": "t1"
        }))
        .expect("todo parses");
        let note = notification(row);
        assert_eq!(
            (note.kind.as_str(), note.reason.as_str(), note.number),
            ("PullRequest", "review requested", Some(42))
        );
        assert_eq!(
            (note.repo.as_str(), note.unread, note.updated_at.as_str()),
            ("acme/api", true, "t1")
        );
    }
}
