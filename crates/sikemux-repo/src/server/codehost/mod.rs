//! A project's pull requests, runs, issues and releases on its code host, as
//! clients list them, with what the core works out for each row. The core
//! keeps each list a connection shows, reads it again while one does, and
//! tells clients when it changed, so no client keeps time of its own.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::codehost::{CodeHostProjectParams, PullsParams, TrackedContextParams};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::{
    Comment, Issue, IssuePage, Label, ListedPull, Pull, PullList, PullState, TrackedItem,
    TrackedKind,
};
use sikemux_process::sync::Locked;

pub mod ci;
pub mod log;
pub mod repo;

use super::changed::Topic;
use super::connection::ClientId;
use super::shown_cache::{self, ShownCache, Shows};
use super::workspace::project_of;
use super::Core;
use repo::{of_project, Repo, RepoId};
use sikemux_calls::plugins::{PluginFailure, PluginOutcome as Answer};
use sikemux_plugin_api::FailureCategory;

/// A list read this recently is answered as it is.
const FRESH: Duration = Duration::from_secs(60);
/// A repository no connection shows any more is kept this long.
const GRACE: Duration = Duration::from_secs(60);
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum List {
    Pulls { state: String },
    Tracked,
}

impl List {
    /// Which of a project's lists it is, whatever it was asked with.
    fn kind(&self) -> u8 {
        match self {
            Self::Pulls { .. } => 0,
            Self::Tracked => 1,
        }
    }

    /// What a `codeHost` change names it by.
    fn name(&self) -> &'static str {
        match self {
            Self::Pulls { .. } => "pulls",
            Self::Tracked => "tracked",
        }
    }
}

/// A list of one repository as one account sees it, shared by every project
/// and connection that shows it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    repo: RepoId,
    account: Option<String>,
    list: List,
}

impl Key {
    fn of(repo: &Repo, list: List) -> Self {
        Self {
            repo: repo.id(),
            account: repo.account().map(str::to_owned),
            list,
        }
    }
}

/// A list as the code host gave it, and how long it holds.
struct Read {
    answer: Value,
    fresh: Duration,
}

impl Read {
    fn of(repo: &Repo, list: impl Serialize, fresh: Duration) -> Result<Self, PluginFailure> {
        Ok(Self {
            answer: shaped(repo, list)?,
            fresh,
        })
    }
}

/// The repository a project's code host shows, as the core last worked it out.
struct Found {
    repo: Repo,
    read: Instant,
}

#[derive(Default)]
struct State {
    /// By project.
    found: HashMap<String, Found>,
}

#[derive(Default)]
pub struct Lists {
    lists: ShownCache<Listed>,
    state: Mutex<State>,
}

/// The lists connections show, each kept for the place and kind of list it is
/// shown as.
struct Listed;

impl Shows for Listed {
    type Key = Key;
    /// The project's folder, and which of its lists.
    type Slot = (String, u8);
    type With = Repo;
    const LONGEST_BACKOFF: Duration = FRESH;

    fn cache(core: &Core) -> &ShownCache<Self> {
        &core.code_host_lists().lists
    }

    fn served_within(_pace: Duration) -> Duration {
        FRESH
    }

    async fn read(_: Arc<Core>, key: Key, repo: Repo) -> Result<(Value, Duration), PluginFailure> {
        let read = read(&repo, &key.list).await?;
        Ok((read.answer, read.fresh))
    }

    /// Tells clients about each project that shows a list that changed.
    fn moved(core: &Core, moved: Vec<(Key, Vec<(String, u8)>)>) {
        let mut names: HashMap<String, HashSet<&'static str>> = HashMap::new();
        for (key, slots) in moved {
            for (project, _) in slots {
                names.entry(project).or_default().insert(key.list.name());
            }
        }
        for (project, names) in names {
            let names = names.into_iter().map(str::to_owned).collect();
            core.changed(What::ReadIn(Topic::CodeHost, Some(&project), Some(names)));
        }
    }

