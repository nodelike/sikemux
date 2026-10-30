//! Pull requests and their checks on the project's code host, through its
//! plugin and the person's own sign-in to it, and what reviewers say on them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::protocol::{CodeHostChoice, ProjectCodeHost, ProjectPull, PullChecks};
use crate::server::board::Pull;
use crate::server::plugins::{self, Category, CodeHostSpec, Failure, Provided};
use crate::server::workspace::project_of;
use crate::server::Core;
use sikemux_plugin_api::codehost::{Repo as RemoteRepo, Resolved};

/// A code host the core reaches through its plugin, as the plugin's manifest
/// describes it.
#[derive(Clone, Copy)]
pub(crate) struct Host {
    /// Its plugin, such as `sikemux.github`.
    plugin: &'static str,
    spec: &'static CodeHostSpec,
}

impl Host {
    /// What the board and clients know it by, such as `github`.
    fn id(&self) -> &'static str {
        self.plugin.strip_prefix("sikemux.").unwrap_or(self.plugin)
    }

    fn plugin(&self) -> &'static str {
        self.plugin
    }

    /// Whether `server` is a name the host's website goes by for everyone.
    fn public(&self, server: &str) -> bool {
        self.spec
            .web_hosts
            .iter()
            .any(|known| match known.strip_suffix('*') {
                Some(prefix) => server.starts_with(prefix),
                None => server == known,
            })
    }

    /// The ways to merge, tried in order; none is the repository's own default.
    fn merges(&self) -> Vec<Option<&'static str>> {
        if self.spec.merges.is_empty() {
            return vec![None];
        }
        self.spec
            .merges
            .iter()
            .map(|method| Some(method.as_str()))
            .collect()
    }
}

/// Every code host a plugin speaks for.
fn hosts(core: &Core) -> impl Iterator<Item = Host> {
    plugins::manifests(core).into_iter().filter_map(|manifest| {
        Some(Host {
            plugin: &manifest.id,
            spec: manifest.code_host.as_ref()?,
        })
    })
}

/// The plugin that speaks for the code host `id`.
fn plugin_of(id: &str) -> String {
    format!("sikemux.{id}")
}

fn host(core: &Core, id: &str) -> Result<Host, String> {
    hosts(core)
        .find(|host| host.id() == id)
        .ok_or_else(|| format!("Sikemux does not know the code host {id}"))
}

/// A repository on a code host, and the account that reaches it.
#[derive(Clone)]
pub(crate) struct Repo {
    plugins: Option<Provided>,
    host: Host,
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
    fn repo(&self, core: &Core) -> Result<Repo, String> {
        Ok(Repo {
            plugins: core.plugins,
            host: host(core, &self.host)?,
            owner: self.owner.clone(),
            name: self.name.clone(),
            account: self.account.clone(),
        })
    }
}

impl Repo {
    /// Calls `method` of the host's plugin about this repository, with `more`.
    pub(crate) async fn call(&self, method: &str, more: Value) -> Result<Value, Failure> {
        plugins::call_on(self.plugins, self.host.plugin(), method, self.params(more)).await
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
    core: &Core,
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
) -> Option<(Host, String, String)> {
    for host in hosts(core) {
        let Ok(found) =
            plugins::call(core, host.plugin(), "resolveRemote", json!({ "url": url })).await
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
            .get(host.plugin())
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
    core: &Core,
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
) -> Result<Repo, String> {
    let Some((host, owner, name)) = locate(core, url, picks).await else {
        let labels: Vec<&str> = hosts(core).map(|host| host.spec.label.as_str()).collect();
        return Err(format!(
            "this project's origin is on none of {}",
            labels.join(", ")
        ));
    };
    let pick = picks.get(host.plugin()).cloned().unwrap_or_default();
    let account = match pick.account {
        Some(account) => Some(account),
        None => plugins::call(
            core,
            host.plugin(),
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
            host.spec.label
        ));
    }
    Ok(Repo {
        plugins: core.plugins,
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
            Ok((crate::git::origin(&folder), picks(&core, &folder)))
        })
        .await
    };
    let unsupported = |message: &str| Failure::new("codehost", Category::Unsupported, message);
    let (Some(origin), picks) = found.map_err(|error| unsupported(&error.to_string()))? else {
        return Err(unsupported("this project has no origin remote"));
    };
    let Some((host, _, _)) = locate(core, &origin, &picks).await else {
        return Err(unsupported(
            "this project's origin is on no code host Sikemux knows",
        ));
    };
    find(core, &origin, &picks)
        .await
        .map_err(|message| Failure::new(host.plugin(), Category::Unconfigured, message))
}

