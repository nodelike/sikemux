//! Pull requests and their checks on the project's code host, through its
//! plugin and the person's own sign-in to it, and what reviewers say on them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::auto::Pull;
use crate::protocol::{
    CiAbilities, CodeHostChoice, ProjectCodeHost, ProjectPull, PullAbilities, PullChecks,
};
use crate::server::names::project_of;
use crate::server::plugins::{self, Failure};
use crate::server::Core;

/// A code host the core reaches through its plugin, `sikemux.<id>`.
struct Host {
    id: &'static str,
    label: &'static str,
    /// What the host calls its CI.
    ci_name: &'static str,
    /// The ways to merge, tried in order while the repository refuses one;
    /// none is the repository's own default.
    merges: &'static [Option<&'static str>],
    /// What comes before a run's number in an address on the host.
    run_path: &'static str,
    /// What comes before a job's number, where an address names one.
    job_path: Option<&'static str>,
    /// Where the host keeps a pull request's head for fetching, before its
    /// number, when it keeps one; a fork's branch is fetched from there.
    pull_heads: Option<&'static str>,
    /// The ways a person can merge a pull request there, in the order offered.
    merge_methods: &'static [&'static str],
    request_changes: bool,
    reopen: bool,
    mergeability: bool,
    ci: CiAbilities,
}

const HOSTS: [Host; 3] = [
    Host {
        id: "github",
        ci_name: "Actions",
        label: "GitHub",
        merges: &[Some("squash"), Some("merge"), Some("rebase")],
        run_path: "/actions/runs/",
        job_path: Some("/job/"),
        pull_heads: Some("pull"),
        merge_methods: &["squash", "merge", "rebase"],
        request_changes: true,
        reopen: true,
        mergeability: true,
        ci: CiAbilities {
            attempts: true,
            approvals: true,
            dispatch: true,
            annotations: true,
            summaries: true,
            artifacts: true,
            billing: true,
            workflow_file: true,
            rerun_failed: true,
            rerun_job: true,
            debug_logs: true,
            delete_runs: true,
            delete_logs: true,
        },
    },
    Host {
        id: "bitbucket",
        ci_name: "Pipelines",
        label: "Bitbucket",
        merges: &[None],
        run_path: "/pipelines/results/",
        job_path: None,
        pull_heads: None,
        merge_methods: &["squash", "merge"],
        request_changes: true,
        reopen: false,
        mergeability: true,
        ci: CiAbilities {
            attempts: false,
            approvals: false,
            dispatch: true,
            annotations: false,
            summaries: false,
            artifacts: false,
            billing: false,
            workflow_file: true,
            rerun_failed: false,
            rerun_job: false,
            debug_logs: false,
            delete_runs: false,
            delete_logs: false,
        },
    },
    Host {
        id: "gitlab",
        ci_name: "Pipelines",
        label: "GitLab",
        merges: &[None],
        run_path: "/-/pipelines/",
        job_path: Some("/-/jobs/"),
        pull_heads: Some("merge-requests"),
        merge_methods: &["merge", "squash"],
        request_changes: false,
        reopen: true,
        mergeability: true,
        ci: CiAbilities {
            attempts: false,
            approvals: false,
            dispatch: true,
            annotations: false,
            summaries: false,
            artifacts: false,
            billing: false,
            workflow_file: true,
            rerun_failed: true,
            rerun_job: true,
            debug_logs: false,
            delete_runs: true,
            delete_logs: false,
        },
    },
];

fn host(id: &str) -> Result<&'static Host, String> {
    HOSTS
        .iter()
        .find(|host| host.id == id)
        .ok_or_else(|| format!("Sikemux does not know the code host {id}"))
}

/// A repository on a code host, and the account that reaches it.
#[derive(Clone)]
pub(crate) struct Repo {
    host: &'static Host,
    owner: String,
    name: String,
    account: Option<String>,
}

impl Repo {
    fn params(&self, more: Value) -> Value {
        let mut params = json!({ "owner": self.owner, "name": self.name, "account": self.account });
        if let (Some(params), Value::Object(more)) = (params.as_object_mut(), more) {
            params.extend(more);
        }
        params
    }
}

impl Pull {
    fn repo(&self) -> Result<Repo, String> {
        Ok(Repo {
            host: host(&self.host)?,
            owner: self.owner.clone(),
            name: self.name.clone(),
            account: self.account.clone(),
        })
    }
}

