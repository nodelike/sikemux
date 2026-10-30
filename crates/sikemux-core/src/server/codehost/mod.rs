//! A project's pull requests, runs, issues and releases on its code host, as
//! clients list them, with what the core works out for each row. The core
//! keeps each list a connection shows, reads it again while one does, and
//! tells clients when it changed, so no client keeps time of its own.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::{
    ChangedFile, Check, CheckTally, Comment, Issue, IssueList, IssuePage, Job, JobLog, Label,
    ListedJob, ListedPull, ListedRun, MergeReadiness, OpenPull, OpenRun, Outcome, PendingApproval,
    Pull, PullCounts, PullList, PullState, Release, ReleaseList, Review, ReviewVerdict, Run,
    RunDetail, RunList, RunPage, RunTiming, TrackedItem, TrackedKind, WorkflowFile,
};
use sikemux_process::sync::Locked;
use tokio::task::JoinSet;

pub(crate) mod ci;
pub(crate) mod repo;

use super::changed::Topic;
use super::connection::ClientId;
use super::job_graph::{self, WorkflowFiles};
use super::plugins::{Category, Failure, Outcome as Answer};
use super::shown_cache::{self, ShownCache, Shows};
use super::workspace::project_of;
use super::Core;
use repo::{of_project, Repo, RepoId};

/// A list read this recently is answered as it is.
const FRESH: Duration = Duration::from_secs(60);
/// A repository no connection shows any more is kept this long.
const GRACE: Duration = Duration::from_secs(60);
/// The repository's latest runs, among which most open pull requests' commits are.
const RECENT_RUNS: u32 = 100;
/// Pull requests whose commits are past those that ask for their own runs.
const OWN_RUNS_AT_MOST: usize = 30;
/// A commit's runs, once all are over, are not asked for again this long.
const SETTLED_RUNS: Duration = Duration::from_secs(600);
const COMMITS_AT_MOST: usize = 500;
/// A run still going is read again this often while it is shown.
const LIVE_RUN: Duration = Duration::from_secs(4);
/// And the log of a job still going.
const LIVE_LOG: Duration = Duration::from_secs(5);
/// A commit's workflow files never change, but are dropped after a while.
const WORKFLOWS_KEPT: Duration = Duration::from_secs(3600);
const WORKFLOWS_AT_MOST: usize = 32;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workflow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    per_page: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum List {
    Pulls { state: String },
    Runs(RunQuery),
    Issues { state: String, page: u32 },
    Releases,
    Tracked,
    Pull { number: u64 },
    PullFiles { number: u64 },
    Run { id: String, attempt: Option<u64> },
    JobLog { run: String, job: String },
}

impl List {
    /// Which of a project's lists it is, whatever it was asked with.
    fn kind(&self) -> u8 {
        match self {
            Self::Pulls { .. } => 0,
            Self::Runs(_) => 1,
            Self::Issues { .. } => 2,
            Self::Releases => 3,
            Self::Tracked => 4,
            Self::Pull { .. } => 5,
            Self::Run { .. } => 6,
            Self::JobLog { .. } => 7,
            Self::PullFiles { .. } => 8,
        }
    }

    /// What a `codeHost` change names it by.
    fn name(&self) -> &'static str {
        match self {
            Self::Pulls { .. } => "pulls",
            Self::Runs(_) => "runs",
            Self::Issues { .. } => "issues",
            Self::Releases => "releases",
            Self::Tracked => "tracked",
            Self::Pull { .. } => "pull",
            Self::Run { .. } => "run",
            Self::JobLog { .. } => "jobLog",
            Self::PullFiles { .. } => "pullFiles",
        }
    }
}

