//! A CI run as what `who_wrote` is asked about: the run that failed, the last
//! run of its workflow on its branch that passed, and the commits between
//! them, those whose changes the failing job's error points at first.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use super::{git, remember, Found};
use crate::protocol::CodeHostChoice;
use crate::server::board::codehost::{self, Conclusion, Repo};

const ASK_BUDGET: Duration = Duration::from_secs(10);
const LOG_BUDGET: Duration = Duration::from_secs(20);
const LOG_LINES: usize = 800;
const MAX_LINE_CHARS: usize = 240;
const EXCERPT_BYTES: usize = 1500;
const MAX_NAMED: usize = 24;
/// Commits between the passing run and the failing one that are ranked.
const MAX_RANGE: usize = 200;
const MAX_ALSO_FAILING: usize = 4;
/// What GitHub writes when a step fails; what follows is the job cleaning up.
const STEP_FAILED: &str = "##[error]Process completed with exit code";
/// Folder names too common to say where an error is.
const COMMON_FOLDERS: [&str; 24] = [
    "src",
    "lib",
    "app",
    "apps",
    "test",
    "tests",
    "spec",
    "dist",
    "build",
    "components",
    "crates",
    "packages",
    "internal",
    "pkg",
    "cmd",
    "utils",
    "common",
    "shared",
    "server",
    "client",
    "core",
    "main",
    "scripts",
    "bin",
];
/// Paths in a log that point into installed dependencies, not the project.
const DEPENDENCIES: [&str; 6] = [
    "node_modules/",
    ".pnpm/",
    ".cargo/",
    "/rustc/",
    "site-packages/",
    "/usr/",
];

/// The CI run a target names.
pub(super) enum Asked {
    /// A run by its address on the code host, or its id.
    Run(String),
    /// The latest failing run on `branch`, of `workflow` when one is named.
    Latest {
        branch: String,
        workflow: Option<String>,
    },
}

/// What is read of a finished failing run, kept by its id since it no
/// longer changes.
pub(super) struct Failing {
    run: Value,
    sha: String,
    /// The latest earlier run of its workflow on its branch that passed: its
    /// commit, and its address.
    passed: Option<(String, String)>,
    /// The earlier run since then where the workflow first failed, when the
    /// one asked about is not it: its commit, and its address.
    began: Option<(String, String)>,
    error: Option<String>,
    named: Vec<String>,
}

/// A failing run, with what was found about the branch's other runs.
pub(super) struct Ci {
    failing: Arc<Failing>,
    notes: Vec<String>,
    also_failing: Vec<Value>,
}

/// The CI run `target` names, if it names one: a run's address or id, a
/// workflow's name, or `ci` and `main` for the latest failing run on the
/// current branch and on main.
pub(super) fn asked(top: &Path, target: &str) -> Result<Option<Asked>, String> {
    if codehost::run_in(target).is_some() {
        return Ok(Some(Asked::Run(target.to_owned())));
    }
    let latest =
        |branch: String, workflow: Option<String>| Ok(Some(Asked::Latest { branch, workflow }));
    match target.to_ascii_lowercase().as_str() {
        "ci" => return latest(current_branch(top)?, None),
        "main" => return latest(main_branch(top), None),
        _ => {}
    }
    if target.chars().all(|c| c.is_ascii_digit()) && !is_commit(top, target) {
        return Ok(Some(Asked::Run(target.to_owned())));
    }
    if is_workflow(top, target) {
        return latest(current_branch(top)?, Some(target.to_owned()));
    }
    Ok(None)
}