impl Host {
    fn plugin(&self) -> String {
        format!("sikemux.{}", self.id)
    }
}

impl Repo {
    /// Calls `method` of the host's plugin about this repository, with `more`.
    pub(crate) async fn call(&self, method: &str, more: Value) -> Result<Value, Failure> {
        plugins::call(&self.host.plugin(), method, self.params(more)).await
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

impl Repo {
    /// `owner/name`, as the code host writes it in its addresses.
    pub(crate) fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// What the person picked by hand on each code host for the project `dir`
/// is in, its worktrees included.
pub(crate) fn picks(core: &Core, dir: &Path) -> BTreeMap<String, CodeHostChoice> {
    let project = PathBuf::from(project_of(dir, &core.workspaces.project_paths()));
    core.workspaces
        .view()
        .projects
        .into_iter()
        .find(|known| known.path == project)
        .map(|known| known.code_hosts)
        .unwrap_or_default()
}

/// Where the remote `url` is on a code host, or the repository `picks`
/// names there in its place, signed in to or not.
async fn locate(
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
) -> Option<(&'static Host, String, String)> {
    for host in &HOSTS {
        let Ok(found) = plugins::call(&host.plugin(), "resolveRemote", json!({ "url": url })).await
        else {
            continue;
        };
        let repo = &found["repo"];
        let (Some(owner), Some(name)) = (text(repo, "owner"), text(repo, "name")) else {
            continue;
        };
        if found["sameHost"] != Value::Bool(true) {
            continue;
        }
        let picked = picks
            .get(&host.plugin())
            .and_then(|pick| pick.repo.as_deref())
            .and_then(|slug| slug.split_once('/'));
        return Some(match picked {
            Some((owner, name)) => (host, owner.to_owned(), name.to_owned()),
            None => (host, owner, name),
        });
    }
    None
}

/// The code host the remote `url` is on, with an account signed in to it,
/// or the repository and account in `picks` for it.
pub(crate) async fn find(
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
) -> Result<Repo, String> {
    let Some((host, owner, name)) = locate(url, picks).await else {
        let labels: Vec<&str> = HOSTS.iter().map(|host| host.label).collect();
        return Err(format!(
            "this project's origin is on none of {}",
            labels.join(", ")
        ));
    };
    let pick = picks.get(&host.plugin()).cloned().unwrap_or_default();
    let account = match pick.account {
        Some(account) => Some(account),
        None => plugins::call(
            &host.plugin(),
            "accountFor",
            json!({ "owner": owner, "name": name }),
        )
        .await
        .ok()
        .and_then(|account| account.as_str().map(str::to_owned)),
    };
    if account.is_none() {
        return Err(format!(
            "sign in to {} in Sikemux with an account that can reach {owner}/{name}",
            host.label
        ));
    }
    Ok(Repo {
        host,
        owner,
        name,
        account,
    })
}

/// The repository the project at `project` is on, and the account that
/// reaches it, as a failure its client reads when there is none.
pub(crate) async fn of_project(core: &Arc<Core>, project: &str) -> Result<Repo, Failure> {
    let found = {
        let folder = PathBuf::from(project);
        let core = core.clone();
        crate::server::connection::blocking(move || {
            Ok((super::git::origin(&folder), picks(&core, &folder)))
        })
        .await
    };
    let unsupported = |message: &str| Failure::new("codehost", "unsupported", message);
    let (Some(origin), picks) = found.map_err(|error| unsupported(&error.to_string()))? else {
        return Err(unsupported("this project has no origin remote"));
    };
    let Some((host, _, _)) = locate(&origin, &picks).await else {
        return Err(unsupported(
            "this project's origin is on no code host Sikemux knows",
        ));
    };
    find(&origin, &picks)
        .await
        .map_err(|message| Failure::new(&host.plugin(), "unconfigured", message))
}

impl Repo {
    /// The repository `owner/name` on the code host whose plugin is `plugin`,
    /// reached as `account`.
    pub(crate) fn named(
        plugin: &str,
        owner: &str,
        name: &str,
        account: Option<String>,
    ) -> Result<Self, Failure> {
        let id = plugin.strip_prefix("sikemux.").unwrap_or(plugin);
        let host = host(id).map_err(|message| Failure::new(plugin, "unsupported", message))?;
        Ok(Self {
            host,
            owner: owner.to_owned(),
            name: name.to_owned(),
            account,
        })
    }

    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }

    /// The ref the host keeps pull request `number`'s head at, when it keeps one.
    pub(crate) fn pull_head(&self, number: u64) -> Option<String> {
        self.host
            .pull_heads
            .map(|heads| format!("{heads}/{number}/head"))
    }

    /// What the host is called, as people know it.
    pub(crate) fn label(&self) -> &'static str {
        self.host.label
    }

    pub(crate) fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    /// What a `ci` change names the repository by.
    pub(crate) fn key(&self) -> String {
        format!("{}:{}/{}", self.host.plugin(), self.owner, self.name)
    }
}

/// What clients show of the code host the remote `url` is on.
pub(crate) async fn shown(
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
) -> Option<ProjectCodeHost> {
    let (host, owner, name) = locate(url, picks).await?;
    Some(ProjectCodeHost {
        host: host.id.to_owned(),
        repo: format!("{owner}/{name}"),
        ci_name: host.ci_name.to_owned(),
        sections: ["pulls", "actions", "issues", "releases"]
            .map(str::to_owned)
            .to_vec(),
        pulls: PullAbilities {
            merge_methods: host
                .merge_methods
                .iter()
                .map(|method| (*method).to_owned())
                .collect(),
            request_changes: host.request_changes,
            reopen: host.reopen,
            mergeability: host.mergeability,
        },
        ci: host.ci.clone(),
    })
}

/// Branches others usually merge into, in the order one is picked as a base.
const USUAL_BASES: [&str; 4] = ["main", "master", "develop", "trunk"];

/// A branch others usually merge into, which needs no pull request of its own.
pub(crate) fn is_usual_base(branch: &str) -> bool {
    USUAL_BASES.contains(&branch)
}

/// The branch a new pull request from `head` goes into: a usual base among
/// `branches`, else the first other one.
pub(crate) fn default_base<'a>(branches: &'a [String], head: Option<&str>) -> Option<&'a str> {
    let others = || {
        branches
            .iter()
            .map(String::as_str)
            .filter(move |branch| Some(*branch) != head)
    };
    USUAL_BASES
        .iter()
        .find_map(|usual| others().find(|branch| branch == usual))
        .or_else(|| others().next())
}