/// A repository a client names outright, rather than through a project.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub(crate) struct Named {
    #[serde(flatten)]
    id: RepoId,
    #[serde(default)]
    account: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Place {
    /// A project's folder, whose code host the core works out.
    Project(String),
    Repo(Named),
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
    fn of(repo: &Repo, list: impl Serialize, fresh: Duration) -> Result<Self, Failure> {
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

/// The runs on one commit, and when they were read.
struct CommitRuns {
    runs: Vec<Run>,
    read: Instant,
}

#[derive(Default)]
struct State {
    /// By project.
    found: HashMap<String, Found>,
    /// By repository and commit.
    commits: HashMap<(RepoId, String), CommitRuns>,
    /// By repository, commit and the run's own workflow file.
    workflows: HashMap<(RepoId, String, String), (Arc<WorkflowFiles>, Instant)>,
}

#[derive(Default)]
pub(crate) struct Lists {
    lists: ShownCache<Listed>,
    state: Mutex<State>,
}

/// The lists connections show, each kept for the place and kind of list it is
/// shown as.
struct Listed;

impl Shows for Listed {
    type Key = Key;
    type Slot = (Place, u8);
    type With = Repo;
    const LONGEST_BACKOFF: Duration = FRESH;

    fn cache(core: &Core) -> &ShownCache<Self> {
        &core.code_host_lists.lists
    }

    fn served_within(_pace: Duration) -> Duration {
        FRESH
    }

    async fn read(core: Arc<Core>, key: Key, repo: Repo) -> Result<(Value, Duration), Failure> {
        let read = read(&core, &repo, &key.list).await?;
        Ok((read.answer, read.fresh))
    }

    /// Tells clients about each project that shows a list that changed.
    fn moved(core: &Core, moved: Vec<(Key, Vec<(Place, u8)>)>) {
        let mut names: HashMap<Option<String>, HashSet<&'static str>> = HashMap::new();
        for (key, slots) in moved {
            let mut showing: Vec<Option<String>> = slots
                .into_iter()
                .map(|(place, _)| match place {
                    Place::Project(project) => Some(project),
                    Place::Repo(_) => None,
                })
                .collect();
            if showing.is_empty() {
                showing.push(None);
            }
            for project in showing {
                names.entry(project).or_default().insert(key.list.name());
            }
        }
        for (project, names) in names {
            let names = names.into_iter().map(str::to_owned).collect();
            core.changes
                .changed(Topic::CodeHost, project.as_deref(), Some(names));
        }
    }

    fn tidy(core: &Core, shown: &HashSet<(Place, u8)>) {
        let projects: HashSet<&String> = shown
            .iter()
            .filter_map(|(place, _)| match place {
                Place::Project(project) => Some(project),
                Place::Repo(_) => None,
            })
            .collect();
        let mut state = core.code_host_lists.state.locked();
        state
            .commits
            .retain(|_, commit| commit.read.elapsed() < SETTLED_RUNS);
        state
            .found
            .retain(|project, found| projects.contains(project) || found.read.elapsed() < GRACE);
        state
            .workflows
            .retain(|_, (_, read)| read.elapsed() < WORKFLOWS_KEPT);
    }
}

fn listed_state(state: Option<String>) -> Result<String, Failure> {
    let state = state.unwrap_or_else(|| "open".to_owned());
    match state.as_str() {
        "open" | "closed" | "all" => Ok(state),
        _ => Err(Failure::new(
            "codehost",
            Category::Invalid,
            format!("{state} is not open, closed or all"),
        )),
    }
}

#[derive(Deserialize)]
pub(crate) struct PullsParams {
    project: String,
    state: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct RunsParams {
    project: String,
    #[serde(flatten)]
    query: RunQuery,
}

#[derive(Deserialize)]
pub(crate) struct IssuesParams {
    project: String,
    state: Option<String>,
    page: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct ReleasesParams {
    pub project: String,
}

pub(crate) async fn pulls(core: &Arc<Core>, client: ClientId, params: PullsParams) -> Answer {
    match listed_state(params.state) {
        Ok(state) => answered(core, client, params.project, List::Pulls { state }).await,
        Err(failure) => Answer::Failed(failure),
    }
}

pub(crate) async fn runs(core: &Arc<Core>, client: ClientId, params: RunsParams) -> Answer {
    answered(core, client, params.project, List::Runs(params.query)).await
}

pub(crate) async fn issues(core: &Arc<Core>, client: ClientId, params: IssuesParams) -> Answer {
    match listed_state(params.state) {
        Ok(state) => {
            let page = params.page.unwrap_or(1).max(1);
            answered(core, client, params.project, List::Issues { state, page }).await
        }
        Err(failure) => Answer::Failed(failure),
    }
}

pub(crate) async fn tracked(core: &Arc<Core>, client: ClientId, params: ReleasesParams) -> Answer {
    answered(core, client, params.project, List::Tracked).await
}

/// A pull request of a project's code host, or of a repository named outright
/// as the inbox does.
#[derive(Deserialize)]
pub(crate) struct PullParams {
    project: Option<String>,
    repo: Option<Named>,
    number: u64,
}

pub(crate) async fn pull(core: &Arc<Core>, client: ClientId, params: PullParams) -> Answer {
    let list = List::Pull {
        number: params.number,
    };
    let project = params.project.clone();
    let answer = named_or_project(core, client, params.project, params.repo, list).await;
    match (answer, project) {
        (Answer::Value(mut view), Some(project)) if view["merge"] == "conflicts" => {
            let folder = project_of(Path::new(&project), &core.workspaces.project_paths());
            let ends = (
                view["pull"]["base"].as_str(),
                view["pull"]["headSha"].as_str(),
            );
            if let (Some(base), Some(head)) = ends {
                let found = crate::git::remote::git_conflicts(folder, base.into(), head.into());
                if let Ok(files) = found.await {
                    view["conflicts"] = json!(files);
                }
            }
            Answer::Value(view)
        }
        (answer, _) => answer,
    }
}

/// The files a pull request changes, each with its patch as the rows of a diff.
pub(crate) async fn pull_files(core: &Arc<Core>, client: ClientId, params: PullParams) -> Answer {
    let list = List::PullFiles {
        number: params.number,
    };
    named_or_project(core, client, params.project, params.repo, list).await
}

#[derive(Deserialize)]
pub(crate) struct LinkParams {
    url: String,
}

/// The pull request, issue or run a web address shows, with what clients show of its
/// repository's host, when the address is one of a host Sikemux reads; else null.
pub(crate) async fn link(core: &Core, params: LinkParams) -> Value {
    let Some((repo_url, item)) = linked(&params.url) else {
        return Value::Null;
    };
    match repo::shown(core, &repo_url, &Default::default()).await {
        Some(host) => json!({ "codeHost": host, "item": item }),
        None => Value::Null,
    }
}

/// Splits a web address into its repository's address and what it shows there: a pull
/// request as GitHub (`/pull/12`), Bitbucket (`/pull-requests/12`) and GitLab
/// (`/-/merge_requests/12`) write it, an issue (`/issues/12`, `/-/issues/12`), or a run
/// of GitHub's Actions (`/actions/runs/34`).
fn linked(address: &str) -> Option<(String, Value)> {
    let url = url::Url::parse(address).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let parts: Vec<&str> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    let (marker, item) = parts.iter().enumerate().find_map(|(at, part)| {
        let next = parts.get(at + 1)?;
        match *part {
            "pull" | "pull-requests" | "merge_requests" => Some((
                at,
                json!({ "kind": "pull", "number": next.parse::<u64>().ok()? }),
            )),
            "issues" => Some((
                at,
                json!({ "kind": "issue", "number": next.parse::<u64>().ok()? }),
            )),
            "runs" if at > 0 && parts[at - 1] == "actions" => {
                next.parse::<u64>().ok()?;
                Some((at - 1, json!({ "kind": "run", "id": next })))
            }
            _ => None,
        }
    })?;
    let repo = match parts[..marker] {
        [ref repo @ .., "-"] => repo,
        ref repo => repo,
    };
    if repo.len() < 2 {
        return None;
    }
    let mut origin = url.origin().ascii_serialization();
    for part in repo {
        origin.push('/');
        origin.push_str(part);
    }
    Some((origin, item))
}

pub(crate) async fn releases(core: &Arc<Core>, client: ClientId, params: ReleasesParams) -> Answer {
    answered(core, client, params.project, List::Releases).await
}

/// One attempt of a run, the latest when none is named.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunParams {
    project: Option<String>,
    repo: Option<Named>,
    run_id: String,
    attempt: Option<u64>,
}

pub(crate) async fn run(core: &Arc<Core>, client: ClientId, params: RunParams) -> Answer {
    let list = List::Run {
        id: params.run_id,
        attempt: params.attempt,
    };
    named_or_project(core, client, params.project, params.repo, list).await
}

/// A job's log, or `count` of its lines from `from` on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JobLogParams {
    project: Option<String>,
    repo: Option<Named>,
    run_id: String,
    job_id: String,
    from: Option<usize>,
    count: Option<usize>,
}

/// The connection shows none of the code host's lists any more, so they stop being read
/// again for it until it asks for one.
pub(crate) fn away(core: &Core, client: ClientId) {
    core.code_host_lists.lists.away(client, |_| true);
}

pub(crate) async fn job_log(core: &Arc<Core>, client: ClientId, params: JobLogParams) -> Answer {
    let list = List::JobLog {
        run: params.run_id,
        job: params.job_id,
    };
    let answer = named_or_project(core, client, params.project, params.repo, list).await;
    match (answer, params.from, params.count) {
        (Answer::Value(mut log), from, count) if from.is_some() || count.is_some() => {
            if let Some(lines) = log["lines"].as_array_mut() {
                let from = from.unwrap_or_default().min(lines.len());
                let to = count.map_or(lines.len(), |count| {
                    from.saturating_add(count).min(lines.len())
                });
                *lines = lines.drain(from..to).collect();
                log["from"] = json!(from);
            }
            Answer::Value(log)
        }
        (answer, _, _) => answer,
    }
}

async fn named_or_project(
    core: &Arc<Core>,
    client: ClientId,
    project: Option<String>,
    repo: Option<Named>,
    list: List,
) -> Answer {
    match (project, repo) {
        (Some(project), _) => answered(core, client, project, list).await,
        (None, Some(repo)) => answered_at(core, client, Place::Repo(repo), list).await,
        (None, None) => Answer::Failed(Failure::new(
            "codehost",
            Category::Invalid,
            "this needs its project or its repository",
        )),
    }
}

async fn answered(core: &Arc<Core>, client: ClientId, project: String, list: List) -> Answer {
    let project = project_of(Path::new(&project), &core.workspaces.project_paths());
    answered_at(core, client, Place::Project(project), list).await
}

async fn answered_at(core: &Arc<Core>, client: ClientId, place: Place, list: List) -> Answer {
    let repo = match repo_of(core, &place).await {
        Ok(repo) => repo,
        Err(failure) => return Answer::Failed(failure),
    };
    let kind = list.kind();
    let key = Key::of(&repo, list);
    if let Some(answer) = core.code_host_lists.lists.show(client, (place, kind), &key) {
        return Answer::Value(answer);
    }
    match read(core, &repo, &key.list).await {
        Ok(read) => {
            let answer = read.answer.clone();
            shown_cache::keep::<Listed>(core, key, read.answer, read.fresh, repo);
            Answer::Value(answer)
        }
        Err(failure) => Answer::Failed(failure),
    }
}

/// The repository `place` shows; a project's is worked out again once a minute.
async fn repo_of(core: &Arc<Core>, place: &Place) -> Result<Repo, Failure> {
    let project = match place {
        Place::Project(project) => project,
        Place::Repo(named) => {
            return Repo::named(
                core,
                &named.id.provider,
                &named.id.owner,
                &named.id.name,
                named.account.clone(),
            )
        }
    };
    let found = core
        .code_host_lists
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
    core.code_host_lists
        .state
        .locked()
        .found
        .insert(project.clone(), found);
    Ok(repo)
}

/// What the code host lists read lately say of each pull request and issue at `urls`: `open`,
/// `draft`, `closed`, `merged`, or `notPlanned` for an issue closed as not planned. Nothing is
/// read for it, so one no list held lately is left out.
pub(crate) fn known_states(core: &Core, urls: &[&str]) -> HashMap<String, String> {
    let mut known = HashMap::new();
    if urls.is_empty() {
        return known;
    }
    core.code_host_lists.lists.each(|_, answer| {
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
pub(crate) fn picked(core: &Core, project: &str) {
    let forgot = core
        .code_host_lists
        .state
        .locked()
        .found
        .remove(project)
        .is_some();
    if forgot {
        core.changes.changed(Topic::CodeHost, Some(project), None);
    }
}

/// A window changed the repository `owner/name` through the plugin `plugin`,
/// so its lists are read again at once, and again once the host has settled.
pub(crate) fn wrote(core: &Arc<Core>, plugin: &str, params: &Value) {
    let (Some(owner), Some(name)) = (params["owner"].as_str(), params["name"].as_str()) else {
        return;
    };
    let repo = RepoId {
        provider: plugin.to_owned(),
        owner: owner.to_owned(),
        name: name.to_owned(),
    };
    let keys = core.code_host_lists.lists.keys(|key| key.repo == repo);
    shown_cache::read_again_settled::<Listed>(core, keys);
}

/// The refs of `projects` moved, as a push, pull or commit moves them, so
/// their pull requests and runs are read again now.
pub(crate) fn refs_moved(core: &Arc<Core>, projects: &[String]) {
    let repos: HashSet<RepoId> = {
        let state = core.code_host_lists.state.locked();
        projects
            .iter()
            .filter_map(|project| state.found.get(project))
            .map(|found| found.repo.id())
            .collect()
    };
    let keys = core.code_host_lists.lists.keys(|key| {
        repos.contains(&key.repo)
            && matches!(
                key.list,
                List::Pulls { .. }
                    | List::Runs(_)
                    | List::Tracked
                    | List::Pull { .. }
                    | List::PullFiles { .. }
            )
    });
    shown_cache::read_again::<Listed>(core, keys);
}

/// The runs of the repository `repo` moved, so its runs and the checks on its
/// pull requests are read again now.
pub(crate) fn ci_moved(core: &Arc<Core>, repo: &RepoId) {
    let keys = core.code_host_lists.lists.keys(|key| {
        &key.repo == repo
            && matches!(
                key.list,
                List::Pulls { .. } | List::Runs(_) | List::Pull { .. } | List::Run { .. }
            )
    });
    shown_cache::read_again::<Listed>(core, keys);
}

async fn read(core: &Arc<Core>, repo: &Repo, list: &List) -> Result<Read, Failure> {
    match list {
        List::Pulls { state } => Read::of(repo, pull_list(core, repo, state).await?, FRESH),
        List::Runs(query) => Read::of(repo, run_list(repo, query).await?, FRESH),
        List::Issues { state, page } => {
            Read::of(repo, issue_list(repo, state, *page).await?, FRESH)
        }
        List::Releases => Read::of(repo, release_list(repo).await?, FRESH),
        List::Tracked => Read::of(repo, tracked_list(repo).await?, FRESH),
        List::Pull { number } => Read::of(repo, open_pull(repo, *number).await?, FRESH),
        List::PullFiles { number } => Read::of(repo, pull_files_of(repo, *number).await?, FRESH),
        List::Run { id, attempt } => {
            let run = open_run(core, repo, id, *attempt).await?;
            let fresh = pace(&run.run, attempt.is_some());
            Read::of(repo, run, fresh)
        }
        List::JobLog { run, job } => job_log_of(core, repo, run, job).await,
    }
}

fn shaped(repo: &Repo, list: impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(list)
        .map_err(|error| Failure::new(&repo.id().to_string(), Category::Invalid, error.to_string()))
}

fn typed<T: DeserializeOwned>(repo: &Repo, found: Value) -> Result<T, Failure> {
    serde_json::from_value(found).map_err(|error| {
        Failure::new(
            &repo.id().to_string(),
            Category::Invalid,
            format!("the code host answered in an unknown shape: {error}"),
        )
    })
}

/// A file a pull request changes, with its patch as the rows the Git tab's diff draws.
#[derive(Serialize)]
struct PulledFile {
    #[serde(flatten)]
    file: ChangedFile,
    rows: Vec<crate::git::diff::DiffRow>,
}

async fn pull_files_of(repo: &Repo, number: u64) -> Result<Vec<PulledFile>, Failure> {
    let asked = json!({ "number": number, "fullPatches": true });
    let files: Vec<ChangedFile> = typed(repo, repo.call("pullFiles", asked).await?)?;
    Ok(files
        .into_iter()
        .map(|mut file| {
            let rows = file
                .patch
                .take()
                .map(|patch| crate::git::diff::patch_rows(&patch))
                .unwrap_or_default();
            PulledFile { file, rows }
        })
        .collect())
}

/// How many are open and closed, from hosts that count them cheaply.
async fn counts(repo: &Repo, method: &str) -> Option<PullCounts> {
    let found = repo.call(method, json!({})).await.ok()?;
    serde_json::from_value(found).ok()
}

async fn pull_list(core: &Core, repo: &Repo, state: &str) -> Result<PullList, Failure> {
    let (pulls, recent, counts) = tokio::join!(
        repo.call("pulls", json!({ "state": state })),
        repo.call("runs", json!({ "perPage": RECENT_RUNS })),
        counts(repo, "pullCounts"),
    );
    let pulls: Vec<Pull> = typed(repo, pulls?)?;
    let mut by_sha: HashMap<String, Vec<Run>> = HashMap::new();
    if let Ok(page) = recent.and_then(|found| typed::<RunPage>(repo, found)) {
        for run in page.runs {
            by_sha.entry(run.sha.clone()).or_default().push(run);
        }
    }
    let past: HashSet<String> = pulls
        .iter()
        .filter_map(|pull| pull.head_sha.clone())
        .filter(|sha| !by_sha.contains_key(sha))
        .take(OWN_RUNS_AT_MOST)
        .collect();
    by_sha.extend(own_runs(core, repo, past).await);
    let pulls = pulls
        .into_iter()
        .map(|pull| {
            let checks = pull
                .head_sha
                .as_ref()
                .and_then(|sha| by_sha.get(sha))
                .filter(|runs| !runs.is_empty())
                .map(|runs| tally(runs.iter().map(|run| run.outcome)));
            ListedPull {
                state: state_of(&pull),
                closes: closes(&pull.body),
                own_branch: own_branch(&pull, repo.owner()),
                checks,
                pull,
            }
        })
        .collect();
    Ok(PullList { pulls, counts })
}

/// The runs on each of `shas`, which are past the repository's latest. A
/// commit whose runs are all over is not asked for again for a while.
async fn own_runs(core: &Core, repo: &Repo, shas: HashSet<String>) -> HashMap<String, Vec<Run>> {
    let mut runs = HashMap::new();
    let mut ask = Vec::new();
    {
        let state = core.code_host_lists.state.locked();
        for sha in shas {
            match state.commits.get(&(repo.id(), sha.clone())) {
                Some(settled) => {
                    runs.insert(sha, settled.runs.clone());
                }
                None => ask.push(sha),
            }
        }
    }
    let mut asked = JoinSet::new();
    for sha in ask {
        let repo = repo.clone();
        asked.spawn(async move {
            let found = repo
                .call("runs", json!({ "headSha": sha, "perPage": 30 }))
                .await;
            let page = found
                .ok()
                .and_then(|found| serde_json::from_value::<RunPage>(found).ok());
            (sha, page)
        });
    }
    let mut settled = Vec::new();
    while let Some(Ok((sha, page))) = asked.join_next().await {
        let Some(page) = page else { continue };
        if !page.runs.iter().any(unfinished) {
            settled.push((sha.clone(), page.runs.clone()));
        }
        runs.insert(sha, page.runs);
    }
    let mut state = core.code_host_lists.state.locked();
    for (sha, found) in settled {
        let read = Instant::now();
        state
            .commits
            .insert((repo.id(), sha), CommitRuns { runs: found, read });
    }
    while state.commits.len() > COMMITS_AT_MOST {
        let Some(oldest) = state
            .commits
            .iter()
            .min_by_key(|(_, commit)| commit.read)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        state.commits.remove(&oldest);
    }
    runs
}

/// The local branch checking `pull` out makes, when it can be checked out.
fn checkout_branch(repo: &Repo, pull: &Pull, own: bool) -> Option<String> {
    if own {
        return pull.head.clone();
    }
    repo.pull_head(pull.number)
        .map(|_| format!("pr-{}", pull.number))
}

#[derive(Deserialize)]
pub(crate) struct CheckoutParams {
    project: String,
    number: u64,
}

/// The `codeHostCheckout` call: checks pull request `number` out in the project,
/// on its own branch when it lives on the repository, else on `pr-<number>` from
/// the host's ref for it. Uncommitted work is kept as the git pane's checkout keeps it.
pub(crate) async fn checkout(core: &Arc<Core>, params: CheckoutParams) -> Answer {
    let project = project_of(Path::new(&params.project), &core.workspaces.project_paths());
    let repo = match repo_of(core, &Place::Project(project.clone())).await {
        Ok(repo) => repo,
        Err(failure) => return Answer::Failed(failure),
    };
    let failed = |message: String| {
        Answer::Failed(Failure::new(
            &repo.id().to_string(),
            Category::own("checkout"),
            message,
        ))
    };
    let pull: Pull = match repo.call("pull", json!({ "number": params.number })).await {
        Ok(found) => match typed(&repo, found) {
            Ok(pull) => pull,
            Err(failure) => return Answer::Failed(failure),
        },
        Err(failure) => return Answer::Failed(failure),
    };
    let own = own_branch(&pull, repo.owner());
    let Some(branch) = checkout_branch(&repo, &pull, own) else {
        return failed(format!(
            "{} keeps no ref to fetch a pull request from a fork",
            repo.label()
        ));
    };
    let done = if own {
        checkout_own(project, branch.clone()).await
    } else {
        let source = repo.pull_head(pull.number).unwrap_or_default();
        checkout_fetched(project, source, branch.clone()).await
    };
    match done {
        Ok(()) => Answer::Value(json!({ "branch": branch })),
        Err(error) => failed(error),
    }
}

async fn checkout_own(project: String, branch: String) -> Result<(), String> {
    use crate::git::{branches, remote};
    remote::git_fetch(project.clone(), Some("origin".into())).await?;
    let local = branches::git_branches(project.clone())
        .await?
        .iter()
        .any(|found| found.name == branch);
    if local {
        branches::git_checkout_smart(project, branch)
            .await
            .map(drop)
    } else {
        remote::git_checkout_remote_branch(project, "origin".into(), branch.clone(), Some(branch))
            .await
    }
}

async fn checkout_fetched(project: String, source: String, branch: String) -> Result<(), String> {
    use crate::git::{branches, remote};
    remote::git_fetch_ref(project.clone(), "origin".into(), source, branch.clone()).await?;
    branches::git_checkout_smart(project, branch)
        .await
        .map(drop)
}

#[derive(Deserialize)]
pub(crate) struct NewPullParams {
    project: String,
}

/// The `codeHostNewPull` call: what a new pull request from the project starts
/// with. Its head is the project's branch unless others usually merge into it,
/// its base the branch it goes into by default; `branches` are the host's, and
/// `pushed` says the head is on the host already.
pub(crate) async fn new_pull(core: &Arc<Core>, params: NewPullParams) -> Answer {
    use repo::{default_base, is_usual_base};
    let project = project_of(Path::new(&params.project), &core.workspaces.project_paths());
    let repo = match repo_of(core, &Place::Project(project.clone())).await {
        Ok(repo) => repo,
        Err(failure) => return Answer::Failed(failure),
    };
    let branches: Vec<String> = match repo.call("branches", json!({})).await {
        Ok(found) => match typed(&repo, found) {
            Ok(branches) => branches,
            Err(failure) => return Answer::Failed(failure),
        },
        Err(failure) => return Answer::Failed(failure),
    };
    let head = crate::git::status::git_chip(project)
        .await
        .ok()
        .map(|chip| chip.branch)
        .filter(|branch| !branch.is_empty() && !is_usual_base(branch));
    let base = default_base(&branches, head.as_deref()).map(str::to_owned);
    let pushed = head
        .as_ref()
        .is_none_or(|head| branches.is_empty() || branches.contains(head));
    Answer::Value(json!({ "head": head, "base": base, "branches": branches, "pushed": pushed }))
}

/// The longest description an agent's context keeps, and comment.
const CONTEXT_BODY: usize = 12_000;
const CONTEXT_COMMENT: usize = 2_000;
/// The latest comments an agent's context keeps.
const CONTEXT_COMMENTS: usize = 5;

#[derive(Serialize, Deserialize)]
pub struct TrackedParams {
    pub project: String,
    pub kind: TrackedKind,
    pub number: u64,
}

/// The `codeHostTrackedContext` call: an issue or pull request as context an
/// agent can work from, `{uri, title, text}`.
pub(crate) async fn tracked_context(core: &Arc<Core>, params: TrackedParams) -> Answer {
    let project = project_of(Path::new(&params.project), &core.workspaces.project_paths());
    let repo = match repo_of(core, &Place::Project(project)).await {
        Ok(repo) => repo,
        Err(failure) => return Answer::Failed(failure),
    };
    match context_of(&repo, params.kind, params.number).await {
        Ok(context) => Answer::Value(context),
        Err(failure) => Answer::Failed(failure),
    }
}

#[derive(Deserialize)]
pub(crate) struct WorkOnParams {
    project: String,
    number: u64,
}

/// The `codeHostWorkOn` call: starts a chat in the project, as a new chat there
/// starts, with the issue in its composer for the person to read and send.
/// Answers the agent.
pub(crate) async fn work_on(core: &Arc<Core>, params: WorkOnParams) -> Answer {
    let folder = project_of(Path::new(&params.project), &core.workspaces.project_paths());
    let failed =
        |message: &str| Answer::Failed(Failure::new("codehost", Category::own("workOn"), message));
    let repo = match repo_of(core, &Place::Project(folder.clone())).await {
        Ok(repo) => repo,
        Err(failure) => return Answer::Failed(failure),
    };
    let context = match context_of(&repo, TrackedKind::Issue, params.number).await {
        Ok(context) => context,
        Err(failure) => return Answer::Failed(failure),
    };
    let view = core.workspace_view();
    let Some(project) = view
        .projects
        .iter()
        .find(|project| project.path.to_string_lossy() == folder)
    else {
        return failed("Open the project to start an agent in it");
    };
    let Some(launcher) = project.launcher.clone() else {
        return failed("No agent that can chat is installed");
    };
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4().simple());
    let chosen = super::launching::Launch {
        agent_id: agent_id.clone(),
        launcher,
        cwd: project.path.clone(),
        permission_mode: None,
        model: None,
        model_provider: None,
        effort: None,
        resume_id: None,
        account: None,
        fallbacks: Vec::new(),
        program: None,
        waits_for_person: true,
    };
    if let Err(error) = super::launching::start(core, None, chosen).await {
        return failed(&error.to_string());
    }
    let key = super::drafts::chat_key(&agent_id);
    let extra = json!({ "context": [context] });
    if let Err(error) = core.drafts.set(&key, String::new(), Some(extra)) {
        return failed(&error.to_string());
    }
    super::drafts::changed(core, vec![key]);
    let tab = json!({ "project": project.id, "kind": "agent", "target": agent_id });
    if let Ok(tab) = serde_json::from_value(tab) {
        let _ = super::layout::open_tab(core, tab);
    }
    Answer::Value(json!({ "agentId": agent_id }))
}

async fn context_of(repo: &Repo, kind: TrackedKind, number: u64) -> Result<Value, Failure> {
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
            let state = match state_of(pull) {
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

fn state_of(pull: &Pull) -> PullState {
    if pull.state == "merged" || pull.merged_at.is_some() {
        PullState::Merged
    } else if pull.state == "open" && pull.draft {
        PullState::Draft
    } else if pull.state == "open" {
        PullState::Open
    } else {
        PullState::Closed
    }
}

/// The issues a description says it closes, as GitHub links them: "Fixes
/// #12", "closes owner/repo#3".
fn closes(body: &str) -> Vec<u64> {
    static CLOSING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?iu)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s*:?\s+(?:[\w.-]+/[\w.-]+)?#(\d+)",
        )
        .expect("the closing pattern compiles")
    });
    let mut found = Vec::new();
    for number in CLOSING
        .captures_iter(body)
        .filter_map(|caught| caught[1].parse().ok())
    {
        if !found.contains(&number) {
            found.push(number);
        }
    }
    found
}

fn tally(outcomes: impl IntoIterator<Item = Outcome>) -> CheckTally {
    let mut tally = CheckTally::default();
    for outcome in outcomes {
        tally.total += 1;
        if outcome == Outcome::Skipped {
            tally.skipped += 1;
        }
        match outcome {
            Outcome::Success | Outcome::Skipped => tally.passed += 1,
            Outcome::Failure => tally.failing += 1,
            Outcome::Running => tally.running += 1,
            Outcome::Queued => tally.queued += 1,
            Outcome::Blocked => tally.waiting += 1,
            Outcome::Cancelled => tally.cancelled += 1,
            Outcome::Unknown => tally.unknown += 1,
        }
    }
    tally.outcome = if tally.failing > 0 {
        Outcome::Failure
    } else if tally.running > 0 {
        Outcome::Running
    } else if tally.queued > 0 {
        Outcome::Queued
    } else if tally.waiting > 0 {
        Outcome::Blocked
    } else {
        Outcome::Success
    };
    tally
}

fn unfinished(run: &Run) -> bool {
    run.status != "completed"
        && matches!(
            run.outcome,
            Outcome::Running | Outcome::Queued | Outcome::Blocked
        )
}

async fn run_list(repo: &Repo, query: &RunQuery) -> Result<RunList, Failure> {
    let asked = serde_json::to_value(query).unwrap_or_default();
    let page: RunPage = typed(repo, repo.call("runs", asked).await?)?;
    Ok(RunList {
        runs: page
            .runs
            .into_iter()
            .map(|run| ListedRun {
                unfinished: unfinished(&run),
                run,
            })
            .collect(),
        total: page.total,
        next_page: page.next_page,
    })
}

async fn issue_list(repo: &Repo, state: &str, page: u32) -> Result<IssueList, Failure> {
    let (issues, counts) = tokio::join!(
        repo.call("issues", json!({ "state": state, "page": page })),
        counts(repo, "issueCounts"),
    );
    let issues: IssuePage = typed(repo, issues?)?;
    Ok(IssueList {
        issues: issues.issues,
        total: issues.total,
        next_page: issues.next_page,
        counts,
    })
}

/// The open issues and pull requests together, newest first. A host that
/// lists pull requests among its issues has each kept once, as a pull request.
async fn tracked_list(repo: &Repo) -> Result<Vec<TrackedItem>, Failure> {
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

async fn open_pull(repo: &Repo, number: u64) -> Result<OpenPull, Failure> {
    let (pull, reviews) = tokio::join!(
        repo.call("pull", json!({ "number": number })),
        repo.call("pullReviews", json!({ "number": number })),
    );
    let pull: Pull = typed(repo, pull?)?;
    let reviews: Vec<Review> = reviews
        .ok()
        .and_then(|found| serde_json::from_value(found).ok())
        .unwrap_or_default();
    let (runs, jobs): (Vec<Run>, Vec<Check>) = match &pull.head_sha {
        Some(sha) => {
            let (runs, jobs) = tokio::join!(
                repo.call("runs", json!({ "headSha": sha, "perPage": 30 })),
                repo.call("checks", json!({ "sha": sha, "base": pull.base })),
            );
            (
                runs.ok()
                    .and_then(|found| serde_json::from_value::<RunPage>(found).ok())
                    .map(|page| page.runs)
                    .unwrap_or_default(),
                jobs.ok()
                    .and_then(|found| serde_json::from_value(found).ok())
                    .unwrap_or_default(),
            )
        }
        None => (Vec::new(), Vec::new()),
    };
    let checks = match jobs.is_empty() {
        true => (!runs.is_empty()).then(|| tally(runs.iter().map(|run| run.outcome))),
        false => Some(tally(jobs.iter().map(|job| job.outcome))),
    };
    let own = own_branch(&pull, repo.owner());
    Ok(OpenPull {
        state: state_of(&pull),
        closes: closes(&pull.body),
        own_branch: own,
        checkout: checkout_branch(repo, &pull, own),
        verdict: verdict(&reviews),
        reviewed: !reviews.is_empty(),
        merge: readiness(pull.merge_state.as_deref()),
        checks,
        jobs,
        conflicts: Vec::new(),
        runs: runs
            .into_iter()
            .map(|run| ListedRun {
                unfinished: unfinished(&run),
                run,
            })
            .collect(),
        pull,
    })
}

/// Only a person's latest review counts, which is how GitHub scores it too.
fn verdict(reviews: &[Review]) -> Option<ReviewVerdict> {
    let mut latest: HashMap<&str, &str> = HashMap::new();
    for review in reviews {
        if review.state == "COMMENTED" || review.state == "DISMISSED" {
            continue;
        }
        latest.insert(review.author.as_deref().unwrap_or_default(), &review.state);
    }
    if latest.values().any(|state| *state == "CHANGES_REQUESTED") {
        Some(ReviewVerdict::ChangesRequested)
    } else if latest.values().any(|state| *state == "APPROVED") {
        Some(ReviewVerdict::Approved)
    } else {
        None
    }
}

fn readiness(merge_state: Option<&str>) -> MergeReadiness {
    match merge_state {
        Some("dirty") => MergeReadiness::Conflicts,
        Some("blocked") => MergeReadiness::Blocked,
        Some("behind") => MergeReadiness::Behind,
        Some("clean" | "unstable" | "has_hooks") => MergeReadiness::Ready,
        _ => MergeReadiness::Checking,
    }
}

/// One attempt of a run, or the latest, with its jobs laid out by the
/// workflow files at its commit.
async fn open_run(
    core: &Core,
    repo: &Repo,
    id: &str,
    attempt: Option<u64>,
) -> Result<OpenRun, Failure> {
    let latest = repo.call("run", json!({ "runId": id }));
    let (detail, attempts): (RunDetail, u64) = match attempt {
        None => {
            let detail: RunDetail = typed(repo, latest.await?)?;
            let attempts = detail.run.attempt;
            (detail, attempts)
        }
        Some(attempt) => {
            let (older, latest) = tokio::join!(
                repo.call("runAttempt", json!({ "runId": id, "attempt": attempt })),
                latest,
            );
            let latest = latest
                .ok()
                .and_then(|found| typed::<RunDetail>(repo, found).ok())
                .map_or(attempt, |latest| latest.run.attempt);
            (typed(repo, older?)?, latest.max(attempt))
        }
    };
    let RunDetail { run, jobs } = detail;
    let unfinished = unfinished(&run);
    let waiting = may_be_waiting(&run);
    let (files, timing, approvals) = tokio::join!(
        workflow_files(core, repo, &run),
        async {
            if unfinished {
                return None;
            }
            let found = repo.call("runTiming", json!({ "runId": id })).await.ok()?;
            typed::<RunTiming>(repo, found).ok()
        },
        async {
            if !waiting {
                return Vec::new();
            }
            let found = repo.call("pendingApprovals", json!({ "runId": id })).await;
            found
                .ok()
                .and_then(|found| typed::<Vec<PendingApproval>>(repo, found).ok())
                .unwrap_or_default()
        },
    );
    let graph = job_graph::graph_of(&jobs, &files, run.path.as_deref());
    let failed = |job: &&Job| job.outcome == Outcome::Failure;
    let summaries = summaries(&jobs, unfinished);
    Ok(OpenRun {
        attempts,
        unfinished,
        done: jobs.iter().filter(|job| job.status == "completed").count() as u32,
        failed: jobs.iter().filter(failed).count() as u32,
        failed_job: jobs.iter().find(failed).map(|job| job.id.clone()),
        graph,
        timing,
        summaries,
        approvals,
        jobs: jobs
            .into_iter()
            .map(|job| ListedJob {
                stopped_at: job
                    .steps
                    .iter()
                    .find(|step| step.outcome == Outcome::Failure)
                    .map(|step| step.name.clone()),
                job,
            })
            .collect(),
        run,
    })
}

/// Only a run its host is holding has anybody to ask about approving it; asking
/// of any other would spend a request to be told nothing waits.
fn may_be_waiting(run: &Run) -> bool {
    matches!(run.status.as_str(), "waiting" | "action_required")
        || run.conclusion.as_deref() == Some("action_required")
}

/// How soon a shown run is read again. An earlier attempt is over for good,
/// and a run waiting on a person moves only when someone answers it here or
/// its CI moves, so only one that is going is read closely.
fn pace(run: &Run, earlier_attempt: bool) -> Duration {
    if earlier_attempt {
        SETTLED_RUNS
    } else if unfinished(run) && run.outcome != Outcome::Blocked {
        LIVE_RUN
    } else {
        FRESH
    }
}

/// The jobs that may have written a summary. Each is its own request, so a
/// run still going has none asked for until it ends.
fn summaries(jobs: &[Job], unfinished: bool) -> Vec<String> {
    if unfinished {
        return Vec::new();
    }
    jobs.iter()
        .filter(|job| {
            job.status == "completed"
                && job.check_run_id.is_some()
                && job.conclusion.as_deref() != Some("skipped")
        })
        .map(|job| job.id.clone())
        .collect()
}

/// The workflow file a run came from and every file it calls, as they were
/// at the run's commit. Only GitHub's are read.
async fn workflow_files(core: &Core, repo: &Repo, run: &Run) -> Arc<WorkflowFiles> {
    let Some(path) = run
        .path
        .as_deref()
        .filter(|path| path.starts_with(".github/"))
    else {
        return Arc::default();
    };
    let key = (repo.id(), run.sha.clone(), path.to_owned());
    if let Some((files, _)) = core.code_host_lists.state.locked().workflows.get(&key) {
        return files.clone();
    }
    let mut files = WorkflowFiles::new();
    let mut pending = vec![path.to_owned()];
    for _ in 0..job_graph::CALLS_AT_MOST {
        if pending.is_empty() {
            break;
        }
        let mut reading = JoinSet::new();
        for each in pending.drain(..) {
            let repo = repo.clone();
            let sha = run.sha.clone();
            reading.spawn(async move {
                let found = repo
                    .call("workflowFileAt", json!({ "path": each, "gitRef": sha }))
                    .await
                    .ok()
                    .and_then(|found| serde_json::from_value::<WorkflowFile>(found).ok());
                (
                    each,
                    found.and_then(|file| job_graph::read_jobs(&file.text)),
                )
            });
        }
        let mut next = HashSet::new();
        while let Some(Ok((each, jobs))) = reading.join_next().await {
            for called in jobs.iter().flat_map(|jobs| job_graph::calls(jobs)) {
                next.insert(called.to_owned());
            }
            files.insert(each, jobs);
        }
        pending = next
            .into_iter()
            .filter(|called| !files.contains_key(called))
            .collect();
    }
    let files = Arc::new(files);
    let mut state = core.code_host_lists.state.locked();
    state.workflows.insert(key, (files.clone(), Instant::now()));
    while state.workflows.len() > WORKFLOWS_AT_MOST {
        let Some(oldest) = state
            .workflows
            .iter()
            .min_by_key(|(_, (_, read))| *read)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        state.workflows.remove(&oldest);
    }
    files
}

/// A job's log split into its steps. A job still going is read again soon;
/// one that was over before this read began has written all it will.
async fn job_log_of(core: &Core, repo: &Repo, run: &str, job: &str) -> Result<Read, Failure> {
    let known = shown_job(core, &repo.id(), run, job);
    let found = match known {
        Some(found) => Some(found),
        None => repo
            .call("run", json!({ "runId": run }))
            .await
            .ok()
            .and_then(|found| typed::<RunDetail>(repo, found).ok())
            .and_then(|detail| detail.jobs.into_iter().find(|each| each.id == job)),
    };
    let fresh = match &found {
        Some(found) if found.status == "completed" => SETTLED_RUNS,
        Some(_) => LIVE_LOG,
        None => FRESH,
    };
    let log: JobLog = typed(repo, repo.call("jobLog", json!({ "jobId": job })).await?)?;
    let steps = found.map(|found| found.steps).unwrap_or_default();
    Read::of(repo, job_graph::log_of(log, &steps), fresh)
}

/// The job `job` of the run `run` as the core last read the run.
fn shown_job(core: &Core, repo: &RepoId, run: &str, job: &str) -> Option<Job> {
    let mut found = None;
    core.code_host_lists.lists.each(|key, answer| {
        if found.is_some()
            || &key.repo != repo
            || !matches!(&key.list, List::Run { id, .. } if id == run)
        {
            return;
        }
        found = answer["jobs"]
            .as_array()
            .and_then(|jobs| jobs.iter().find(|listed| listed["job"]["id"] == job))
            .and_then(|listed| serde_json::from_value(listed["job"].clone()).ok());
    });
    found
}

async fn release_list(repo: &Repo) -> Result<ReleaseList, Failure> {
    let releases: Vec<Release> = typed(repo, repo.call("releases", json!({})).await?)?;
    let latest = releases
        .iter()
        .find(|release| !release.draft && !release.prerelease)
        .map(|release| release.id);
    Ok(ReleaseList { releases, latest })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(outcome: Outcome) -> Run {
        run_as("completed", outcome)
    }

    fn run_as(status: &str, outcome: Outcome) -> Run {
        serde_json::from_value(json!({
            "id": "1", "name": "CI", "title": "", "workflowId": "1", "path": null,
            "runNumber": 1, "attempt": 1, "event": "push", "status": status,
            "conclusion": null, "outcome": outcome, "branch": null, "sha": "abc",
            "shortSha": "abc", "actor": null, "avatarUrl": null, "createdAt": "",
            "startedAt": null, "updatedAt": "", "pullRequests": [], "url": ""
        }))
        .expect("a run")
    }

    #[test]
    fn queued_running_and_waiting_for_approval_are_unfinished_and_nothing_completed_is() {
        assert!(unfinished(&run_as("in_progress", Outcome::Running)));
        assert!(unfinished(&run_as("queued", Outcome::Queued)));
        assert!(unfinished(&run_as("waiting", Outcome::Blocked)));
        assert!(!unfinished(&run_as("completed", Outcome::Success)));
        assert!(!unfinished(&run_as("completed", Outcome::Blocked)));
    }

    #[test]
    fn only_a_run_that_is_going_is_read_closely() {
        assert_eq!(
            pace(&run_as("in_progress", Outcome::Running), false),
            LIVE_RUN
        );
        assert_eq!(pace(&run_as("queued", Outcome::Queued), false), LIVE_RUN);
        assert_eq!(pace(&run_as("waiting", Outcome::Blocked), false), FRESH);
        assert_eq!(pace(&run_as("completed", Outcome::Success), false), FRESH);
        assert_eq!(
            pace(&run_as("in_progress", Outcome::Running), true),
            SETTLED_RUNS
        );
    }

    fn job(id: u32, conclusion: &str, check_run: Option<&str>) -> Job {
        serde_json::from_value(json!({
            "id": id.to_string(), "name": format!("job {id}"), "status": "completed",
            "conclusion": conclusion, "outcome": "success", "startedAt": null,
            "completedAt": null, "runner": null, "url": null, "checkRunId": check_run,
            "steps": []
        }))
        .expect("a job")
    }

    #[test]
    fn asks_for_no_summaries_while_the_run_is_still_going() {
        assert!(summaries(&[job(1, "success", Some("1"))], true).is_empty());
    }

    #[test]
    fn skips_jobs_that_could_not_have_written_a_summary() {
        let jobs = [
            job(1, "success", Some("1")),
            job(2, "skipped", Some("2")),
            job(3, "success", None),
        ];
        assert_eq!(summaries(&jobs, false), ["1"]);
    }

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

    fn review(author: &str, state: &str) -> Review {
        serde_json::from_value(json!({
            "author": author, "avatarUrl": null, "state": state, "body": "", "submittedAt": null
        }))
        .expect("a review")
    }

    #[test]
    fn only_each_persons_latest_review_counts_and_changes_outweigh_an_approval() {
        assert_eq!(verdict(&[]), None);
        assert_eq!(verdict(&[review("a", "COMMENTED")]), None);
        assert_eq!(
            verdict(&[review("a", "APPROVED")]),
            Some(ReviewVerdict::Approved)
        );
        assert_eq!(
            verdict(&[review("a", "APPROVED"), review("b", "CHANGES_REQUESTED")]),
            Some(ReviewVerdict::ChangesRequested)
        );
        assert_eq!(
            verdict(&[review("a", "CHANGES_REQUESTED"), review("a", "APPROVED")]),
            Some(ReviewVerdict::Approved)
        );
        assert_eq!(
            verdict(&[review("a", "APPROVED"), review("b", "DISMISSED")]),
            Some(ReviewVerdict::Approved)
        );
    }

    fn tally_runs(runs: &[Run]) -> CheckTally {
        tally(runs.iter().map(|run| run.outcome))
    }

    #[test]
    fn a_commit_fails_on_any_failure_and_runs_while_anything_goes() {
        let failing = tally_runs(&[
            run(Outcome::Success),
            run(Outcome::Failure),
            run(Outcome::Running),
        ]);
        assert_eq!(
            (failing.outcome, failing.passed, failing.total),
            (Outcome::Failure, 1, 3)
        );
        assert_eq!(
            tally_runs(&[run(Outcome::Success), run(Outcome::Running)]).outcome,
            Outcome::Running
        );
        assert_eq!(
            tally_runs(&[run(Outcome::Success), run(Outcome::Queued)]).outcome,
            Outcome::Queued
        );
        assert_eq!(
            tally_runs(&[run(Outcome::Queued), run(Outcome::Blocked)]).outcome,
            Outcome::Queued
        );
        assert_eq!(
            tally_runs(&[run(Outcome::Blocked), run(Outcome::Success)]).outcome,
            Outcome::Blocked
        );
        let passed = tally_runs(&[run(Outcome::Success), run(Outcome::Skipped)]);
        assert_eq!((passed.outcome, passed.passed), (Outcome::Success, 2));
    }

    fn headed(number: u64, head: Option<&str>, head_label: Option<&str>) -> Pull {
        serde_json::from_value(json!({
            "number": number, "title": "", "body": "", "state": "open", "draft": false,
            "head": head, "headLabel": head_label, "createdAt": "", "updatedAt": "",
            "labels": [], "reviewers": [], "assignees": [], "avatars": {}, "url": ""
        }))
        .expect("a pull request")
    }

    fn checks_out_as(repo: &Repo, pull: &Pull) -> Option<String> {
        checkout_branch(repo, pull, own_branch(pull, repo.owner()))
    }

    #[test]
    fn a_pull_request_on_the_repository_checks_out_on_its_own_branch() {
        let repo = Repo::on_test_host(Some("pull"));
        let pull = headed(7, Some("feat/x"), Some("nodelike:feat/x"));
        assert!(own_branch(&pull, repo.owner()));
        assert_eq!(checks_out_as(&repo, &pull).as_deref(), Some("feat/x"));
    }

    #[test]
    fn a_forks_pull_request_checks_out_on_a_branch_named_after_its_number() {
        let repo = Repo::on_test_host(Some("pull"));
        let pull = headed(
            49,
            Some("feat/github-actions-plugin"),
            Some("Sujal85526:feat/github-actions-plugin"),
        );
        assert!(!own_branch(&pull, repo.owner()));
        assert_eq!(checks_out_as(&repo, &pull).as_deref(), Some("pr-49"));
        let bitbucket = Repo::on_test_host(None);
        assert_eq!(checks_out_as(&bitbucket, &pull), None);
    }

    #[test]
    fn a_branch_whose_owner_the_host_did_not_say_is_the_repositorys_own() {
        assert!(own_branch(&headed(1, Some("feat/x"), None), "nodelike"));
        assert!(!own_branch(
            &headed(1, None, Some("nodelike:feat/x")),
            "nodelike"
        ));
    }

    #[test]
    fn only_a_run_its_host_is_holding_is_asked_about_approvals() {
        let concluded = |conclusion: &str| {
            let mut run = run_as("completed", Outcome::Blocked);
            run.conclusion = Some(conclusion.to_owned());
            run
        };
        assert!(may_be_waiting(&run_as("waiting", Outcome::Blocked)));
        assert!(may_be_waiting(&run_as("action_required", Outcome::Blocked)));
        assert!(may_be_waiting(&concluded("action_required")));
        assert!(!may_be_waiting(&run_as("in_progress", Outcome::Running)));
        assert!(!may_be_waiting(&concluded("success")));
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
