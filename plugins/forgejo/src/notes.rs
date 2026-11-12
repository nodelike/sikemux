// What people wrote on a pull request or an issue, and the history the server
// keeps of one, read back into the event names every code host uses.

use std::collections::HashMap;
use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;
use sikemux_plugin_api::codehost::asks::{NewComment, PullRef, ThreadOf, ThreadRef};
use sikemux_plugin_api::codehost::{Comment, Mention, TimelineItem};

use crate::client;
use crate::error::{ForgejoError, ForgejoResult};
use crate::pulls::{self, LabelRow};
use crate::repo::{avatar_of, login_of, ApiPath, User};

#[derive(Deserialize)]
struct CommentRow {
    id: u64,
    #[serde(default)]
    body: String,
    user: Option<User>,
    created_at: String,
    html_url: Option<String>,
}

pub async fn comments(data_dir: &Path, thread: ThreadRef) -> ForgejoResult<Vec<Comment>> {
    let rows: Vec<CommentRow> =
        client::get_all(data_dir, &thread.path("/comments")?, &[], 5).await?;
    Ok(rows
        .into_iter()
        .map(|row| Comment {
            id: row.id,
            author: login_of(row.user.as_ref()),
            avatar_url: avatar_of(row.user.as_ref()),
            author_association: None,
            body: row.body,
            created_at: row.created_at,
            url: row.html_url,
        })
        .collect())
}

pub async fn add_comment(data_dir: &Path, input: NewComment) -> ForgejoResult<()> {
    let body = input.body.trim();
    if body.is_empty() {
        return Err(ForgejoError::BadArg("a comment needs some text".into()));
    }
    client::API
        .write(
            data_dir,
            Method::POST,
            &input.thread.path("/comments")?,
            Some(&json!({ "body": body })),
        )
        .await
}

#[derive(Deserialize)]
struct PullMeta {
    #[serde(default)]
    merged: bool,
}

#[derive(Deserialize)]
struct RepoMeta {
    full_name: Option<String>,
}

#[derive(Deserialize)]
struct RefIssue {
    number: u64,
    state: Option<String>,
    pull_request: Option<PullMeta>,
    repository: Option<RepoMeta>,
}

#[derive(Deserialize)]
struct Team {
    name: Option<String>,
}

#[derive(Deserialize)]
pub struct EventRow {
    id: u64,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    body: String,
    user: Option<User>,
    created_at: String,
    label: Option<LabelRow>,
    assignee: Option<User>,
    assignee_team: Option<Team>,
    #[serde(default)]
    removed_assignee: bool,
    new_title: Option<String>,
    new_ref: Option<String>,
    ref_commit_sha: Option<String>,
    ref_issue: Option<RefIssue>,
    review_id: Option<u64>,
}

#[derive(Deserialize, Default)]
struct Pushed {
    #[serde(default)]
    is_force_push: bool,
    #[serde(default)]
    commit_ids: Vec<String>,
}

fn event(kind: &str) -> TimelineItem {
    TimelineItem {
        kind: kind.into(),
        id: None,
        actor: None,
        avatar_url: None,
        association: None,
        at: None,
        body: None,
        state: None,
        sha: None,
        message: None,
        subject: None,
        source: None,
    }
}

/// The events one entry of the server's history stands for. A push names each
/// commit it added, so it can stand for several. `verdicts` holds each review's
/// verdict by its id, which the history itself leaves out.
fn events_of(row: &EventRow, verdicts: &HashMap<u64, &'static str>) -> Vec<TimelineItem> {
    let about = |kind: &str, subject: Option<String>| TimelineItem {
        subject,
        ..event(kind)
    };
    let assignee = || {
        login_of(row.assignee.as_ref()).or_else(|| {
            row.assignee_team
                .as_ref()
                .and_then(|team| team.name.clone())
        })
    };
    let item = match row.kind.as_str() {
        "comment" => TimelineItem {
            body: Some(row.body.clone()),
            ..event("commented")
        },
        "close" => event("closed"),
        "reopen" => event("reopened"),
        "merge_pull" => event("merged"),
        "delete_branch" => event("head_ref_deleted"),
        "dismiss_review" => event("review_dismissed"),
        "label" => about(
            if row.body == "1" {
                "labeled"
            } else {
                "unlabeled"
            },
            row.label.as_ref().map(|label| label.name.clone()),
        ),
        "assignees" if row.removed_assignee => about("unassigned", assignee()),
        "assignees" => about("assigned", assignee()),
        "review_request" if row.removed_assignee => about("review_request_removed", assignee()),
        "review_request" => about("review_requested", assignee()),
        "change_title" => about("renamed", row.new_title.clone()),
        "change_target_branch" => about("base_ref_changed", row.new_ref.clone()),
        "commit_ref" => TimelineItem {
            sha: row.ref_commit_sha.clone(),
            ..event("referenced")
        },
        "issue_ref" | "pull_ref" | "comment_ref" => {
            let Some(found) = row.ref_issue.as_ref() else {
                return Vec::new();
            };
            TimelineItem {
                source: Some(Mention {
                    number: found.number,
                    pull: found.pull_request.is_some(),
                    state: found.state.clone(),
                    merged: found.pull_request.as_ref().is_some_and(|pull| pull.merged),
                    repo: found
                        .repository
                        .as_ref()
                        .and_then(|repo| repo.full_name.clone()),
                }),
                ..event("cross-referenced")
            }
        }
        "review" => TimelineItem {
            body: Some(row.body.clone()).filter(|body| !body.is_empty()),
            state: row
                .review_id
                .and_then(|id| verdicts.get(&id))
                .map(|verdict| verdict.to_ascii_lowercase()),
            ..event("reviewed")
        },
        "pull_push" => {
            let pushed: Pushed = serde_json::from_str(&row.body).unwrap_or_default();
            if pushed.is_force_push {
                return vec![event("head_ref_force_pushed")];
            }
            return pushed
                .commit_ids
                .into_iter()
                .map(|sha| TimelineItem {
                    sha: Some(sha),
                    ..event("committed")
                })
                .collect();
        }
        _ => return Vec::new(),
    };
    vec![item]
}