/// The open pull request `branch` is the head of, with how the checks on its
/// latest commit stand.
pub(crate) async fn branch_pull(repo: &Repo, branch: &str) -> Result<Option<ProjectPull>, String> {
    let open = repo.call("pulls", json!({ "state": "open" })).await?;
    let Some(found) = open
        .as_array()
        .into_iter()
        .flatten()
        .find(|pull| text(pull, "head").as_deref() == Some(branch))
    else {
        return Ok(None);
    };
    let pull = pull_of(repo, found)?;
    let checks = match text(found, "headSha") {
        Some(sha) => match checks(&pull, branch, &sha).await? {
            Checks::None => None,
            Checks::Running => Some(PullChecks::Pending),
            Checks::Passed => Some(PullChecks::Passing),
            Checks::Failed(_) => Some(PullChecks::Failing),
        },
        None => None,
    };
    Ok(Some(ProjectPull {
        number: pull.number,
        title: text(found, "title").unwrap_or_default(),
        draft: found["draft"].as_bool().unwrap_or(false),
        url: pull.url,
        checks,
    }))
}

impl Repo {
    /// The repository as the CI watch names it.
    pub(crate) fn watched(&self) -> super::super::ci::Shown {
        super::super::ci::Shown {
            provider: self.host.plugin(),
            owner: self.owner.clone(),
            name: self.name.clone(),
            account: self.account.clone(),
            host: None,
        }
    }
}

/// The pull requests the commit `sha` came in through, the merged one first,
/// when the code host the remote `url` is on can tell.
pub(crate) async fn pulls_of(
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
    sha: &str,
) -> Result<Vec<Value>, String> {
    let repo = find(url, picks).await?;
    let found = repo.call("commitPulls", json!({ "sha": sha })).await?;
    Ok(found.as_array().cloned().unwrap_or_default())
}

fn pull_of(repo: &Repo, found: &Value) -> Result<Pull, String> {
    let number = found["number"]
        .as_u64()
        .ok_or("the code host answered without the pull request's number")?;
    Ok(Pull {
        host: repo.host.id.to_owned(),
        owner: repo.owner.clone(),
        name: repo.name.clone(),
        number,
        url: text(found, "url").unwrap_or_default(),
        account: repo.account.clone(),
    })
}

