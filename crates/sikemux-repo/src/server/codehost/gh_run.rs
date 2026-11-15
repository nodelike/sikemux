//! `gh run list`, `gh run view` and `gh run watch` as an agent runs them, answered from the CI the
//! core already holds for everyone rather than by each agent asking GitHub. Anything it does
//! not understand is handed back for the real `gh` to run.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use super::repo::{self, of_project, Repo};
use crate::server::Core;

const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 100;
const FAILED_LOG_LINES: usize = 300;

/// What the shim does with the answer: print `out` and exit with `code`, or run the real `gh`.
pub async fn call(core: &Arc<Core>, project: &str, args: &[String]) -> Result<Value, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some(ask) = Ask::of(&words) else {
        return Ok(pass());
    };
    if let Ask::Watch { run } = &ask {
        return Ok(json!({
            "err": format!(
                "gh run watch asks GitHub every few seconds. Sikemux tells you when the run ends: call trigger_add with ci set to {} and end your turn.",
                run.as_deref().unwrap_or("the run's id or address")
            ),
            "code": 1,
        }));
    }
    let repo = of_project(core, project)
        .await
        .map_err(|failure| failure.message)?;
    if ask
        .repo()
        .is_some_and(|named| !named.eq_ignore_ascii_case(&repo.slug()))
    {
        return Ok(pass());
    }
    let answered = match ask {
        Ask::List(list) => list.answer(&repo).await?,
        Ask::View(view) => view.answer(core, &repo).await?,
        Ask::Watch { .. } => None,
    };
    Ok(answered.unwrap_or_else(pass))
}

fn pass() -> Value {
    json!({ "pass": true })
}

fn printed(out: String, code: i32) -> Option<Value> {
    Some(json!({ "out": out, "code": code }))
}

enum Ask {
    List(List),
    View(View),
    Watch { run: Option<String> },
}

#[derive(Default)]
struct List {
    branch: Option<String>,
    commit: Option<String>,
    workflow: Option<String>,
    status: Option<String>,
    event: Option<String>,
    user: Option<String>,
    limit: Option<u64>,
    json: Option<Vec<String>>,
    repo: Option<String>,
}

#[derive(Default)]
struct View {
    run: Option<String>,
    job: Option<String>,
    log: bool,
    log_failed: bool,
    exit_status: bool,
    json: Option<Vec<String>>,
    repo: Option<String>,
}

/// The flags of one command: each as `--name value`, `--name=value` or a lone switch.
struct Flags<'a> {
    words: std::slice::Iter<'a, &'a str>,
}

impl<'a> Flags<'a> {
    /// The next flag and, for those that take one, its value; a positional word comes as itself.
    fn next(
        &mut self,
        takes_value: impl Fn(&str) -> bool,
    ) -> Option<Option<(&'a str, Option<String>)>> {
        let word = *self.words.next()?;
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) if name.starts_with('-') => (name, Some(value.to_owned())),
            _ => (word, None),
        };
        if !takes_value(name) {
            return Some(Some((name, inline)));
        }
        let value = match inline {
            Some(value) => Some(value),
            None => self.words.next().map(|value| (*value).to_owned()),
        };
        Some(value.map(|value| (name, Some(value))))
    }
}

fn fields(value: Option<String>) -> Option<Vec<String>> {
    let fields: Vec<String> = (value?.split(','))
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(str::to_owned)
        .collect();
    (!fields.is_empty()).then_some(fields)
}

