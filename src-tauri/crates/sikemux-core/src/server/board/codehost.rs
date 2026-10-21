//! Pull requests and their checks on the project's code host, through its
//! plugin and the person's own sign-in to it, and what reviewers say on them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::auto::Pull;
use crate::protocol::CodeHostChoice;
use crate::server::names::project_of;
use crate::server::plugins::{self, Failure};
use crate::server::Core;

/// A code host the core reaches through its plugin, `sikemux.<id>`.
struct Host {
    id: &'static str,
    label: &'static str,
    /// The ways to merge, tried in order while the repository refuses one;
    /// none is the repository's own default.
    merges: &'static [Option<&'static str>],
    /// What comes before a run's number in an address on the host.
    run_path: &'static str,
    /// What comes before a job's number, where an address names one.
    job_path: Option<&'static str>,
}

const HOSTS: [Host; 3] = [
    Host {
        id: "github",
        label: "GitHub",
        merges: &[Some("squash"), Some("merge"), Some("rebase")],
        run_path: "/actions/runs/",
        job_path: Some("/job/"),
    },
    Host {
        id: "bitbucket",
        label: "Bitbucket",
        merges: &[None],
        run_path: "/pipelines/results/",
        job_path: None,
    },
    Host {
        id: "gitlab",
        label: "GitLab",
        merges: &[None],
        run_path: "/-/pipelines/",
        job_path: Some("/-/jobs/"),
    },
];

fn host(id: &str) -> Result<&'static Host, String> {
    HOSTS
        .iter()
        .find(|host| host.id == id)
        .ok_or_else(|| format!("Sikemux does not know the code host {id}"))
}

/// A repository on a code host, and the account that reaches it.
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
    async fn call(&self, method: &str, more: Value) -> Result<Value, Failure> {
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

/// The code host the remote `url` is on, with an account signed in to it,
/// or the repository and account in `picks` for it.
pub(crate) async fn find(
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
) -> Result<Repo, String> {
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
        let pick = picks.get(&host.plugin()).cloned().unwrap_or_default();
        let (owner, name) = pick
            .repo
            .as_deref()
            .and_then(|slug| slug.split_once('/'))
            .map_or((owner, name), |(owner, name)| {
                (owner.to_owned(), name.to_owned())
            });
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
        return Ok(Repo {
            host,
            owner,
            name,
            account,
        });
    }
    let labels: Vec<&str> = HOSTS.iter().map(|host| host.label).collect();
    Err(format!(
        "this project's origin is on none of {}",
        labels.join(", ")
    ))
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