    fn tidy(core: &Core, shown: &HashSet<(String, u8)>) {
        let projects: HashSet<&String> = shown.iter().map(|(project, _)| project).collect();
        core.code_host_lists()
            .state
            .locked()
            .found
            .retain(|project, found| projects.contains(project) || found.read.elapsed() < GRACE);
    }
}

fn listed_state(state: Option<String>) -> Result<String, PluginFailure> {
    let state = state.unwrap_or_else(|| "open".to_owned());
    match state.as_str() {
        "open" | "closed" | "all" => Ok(state),
        _ => Err(PluginFailure::new(
            "codehost",
            FailureCategory::Invalid,
            format!("{state} is not open, closed or all"),
        )),
    }
}

pub async fn pulls(core: &Arc<Core>, client: ClientId, params: PullsParams) -> Answer {
    match listed_state(params.state) {
        Ok(state) => answered(core, client, params.project, List::Pulls { state }).await,
        Err(failure) => Answer::Failed(failure),
    }
}

pub async fn tracked(core: &Arc<Core>, client: ClientId, params: CodeHostProjectParams) -> Answer {
    answered(core, client, params.project, List::Tracked).await
}

async fn answered(core: &Arc<Core>, client: ClientId, project: String, list: List) -> Answer {
    let project = project_of(Path::new(&project), &core.workspaces().project_paths());
    let repo = match repo_of(core, &project).await {
        Ok(repo) => repo,
        Err(failure) => return Answer::Failed(failure),
    };
    let kind = list.kind();
    let key = Key::of(&repo, list);
    if let Some(answer) = core
        .code_host_lists()
        .lists
        .show(client, (project, kind), &key)
    {
        return Answer::Value(answer);
    }
    match read(&repo, &key.list).await {
        Ok(read) => {
            let answer = read.answer.clone();
            shown_cache::keep::<Listed>(core, key, read.answer, read.fresh, repo);
            Answer::Value(answer)
        }
        Err(failure) => Answer::Failed(failure),
    }
}

/// The repository the project at `project` shows, worked out again once a minute.
async fn repo_of(core: &Arc<Core>, project: &str) -> Result<Repo, PluginFailure> {
    let found = core
        .code_host_lists()
        .state
        .locked()
        .found
        .get(project)
        .filter(|found| found.read.elapsed() < FRESH)
        .map(|found| found.repo.clone());
    if let Some(repo) = found {
        return Ok(repo);
    }
    let repo = of_project(core, project).await?;
    let found = Found {
        repo: repo.clone(),
        read: Instant::now(),
    };
    core.code_host_lists()
        .state
        .locked()
        .found
        .insert(project.to_owned(), found);
    Ok(repo)
}

/// What the code host lists read lately say of each pull request and issue at `urls`: `open`,
/// `draft`, `closed`, `merged`, or `notPlanned` for an issue closed as not planned. Nothing is
/// read for it, so one no list held lately is left out.
pub fn known_states(core: &Core, urls: &[&str]) -> HashMap<String, String> {
    let mut known = HashMap::new();
    if urls.is_empty() {
        return known;
    }
    core.code_host_lists().lists.each(|_, answer| {
        let pulls = answer["pulls"].as_array().into_iter().flatten();
        for listed in pulls {
            if let (Some(url), Some(state)) =
                (listed["pull"]["url"].as_str(), listed["state"].as_str())
            {
                if urls.contains(&url) {
                    known.insert(url.to_owned(), state.to_owned());
                }
            }
        }
        for issue in answer["issues"].as_array().into_iter().flatten() {
            let state = match (issue["state"].as_str(), issue["stateReason"].as_str()) {
                (Some("open"), _) => "open",
                (Some(_), Some("not_planned")) => "notPlanned",
                (Some(_), _) => "closed",
                _ => continue,
            };
            if let Some(url) = issue["url"].as_str().filter(|url| urls.contains(url)) {
                known.insert(url.to_owned(), state.to_owned());
            }
        }
        for tracked in answer.as_array().into_iter().flatten() {
            if let Some(url) = tracked["url"].as_str().filter(|url| urls.contains(url)) {
                known
                    .entry(url.to_owned())
                    .or_insert_with(|| "open".to_owned());
            }
        }
    });
    known
}

