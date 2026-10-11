// Issues for repositories that use Bitbucket's own issue tracker, a
// repository's tags as its releases, and an inbox of the pull requests waiting
// on the person. Bitbucket numbers issues apart from pull requests, so a
// thread says which it is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client;
use crate::error::{BitbucketError, BitbucketResult};
use crate::repo::{avatar_of, login_of, Links, RepoRef, User};

#[derive(Deserialize, Default)]
struct Content {
    raw: Option<String>,
}

#[derive(Deserialize)]
pub struct IssueRow {
    id: u64,
    #[serde(default)]
    title: String,
    content: Option<Content>,
    #[serde(default)]
    state: String,
    kind: Option<String>,
    priority: Option<String>,
    reporter: Option<User>,
    assignee: Option<User>,
    created_on: String,
    updated_on: Option<String>,
    #[serde(default)]
    links: Links,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Label {
    pub name: String,
    pub color: String,
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

/// Bitbucket's states that still need work; the rest say how an issue ended.
const OPEN_STATES: [&str; 4] = ["new", "open", "on hold", "submitted"];

/// Why an issue was closed, in GitHub's words: done, or not going to happen.
fn reason_of(state: &str) -> Option<&'static str> {
    match state {
        "resolved" | "closed" => Some("completed"),
        "invalid" | "duplicate" | "wontfix" => Some("not_planned"),
        _ => None,
    }
}

impl Issue {
    pub fn from_row(repo: &RepoRef, row: IssueRow) -> Self {
        let open = OPEN_STATES.contains(&row.state.as_str());
        let labels = [row.kind.as_deref(), row.priority.as_deref()]
            .into_iter()
            .flatten()
            .filter(|name| !name.is_empty())
            .map(|name| Label {
                name: name.to_string(),
                color: String::new(),
            })
            .collect();
        Issue {
            number: row.id,
            title: row.title,
            body: row
                .content
                .and_then(|content| content.raw)
                .unwrap_or_default(),
            state: if open { "open" } else { "closed" },
            state_reason: reason_of(&row.state).map(str::to_string),
            author: login_of(row.reporter.as_ref()),
            avatar_url: avatar_of(row.reporter.as_ref()),
            closed_at: (!open).then(|| row.updated_on.clone()).flatten(),
            updated_at: row.updated_on.unwrap_or_else(|| row.created_on.clone()),
            created_at: row.created_on,
            comments: 0,
            labels,
            assignees: login_of(row.assignee.as_ref()).into_iter().collect(),
            url: row
                .links
                .html
                .href
                .unwrap_or_else(|| repo.web(&format!("/issues/{}", row.id))),
        }
    }
}

/// A repository whose issues live in Jira, or nowhere, has no tracker to ask.
fn no_tracker(error: BitbucketError) -> BitbucketError {
    match error {
        BitbucketError::NotFound(_) => BitbucketError::NotFound(
            "This repository has no Bitbucket issue tracker; its issues may live in Jira.".into(),
        ),
        other => other,
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

/// The `q` filter for a list: open ones, closed ones, or every one.
pub fn state_filter(state: &str) -> Option<String> {
    let open = OPEN_STATES
        .map(|state| format!("state=\"{state}\""))
        .join(" OR ");
    match state {
        "all" => None,
        "closed" => Some(format!("NOT ({open})")),
        _ => Some(format!("({open})")),
    }
}

#[derive(Deserialize)]
struct IssuePage {
    #[serde(default)]
    values: Vec<IssueRow>,
    next: Option<String>,
    size: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issues {
    pub issues: Vec<Issue>,
    pub total: u64,
    pub next_page: Option<u32>,
}

pub async fn list(data_dir: &Path, input: IssueQuery) -> BitbucketResult<Issues> {
    let page = input.page.max(1);
    let mut query = vec![
        ("sort", "-updated_on".to_string()),
        ("page", page.to_string()),
        ("pagelen", "30".to_string()),
    ];
    if let Some(filter) = state_filter(&input.state) {
        query.push(("q", filter));
    }
    let found: IssuePage = client::get(data_dir, &input.repo.path("/issues")?, &query)
        .await
        .map_err(no_tracker)?;
    let issues: Vec<Issue> = found
        .values
        .into_iter()
        .map(|row| Issue::from_row(&input.repo, row))
        .collect();
    Ok(Issues {
        total: found.size.unwrap_or(issues.len() as u64),
        next_page: found.next.is_some().then_some(page + 1),
        issues,
    })
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IssueRef {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub number: u64,
}

impl IssueRef {
    fn path(&self, rest: &str) -> BitbucketResult<String> {
        self.repo.path(&format!("/issues/{}{rest}", self.number))
    }
}

pub async fn get(data_dir: &Path, input: IssueRef) -> BitbucketResult<Issue> {
    let row: IssueRow = client::get(data_dir, &input.path("")?, &[])
        .await
        .map_err(no_tracker)?;
    let mut issue = Issue::from_row(&input.repo, row);
    issue.comments = comment_rows(data_dir, &input).await?.len() as u64;
    Ok(issue)
}

#[derive(Deserialize)]
pub struct NewIssue {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

pub async fn create(data_dir: &Path, input: NewIssue) -> BitbucketResult<Issue> {
    if input.title.trim().is_empty() {
        return Err(BitbucketError::BadArg("an issue needs a title".into()));
    }
    let row: IssueRow = client::send_json(
        data_dir,
        Method::POST,
        &input.repo.path("/issues")?,
        &json!({ "title": input.title.trim(), "content": { "raw": input.body } }),
    )
    .await
    .map_err(no_tracker)?;
    Ok(Issue::from_row(&input.repo, row))
}

#[derive(Deserialize)]
pub struct SetState {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub state: String,
}

/// Closing marks an issue resolved; reopening marks it open again. Bitbucket
/// records either as a change on the issue.
pub async fn set_state(data_dir: &Path, input: SetState) -> BitbucketResult<()> {
    let state = if input.state == "open" {
        "open"
    } else {
        "resolved"
    };
    let _: Value = client::send_json(
        data_dir,
        Method::POST,
        &input.issue.path("/changes")?,
        &json!({ "changes": { "state": { "new": state } } }),
    )
    .await
    .map_err(no_tracker)?;
    Ok(())
}

#[derive(Deserialize)]
struct CommentRow {
    id: u64,
    content: Option<Content>,
    user: Option<User>,
    created_on: String,
    #[serde(default)]
    links: Links,
}

async fn comment_rows(data_dir: &Path, issue: &IssueRef) -> BitbucketResult<Vec<CommentRow>> {
    let rows: Vec<CommentRow> = client::get_all(
        data_dir,
        &issue.path("/comments")?,
        &[("pagelen", "100".into()), ("sort", "created_on".into())],
        10,
    )
    .await
    .map_err(no_tracker)?;
    Ok(rows
        .into_iter()
        .filter(|row| {
            row.content
                .as_ref()
                .and_then(|content| content.raw.as_deref())
                .is_some_and(|raw| !raw.trim().is_empty())
        })
        .collect())
}

#[derive(Serialize)]
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

pub async fn comments(data_dir: &Path, input: IssueRef) -> BitbucketResult<Vec<Comment>> {
    Ok(comment_rows(data_dir, &input)
        .await?
        .into_iter()
        .map(|row| Comment {
            id: row.id,
            author: login_of(row.user.as_ref()),
            avatar_url: avatar_of(row.user.as_ref()),
            author_association: None,
            body: row
                .content
                .and_then(|content| content.raw)
                .unwrap_or_default(),
            created_at: row.created_on,
            url: row.links.html.href,
        })
        .collect())
}

#[derive(Deserialize)]
pub struct NewComment {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub body: String,
}

pub async fn add_comment(data_dir: &Path, input: NewComment) -> BitbucketResult<()> {
    let body = input.body.trim();
    if body.is_empty() {
        return Err(BitbucketError::BadArg("a comment needs some text".into()));
    }
    let _: Value = client::send_json(
        data_dir,
        Method::POST,
        &input.issue.path("/comments")?,
        &json!({ "content": { "raw": body } }),
    )
    .await
    .map_err(no_tracker)?;
    Ok(())
}

#[derive(Deserialize)]
struct StateChange {
    new: Option<String>,
}

#[derive(Deserialize, Default)]
struct Changes {
    state: Option<StateChange>,
}

#[derive(Deserialize)]
struct ChangeRow {
    id: u64,
    user: Option<User>,
    created_on: String,
    #[serde(default)]
    changes: Changes,
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

/// A state change read as closing or reopening, saying why it closed.
fn state_event(change: ChangeRow) -> Option<TimelineItem> {
    let new = change.changes.state?.new?;
    let kind = if OPEN_STATES.contains(&new.as_str()) {
        "reopened"
    } else {
        "closed"
    };
    Some(TimelineItem {
        kind,
        id: Some(change.id),
        actor: login_of(change.user.as_ref()),
        avatar_url: avatar_of(change.user.as_ref()),
        at: Some(change.created_on),
        state: reason_of(&new).map(str::to_string),
        ..TimelineItem::default()
    })
}

/// An issue's history: its comments, and each time it was closed or reopened.
pub async fn timeline(data_dir: &Path, input: IssueRef) -> BitbucketResult<Vec<TimelineItem>> {
    let changes_path = input.path("/changes")?;
    let changes_query = [("pagelen", "100".to_string())];
    let (comments, changes) = futures::try_join!(
        comment_rows(data_dir, &input),
        client::get_all::<ChangeRow>(data_dir, &changes_path, &changes_query, 5)
    )?;
    let mut items: Vec<TimelineItem> = comments
        .into_iter()
        .map(|row| TimelineItem {
            kind: "commented",
            id: Some(row.id),
            actor: login_of(row.user.as_ref()),
            avatar_url: avatar_of(row.user.as_ref()),
            at: Some(row.created_on),
            body: row.content.and_then(|content| content.raw),
            ..TimelineItem::default()
        })
        .chain(changes.into_iter().filter_map(state_event))
        .collect();
    items.sort_by(|a, b| a.at.cmp(&b.at));
    Ok(items)
}

#[derive(Deserialize)]
struct TagTarget {
    date: Option<String>,
    author: Option<TagAuthor>,
}

#[derive(Deserialize)]
struct TagAuthor {
    user: Option<User>,
}

#[derive(Deserialize)]
struct TagRow {
    name: String,
    message: Option<String>,
    date: Option<String>,
    tagger: Option<TagAuthor>,
    target: Option<TagTarget>,
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
    pub assets: Vec<Value>,
    pub url: String,
}

/// A tag as a release: Bitbucket has no releases, and a tag is what a team cuts one with.
fn release_of(repo: &RepoRef, index: usize, row: TagRow) -> Release {
    let author = row.tagger.and_then(|tagger| tagger.user).or_else(|| {
        row.target
            .as_ref()
            .and_then(|target| target.author.as_ref())
            .and_then(|author| author.user.clone())
    });
    let lower = row.name.to_ascii_lowercase();
    Release {
        id: index as u64 + 1,
        body: row.message.unwrap_or_default().trim().to_string(),
        draft: false,
        prerelease: ["-alpha", "-beta", "-rc", "-nightly", "-pre"]
            .iter()
            .any(|marker| lower.contains(marker)),
        published_at: row
            .date
            .or_else(|| row.target.and_then(|target| target.date)),
        author: login_of(author.as_ref()),
        assets: Vec::new(),
        url: repo.web(&format!("/src/{}", row.name)),
        tag: row.name.clone(),
        name: row.name,
    }
}

pub async fn releases(data_dir: &Path, repo: RepoRef) -> BitbucketResult<Vec<Release>> {
    let rows: Vec<TagRow> = client::get_all(
        data_dir,
        &repo.path("/refs/tags")?,
        &[("pagelen", "50".into()), ("sort", "-target.date".into())],
        1,
    )
    .await?;
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| release_of(&repo, index, row))
        .collect())
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

#[derive(Deserialize)]
struct Destination {
    repository: Option<RepoName>,
}

#[derive(Deserialize)]
struct RepoName {
    full_name: Option<String>,
}

#[derive(Deserialize)]
struct WaitingRow {
    id: u64,
    title: String,
    updated_on: String,
    destination: Option<Destination>,
    #[serde(default)]
    links: Links,
}

/// When each item was marked done, so an update after that brings it back.
type Dismissed = BTreeMap<String, String>;

fn dismissed_path(data_dir: &Path) -> PathBuf {
    data_dir.join("inbox-done.json")
}

fn dismissed(data_dir: &Path) -> Dismissed {
    std::fs::read(dismissed_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_dismissed(data_dir: &Path, done: &Dismissed) -> BitbucketResult<()> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| BitbucketError::Transport(error.to_string()))?;
    std::fs::write(dismissed_path(data_dir), serde_json::to_vec(done)?)
        .map_err(|error| BitbucketError::Transport(error.to_string()))
}

fn notification(row: WaitingRow, reason: &str, done: &Dismissed) -> Option<Notification> {
    let repo = row
        .destination
        .and_then(|destination| destination.repository)
        .and_then(|repo| repo.full_name)?;
    let id = format!("{repo}#{}", row.id);
    let unread = done.get(&id).is_none_or(|at| *at < row.updated_on);
    Some(Notification {
        title: row.title,
        kind: "PullRequest".into(),
        reason: reason.into(),
        number: Some(row.id),
        unread,
        updated_at: row.updated_on,
        url: row.links.html.href,
        repo,
        id,
    })
}

#[derive(Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    pub all: bool,
}

/// Open pull requests that ask the person for a review in the repositories
/// they work in, then their own open ones. Bitbucket keeps no read state, so
/// marking one done is kept here until the pull request changes again.
pub async fn inbox(data_dir: &Path, input: InboxQuery) -> BitbucketResult<Vec<Notification>> {
    let me = format!(
        "{{{}}}",
        crate::client::Session::current(data_dir).await?.account.id
    );
    let repos = crate::repo::mine(data_dir, 20).await?;
    let wanted = format!("state=\"OPEN\" AND reviewers.uuid=\"{me}\"");
    let asks = futures::future::join_all(repos.iter().map(|repo| {
        let path = format!("/repositories/{}/{}/pullrequests", repo.owner, repo.name);
        let query = [("q", wanted.clone()), ("pagelen", "20".to_string())];
        async move { client::get::<client::Page<WaitingRow>>(data_dir, &path, &query).await }
    }))
    .await;
    let mine: client::Page<WaitingRow> = client::get(
        data_dir,
        &format!("/pullrequests/{me}"),
        &[("state", "OPEN".into()), ("pagelen", "30".into())],
    )
    .await?;
    let done = dismissed(data_dir);
    let mut found: Vec<Notification> = asks
        .into_iter()
        .filter_map(Result::ok)
        .flat_map(|page| page.values)
        .filter_map(|row| notification(row, "review requested", &done))
        .chain(
            mine.values
                .into_iter()
                .filter_map(|row| notification(row, "your pull request", &done)),
        )
        .filter(|item| input.all || item.unread)
        .collect();
    found.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    found.dedup_by(|a, b| a.id == b.id);
    Ok(found)
}

#[derive(Deserialize)]
pub struct DoneRef {
    pub id: String,
}

pub fn mark_read(data_dir: &Path, input: DoneRef) -> BitbucketResult<()> {
    let mut done = dismissed(data_dir);
    done.insert(input.id, now_iso());
    save_dismissed(data_dir, &done)
}

/// Marks done everything the inbox shows now.
pub async fn mark_all_read(data_dir: &Path) -> BitbucketResult<()> {
    let waiting = inbox(data_dir, InboxQuery { all: false }).await?;
    let mut done = dismissed(data_dir);
    let at = now_iso();
    for item in waiting {
        done.insert(item.id, at.clone());
    }
    save_dismissed(data_dir, &done)
}

/// Now as Bitbucket writes times, so it compares with them as text.
fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (hour, minute, second) = (secs % 86_400 / 3600, secs % 3600 / 60, secs % 60);
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.000000+00:00")
}

/// The calendar date `days` after 1970-01-01, by Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoRef {
        RepoRef {
            owner: "team".into(),
            name: "app".into(),
        }
    }

    #[test]
    fn an_issue_reads_as_every_hosts_issue_with_why_it_closed() {
        let row: IssueRow = serde_json::from_value(json!({
            "id": 12, "title": "Login loops on Safari", "content": { "raw": "Steps…" }, "state": "wontfix",
            "kind": "bug", "priority": "major", "reporter": { "nickname": "irwan" }, "assignee": { "nickname": "ankit" },
            "created_on": "t1", "updated_on": "t2", "links": { "html": { "href": "https://bitbucket.org/team/app/issues/12" } }
        }))
        .expect("issue parses");
        let issue = Issue::from_row(&repo(), row);
        assert_eq!(
            (issue.number, issue.state, issue.state_reason.as_deref()),
            (12, "closed", Some("not_planned"))
        );
        assert_eq!(issue.closed_at.as_deref(), Some("t2"));
        assert_eq!(
            issue
                .labels
                .iter()
                .map(|label| label.name.as_str())
                .collect::<Vec<_>>(),
            ["bug", "major"]
        );
        assert_eq!(issue.assignees, ["ankit"]);
        let open: IssueRow =
            serde_json::from_value(json!({ "id": 3, "state": "on hold", "created_on": "t" }))
                .expect("parses");
        let open = Issue::from_row(&repo(), open);
        assert_eq!(
            (open.state, open.url.as_str()),
            ("open", "https://bitbucket.org/team/app/issues/3")
        );
    }

    #[test]
    fn lists_ask_bitbucket_for_open_closed_or_every_issue() {
        let open = state_filter("open").expect("filters");
        assert!(open.contains("state=\"new\"") && open.contains("state=\"on hold\""));
        assert_eq!(state_filter("closed"), Some(format!("NOT {open}")));
        assert_eq!(state_filter("all"), None);
    }

    #[test]
    fn a_missing_tracker_says_where_issues_may_be() {
        assert!(no_tracker(BitbucketError::NotFound(
            "Repository has no issue tracker.".into()
        ))
        .to_string()
        .contains("Jira"));
        assert!(matches!(
            no_tracker(BitbucketError::Auth("x".into())),
            BitbucketError::Auth(_)
        ));
    }

    #[test]
    fn closing_and_reopening_read_as_events() {
        let change = |state: &str| ChangeRow {
            id: 1,
            user: None,
            created_on: "t".into(),
            changes: Changes {
                state: Some(StateChange {
                    new: Some(state.into()),
                }),
            },
        };
        let closed = state_event(change("duplicate")).expect("event");
        assert_eq!(
            (closed.kind, closed.state.as_deref()),
            ("closed", Some("not_planned"))
        );
        assert_eq!(state_event(change("open")).expect("event").kind, "reopened");
        let silent = ChangeRow {
            id: 2,
            user: None,
            created_on: "t".into(),
            changes: Changes::default(),
        };
        assert!(state_event(silent).is_none());
    }

    #[test]
    fn a_tag_reads_as_a_release() {
        let row: TagRow = serde_json::from_value(json!({
            "name": "v2.4.0-rc1", "message": "Release candidate\n",
            "target": { "date": "2026-10-01T10:00:00+00:00", "author": { "user": { "nickname": "ankit" } } }
        }))
        .expect("tag parses");
        let release = release_of(&repo(), 0, row);
        assert_eq!(
            (
                release.tag.as_str(),
                release.prerelease,
                release.body.as_str()
            ),
            ("v2.4.0-rc1", true, "Release candidate")
        );
        assert_eq!(release.author.as_deref(), Some("ankit"));
        assert_eq!(release.url, "https://bitbucket.org/team/app/src/v2.4.0-rc1");
    }

    #[test]
    fn a_pull_request_marked_done_comes_back_once_it_changes() {
        let row = |updated: &str| -> WaitingRow {
            serde_json::from_value(json!({
                "id": 7, "title": "Fix VAT", "updated_on": updated,
                "destination": { "repository": { "full_name": "team/app" } }
            }))
            .expect("parses")
        };
        let mut done = Dismissed::new();
        done.insert(
            "team/app#7".into(),
            "2026-10-09T10:00:00.000000+00:00".into(),
        );
        assert!(
            !notification(
                row("2026-10-09T09:00:00.000000+00:00"),
                "review requested",
                &done
            )
            .expect("item")
            .unread
        );
        assert!(
            notification(
                row("2026-10-09T11:00:00.000000+00:00"),
                "review requested",
                &done
            )
            .expect("item")
            .unread
        );
    }

    #[test]
    fn now_is_written_the_way_bitbucket_writes_times() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_736), (2026, 10, 10));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(now_iso().len(), "2026-10-10T00:00:00.000000+00:00".len());
    }
}
