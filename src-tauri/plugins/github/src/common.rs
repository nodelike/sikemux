// The handful of shapes every part of GitHub's API repeats: who did it, what
// it was tagged with, and what someone wrote underneath.

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::{Comment, Label, Mention, TimelineItem};

use crate::client;
use crate::error::GithubResult;
use crate::workflows::RepoRef;

pub const MAX_PER_PAGE: u32 = 100;
/// How many pages of a hundred a list is read to before it stops.
pub const LIST_PAGES: u32 = 10;
/// GitHub lists at most 3,000 of a pull request's files, so every one it will give.
pub const FILE_PAGES: u32 = 30;
/// Releases carry their notes, so only the newest hundred are read, in one request.
pub const RELEASE_PAGES: u32 = 1;

#[derive(Deserialize)]
pub struct ActorRow {
    pub login: String,
    pub avatar_url: Option<String>,
}

#[derive(Deserialize)]
pub struct LabelRow {
    pub name: String,
    pub color: Option<String>,
}

impl From<LabelRow> for Label {
    fn from(row: LabelRow) -> Self {
        Self {
            name: row.name,
            color: row.color.unwrap_or_else(|| "8b8898".into()),
        }
    }
}

pub fn login_of(actor: &Option<ActorRow>) -> Option<String> {
    actor.as_ref().map(|actor| actor.login.clone())
}

pub fn avatar_of(actor: &Option<ActorRow>) -> Option<String> {
    actor.as_ref().and_then(|actor| actor.avatar_url.clone())
}

#[derive(Deserialize)]
struct CommentRow {
    id: u64,
    user: Option<ActorRow>,
    author_association: Option<String>,
    body: Option<String>,
    created_at: String,
    html_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
}

/// Pull requests and issues share one comment thread in GitHub's API, which is
/// why both reach it through the issues path.
pub async fn comments(data_dir: &Path, repo: &RepoRef, number: u64) -> GithubResult<Vec<Comment>> {
    let path = repo.path(&format!("/issues/{number}/comments"))?;
    let rows: Vec<CommentRow> =
        client::get_all(data_dir, &path, &[], LIST_PAGES, |rows| rows).await?;
    Ok(rows
        .into_iter()
        .map(|row| Comment {
            id: row.id,
            author: login_of(&row.user),
            avatar_url: avatar_of(&row.user),
            author_association: row.author_association,
            body: row.body.unwrap_or_default(),
            created_at: row.created_at,
            url: row.html_url,
        })
        .collect())
}

fn source(row: &Value) -> Option<Mention> {
    let issue = row.get("source")?.get("issue")?;
    let pull = issue.get("pull_request");
    Some(Mention {
        number: issue.get("number")?.as_u64()?,
        pull: pull.is_some(),
        state: text(issue, &["state"]),
        merged: pull
            .and_then(|pull| pull.get("merged_at"))
            .is_some_and(|at| !at.is_null()),
        repo: text(issue, &["repository", "full_name"]),
    })
}

fn text(value: &Value, path: &[&str]) -> Option<String> {
    let mut at = value;
    for key in path {
        at = at.get(key)?;
    }
    match at {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn first(value: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| text(value, path))
}

fn timeline_item(row: Value) -> TimelineItem {
    TimelineItem {
        kind: text(&row, &["event"]).unwrap_or_default(),
        id: row.get("id").and_then(Value::as_u64),
        actor: first(
            &row,
            &[&["actor", "login"], &["user", "login"], &["author", "name"]],
        ),
        avatar_url: first(&row, &[&["actor", "avatar_url"], &["user", "avatar_url"]]),
        association: text(&row, &["author_association"]),
        at: first(
            &row,
            &[&["created_at"], &["submitted_at"], &["author", "date"]],
        ),
        body: text(&row, &["body"]),
        state: first(&row, &[&["state"], &["state_reason"]]),
        sha: first(&row, &[&["sha"], &["commit_id"]]),
        message: text(&row, &["message"]),
        subject: first(
            &row,
            &[
                &["requested_reviewer", "login"],
                &["requested_team", "name"],
                &["label", "name"],
                &["assignee", "login"],
                &["rename", "to"],
                &["source", "issue", "title"],
            ],
        ),
        source: source(&row),
    }
}

pub async fn timeline(
    data_dir: &Path,
    repo: &RepoRef,
    number: u64,
) -> GithubResult<Vec<TimelineItem>> {
    let path = repo.path(&format!("/issues/{number}/timeline"))?;
    let rows: Vec<Value> = client::get_all(data_dir, &path, &[], LIST_PAGES, |rows| rows).await?;
    Ok(rows.into_iter().map(timeline_item).collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewComment {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
    pub body: String,
}

pub async fn add_comment(data_dir: &Path, input: NewComment) -> GithubResult<()> {
    let body = input.body.trim();
    if body.is_empty() {
        return Err(crate::error::GithubError::BadArg(
            "a comment cannot be empty".into(),
        ));
    }
    let path = input
        .repo
        .path(&format!("/issues/{}/comments", input.number))?;
    client::post_empty(data_dir, &path, Some(&json!({ "body": body }))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeline_reads_each_event_into_one_shape() {
        let commit = timeline_item(json!({
            "event": "committed",
            "sha": "c1f2023",
            "message": "feat: a thing",
            "author": { "name": "Sujal", "date": "2026-09-27T10:44:50Z" },
        }));
        assert_eq!(commit.actor.as_deref(), Some("Sujal"));
        assert_eq!(commit.at.as_deref(), Some("2026-09-27T10:44:50Z"));
        assert_eq!(commit.sha.as_deref(), Some("c1f2023"));

        let review = timeline_item(json!({
            "event": "reviewed",
            "id": 7,
            "user": { "login": "nodelike", "avatar_url": "https://a/1" },
            "author_association": "OWNER",
            "state": "changes_requested",
            "body": "Please fix",
            "submitted_at": "2026-09-28T01:00:00Z",
        }));
        assert_eq!(review.actor.as_deref(), Some("nodelike"));
        assert_eq!(review.state.as_deref(), Some("changes_requested"));
        assert_eq!(review.at.as_deref(), Some("2026-09-28T01:00:00Z"));

        let asked = timeline_item(json!({
            "event": "review_requested",
            "actor": { "login": "Sujal" },
            "requested_reviewer": { "login": "nodelike" },
            "created_at": "2026-09-27T11:00:00Z",
        }));
        assert_eq!(asked.subject.as_deref(), Some("nodelike"));
    }
}
