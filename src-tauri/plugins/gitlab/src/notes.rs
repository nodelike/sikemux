// Notes: GitLab's word for both the comments on a merge request or issue and
// the lines it writes itself when something happens to one ("approved this
// merge request", "added 2 commits"). Comments become comments; the events
// GitLab writes are read back into the event names the Git pane draws.

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client;
use crate::error::{GitlabError, GitlabResult};
use crate::repo::{avatar_of, login_of, RepoRef, User};

/// Which thread a number names: merge requests and issues are numbered apart.
#[derive(Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ThreadOf {
    #[default]
    Pull,
    Issue,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
    #[serde(default)]
    pub of: ThreadOf,
}

impl Thread {
    pub fn path(&self, rest: &str) -> GitlabResult<String> {
        let kind = match self.of {
            ThreadOf::Pull => "merge_requests",
            ThreadOf::Issue => "issues",
        };
        self.repo.path(&format!("/{kind}/{}{rest}", self.number))
    }
}

#[derive(Deserialize)]
pub struct NoteRow {
    id: u64,
    #[serde(default)]
    body: String,
    author: Option<User>,
    created_at: String,
    #[serde(default)]
    system: bool,
}

#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    pub kind: &'static str,
    pub id: Option<u64>,
    pub actor: Option<String>,
    pub avatar_url: Option<String>,
    pub association: Option<String>,
    pub at: Option<String>,
    pub body: Option<String>,
    pub state: Option<String>,
    pub sha: Option<String>,
    pub message: Option<String>,
    pub subject: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: u64,
    pub author: Option<String>,
    pub avatar_url: Option<String>,
    pub author_association: Option<String>,
    pub body: String,
    pub created_at: String,
    pub url: Option<String>,
}

/// What is named after `after`: `**quoted text**`, `~"a label"`, `~label` or `@name`.
fn named(text: &str, after: &str) -> Option<String> {
    let rest = text.split_once(after)?.1.trim();
    let found = if let Some(bold) = rest.strip_prefix("**") {
        bold.split("**").next()
    } else {
        let bare = rest.trim_start_matches(['@', '~']);
        match bare.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next(),
            None => bare.split([' ', ',']).next(),
        }
    };
    found.map(str::to_string).filter(|name| !name.is_empty())
}

/// The events a system note stands for. A note that adds commits names each one
/// on a line of its own, so it can stand for several.
pub fn events_of(body: &str) -> Vec<TimelineItem> {
    let first = body.lines().next().unwrap_or_default().trim();
    let event = |kind: &'static str| TimelineItem {
        kind,
        ..TimelineItem::default()
    };
    let with_subject = |kind: &'static str, subject: Option<String>| TimelineItem {
        kind,
        subject,
        ..TimelineItem::default()
    };
    if first.starts_with("added ") && first.contains("commit") {
        return body
            .lines()
            .filter_map(|line| line.trim().strip_prefix("* "))
            .filter_map(|line| {
                let (sha, message) = line.split_once(" - ")?;
                Some(TimelineItem {
                    kind: "committed",
                    sha: Some(sha.trim().to_string()),
                    message: Some(message.trim().to_string()),
                    ..TimelineItem::default()
                })
            })
            .collect();
    }
    let item = match first {
        "approved this merge request" => TimelineItem {
            kind: "reviewed",
            state: Some("approved".into()),
            ..TimelineItem::default()
        },
        "merged" => event("merged"),
        "closed" => event("closed"),
        "reopened" => event("reopened"),
        "marked this merge request as **ready**" => event("ready_for_review"),
        "marked this merge request as **draft**" => event("convert_to_draft"),
        _ if first.starts_with("requested review from ") => {
            with_subject("review_requested", named(first, "requested review from"))
        }
        _ if first.starts_with("removed review request for ") => with_subject(
            "review_request_removed",
            named(first, "removed review request for"),
        ),
        _ if first.starts_with("assigned to ") => {
            with_subject("assigned", named(first, "assigned to"))
        }
        _ if first.starts_with("unassigned ") => {
            with_subject("unassigned", named(first, "unassigned"))
        }
        _ if first.starts_with("added ~") => with_subject("labeled", named(first, "added")),
        _ if first.starts_with("removed ~") => with_subject("unlabeled", named(first, "removed")),
        _ if first.starts_with("changed title from ") => {
            with_subject("renamed", named(first, " to "))
        }
        _ if first.starts_with("mentioned in ") => with_subject(
            "cross-referenced",
            Some(first.trim_start_matches("mentioned in ").to_string()),
        ),
        _ if first.starts_with("deleted the ") && first.ends_with(" branch") => {
            event("head_ref_deleted")
        }
        _ if first.starts_with("force-pushed") || first.starts_with("added 0 new commits") => {
            event("head_ref_force_pushed")
        }
        _ => return Vec::new(),
    };
    vec![item]
}

/// A thread's history, oldest first: comments, and the events GitLab wrote.
pub fn timeline_of(rows: Vec<NoteRow>) -> Vec<TimelineItem> {
    rows.into_iter()
        .flat_map(|row| {
            let actor = login_of(row.author.as_ref());
            let avatar = avatar_of(row.author.as_ref());
            let items = if row.system {
                events_of(&row.body)
            } else {
                vec![TimelineItem {
                    kind: "commented",
                    body: Some(row.body.clone()),
                    ..TimelineItem::default()
                }]
            };
            items.into_iter().map(move |item| TimelineItem {
                id: Some(row.id),
                actor: actor.clone(),
                avatar_url: avatar.clone(),
                at: Some(row.created_at.clone()),
                ..item
            })
        })
        .collect()
}

