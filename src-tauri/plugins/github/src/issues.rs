// Issues: the list, one of them, and closing or reopening it. GitHub returns
// pull requests from the issues endpoint too, which is never what the issues
// list is meant to show.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client;
use crate::common::{avatar_of, login_of, ActorRow, Label, LabelRow, MAX_PER_PAGE};
use crate::error::{GithubError, GithubResult};
use crate::workflows::RepoRef;

const DEFAULT_PER_PAGE: u32 = 30;
const STATES: [&str; 3] = ["open", "closed", "all"];

#[derive(Deserialize)]
struct IssueRow {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    state_reason: Option<String>,
    user: Option<ActorRow>,
    created_at: String,
    updated_at: String,
    closed_at: Option<String>,
    comments: Option<u64>,
    html_url: String,
    #[serde(default)]
    labels: Vec<LabelRow>,
    #[serde(default)]
    assignees: Vec<ActorRow>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    /// Why it was closed: `completed`, `not_planned` or `reopened`.
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

impl From<IssueRow> for Issue {
    fn from(row: IssueRow) -> Self {
        Self {
            number: row.number,
            title: row.title,
            body: row.body.unwrap_or_default(),
            state: row.state,
            state_reason: row.state_reason,
            author: login_of(&row.user),
            avatar_url: avatar_of(&row.user),
            created_at: row.created_at,
            updated_at: row.updated_at,
            closed_at: row.closed_at,
            comments: row.comments.unwrap_or(0),
            labels: row.labels.into_iter().map(Label::from).collect(),
            assignees: row.assignees.into_iter().map(|actor| actor.login).collect(),
            url: row.html_url,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub state: Option<String>,
    /// A GitHub login, or `@me` for whoever is signed in.
    pub assignee: Option<String>,
    pub labels: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[derive(Deserialize)]
struct SearchPage {
    total_count: u64,
    items: Vec<IssueRow>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuePage {
    pub issues: Vec<Issue>,
    pub total: u64,
    pub next_page: Option<u32>,
}

/// A label or login goes into a search as one quoted term, so nothing in it
/// can add a qualifier of its own.
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('"', ""))
}

/// Search, because the issues endpoint mixes pull requests into its pages and
/// a page can come back with few issues or none.
fn search_terms(input: &Query, state: &str) -> GithubResult<String> {
    input.repo.checked()?;
    let mut terms = vec![
        format!("repo:{}/{}", input.repo.owner, input.repo.name),
        "is:issue".to_string(),
    ];
    if state != "all" {
        terms.push(format!("is:{state}"));
    }
    if let Some(assignee) = input
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        terms.push(if assignee == "@me" {
            "assignee:@me".to_string()
        } else {
            format!("assignee:{}", quoted(assignee))
        });
    }
    for label in input
        .labels
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        terms.push(format!("label:{}", quoted(label)));
    }
    Ok(terms.join(" "))
}

pub async fn list(data_dir: &Path, input: Query) -> GithubResult<IssuePage> {
    let state = input.state.clone().unwrap_or_else(|| "open".into());
    if !STATES.contains(&state.as_str()) {
        return Err(GithubError::BadArg(format!(
            "`{state}` is not open, closed or all"
        )));
    }
    let per_page = input
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = input.page.unwrap_or(1).max(1);
    let found: SearchPage = client::get(
        data_dir,
        "/search/issues",
        &[
            ("q", search_terms(&input, &state)?),
            ("sort", "updated".to_string()),
            ("order", "desc".to_string()),
            ("per_page", per_page.to_string()),
            ("page", page.to_string()),
        ],
    )
    .await?;
    let seen = u64::from(page - 1) * u64::from(per_page) + found.items.len() as u64;
    Ok(IssuePage {
        next_page: (seen < found.total_count && !found.items.is_empty()).then_some(page + 1),
        total: found.total_count,
        issues: found.items.into_iter().map(Issue::from).collect(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
}

pub async fn get(data_dir: &Path, input: IssueRef) -> GithubResult<Issue> {
    let path = input.repo.path(&format!("/issues/{}", input.number))?;
    let row: IssueRow = client::get(data_dir, &path, &[]).await?;
    Ok(Issue::from(row))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetState {
    #[serde(flatten)]
    pub issue: IssueRef,
    /// `open` or `closed`.
    pub state: String,
}

pub async fn set_state(data_dir: &Path, input: SetState) -> GithubResult<()> {
    if !matches!(input.state.as_str(), "open" | "closed") {
        return Err(GithubError::BadArg(format!(
            "`{}` is not open or closed",
            input.state
        )));
    }
    let path = input
        .issue
        .repo
        .path(&format!("/issues/{}", input.issue.number))?;
    let body = json!({ "state": input.state });
    client::act(data_dir, reqwest::Method::PATCH, &path, Some(&body)).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewIssue {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

pub async fn create(data_dir: &Path, input: NewIssue) -> GithubResult<Issue> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(GithubError::BadArg("an issue needs a title".into()));
    }
    let body = json!({ "title": title, "body": input.body });
    let row: IssueRow = client::send_json(
        data_dir,
        reqwest::Method::POST,
        &input.repo.path("/issues")?,
        &body,
    )
    .await?;
    Ok(Issue::from(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_labels_and_assignees() {
        let row: IssueRow = serde_json::from_value(json!({
            "number": 3, "title": "Crash on open", "state": "open",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "html_url": "https://github.com/a/b/issues/3",
            "labels": [{ "name": "bug", "color": "d73a4a" }],
            "assignees": [{ "login": "nodelike" }],
            "comments": 4,
        }))
        .expect("parses");
        let issue = Issue::from(row);
        assert_eq!(issue.labels.first().map(|l| l.name.as_str()), Some("bug"));
        assert_eq!(issue.assignees, ["nodelike"]);
        assert_eq!(issue.comments, 4);
    }

    #[test]
    fn searches_one_repository_for_issues_only() -> GithubResult<()> {
        let query = Query {
            repo: RepoRef {
                owner: "nodelike".into(),
                name: "sikemux".into(),
            },
            state: None,
            assignee: Some("@me".into()),
            labels: Some("bug, good first issue".into()),
            page: None,
            per_page: None,
        };
        assert_eq!(
            search_terms(&query, "open")?,
            "repo:nodelike/sikemux is:issue is:open assignee:@me label:\"bug\" label:\"good first issue\""
        );
        assert_eq!(
            search_terms(&query, "all")?,
            "repo:nodelike/sikemux is:issue assignee:@me label:\"bug\" label:\"good first issue\""
        );
        Ok(())
    }

    #[test]
    fn a_label_cannot_smuggle_in_a_qualifier() {
        assert_eq!(quoted("x\" repo:evil/x"), "\"x repo:evil/x\"");
    }

    #[tokio::test]
    async fn refuses_a_state_github_would_not_take() {
        let repo = RepoRef {
            owner: "a".into(),
            name: "b".into(),
        };
        let bad = SetState {
            issue: IssueRef { repo, number: 1 },
            state: "archived".into(),
        };
        assert!(set_state(&std::env::temp_dir(), bad).await.is_err());

        let untitled = NewIssue {
            repo: RepoRef {
                owner: "a".into(),
                name: "b".into(),
            },
            title: "   ".into(),
            body: "why".into(),
        };
        assert!(create(&std::env::temp_dir(), untitled).await.is_err());
    }
}