/// A thread's history, oldest first: comments, and the events the server wrote.
fn timeline_of(rows: Vec<EventRow>, verdicts: &HashMap<u64, &'static str>) -> Vec<TimelineItem> {
    rows.iter()
        .flat_map(|row| {
            let actor = login_of(row.user.as_ref());
            let avatar = avatar_of(row.user.as_ref());
            events_of(row, verdicts)
                .into_iter()
                .map(move |item| TimelineItem {
                    id: Some(row.id),
                    actor: actor.clone(),
                    avatar_url: avatar.clone(),
                    at: Some(row.created_at.clone()),
                    ..item
                })
        })
        .collect()
}

pub async fn timeline(data_dir: &Path, thread: ThreadRef) -> ForgejoResult<Vec<TimelineItem>> {
    let path = thread.path("/timeline")?;
    let pull = PullRef {
        repo: thread.repo.clone(),
        number: thread.number,
    };
    let reviews = async {
        match thread.of {
            ThreadOf::Pull => pulls::review_rows(data_dir, &pull).await,
            ThreadOf::Issue => Ok(Vec::new()),
        }
    };
    let (rows, reviews) = futures::try_join!(
        client::get_all::<EventRow>(data_dir, &path, &[], 5),
        reviews
    )?;
    let verdicts = reviews
        .iter()
        .filter_map(|review| Some((review.id, pulls::verdict(review)?)))
        .collect();
    Ok(timeline_of(rows, &verdicts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(value: serde_json::Value) -> Vec<EventRow> {
        serde_json::from_value(value).expect("history parses")
    }

    #[test]
    fn reads_what_the_server_writes_when_something_happens() {
        let history = rows(json!([
            { "id": 1, "type": "comment", "body": "Looks good", "user": { "login": "irwan" }, "created_at": "t1" },
            { "id": 2, "type": "review", "body": "", "review_id": 9, "user": { "login": "irwan" }, "created_at": "t2" },
            { "id": 3, "type": "label", "body": "1", "label": { "name": "bug", "color": "ee0701" }, "created_at": "t3" },
            { "id": 4, "type": "review_request", "assignee": { "login": "ankit" }, "created_at": "t4" },
            { "id": 5, "type": "pull_push", "body": "{\"is_force_push\":false,\"commit_ids\":[\"aa\",\"bb\"]}", "created_at": "t5" },
            { "id": 6, "type": "issue_ref", "ref_issue": { "number": 12, "state": "open", "pull_request": { "merged": false }, "repository": { "full_name": "acme/web" } }, "created_at": "t6" },
            { "id": 7, "type": "start_tracking", "created_at": "t7" }
        ]));
        let verdicts = HashMap::from([(9, "APPROVED")]);
        let items = timeline_of(history, &verdicts);
        let kinds: Vec<(&str, Option<&str>)> = items
            .iter()
            .map(|item| {
                (
                    item.kind.as_str(),
                    item.subject
                        .as_deref()
                        .or(item.state.as_deref())
                        .or(item.sha.as_deref()),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("commented", None),
                ("reviewed", Some("approved")),
                ("labeled", Some("bug")),
                ("review_requested", Some("ankit")),
                ("committed", Some("aa")),
                ("committed", Some("bb")),
                ("cross-referenced", None),
            ]
        );
        let mention = items[6].source.as_ref().expect("a mention");
        assert_eq!(
            (mention.number, mention.pull, mention.repo.as_deref()),
            (12, true, Some("acme/web"))
        );
        assert_eq!(items[0].actor.as_deref(), Some("irwan"));
    }
}