impl Repo {
    /// The repository `owner/name` on the code host whose plugin is `plugin`,
    /// reached as `account`.
    pub(crate) fn named(
        core: &Core,
        plugin: &str,
        owner: &str,
        name: &str,
        account: Option<String>,
    ) -> Result<Self, Failure> {
        let id = plugin.strip_prefix("sikemux.").unwrap_or(plugin);
        let host = host(core, id)
            .map_err(|message| Failure::new(plugin, Category::Unsupported, message))?;
        Ok(Self {
            plugins: core.plugins,
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
            .spec
            .pull_heads
            .as_ref()
            .map(|heads| format!("{heads}/{number}/head"))
    }

    /// What the host is called, as people know it.
    pub(crate) fn label(&self) -> &'static str {
        &self.host.spec.label
    }

    pub(crate) fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub(crate) fn id(&self) -> RepoId {
        RepoId {
            provider: self.host.plugin().to_owned(),
            owner: self.owner.clone(),
            name: self.name.clone(),
        }
    }
}

/// A repository on a code host, by the plugin that speaks for the host, such as
/// `sikemux.github`, and its `owner/name` there.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct RepoId {
    pub provider: String,
    pub owner: String,
    pub name: String,
}

impl std::fmt::Display for RepoId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}/{}", self.provider, self.owner, self.name)
    }
}

impl Pull {
    /// The repository the pull request is on.
    pub(crate) fn repo_id(&self) -> RepoId {
        RepoId {
            provider: plugin_of(&self.host),
            owner: self.owner.clone(),
            name: self.name.clone(),
        }
    }
}

/// What clients show of the code host the remote `url` is on.
pub(crate) async fn shown(
    core: &Core,
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
) -> Option<ProjectCodeHost> {
    let (host, owner, name) = locate(core, url, picks).await?;
    Some(ProjectCodeHost {
        host: host.id().to_owned(),
        plugin: host.plugin().to_owned(),
        label: host.spec.label.clone(),
        repo: format!("{owner}/{name}"),
        ci_name: host.spec.ci_name.clone(),
        sections: ["pulls", "actions", "issues", "releases"]
            .map(str::to_owned)
            .to_vec(),
        pulls: host.spec.pulls.clone(),
        ci: host.spec.ci.clone(),
    })
}

/// Where a new pull request from `branch` opens on the website of the code
/// host the remote `url` is on, signed in to or not.
pub(crate) async fn compare_url(core: &Core, url: &str, branch: &str) -> Result<String, String> {
    for host in hosts(core) {
        let Ok(found) =
            plugins::call(core, host.plugin(), "resolveRemote", json!({ "url": url })).await
        else {
            continue;
        };
        let Ok(found) = serde_json::from_value::<Resolved>(found) else {
            continue;
        };
        let Some(repo) = found
            .repo
            .filter(|repo| found.same_host || host.public(&repo.host))
        else {
            continue;
        };
        return compare_address(host.spec, &repo, branch);
    }
    Err(format!("unsupported remote: {url}"))
}