/// The person picked another repository or account for the project at
/// `project`, so it is worked out again.
pub fn picked(core: &Core, project: &str) {
    let forgot = core
        .code_host_lists()
        .state
        .locked()
        .found
        .remove(project)
        .is_some();
    if forgot {
        core.changed(What::ReadIn(Topic::CodeHost, Some(project), None));
    }
}

/// A window changed the repository `owner/name` through the plugin `plugin`,
/// so its lists are read again at once, and again once the host has settled.
pub fn wrote(core: &Arc<Core>, plugin: &str, params: &Value) {
    let (Some(owner), Some(name)) = (params["owner"].as_str(), params["name"].as_str()) else {
        return;
    };
    let repo = RepoId {
        provider: plugin.to_owned(),
        owner: owner.to_owned(),
        name: name.to_owned(),
    };
    let keys = core.code_host_lists().lists.keys(|key| key.repo == repo);
    shown_cache::read_again_settled::<Listed>(core, keys);
}

/// The refs of `projects` moved, as a push, pull or commit moves them, so
/// their pull requests and runs are read again now.
pub fn refs_moved(core: &Arc<Core>, projects: &[String]) {
    let repos: HashSet<RepoId> = {
        let state = core.code_host_lists().state.locked();
        projects
            .iter()
            .filter_map(|project| state.found.get(project))
            .map(|found| found.repo.id())
            .collect()
    };
    let keys = core
        .code_host_lists()
        .lists
        .keys(|key| repos.contains(&key.repo));
    shown_cache::read_again::<Listed>(core, keys);
}

async fn read(repo: &Repo, list: &List) -> Result<Read, PluginFailure> {
    match list {
        List::Pulls { state } => Read::of(repo, pull_list(repo, state).await?, FRESH),
        List::Tracked => Read::of(repo, tracked_list(repo).await?, FRESH),
    }
}

fn shaped(repo: &Repo, list: impl Serialize) -> Result<Value, PluginFailure> {
    serde_json::to_value(list).map_err(|error| {
        PluginFailure::new(
            &repo.id().to_string(),
            FailureCategory::Invalid,
            error.to_string(),
        )
    })
}

fn typed<T: DeserializeOwned>(repo: &Repo, found: Value) -> Result<T, PluginFailure> {
    serde_json::from_value(found).map_err(|error| {
        PluginFailure::new(
            &repo.id().to_string(),
            FailureCategory::Invalid,
            format!("the code host answered in an unknown shape: {error}"),
        )
    })
}

async fn pull_list(repo: &Repo, state: &str) -> Result<PullList, PluginFailure> {
    let pulls: Vec<Pull> = typed(repo, repo.call("pulls", json!({ "state": state })).await?)?;
    let pulls = pulls
        .into_iter()
        .map(|pull| ListedPull {
            state: pull.listed_state(),
            own_branch: own_branch(&pull, repo.owner()),
            pull,
        })
        .collect();
    Ok(PullList { pulls })
}

/// The longest description an agent's context keeps, and comment.
const CONTEXT_BODY: usize = 12_000;
const CONTEXT_COMMENT: usize = 2_000;
/// The latest comments an agent's context keeps.
const CONTEXT_COMMENTS: usize = 5;