impl Ask {
    fn of(words: &[&str]) -> Option<Self> {
        let (command, rest) = match words {
            ["run", command, rest @ ..] => (*command, rest),
            _ => return None,
        };
        let mut flags = Flags { words: rest.iter() };
        match command {
            "list" | "ls" => {
                let takes = |name: &str| {
                    matches!(
                        name,
                        "-b" | "--branch"
                            | "-c"
                            | "--commit"
                            | "-w"
                            | "--workflow"
                            | "-s"
                            | "--status"
                            | "-e"
                            | "--event"
                            | "-u"
                            | "--user"
                            | "-L"
                            | "--limit"
                            | "--json"
                            | "-R"
                            | "--repo"
                    )
                };
                let mut list = List::default();
                while let Some(flag) = flags.next(takes) {
                    let (name, value) = flag?;
                    match name {
                        "-b" | "--branch" => list.branch = value,
                        "-c" | "--commit" => list.commit = value,
                        "-w" | "--workflow" => list.workflow = value,
                        "-s" | "--status" => list.status = value,
                        "-e" | "--event" => list.event = value,
                        "-u" | "--user" => list.user = value,
                        "-L" | "--limit" => list.limit = Some(value?.parse().ok()?),
                        "--json" => list.json = Some(fields(value)?),
                        "-R" | "--repo" => list.repo = value,
                        _ => return None,
                    }
                }
                Some(Self::List(list))
            }
            "view" => {
                let takes =
                    |name: &str| matches!(name, "-j" | "--job" | "--json" | "-R" | "--repo");
                let mut view = View::default();
                while let Some(flag) = flags.next(takes) {
                    let (name, value) = flag?;
                    match name {
                        "-j" | "--job" => view.job = value,
                        "--log" => view.log = true,
                        "--log-failed" => view.log_failed = true,
                        "--exit-status" => view.exit_status = true,
                        "-v" | "--verbose" => {}
                        "--json" => view.json = Some(fields(value)?),
                        "-R" | "--repo" => view.repo = value,
                        run if !run.starts_with('-') && view.run.is_none() => {
                            view.run = Some(run.to_owned());
                        }
                        _ => return None,
                    }
                }
                Some(Self::View(view))
            }
            "watch" => {
                let run = rest.iter().find(|word| !word.starts_with('-'));
                Some(Self::Watch {
                    run: run.map(|run| (*run).to_owned()),
                })
            }
            _ => None,
        }
    }

    fn repo(&self) -> Option<&str> {
        let named = match self {
            Self::List(list) => list.repo.as_deref(),
            Self::View(view) => view.repo.as_deref(),
            Self::Watch { .. } => None,
        }?;
        Some(
            named
                .rsplit_once("github.com/")
                .map_or(named, |(_, slug)| slug),
        )
    }
}

impl List {
    async fn answer(self, repo: &Repo) -> Result<Option<Value>, String> {
        let limit = self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let workflow = match &self.workflow {
            Some(named) => match workflow_id(repo, named).await? {
                Some(id) => Some(id),
                None => return Ok(None),
            },
            None => None,
        };
        let params = json!({
            "branch": self.branch,
            "headSha": self.commit,
            "workflowId": workflow,
            "status": self.status,
            "event": self.event,
            "actor": self.user,
            "perPage": limit,
        });
        let page = repo
            .call("runs", params)
            .await
            .map_err(|failure| failure.message)?;
        let runs = page["runs"].as_array().cloned().unwrap_or_default();
        if let Some(fields) = &self.json {
            let shown: Option<Vec<Value>> =
                runs.iter().map(|run| run_json(run, None, fields)).collect();
            let Some(shown) = shown else {
                return Ok(None);
            };
            return Ok(printed(pretty(&Value::Array(shown)), 0));
        }
        let lines: Vec<String> = runs
            .iter()
            .map(|run| {
                [
                    text(run, "status"),
                    text(run, "conclusion"),
                    text(run, "title"),
                    text(run, "name"),
                    text(run, "branch"),
                    text(run, "event"),
                    text(run, "id"),
                    text(run, "createdAt"),
                ]
                .join("\t")
            })
            .collect();
        Ok(printed(lines.join("\n"), 0))
    }
}

/// The id of the workflow a name, a file or an id points at.
async fn workflow_id(repo: &Repo, named: &str) -> Result<Option<Value>, String> {
    if named.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(Some(named.into()));
    }
    let workflows = repo::workflows(repo).await?;
    Ok(workflows
        .iter()
        .find(|workflow| {
            text(workflow, "name") == named
                || text(workflow, "path").rsplit('/').next() == Some(named)
        })
        .map(|workflow| workflow["id"].clone()))
}