fn compare_address(spec: &CodeHostSpec, repo: &RemoteRepo, branch: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(&format!("https://{}/", repo.host)).map_err(|error| error.to_string())?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| "cannot build pull request url".to_owned())?;
        segments
            .clear()
            .extend(repo.owner.split('/'))
            .push(&repo.name);
        for part in spec.compare.path.split('/') {
            if part == "{branch}" {
                segments.extend(branch.split('/'));
            } else {
                segments.push(part);
            }
        }
    }
    for (key, value) in &spec.compare.query {
        url.query_pairs_mut()
            .append_pair(key, &value.replace("{branch}", branch));
    }
    Ok(url.into())
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
        Some(sha) => match checks_on(repo, branch, &sha).await? {
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
    pub(crate) fn watched(&self) -> super::ci::Shown {
        super::ci::Shown {
            id: self.id(),
            account: self.account.clone(),
            host: None,
        }
    }
}

/// The pull requests the commit `sha` came in through, the merged one first,
/// when the code host the remote `url` is on can tell.
pub(crate) async fn pulls_of(
    core: &Core,
    url: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
    sha: &str,
) -> Result<Vec<Value>, String> {
    let repo = find(core, url, picks).await?;
    let found = repo.call("commitPulls", json!({ "sha": sha })).await?;
    Ok(found.as_array().cloned().unwrap_or_default())
}

fn pull_of(repo: &Repo, found: &Value) -> Result<Pull, String> {
    let number = found["number"]
        .as_u64()
        .ok_or("the code host answered without the pull request's number")?;
    Ok(Pull {
        host: repo.host.id().to_owned(),
        owner: repo.owner.clone(),
        name: repo.name.clone(),
        number,
        url: text(found, "url").unwrap_or_default(),
        account: repo.account.clone(),
    })
}