fn current_branch(top: &Path) -> Result<String, String> {
    let branch = git(top, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_owned();
    if branch == "HEAD" {
        return Err("this checkout is on no branch; name the CI run or its address".into());
    }
    Ok(branch)
}

/// The branch `origin` names as its main one, else `main`.
fn main_branch(top: &Path) -> String {
    git(
        top,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .and_then(|head| head.trim().strip_prefix("origin/").map(str::to_owned))
    .unwrap_or_else(|| "main".to_owned())
}

fn is_commit(top: &Path, target: &str) -> bool {
    let spec = format!("{target}^{{commit}}");
    git(
        top,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )
    .is_ok()
}

/// Whether one of the checkout's GitHub workflows or Bitbucket pipelines is
/// called `name`.
fn is_workflow(top: &Path, name: &str) -> bool {
    let same = |text: &str| {
        text.trim()
            .trim_matches(['"', '\''])
            .eq_ignore_ascii_case(name)
    };
    let workflows = std::fs::read_dir(top.join(".github/workflows"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yml" || extension == "yaml")
        });
    for path in workflows {
        if path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(same)
        {
            return true;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text
            .lines()
            .filter_map(|line| line.strip_prefix("name:"))
            .any(same)
        {
            return true;
        }
    }
    let Ok(pipelines) = std::fs::read_to_string(top.join("bitbucket-pipelines.yml")) else {
        return false;
    };
    pipelines.lines().any(|line| {
        let indent = line.len() - line.trim_start().len();
        let key = line.trim().strip_suffix(':');
        match indent {
            2 => key == Some("default") && same("default"),
            4 => key.is_some_and(same),
            _ => false,
        }
    })
}

async fn timed<T>(
    budget: Duration,
    asked: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(budget, asked)
        .await
        .map_err(|_| "the code host did not answer in time; try again".to_owned())?
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn conclusion(run: &Value) -> Option<&str> {
    (run["status"] == "completed")
        .then(|| run["conclusion"].as_str())
        .flatten()
}

fn ended(run: &Value) -> Option<Conclusion> {
    conclusion(run).map(Conclusion::of)
}

fn summary(run: &Value) -> Value {
    json!({ "workflow": run["name"], "url": run["url"], "commit": run["shortSha"] })
}

/// Finds the failing run `asked` names on the code host `origin` is on.
pub(super) async fn find(
    origin: &str,
    picks: &BTreeMap<String, CodeHostChoice>,
    asked: Asked,
) -> Result<Ci, String> {
    let repo = timed(ASK_BUDGET, codehost::find(origin, picks)).await?;
    let mut notes = Vec::new();
    let mut also_failing = Vec::new();
    let (id, job) = match asked {
        Asked::Run(address) => {
            if address.contains('/')
                && !address
                    .to_ascii_lowercase()
                    .contains(&format!("/{}/", repo.slug().to_ascii_lowercase()))
            {
                return Err(format!(
                    "that run is not in {}, this project's repository",
                    repo.slug()
                ));
            }
            let id = codehost::run_in(&address).unwrap_or_else(|| address.as_str().into());
            (id, codehost::job_in(&address))
        }
        Asked::Latest { branch, workflow } => {
            let id = match &workflow {
                Some(name) => Some(
                    timed(ASK_BUDGET, codehost::workflows(&repo))
                        .await?
                        .into_iter()
                        .find(|found| is_named(found, name))
                        .map(|found| found["id"].clone())
                        .ok_or_else(|| format!("{} has no workflow called {name}", repo.slug()))?,
                ),
                None => None,
            };
            let runs = timed(
                ASK_BUDGET,
                codehost::runs(&repo, &branch, id.as_ref(), false),
            )
            .await?;
            let picked = latest_failing(&runs, &branch, workflow.as_deref(), &mut notes)?;
            also_failing = red(&runs)
                .into_iter()
                .filter(|run| run["id"] != picked["id"])
                .take(MAX_ALSO_FAILING)
                .map(summary)
                .collect();
            (picked["id"].clone(), None)
        }
    };
    let key = format!("{}\0{}\0{id}", repo.slug(), job.clone().unwrap_or_default());
    if let Some(failing) = cached(&key) {
        return Ok(Ci {
            failing,
            notes,
            also_failing,
        });
    }
    let (failing, whole) = read_run(&repo, &id, job).await?;
    let failing = Arc::new(failing);
    if whole {
        remember_run(&key, &failing);
    }
    Ok(Ci {
        failing,
        notes,
        also_failing,
    })
}

fn is_named(run: &Value, name: &str) -> bool {
    let stem = text(run, "path")
        .rsplit('/')
        .next()
        .and_then(|file| file.split('.').next())
        .unwrap_or_default();
    text(run, "name").eq_ignore_ascii_case(name) || stem.eq_ignore_ascii_case(name)
}

/// Each workflow's latest finished run, when it failed, newest first.
fn red(runs: &[Value]) -> Vec<&Value> {
    let mut seen = HashSet::new();
    runs.iter()
        .filter(|run| conclusion(run).is_some())
        .filter(|run| seen.insert(run["workflowId"].to_string()))
        .filter(|run| ended(run) == Some(Conclusion::Failed))
        .collect()
}

/// The workflow failing now that failed last, else the latest failure that
/// has since been fixed, saying so in `notes`.
fn latest_failing<'a>(
    runs: &'a [Value],
    branch: &str,
    workflow: Option<&str>,
    notes: &mut Vec<String>,
) -> Result<&'a Value, String> {
    if let Some(failing) = red(runs).first() {
        return Ok(failing);
    }
    let which = workflow.map_or_else(|| "run".to_owned(), |name| format!("{name} run"));
    let Some(failed) = runs
        .iter()
        .find(|run| ended(run) == Some(Conclusion::Failed))
    else {
        return Err(format!(
            "no {which} on {branch} failed among the latest {}; CI there is passing",
            runs.len()
        ));
    };
    if let Some(fixed) = runs.iter().find(|run| {
        run["workflowId"] == failed["workflowId"] && ended(run) == Some(Conclusion::Passed)
    }) {
        notes.push(format!(
            "No workflow on {branch} is failing now. This is its latest failure; {} passed since, at {}.",
            text(fixed, "name"),
            text(fixed, "shortSha"),
        ));
    }
    Ok(failed)
}

/// The run of `history`, newest first, that last passed before `run`, and
/// the first of the runs that failed since, where the failure began.
fn streak<'a>(history: &'a [Value], run: &Value) -> (Option<&'a Value>, Option<&'a Value>) {
    let mut began = None;
    for earlier in history
        .iter()
        .filter(|earlier| text(earlier, "createdAt") < text(run, "createdAt"))
    {
        match ended(earlier) {
            Some(Conclusion::Passed) => return (Some(earlier), began),
            Some(Conclusion::Failed) => began = Some(earlier),
            _ => {}
        }
    }
    (None, began)
}

fn sha_and_url(run: &Value) -> (String, String) {
    (text(run, "sha").to_owned(), text(run, "url").to_owned())
}

/// Reads the run `id`: its failing jobs, the end of the first one's log, the
/// latest earlier run of its workflow on its branch that passed, and the run
/// since then where it first failed.
/// Whether every part was read, so the answer can be kept.
async fn read_run(repo: &Repo, id: &Value, job: Option<Value>) -> Result<(Failing, bool), String> {
    let detail = timed(ASK_BUDGET, codehost::run(repo, id)).await?;
    let run = &detail["run"];
    match conclusion(run) {
        None => return Err("that run has not finished; ask again once it has".into()),
        Some(found) if Conclusion::of(found) != Conclusion::Failed => {
            return Err(format!(
                "that run ended {found}, not failed; name a failing run, or ci for the latest one"
            ))
        }
        Some(_) => {}
    }
    let sha = text(run, "sha").to_owned();
    if sha.is_empty() {
        return Err("the code host did not say which commit that run built".into());
    }
    let failed_jobs: Vec<&Value> = detail["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|found| ended(found) == Some(Conclusion::Failed))
        .collect();
    let read = failed_jobs
        .iter()
        .find(|found| job.as_ref().is_some_and(|job| found["id"] == *job))
        .or(failed_jobs.first())
        .map(|found| found["id"].clone());
    let reads_log = read.is_some();
    let history = async {
        let branch = run["branch"].as_str()?;
        let history = timed(
            ASK_BUDGET,
            codehost::runs(repo, branch, Some(&run["workflowId"]), false),
        )
        .await
        .ok()?;
        let (passed, began) = streak(&history, run);
        let floor = text(began.unwrap_or(run), "createdAt").to_owned();
        let began = began.map(sha_and_url);
        if let Some(passed) = passed {
            return Some((Some(sha_and_url(passed)), began));
        }
        let passed = timed(
            ASK_BUDGET,
            codehost::runs(repo, branch, Some(&run["workflowId"]), true),
        )
        .await
        .ok()?
        .iter()
        .find(|earlier| text(earlier, "createdAt") < floor.as_str())
        .map(sha_and_url);
        Some((passed, began))
    };
    let log = async {
        let job = read?;
        timed(LOG_BUDGET, codehost::log_tail(repo, id, &job, LOG_LINES))
            .await
            .ok()
    };
    let (history, log) = tokio::join!(history, log);
    let whole = history.is_some() && (log.is_some() || !reads_log);
    let (passed, began) = history.unwrap_or_default();
    let (error, named) = log.as_deref().map(read_error).unwrap_or_default();
    let failed_jobs: Vec<&str> = failed_jobs
        .iter()
        .map(|found| text(found, "name"))
        .collect();
    let failing = Failing {
        run: json!({
            "workflow": run["name"],
            "url": run["url"],
            "commit": run["shortSha"],
            "branch": run["branch"],
            "conclusion": run["conclusion"],
            "at": run["createdAt"],
            "failedJobs": failed_jobs,
        }),
        sha,
        passed,
        began,
        error,
        named,
    };
    Ok((failing, whole))
}

fn cached(key: &str) -> Option<Arc<Failing>> {
    runs().locked().get(key).cloned()
}

fn remember_run(key: &str, failing: &Arc<Failing>) {
    remember(runs(), key, failing);
}

fn runs() -> &'static Mutex<HashMap<String, Arc<Failing>>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Arc<Failing>>>> = OnceLock::new();
    RUNS.get_or_init(Default::default)
}

/// `line` without terminal colours, cut to a readable length.
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    if out.chars().count() > MAX_LINE_CHARS {
        out = out.chars().take(MAX_LINE_CHARS).collect::<String>() + "…";
    }
    out
}