impl View {
    async fn answer(self, core: &Core, repo: &Repo) -> Result<Option<Value>, String> {
        let Some(given) = &self.run else {
            return Ok(None);
        };
        let job_named = self
            .job
            .clone()
            .or_else(|| repo::job_in(core, given).and_then(|job| job.as_str().map(str::to_owned)));
        let run_id = repo::run_in(core, given).unwrap_or_else(|| given.as_str().into());
        if !run_id
            .as_str()
            .is_some_and(|id| id.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Ok(None);
        }
        let detail = repo::run(repo, &run_id).await?;
        let run = &detail["run"];
        let jobs = detail["jobs"].as_array().cloned().unwrap_or_default();
        let failed = text(run, "status") == "completed"
            && !matches!(text(run, "conclusion"), "success" | "skipped" | "neutral");
        let code = i32::from(self.exit_status && failed);
        if let Some(fields) = &self.json {
            return Ok(
                run_json(run, Some(&jobs), fields).and_then(|shown| printed(pretty(&shown), code))
            );
        }
        let job = job_named.and_then(|id| jobs.iter().find(|job| text(job, "id") == id).cloned());
        if self.log || self.log_failed {
            let picked: Vec<&Value> = match &job {
                Some(job) => vec![job],
                None if self.log_failed => jobs.iter().filter(|job| job_failed(job)).collect(),
                None => return Ok(None),
            };
            let mut out = Vec::new();
            for job in picked {
                let lines = if self.log {
                    whole_log(repo, &run_id, job).await?
                } else {
                    repo::log_tail(repo, &run_id, &job["id"], FAILED_LOG_LINES).await?
                };
                let name = text(job, "name");
                out.extend(lines.iter().map(|line| format!("{name}\t{line}")));
            }
            return Ok(printed(out.join("\n"), code));
        }
        let mut out = vec![
            format!("{} · {}", text(run, "title"), text(run, "id")),
            format!(
                "{} on {} ({}): {} {}",
                text(run, "name"),
                text(run, "branch"),
                text(run, "shortSha"),
                text(run, "status"),
                text(run, "conclusion"),
            ),
            text(run, "url").to_owned(),
            String::new(),
        ];
        match &job {
            Some(job) => {
                out.push(format!(
                    "{} ({}) ID {}",
                    text(job, "name"),
                    state(job),
                    text(job, "id")
                ));
                for step in job["steps"].as_array().into_iter().flatten() {
                    out.push(format!("  {} ({})", text(step, "name"), state(step)));
                }
            }
            None => {
                out.push("JOBS".to_owned());
                for job in &jobs {
                    out.push(format!(
                        "{} ({}) ID {}",
                        text(job, "name"),
                        state(job),
                        text(job, "id")
                    ));
                }
            }
        }
        if failed {
            out.push(String::new());
            out.push(format!(
                "For what failed: gh run view {} --log-failed, or who_wrote with this run's address.",
                text(run, "id")
            ));
        }
        Ok(printed(out.join("\n"), code))
    }
}

async fn whole_log(repo: &Repo, run_id: &Value, job: &Value) -> Result<Vec<String>, String> {
    let log = repo
        .call("jobLog", json!({ "runId": run_id, "jobId": job["id"] }))
        .await
        .map_err(|failure| failure.message)?;
    Ok(log["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|line| text(line, "text").to_owned())
        .collect())
}

fn job_failed(job: &Value) -> bool {
    matches!(
        text(job, "conclusion"),
        "failure" | "timed_out" | "startup_failure"
    )
}

fn state(item: &Value) -> &str {
    match text(item, "conclusion") {
        "" => text(item, "status"),
        conclusion => conclusion,
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn number(value: &Value, key: &str) -> Value {
    match &value[key] {
        Value::String(text) => text.parse::<u64>().map_or(Value::Null, Value::from),
        other => other.clone(),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// A run as `gh --json` names its fields, or none when a field asked for is one it does not keep.
fn run_json(run: &Value, jobs: Option<&[Value]>, fields: &[String]) -> Option<Value> {
    let mut shown = Map::new();
    for field in fields {
        let value = match field.as_str() {
            "attempt" => run["attempt"].clone(),
            "conclusion" => text(run, "conclusion").into(),
            "createdAt" => run["createdAt"].clone(),
            "databaseId" => number(run, "id"),
            "displayTitle" => run["title"].clone(),
            "event" => run["event"].clone(),
            "headBranch" => run["branch"].clone(),
            "headSha" => run["sha"].clone(),
            "name" | "workflowName" => run["name"].clone(),
            "number" => run["runNumber"].clone(),
            "startedAt" => run["startedAt"].clone(),
            "status" => run["status"].clone(),
            "updatedAt" => run["updatedAt"].clone(),
            "url" => run["url"].clone(),
            "workflowDatabaseId" => number(run, "workflowId"),
            "jobs" => Value::Array(jobs?.iter().map(job_json).collect()),
            _ => return None,
        };
        shown.insert(field.clone(), value);
    }
    Some(Value::Object(shown))
}

fn job_json(job: &Value) -> Value {
    let steps: Vec<Value> = (job["steps"].as_array().into_iter().flatten())
        .map(|step| {
            json!({
                "name": step["name"],
                "number": step["number"],
                "status": step["status"],
                "conclusion": text(step, "conclusion"),
                "startedAt": step["startedAt"],
                "completedAt": step["completedAt"],
            })
        })
        .collect();
    json!({
        "databaseId": number(job, "id"),
        "name": job["name"],
        "status": job["status"],
        "conclusion": text(job, "conclusion"),
        "startedAt": job["startedAt"],
        "completedAt": job["completedAt"],
        "url": job["url"],
        "steps": steps,
    })
}