/// The `codeHostTrackedContext` call: an issue or pull request as context an
/// agent can work from, `{uri, title, text}`.
pub async fn tracked_context(core: &Arc<Core>, params: TrackedContextParams) -> Answer {
    let project = project_of(
        Path::new(&params.project),
        &core.workspaces().project_paths(),
    );
    let repo = match repo_of(core, &project).await {
        Ok(repo) => repo,
        Err(failure) => return Answer::Failed(failure),
    };
    match context_of(&repo, params.kind, params.number).await {
        Ok(context) => Answer::Value(context),
        Err(failure) => Answer::Failed(failure),
    }
}

async fn context_of(repo: &Repo, kind: TrackedKind, number: u64) -> Result<Value, PluginFailure> {
    let method = match kind {
        TrackedKind::Issue => "issue",
        TrackedKind::Pull => "pull",
    };
    let (item, comments) = tokio::join!(
        repo.call(method, json!({ "number": number })),
        repo.call("comments", json!({ "number": number })),
    );
    let comments: Vec<Comment> = comments
        .ok()
        .and_then(|found| serde_json::from_value(found).ok())
        .unwrap_or_default();
    let tracked = match kind {
        TrackedKind::Issue => Tracked::Issue(Box::new(typed(repo, item?)?)),
        TrackedKind::Pull => Tracked::Pull(Box::new(typed(repo, item?)?)),
    };
    Ok(context(&tracked, &comments))
}

enum Tracked {
    Issue(Box<Issue>),
    Pull(Box<Pull>),
}

/// An issue or pull request as text an agent can work from: what it is, what it
/// says, and the latest of what was said about it.
fn context(tracked: &Tracked, comments: &[Comment]) -> Value {
    let clip = |text: &str, limit: usize| match text.char_indices().nth(limit) {
        Some((at, _)) => format!(
            "{}\n\n[… cut, {} more characters]",
            text[..at].trim_end(),
            text.chars().count() - limit
        ),
        None => text.to_owned(),
    };
    let labelled = |labels: &[Label]| {
        (!labels.is_empty()).then(|| {
            let names: Vec<&str> = labels.iter().map(|label| label.name.as_str()).collect();
            format!("Labels: {}", names.join(", "))
        })
    };
    let (noun, number, title, url, body, facts) = match tracked {
        Tracked::Issue(issue) => (
            "Issue",
            issue.number,
            &issue.title,
            &issue.url,
            &issue.body,
            vec![
                Some(format!("State: {}", issue.state)),
                labelled(&issue.labels),
                issue
                    .author
                    .as_ref()
                    .map(|author| format!("Author: @{author}")),
            ],
        ),
        Tracked::Pull(pull) => {
            let state = match pull.listed_state() {
                PullState::Open => "open",
                PullState::Draft => "draft",
                PullState::Closed => "closed",
                PullState::Merged => "merged",
            };
            (
                "Pull request",
                pull.number,
                &pull.title,
                &pull.url,
                &pull.body,
                vec![
                    Some(format!("State: {state}")),
                    labelled(&pull.labels),
                    pull.author
                        .as_ref()
                        .map(|author| format!("Author: @{author}")),
                    pull.head
                        .as_ref()
                        .zip(pull.base.as_ref())
                        .map(|(head, base)| format!("Branch: {head} → {base}")),
                ],
            )
        }
    };
    let facts: Vec<String> = facts.into_iter().flatten().collect();
    let body = if body.trim().is_empty() {
        "(no description)".to_owned()
    } else {
        clip(body.trim(), CONTEXT_BODY)
    };
    let mut lines = vec![
        format!("{noun} #{number}: {title}"),
        facts.join(" · "),
        format!("URL: {url}"),
        String::new(),
        body,
    ];
    let latest = &comments[comments.len().saturating_sub(CONTEXT_COMMENTS)..];
    if !latest.is_empty() {
        lines.push(String::new());
        lines.push(if latest.len() < comments.len() {
            format!("## Latest {} of {} comments", latest.len(), comments.len())
        } else {
            "## Comments".to_owned()
        });
        for comment in latest {
            lines.push(String::new());
            let day = comment.created_at.get(..10).unwrap_or(&comment.created_at);
            lines.push(format!(
                "@{}, {day}:",
                comment.author.as_deref().unwrap_or("someone")
            ));
            lines.push(clip(comment.body.trim(), CONTEXT_COMMENT));
        }
    }
    json!({ "uri": url, "title": format!("#{number} {title}"), "text": lines.join("\n") })
}