fn says_failed(line: &str) -> bool {
    let lower = line.to_lowercase();
    let failed = ["error", "failed", "panicked", "fail ", "✗", "assertion"]
        .iter()
        .any(|word| lower.contains(word))
        || lower.trim_start().starts_with("fail");
    failed && (!lower.contains("warning") || lower.contains("error:"))
}

/// Lines that only repeat that something failed, or where: stack frames and
/// the exit status, also behind a tool's `name: ` prefix.
fn is_noise(line: &str) -> bool {
    let trimmed = line.trim();
    let said = trimmed
        .split_once(": ")
        .filter(|(prefix, _)| prefix.len() <= 40 && !prefix.contains(['/', '.']))
        .map_or(trimmed, |(_, said)| said.trim());
    said.is_empty() || said.starts_with("at ") || trimmed.starts_with(STEP_FAILED)
}

/// A short excerpt of the error in a failing job's log `lines`, and the
/// files it names.
fn read_error(lines: &[String]) -> (Option<String>, Vec<String>) {
    let ended = lines
        .iter()
        .position(|line| line.starts_with(STEP_FAILED))
        .map_or(lines.len(), |at| at + 1);
    let lines: Vec<String> = lines[..ended].iter().map(|line| plain(line)).collect();
    let mut around = BTreeSet::new();
    for (at, line) in lines.iter().enumerate() {
        if says_failed(line) && !is_noise(line) {
            around.extend(at.saturating_sub(1)..(at + 4).min(lines.len()));
        }
    }
    let mut excerpt = String::new();
    let mut last = None;
    let mut named = Vec::new();
    for at in around {
        let line = &lines[at];
        for path in paths_in(line) {
            if named.len() < MAX_NAMED && !named.contains(&path) {
                named.push(path);
            }
        }
        if is_noise(line) || excerpt.len() + line.len() > EXCERPT_BYTES {
            continue;
        }
        if last.is_some_and(|last| last + 1 != at) {
            excerpt.push_str("…\n");
        }
        excerpt.push_str(line.trim_end());
        excerpt.push('\n');
        last = Some(at);
    }
    let excerpt = excerpt.trim_end();
    ((!excerpt.is_empty()).then(|| excerpt.to_owned()), named)
}

