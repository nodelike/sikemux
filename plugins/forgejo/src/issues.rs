// Issues and releases of a repository, and the person's notifications: what is
// waiting on them across every repository on the server.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;
use sikemux_plugin_api::codehost::asks::{
    InboxQuery, IssueQuery, IssueRef, NewIssue, RepoRef, SetIssueState,
};
use sikemux_plugin_api::codehost::{Issue, IssuePage, Notification, Release, ReleaseAsset};

use crate::client;
use crate::error::{ForgejoError, ForgejoResult};
use crate::pulls::{label_of, LabelRow};
use crate::repo::{avatar_of, login_of, ApiPath, User};

#[derive(Deserialize)]
pub struct IssueRow {
    number: u64,
    #[serde(default)]
    title: String,
    body: Option<String>,
    #[serde(default)]
    state: String,
    user: Option<User>,
    created_at: String,
    updated_at: Option<String>,
    closed_at: Option<String>,
    #[serde(default)]
    comments: u64,
    labels: Option<Vec<LabelRow>>,
    assignees: Option<Vec<User>>,
    html_url: String,
}

pub fn issue_of(row: IssueRow) -> Issue {
    let closed = row.state == "closed";
    Issue {
        number: row.number,
        title: row.title,
        body: row.body.unwrap_or_default(),
        state: if closed { "closed" } else { "open" }.into(),
        state_reason: closed.then(|| "completed".into()),
        author: login_of(row.user.as_ref()),
        avatar_url: avatar_of(row.user.as_ref()),
        updated_at: row.updated_at.unwrap_or_else(|| row.created_at.clone()),
        created_at: row.created_at,
        closed_at: row.closed_at,
        comments: row.comments,
        labels: row
            .labels
            .unwrap_or_default()
            .into_iter()
            .map(label_of)
            .collect(),
        assignees: row
            .assignees
            .unwrap_or_default()
            .iter()
            .filter_map(|user| login_of(Some(user)))
            .collect(),
        url: row.html_url,
    }
}

pub async fn list(data_dir: &Path, input: IssueQuery) -> ForgejoResult<IssuePage> {
    let state = match input.state.as_deref().unwrap_or_default() {
        "closed" => "closed",
        "all" => "all",
        _ => "open",
    };
    let mut query = vec![("state", state.to_string()), ("type", "issues".to_string())];
    if let Some(labels) = input.labels.filter(|labels| !labels.is_empty()) {
        query.push(("labels", labels));
    }
    if let Some(assignee) = input.assignee.filter(|assignee| !assignee.is_empty()) {
        query.push(("assigned_by", assignee));
    }
    let found: client::Page<IssueRow> = client::get_page(
        data_dir,
        &input.repo.path("/issues")?,
        &query,
        input.page.unwrap_or(1),
        input.per_page.unwrap_or(30),
    )
    .await?;
    let issues: Vec<Issue> = found.items.into_iter().map(issue_of).collect();
    Ok(IssuePage {
        total: found.total.unwrap_or(issues.len() as u64),
        next_page: found.next,
        issues,
    })
}

pub async fn get(data_dir: &Path, input: IssueRef) -> ForgejoResult<Issue> {
    let row: IssueRow = client::API.get(data_dir, &input.path("")?, &[]).await?;
    Ok(issue_of(row))
}

pub async fn create(data_dir: &Path, input: NewIssue) -> ForgejoResult<Issue> {
    if input.title.trim().is_empty() {
        return Err(ForgejoError::BadArg("an issue needs a title".into()));
    }
    let row: IssueRow = client::API
        .send_json(
            data_dir,
            Method::POST,
            &input.repo.path("/issues")?,
            &json!({ "title": input.title.trim(), "body": input.body }),
        )
        .await?;
    Ok(issue_of(row))
}

pub async fn set_state(data_dir: &Path, input: SetIssueState) -> ForgejoResult<()> {
    let state = if input.state == "open" {
        "open"
    } else {
        "closed"
    };
    client::API
        .write(
            data_dir,
            Method::PATCH,
            &input.issue.path("")?,
            Some(&json!({ "state": state })),
        )
        .await
}