/// Opens a pull request for `head` onto `base`, or finds the one already open.
pub(super) async fn open(
    repo: &Repo,
    title: &str,
    head: &str,
    base: &str,
    body: &str,
) -> Result<Pull, String> {
    let made = json!({ "title": title, "head": head, "base": base, "body": body });
    let error = match repo.call("createPull", made).await {
        Ok(found) => return pull_of(repo, &found),
        Err(error) => error,
    };
    let open = repo
        .call("pulls", json!({ "state": "open" }))
        .await
        .unwrap_or_default();
    open.as_array()
        .into_iter()
        .flatten()
        .find(|pull| text(pull, "head").as_deref() == Some(head))
        .map(|found| pull_of(repo, found))
        .unwrap_or_else(|| Err(error.into()))
}

pub(super) enum Checks {
    /// Nothing has run on the commit yet.
    None,
    Running,
    Passed,
    /// What failed, a line each.
    Failed(String),
}

/// How the CI runs on `sha` went.
pub(super) async fn checks(pull: &Pull, branch: &str, sha: &str) -> Result<Checks, String> {
    let repo = pull.repo()?;
    let page = repo
        .call(
            "runs",
            json!({ "headSha": sha, "branch": branch, "perPage": 50 }),
        )
        .await?;
    let runs: Vec<&Value> = page["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|run| run["sha"].as_str() == Some(sha))
        .collect();
    if runs.is_empty() {
        return Ok(Checks::None);
    }
    if runs
        .iter()
        .any(|run| run["status"].as_str() != Some("completed"))
    {
        return Ok(Checks::Running);
    }
    let failed: Vec<String> = runs
        .iter()
        .filter(|run| run["conclusion"].as_str().map(Conclusion::of) != Some(Conclusion::Passed))
        .map(|run| {
            format!(
                "{}: {} {}",
                text(run, "name").unwrap_or_else(|| "a check".to_owned()),
                text(run, "conclusion").unwrap_or_else(|| "failed".to_owned()),
                text(run, "url").unwrap_or_default()
            )
            .trim_end()
            .to_owned()
        })
        .collect();
    Ok(if failed.is_empty() {
        Checks::Passed
    } else {
        Checks::Failed(failed.join("\n"))
    })
}

/// How a finished CI run or job ended, by the `conclusion` its host gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Conclusion {
    /// It lets a change through.
    Passed,
    Failed,
    /// It was stopped, or ended some way that says nothing of the change.
    Stopped,
}

impl Conclusion {
    pub(crate) fn of(conclusion: &str) -> Self {
        match conclusion {
            "success" | "skipped" | "neutral" => Self::Passed,
            "failure" | "timed_out" | "startup_failure" => Self::Failed,
            _ => Self::Stopped,
        }
    }
}

/// The latest CI runs on `branch`, newest first: of one workflow when
/// `workflow` is given, and only those that passed when `passing`.
pub(crate) async fn runs(
    repo: &Repo,
    branch: &str,
    workflow: Option<&Value>,
    passing: bool,
) -> Result<Vec<Value>, String> {
    let params = json!({
        "branch": branch,
        "workflowId": workflow,
        "status": passing.then_some("success"),
        "perPage": 50,
    });
    let page = repo.call("runs", params).await?;
    Ok(page["runs"].as_array().cloned().unwrap_or_default())
}

/// The repository's CI workflows, each with its `id`, `name` and `path`.
pub(crate) async fn workflows(repo: &Repo) -> Result<Vec<Value>, String> {
    let found = repo.call("workflows", json!({})).await?;
    Ok(found.as_array().cloned().unwrap_or_default())
}

/// One run, `{ run, jobs }`.
pub(crate) async fn run(repo: &Repo, id: &Value) -> Result<Value, String> {
    Ok(repo.call("run", json!({ "runId": id })).await?)
}