/// The file paths `line` names, without their line and column, leading `./`
/// and `../`, and paths into installed dependencies.
fn paths_in(line: &str) -> Vec<String> {
    line.split(|c: char| c.is_whitespace() || "\"'`()[]{}<>,;|│╭╰─".contains(c))
        .filter_map(|token| {
            let token = token.strip_prefix("file://").unwrap_or(token);
            if token.contains("://") || DEPENDENCIES.iter().any(|at| token.contains(at)) {
                return None;
            }
            let mut path = token.trim_end_matches(['.', ':']);
            while let Some((rest, tail)) = path.rsplit_once(':') {
                if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
                    break;
                }
                path = rest;
            }
            while let Some(rest) = path.strip_prefix("./").or_else(|| path.strip_prefix("../")) {
                path = rest;
            }
            let (stem, extension) = path.rsplit_once('.')?;
            let named = !stem.is_empty()
                && !stem.ends_with('/')
                && (1..=6).contains(&extension.len())
                && extension.starts_with(|c: char| c.is_ascii_alphabetic())
                && extension.chars().all(|c| c.is_ascii_alphanumeric());
            named.then(|| path.to_owned())
        })
        .collect()
}

/// Whether the error's `named` path is the repository's file `file`.
fn names(named: &str, file: &str) -> bool {
    match named.contains('/') {
        true => {
            named == file
                || file.ends_with(&format!("/{named}"))
                || (file.contains('/') || named.starts_with('/'))
                    && named.ends_with(&format!("/{file}"))
        }
        false => file.rsplit('/').next() == Some(named),
    }
}