fn own_branch(pull: &Pull, owner: &str) -> bool {
    pull.head.is_some()
        && pull
            .head_label
            .as_deref()
            .and_then(|label| label.split(':').next())
            .is_none_or(|head| head.is_empty() || head == owner)
}

/// The open issues and pull requests together, newest first. A host that
/// lists pull requests among its issues has each kept once, as a pull request.
async fn tracked_list(repo: &Repo) -> Result<Vec<TrackedItem>, PluginFailure> {
    let (pulls, issues) = tokio::join!(
        repo.call("pulls", json!({ "state": "open" })),
        repo.call("issues", json!({ "state": "open", "page": 1 })),
    );
    let pulls: Vec<Pull> = typed(repo, pulls?)?;
    let issues = issues
        .ok()
        .and_then(|found| serde_json::from_value::<IssuePage>(found).ok())
        .map(|page| page.issues)
        .unwrap_or_default();
    Ok(tracked_items(pulls, issues))
}

fn tracked_items(pulls: Vec<Pull>, issues: Vec<Issue>) -> Vec<TrackedItem> {
    let numbers: HashSet<u64> = pulls.iter().map(|pull| pull.number).collect();
    let pulls = pulls.into_iter().map(|pull| TrackedItem {
        kind: TrackedKind::Pull,
        number: pull.number,
        title: pull.title,
        created_at: pull.created_at,
        url: pull.url,
    });
    let issues = issues
        .into_iter()
        .filter(|issue| !numbers.contains(&issue.number))
        .map(|issue| TrackedItem {
            kind: TrackedKind::Issue,
            number: issue.number,
            title: issue.title,
            created_at: issue.created_at,
            url: issue.url,
        });
    let mut items: Vec<TrackedItem> = pulls.chain(issues).collect();
    items.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then(b.number.cmp(&a.number))
    });
    items
}