#[derive(Deserialize)]
struct AssetRow {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    download_count: u64,
}

#[derive(Deserialize)]
struct ReleaseRow {
    id: u64,
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
    created_at: Option<String>,
    author: Option<User>,
    assets: Option<Vec<AssetRow>>,
    #[serde(default)]
    html_url: String,
}

pub async fn releases(data_dir: &Path, repo: RepoRef) -> ForgejoResult<Vec<Release>> {
    let rows: Vec<ReleaseRow> = client::get_all(data_dir, &repo.path("/releases")?, &[], 2).await?;
    Ok(rows
        .into_iter()
        .map(|row| Release {
            id: row.id,
            name: row
                .name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| row.tag_name.clone()),
            tag: row.tag_name,
            body: row.body.unwrap_or_default(),
            published_at: if row.draft {
                None
            } else {
                row.published_at.or(row.created_at)
            },
            draft: row.draft,
            prerelease: row.prerelease,
            author: login_of(row.author.as_ref()),
            avatar_url: avatar_of(row.author.as_ref()),
            assets: row
                .assets
                .unwrap_or_default()
                .into_iter()
                .map(|asset| ReleaseAsset {
                    id: asset.id,
                    name: asset.name,
                    size_bytes: asset.size,
                    downloads: asset.download_count,
                })
                .collect(),
            url: row.html_url,
        })
        .collect())
}

#[derive(Deserialize)]
struct Subject {
    #[serde(default)]
    title: String,
    #[serde(rename = "type", default)]
    kind: String,
    html_url: Option<String>,
}

#[derive(Deserialize)]
struct RepoName {
    full_name: Option<String>,
}

#[derive(Deserialize)]
pub struct ThreadRow {
    id: u64,
    subject: Option<Subject>,
    repository: Option<RepoName>,
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    updated_at: String,
}

/// A notification as the inbox reads every host's. The server does not say why
/// one came, and names its issue or pull request only in its address.
pub fn notification(row: ThreadRow) -> Notification {
    let subject = row.subject.unwrap_or(Subject {
        title: String::new(),
        kind: String::new(),
        html_url: None,
    });
    let kind = match subject.kind.as_str() {
        "Pull" => "PullRequest",
        "" => "Notification",
        other => other,
    };
    let number = matches!(kind, "PullRequest" | "Issue")
        .then(|| subject.html_url.as_deref())
        .flatten()
        .and_then(|url| url.rsplit('/').next())
        .and_then(|last| last.parse().ok());
    Notification {
        id: row.id.to_string(),
        title: subject.title,
        kind: kind.to_string(),
        reason: String::new(),
        repo: row
            .repository
            .and_then(|repo| repo.full_name)
            .unwrap_or_default(),
        number,
        unread: row.unread,
        updated_at: row.updated_at,
        url: subject.html_url.filter(|url| !url.is_empty()),
    }
}

pub async fn inbox(data_dir: &Path, input: InboxQuery) -> ForgejoResult<Vec<Notification>> {
    let rows: Vec<ThreadRow> = client::get_all(
        data_dir,
        "/notifications",
        &[("all", input.all.to_string())],
        2,
    )
    .await?;
    Ok(rows.into_iter().map(notification).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_names_its_pull_request_from_its_address() {
        let row: ThreadRow = serde_json::from_value(json!({
            "id": 102, "unread": true, "updated_at": "t1",
            "repository": { "full_name": "acme/api" },
            "subject": { "title": "Fix the VAT rounding", "type": "Pull", "html_url": "https://codeberg.org/acme/api/pulls/42" }
        }))
        .expect("notification parses");
        let note = notification(row);
        assert_eq!(
            (note.kind.as_str(), note.number, note.repo.as_str()),
            ("PullRequest", Some(42), "acme/api")
        );
        assert!(note.unread);
    }
}