/// The folders the error's paths are in, by their last name, when that name
/// says something.
fn folders(named: &[String]) -> Vec<String> {
    let mut folders: Vec<String> = named
        .iter()
        .filter_map(|path| path.rsplit_once('/')?.0.rsplit('/').next())
        .filter(|folder| {
            !folder.is_empty()
                && !COMMON_FOLDERS.contains(folder)
                && folder
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .map(str::to_owned)
        .collect();
    folders.sort_unstable();
    folders.dedup();
    folders
}

/// The commits from the run that passed to the one that failed, those whose
/// changes the error points at first, each with why.
pub(super) fn commits(top: &Path, ci: &Ci) -> Result<Found, String> {
    let failing = &ci.failing;
    let mut found = Found::new("ci");
    if !is_commit(top, &failing.sha) {
        return Err(format!(
            "the failing commit {} is not in this checkout; fetch it first",
            &failing.sha[..failing.sha.len().min(12)]
        ));
    }
    let passed = failing
        .passed
        .as_ref()
        .filter(|(sha, _)| is_commit(top, sha))
        .map(|(sha, _)| sha.as_str());
    let Some(passed) = passed.filter(|passed| *passed != failing.sha) else {
        found.notes.push(match passed {
            Some(_) => "The same commit passed in an earlier run, so the failure may be flaky; this is the failing commit.".to_owned(),
            None => "No earlier run of this workflow on its branch passed that this checkout has; this is the failing commit.".to_owned(),
        });
        found.commits.push((failing.sha.clone(), Vec::new()));
        return Ok(found);
    };
    let end = match failing
        .began
        .as_ref()
        .filter(|(sha, _)| is_commit(top, sha) && sha != passed)
    {
        Some((sha, url)) => {
            found.notes.push(format!(
                "It first failed at {} ({url}), so only the commits up to there can have broken it.",
                &sha[..sha.len().min(7)]
            ));
            sha.as_str()
        }
        None => failing.sha.as_str(),
    };
    let range = format!("{passed}..{end}");
    let total: usize = git(top, &["rev-list", "--count", &range])?
        .trim()
        .parse()
        .unwrap_or_default();
    let most = format!("--max-count={MAX_RANGE}");
    let listed = git(
        top,
        &["log", &most, "--format=%x01%H", "--name-only", &range, "--"],
    )?;
    let mut touched: Vec<(String, Vec<&str>)> = Vec::new();
    for record in listed
        .split('\x01')
        .filter(|record| !record.trim().is_empty())
    {
        let mut lines = record.lines();
        let Some(hash) = lines.next() else { continue };
        touched.push((
            hash.to_owned(),
            lines.filter(|line| !line.is_empty()).collect(),
        ));
    }
    let folders = folders(&failing.named);
    let naming: HashSet<String> = match folders.is_empty() {
        true => HashSet::new(),
        false => {
            let pattern = format!("(^|[^A-Za-z0-9_.-])({})/", folders.join("|"));
            git(
                top,
                &["log", &most, "--format=%H", "-G", &pattern, &range, "--"],
            )
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
        }
    };
    let mut ranked: Vec<(u8, String, Option<String>)> = touched
        .into_iter()
        .map(|(hash, files)| {
            let named: Vec<&str> = files
                .iter()
                .copied()
                .filter(|file| failing.named.iter().any(|named| names(named, file)))
                .collect();
            let in_folder = files.iter().find(|file| {
                file.rsplit_once('/')
                    .and_then(|(folder, _)| folder.rsplit('/').next())
                    .is_some_and(|folder| folders.iter().any(|named| named == folder))
            });
            let (rank, why) = if !named.is_empty() {
                (
                    3,
                    Some(format!(
                        "changes {}, which the error names",
                        named.join(", ")
                    )),
                )
            } else if naming.contains(&hash) {
                (
                    2,
                    Some(format!(
                        "changes a line naming {}/, where the error is",
                        folders.join("/, ")
                    )),
                )
            } else if let Some(file) = in_folder {
                (
                    1,
                    Some(format!("changes {file}, beside a file the error names")),
                )
            } else {
                (0, None)
            };
            (rank, hash, why)
        })
        .collect();
    ranked.sort_by_key(|(rank, ..)| std::cmp::Reverse(*rank));
    found.total = Some(total);
    if total > MAX_RANGE {
        found.notes.push(format!(
            "Only the latest {MAX_RANGE} of them were weighed against the error."
        ));
    }
    for (_, hash, why) in ranked {
        if let Some(why) = why {
            found.why.insert(hash.clone(), why);
        }
        found.commits.push((hash, Vec::new()));
    }
    Ok(found)
}

impl Ci {
    /// What the answer says of the run, beside its commits.
    pub(super) fn describe(&self, answer: &mut Value) {
        let failing = &self.failing;
        answer["run"] = failing.run.clone();
        answer["lastPassed"] = match &failing.passed {
            Some((sha, url)) => json!({ "commit": &sha[..sha.len().min(7)], "url": url }),
            None => Value::Null,
        };
        answer["error"] = failing.error.clone().into();
        if !self.also_failing.is_empty() {
            answer["alsoFailing"] = self.also_failing.clone().into();
        }
    }

    pub(super) fn notes(&self) -> &[String] {
        &self.notes
    }
}