impl super::part::Part for Lists {
    fn client_gone(&self, _core: &Arc<Core>, client: ClientId) {
        self.lists.client_gone(client);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull(number: u64, head_label: &str, created_at: &str) -> Pull {
        serde_json::from_value(json!({
            "number": number, "title": format!("pull {number}"), "body": "", "state": "open",
            "draft": false, "author": null, "avatarUrl": null, "authorAssociation": null,
            "head": "main", "headLabel": head_label, "base": "main", "headSha": null,
            "createdAt": created_at, "updatedAt": created_at, "comments": null, "additions": null,
            "deletions": null, "changedFiles": null, "mergeable": null, "mergeState": null,
            "labels": [], "reviewers": [], "assignees": [], "milestone": null, "commits": null,
            "mergedAt": null, "mergedBy": null, "mergeCommitSha": null, "avatars": {}, "url": ""
        }))
        .expect("a pull request")
    }

    fn issue(number: u64, created_at: &str) -> Issue {
        serde_json::from_value(json!({
            "number": number, "title": format!("issue {number}"), "body": "", "state": "open",
            "stateReason": null, "author": null, "avatarUrl": null, "createdAt": created_at,
            "updatedAt": created_at, "closedAt": null, "comments": 0, "labels": [],
            "assignees": [], "url": ""
        }))
        .expect("an issue")
    }

    #[test]
    fn a_forks_branch_only_shares_a_name_with_one_here() {
        assert!(own_branch(&pull(1, "nodelike:main", ""), "nodelike"));
        assert!(own_branch(&pull(1, "", ""), "nodelike"));
        assert!(!own_branch(&pull(49, "Sujal85526:main", ""), "nodelike"));
    }

    #[test]
    fn tracked_items_are_newest_first_and_a_pull_request_listed_as_an_issue_once() {
        let items = tracked_items(
            vec![pull(2, "", "2026-02-01")],
            vec![
                issue(1, "2026-01-01"),
                issue(2, "2026-02-01"),
                issue(3, "2026-03-01"),
            ],
        );
        let shown: Vec<_> = items.iter().map(|item| (item.kind, item.number)).collect();
        assert_eq!(
            shown,
            [
                (TrackedKind::Issue, 3),
                (TrackedKind::Pull, 2),
                (TrackedKind::Issue, 1)
            ]
        );
    }

    fn headed(number: u64, head: Option<&str>, head_label: Option<&str>) -> Pull {
        serde_json::from_value(json!({
            "number": number, "title": "", "body": "", "state": "open", "draft": false,
            "head": head, "headLabel": head_label, "createdAt": "", "updatedAt": "",
            "labels": [], "reviewers": [], "assignees": [], "avatars": {}, "url": ""
        }))
        .expect("a pull request")
    }

    #[test]
    fn a_branch_whose_owner_the_host_did_not_say_is_the_repositorys_own() {
        assert!(own_branch(&headed(1, Some("feat/x"), None), "nodelike"));
        assert!(!own_branch(
            &headed(1, None, Some("nodelike:feat/x")),
            "nodelike"
        ));
    }

    fn comment(id: u64, body: &str) -> Comment {
        serde_json::from_value(json!({
            "id": id, "author": format!("c{id}"), "avatarUrl": null, "authorAssociation": null,
            "body": body, "createdAt": format!("2026-09-0{id}T10:00:00Z"), "url": null
        }))
        .expect("a comment")
    }

    fn text_of(tracked: &Tracked, comments: &[Comment]) -> String {
        context(tracked, comments)["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn an_issues_context_has_its_facts_body_and_latest_comments() {
        let mut found = issue(12, "2026-01-01");
        found.title = "Login crashes".into();
        found.body = "Steps to reproduce.".into();
        found.author = Some("ana".into());
        found.url = "https://github.com/o/r/issues/12".into();
        found.labels = vec![Label {
            name: "bug".into(),
            color: "f00".into(),
        }];
        let comments: Vec<Comment> = (1..=6)
            .map(|id| comment(id, &format!("note {id}")))
            .collect();
        let written = context(&Tracked::Issue(Box::new(found)), &comments);
        assert_eq!(written["uri"], "https://github.com/o/r/issues/12");
        assert_eq!(written["title"], "#12 Login crashes");
        let text = written["text"].as_str().unwrap_or_default();
        assert!(text.contains(
            "Issue #12: Login crashes\nState: open · Labels: bug · Author: @ana\nURL: https://github.com/o/r/issues/12"
        ));
        assert!(text.contains("Steps to reproduce."));
        assert!(text.contains("## Latest 5 of 6 comments"));
        assert!(!text.contains("note 1\n"));
        assert!(text.contains("@c6, 2026-09-06:\nnote 6"));
    }

    #[test]
    fn a_pull_requests_context_names_its_branches() {
        let mut found = pull(9, "", "2026-01-01");
        found.title = "Faster boot".into();
        found.author = Some("bo".into());
        found.head = Some("fast".into());
        assert!(text_of(&Tracked::Pull(Box::new(found)), &[]).contains(
            "Pull request #9: Faster boot\nState: open · Author: @bo · Branch: fast → main"
        ));
    }

    #[test]
    fn a_very_long_body_is_cut_short() {
        let mut found = issue(1, "2026-01-01");
        found.body = "x".repeat(20_000);
        let text = text_of(&Tracked::Issue(Box::new(found)), &[]);
        assert!(text.len() < 13_000);
        assert!(text.contains("8000 more characters"));
    }
}