/// Opens a pull request for `head` onto `base`, or finds the one already open.
pub(crate) async fn open(
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

pub(crate) enum Checks {
    /// Nothing has run on the commit yet.
    None,
    Running,
    Passed,
    /// What failed, a line each.
    Failed(String),
}

/// How the CI runs on `sha` went.
pub(crate) async fn checks(
    core: &Core,
    pull: &Pull,
    branch: &str,
    sha: &str,
) -> Result<Checks, String> {
    checks_on(&pull.repo(core)?, branch, sha).await
}

async fn checks_on(repo: &Repo, branch: &str, sha: &str) -> Result<Checks, String> {
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
pub(crate) fn run_in(core: &Core, address: &str) -> Option<Value> {
    hosts(core)
        .find_map(|host| address.split_once(host.spec.run_path.as_str()))
        .and_then(|(_, rest)| digits(rest))
}

/// The job an address on a run points at, such as GitHub's `/job/<id>`.
pub(crate) fn job_in(core: &Core, address: &str) -> Option<Value> {
    hosts(core)
        .filter_map(|host| host.spec.job_path.as_deref())
        .find_map(|path| address.split_once(path))
        .and_then(|(_, rest)| digits(rest))
}

fn digits(rest: &str) -> Option<Value> {
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| digits.into())
}

/// Where a pull request stands.
pub(crate) struct Looked {
    /// `open`, `merged` or `closed`.
    pub state: String,
    /// Moves whenever anything on the pull request does, such as a comment.
    pub mark: String,
}

pub(crate) async fn look(core: &Core, pull: &Pull) -> Result<Looked, String> {
    let repo = pull.repo(core)?;
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
pub(crate) async fn feedback(
    core: &Core,
    pull: &Pull,
    since: Option<&str>,
) -> Result<Vec<Feedback>, String> {
    let repo = pull.repo(core)?;
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
pub(crate) async fn answer(
    core: &Core,
    pull: &Pull,
    replies: &[(String, String, String)],
    reviewers: &[String],
) -> Result<(), String> {
    let repo = pull.repo(core)?;
    let replies: Vec<Value> = replies
        .iter()
        .map(|(to, body, quote)| json!({ "to": to, "body": body, "quote": quote }))
        .collect();
    let answered = json!({ "number": pull.number, "replies": replies, "reviewers": reviewers });
    Ok(repo.call("answerReview", answered).await.map(drop)?)
}

/// Closes the pull request without merging it.
pub(crate) async fn close(core: &Core, pull: &Pull) -> Result<(), String> {
    let repo = pull.repo(core)?;
    let closed = json!({ "number": pull.number, "state": "closed" });
    Ok(repo.call("setPullState", closed).await.map(drop)?)
}

/// Merges the pull request at `sha`, the commit whose checks passed, the
/// first way the repository allows.
pub(crate) async fn merge(core: &Core, pull: &Pull, sha: &str) -> Result<(), String> {
    let repo = pull.repo(core)?;
    let mut refused = None;
    for method in repo.host.merges() {
        let asked = json!({ "number": pull.number, "method": method, "sha": sha });
        match repo.call("mergePull", asked).await {
            Ok(_) => return Ok(()),
            Err(failure) if failure.category == Category::MergeMethod => refused = Some(failure),
            Err(failure) => return Err(failure.into()),
        }
    }
    Err(refused.map_or_else(|| "the code host merges no way".to_owned(), String::from))
}

#[cfg(test)]
impl Repo {
    /// `nodelike/sikemux` on a host that keeps pull requests' heads under
    /// `pull_heads`, as GitHub does and Bitbucket does not.
    pub(crate) fn on_test_host(pull_heads: Option<&str>) -> Self {
        let spec = Box::leak(Box::new(CodeHostSpec {
            pull_heads: pull_heads.map(str::to_owned),
            ..CodeHostSpec::default()
        }));
        Self {
            plugins: None,
            host: Host {
                plugin: "sikemux.test",
                spec,
            },
            owner: "nodelike".to_owned(),
            name: "sikemux".to_owned(),
            account: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(branches: &[&str]) -> Vec<String> {
        branches.iter().map(|branch| (*branch).to_owned()).collect()
    }

    fn host(manifest: &str) -> Host {
        let manifest = sikemux_plugin_api::Manifest::from_json(manifest).expect("a manifest");
        let spec = manifest.code_host.expect("a code host");
        Host {
            plugin: "sikemux.test",
            spec: Box::leak(Box::new(spec)),
        }
    }

    fn compare(host: &Host, server: &str, slug: &str, branch: &str) -> Result<String, String> {
        let (owner, name) = slug.rsplit_once('/').expect("owner/name");
        let repo = RemoteRepo {
            host: server.to_owned(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        compare_address(host.spec, &repo, branch)
    }

    #[test]
    fn pull_request_urls_only_point_at_known_hosts() {
        let github = host(include_str!("../../../../../plugins/github/manifest.json"));
        let bitbucket = host(include_str!(
            "../../../../../plugins/bitbucket/manifest.json"
        ));
        let gitlab = host(include_str!("../../../../../plugins/gitlab/manifest.json"));
        assert_eq!(
            compare(&github, "github.com", "nodelike/sikemux", "feat/x").as_deref(),
            Ok("https://github.com/nodelike/sikemux/compare/feat/x?expand=1")
        );
        assert_eq!(
            compare(&bitbucket, "bitbucket.org", "team/app", "feat/x").as_deref(),
            Ok("https://bitbucket.org/team/app/pull-requests/new?source=feat%2Fx")
        );
        assert_eq!(
            compare(&gitlab, "gitlab.com", "team/app", "feat/x").as_deref(),
            Ok("https://gitlab.com/team/app/-/merge_requests/new?merge_request%5Bsource_branch%5D=feat%2Fx")
        );
        assert_eq!(
            compare(&gitlab, "gitlab.acme.dev", "platform/payments/billing-api", "fix").as_deref(),
            Ok("https://gitlab.acme.dev/platform/payments/billing-api/-/merge_requests/new?merge_request%5Bsource_branch%5D=fix")
        );
        assert_eq!(
            compare(&github, "github.com", "o/r", "a#b?c d").as_deref(),
            Ok("https://github.com/o/r/compare/a%23b%3Fc%20d?expand=1")
        );
        assert!(github.public("github.com") && bitbucket.public("bitbucket.org"));
        assert!(gitlab.public("gitlab.com") && gitlab.public("gitlab.acme.dev"));
        for server in ["github.com.evil.test", "evil.test", "evilgitlab.com"] {
            for host in [&github, &bitbucket, &gitlab] {
                assert!(!host.public(server), "{server}");
            }
        }
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