/// Oldest first. Read newest first, so a thread longer than the pages read
/// loses its oldest notes rather than its newest.
async fn notes(data_dir: &Path, thread: &Thread) -> GitlabResult<Vec<NoteRow>> {
    let mut rows: Vec<NoteRow> = client::get_all(
        data_dir,
        &thread.path("/notes")?,
        &[("sort", "desc".into()), ("order_by", "created_at".into())],
        5,
    )
    .await?;
    rows.reverse();
    Ok(rows)
}

pub async fn timeline(data_dir: &Path, thread: Thread) -> GitlabResult<Vec<TimelineItem>> {
    Ok(timeline_of(notes(data_dir, &thread).await?))
}

pub async fn comments(data_dir: &Path, thread: Thread) -> GitlabResult<Vec<Comment>> {
    Ok(notes(data_dir, &thread)
        .await?
        .into_iter()
        .filter(|row| !row.system)
        .map(|row| Comment {
            id: row.id,
            author: login_of(row.author.as_ref()),
            avatar_url: avatar_of(row.author.as_ref()),
            author_association: None,
            body: row.body,
            created_at: row.created_at,
            url: None,
        })
        .collect())
}

#[derive(Deserialize)]
pub struct NewComment {
    #[serde(flatten)]
    pub thread: Thread,
    pub body: String,
}

pub async fn add_comment(data_dir: &Path, input: NewComment) -> GitlabResult<()> {
    let body = input.body.trim();
    if body.is_empty() {
        return Err(GitlabError::BadArg("a comment needs some text".into()));
    }
    client::write(
        data_dir,
        Method::POST,
        &input.thread.path("/notes")?,
        Some(&json!({ "body": body })),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(body: &str) -> Vec<(&'static str, Option<String>)> {
        events_of(body)
            .into_iter()
            .map(|item| (item.kind, item.subject))
            .collect()
    }

    #[test]
    fn reads_what_gitlab_writes_when_something_happens() {
        assert_eq!(
            events_of("approved this merge request")[0].state.as_deref(),
            Some("approved")
        );
        assert_eq!(kinds("merged"), [("merged", None)]);
        assert_eq!(
            kinds("requested review from @irwan"),
            [("review_requested", Some("irwan".into()))]
        );
        assert_eq!(
            kinds("assigned to @ankit"),
            [("assigned", Some("ankit".into()))]
        );
        assert_eq!(
            kinds("added ~\"needs review\" label"),
            [("labeled", Some("needs review".into()))]
        );
        assert_eq!(
            kinds("added ~backend label"),
            [("labeled", Some("backend".into()))]
        );
        assert_eq!(
            kinds("changed title from **Draft: fix** to **Fix the VAT rounding**"),
            [("renamed", Some("Fix the VAT rounding".into()))]
        );
        assert_eq!(
            kinds("marked this merge request as **ready**"),
            [("ready_for_review", None)]
        );
        assert_eq!(
            kinds("mentioned in merge request !51"),
            [("cross-referenced", Some("merge request !51".into()))]
        );
        assert!(events_of("changed the description").is_empty());
    }

    #[test]
    fn a_note_that_adds_commits_names_each_one() {
        let items = events_of("added 2 commits\n\n<ul><li>9c41e2a0 - fix(invoices): round VAT per line</li></ul>\n* 9c41e2a0 - fix(invoices): round VAT per line\n* 1a2b3c4d - test: VAT on a credit note\n\n[Compare with previous version](x)");
        let commits: Vec<(Option<&str>, Option<&str>)> = items
            .iter()
            .map(|item| (item.sha.as_deref(), item.message.as_deref()))
            .collect();
        assert_eq!(
            commits,
            [
                (Some("9c41e2a0"), Some("fix(invoices): round VAT per line")),
                (Some("1a2b3c4d"), Some("test: VAT on a credit note"))
            ]
        );
    }

    #[test]
    fn comments_and_events_share_one_history_with_who_and_when() {
        let rows: Vec<NoteRow> = serde_json::from_value(json!([
            { "id": 1, "body": "Looks good, one nit", "author": { "username": "irwan", "avatar_url": "https://a/i.png" }, "created_at": "t1", "system": false },
            { "id": 2, "body": "approved this merge request", "author": { "username": "irwan" }, "created_at": "t2", "system": true },
            { "id": 3, "body": "changed the description", "author": { "username": "ankit" }, "created_at": "t3", "system": true }
        ]))
        .expect("notes parse");
        let items = timeline_of(rows);
        assert_eq!(items.len(), 2);
        assert_eq!(
            (
                items[0].kind,
                items[0].body.as_deref(),
                items[0].actor.as_deref()
            ),
            ("commented", Some("Looks good, one nit"), Some("irwan"))
        );
        assert_eq!(
            (items[1].kind, items[1].id, items[1].at.as_deref()),
            ("reviewed", Some(2), Some("t2"))
        );
    }
}