/// The last `lines` lines of a job's log.
pub(crate) async fn log_tail(
    repo: &Repo,
    run: &Value,
    job: &Value,
    lines: usize,
) -> Result<Vec<String>, String> {
    let params = json!({ "runId": run, "jobId": job, "tail": lines });
    let found = repo.call("jobLogExcerpt", params).await?;
    Ok(found["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|line| line["text"].as_str().map(str::to_owned))
        .collect())
}

/// A run's id as its code host takes it from an address on it, such as
/// GitHub's `/actions/runs/<id>`.
pub(crate) fn run_in(address: &str) -> Option<Value> {
    HOSTS
        .iter()
        .find_map(|host| address.split_once(host.run_path))
        .and_then(|(_, rest)| digits(rest))
}

/// The job an address on a run points at, such as GitHub's `/job/<id>`.
pub(crate) fn job_in(address: &str) -> Option<Value> {
    HOSTS
        .iter()
        .filter_map(|host| host.job_path)
        .find_map(|path| address.split_once(path))
        .and_then(|(_, rest)| digits(rest))
}

fn digits(rest: &str) -> Option<Value> {
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| digits.into())
}

/// Where a pull request stands.
pub(super) struct Looked {
    /// `open`, `merged` or `closed`.
    pub state: String,
    /// Moves whenever anything on the pull request does, such as a comment.
    pub mark: String,
}

pub(super) async fn look(pull: &Pull) -> Result<Looked, String> {
    let repo = pull.repo()?;
    let found = repo.call("pull", json!({ "number": pull.number })).await?;
    Ok(Looked {
        state: text(&found, "state").unwrap_or_else(|| "open".to_owned()),
        mark: format!("{} {}", found["updatedAt"], found["comments"]),
    })
}

/// A comment or review left on a pull request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Feedback {
    pub id: String,
    pub author: Option<String>,
    #[serde(default, skip_serializing)]
    pub bot: bool,
    #[serde(default)]
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub state: Option<String>,
    #[serde(default, skip_serializing)]
    pub at: String,
}

/// The comments and reviews on the pull request that changed at or after `since`.
pub(super) async fn feedback(pull: &Pull, since: Option<&str>) -> Result<Vec<Feedback>, String> {
    let repo = pull.repo()?;
    let found = repo
        .call(
            "reviewFeedback",
            json!({ "number": pull.number, "since": since }),
        )
        .await?;
    serde_json::from_value(found).map_err(|error| error.to_string())
}

/// Answers each comment in its own thread with `(comment id, text, what it
/// said)`, then asks `reviewers` to look again where the host can.
pub(super) async fn answer(
    pull: &Pull,
    replies: &[(String, String, String)],
    reviewers: &[String],
) -> Result<(), String> {
    let repo = pull.repo()?;
    let replies: Vec<Value> = replies
        .iter()
        .map(|(to, body, quote)| json!({ "to": to, "body": body, "quote": quote }))
        .collect();
    let answered = json!({ "number": pull.number, "replies": replies, "reviewers": reviewers });
    Ok(repo.call("answerReview", answered).await.map(drop)?)
}

/// Closes the pull request without merging it.
pub(super) async fn close(pull: &Pull) -> Result<(), String> {
    let repo = pull.repo()?;
    let closed = json!({ "number": pull.number, "state": "closed" });
    Ok(repo.call("setPullState", closed).await.map(drop)?)
}

/// Merges the pull request at `sha`, the commit whose checks passed, the
/// first way the repository allows.
pub(super) async fn merge(pull: &Pull, sha: &str) -> Result<(), String> {
    let repo = pull.repo()?;
    let mut refused = None;
    for method in repo.host.merges {
        let asked = json!({ "number": pull.number, "method": method, "sha": sha });
        match repo.call("mergePull", asked).await {
            Ok(_) => return Ok(()),
            Err(failure) if failure.category == "merge-method" => refused = Some(failure),
            Err(failure) => return Err(failure.into()),
        }
    }
    Err(refused.map_or_else(|| "the code host merges no way".to_owned(), String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(branches: &[&str]) -> Vec<String> {
        branches.iter().map(|branch| (*branch).to_owned()).collect()
    }

    #[test]
    fn a_new_pull_request_goes_into_the_branch_a_repository_usually_merges_into() {
        let branches = names(&["feat/x", "develop", "main"]);
        assert_eq!(default_base(&branches, Some("feat/x")), Some("main"));
        let branches = names(&["feat/x", "master"]);
        assert_eq!(default_base(&branches, Some("feat/x")), Some("master"));
    }

    #[test]
    fn a_new_pull_request_never_goes_into_the_branch_its_changes_are_on() {
        assert_eq!(default_base(&names(&["main"]), Some("main")), None);
        let branches = names(&["feat/x", "release/1"]);
        assert_eq!(default_base(&branches, Some("feat/x")), Some("release/1"));
    }
}
