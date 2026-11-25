//! Triggers that fire once CI on a commit or a run ends, so an agent ends its turn rather than
//! sleeping and asking the code host again. The core's one CI watch says when the repository's
//! runs move; a trigger also looks on its own now and then, in case a move was missed.

use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::{fire, messages, take_for_firing, Firing, Trigger};
use crate::git::Git;
use crate::server::codehost::ci::Shown;
use crate::server::codehost::repo::{self as codehost, Checks, Conclusion, CI_START_WAIT_MS};
use crate::server::connection::blocking;
use crate::server::notify::{CiEnded, Signal};
use crate::server::Core;

/// How long a CI trigger goes without looking, when the CI watch says nothing moved.
const LOOK_AGAIN_MS: u64 = 5 * 60 * 1000;
/// The same, once its CI is running in a repository whose CI webhook tells the core of each move.
const FED_LOOK_AGAIN_MS: u64 = 30 * 60 * 1000;
/// How many looks in a row may fail to read the CI before its wait ends as unread.
const MAX_UNREAD: u32 = 12;
/// How long a CI trigger waits on CI that never ends, such as a run left queued, before it stops.
const MAX_WAIT_MS: u64 = 12 * 60 * 60 * 1000;
const SHORT_SHA: usize = 9;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CiWatch {
    /// The checkout it was set in, whose origin names the repository.
    dir: String,
    repo: Shown,
    on: CiOn,
    /// When it was set, in Unix milliseconds.
    since: u64,
    /// When the core first saw its CI running, which its estimate counts from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    running_since: Option<u64>,
    /// How many looks in a row failed to read its CI.
    #[serde(default, skip_serializing_if = "is_zero")]
    unread: u32,
    /// Its repository's CI webhook tells the core when its runs move.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    fed: bool,
    /// The agents whose push it watches, when the core set it rather than an agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pushed_by: Vec<String>,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

/// How the CI a trigger waits on stands.
pub(super) enum Ci {
    /// It goes on; `running` once a run of it has started.
    Going { running: bool },
    /// It ended, as the prompt tells it and as the person is told it.
    Ended(String, CiEnded),
}

impl CiWatch {
    /// When its wait began: when its CI was first seen running, else when it was set.
    pub(super) fn since(&self) -> u64 {
        self.running_since.unwrap_or(self.since)
    }

    /// How long it goes without looking. Where a webhook tells of every move, it looks only to
    /// say no run started, or in case a move was lost.
    pub(super) fn look_again_ms(&self) -> u64 {
        match (self.fed, self.running_since) {
            (false, _) => LOOK_AGAIN_MS,
            (true, None) => CI_START_WAIT_MS,
            (true, Some(_)) => FED_LOOK_AGAIN_MS,
        }
    }

    /// The run or commit it waits on, which every agent waiting on it shares.
    fn run(&self) -> String {
        let on = match &self.on {
            CiOn::Commit(sha) => sha.clone(),
            CiOn::Run(id) => id_text(id),
        };
        format!("{} {on}", self.repo.id)
    }

    pub(super) fn after_push(&self) -> bool {
        !self.pushed_by.is_empty()
    }

    pub(super) fn pushed_by(&self) -> &[String] {
        &self.pushed_by
    }

    pub(super) fn same_repo(&self, other: &CiWatch) -> bool {
        self.repo == other.repo
    }

    pub(super) fn same_run(&self, other: &CiWatch) -> bool {
        self.run() == other.run()
    }

    pub(super) fn forget(&mut self, agents: &[String]) {
        self.pushed_by.retain(|agent| !agents.contains(agent));
    }

    pub(super) fn join(&mut self, agents: &[String]) {
        for agent in agents {
            if !self.pushed_by.contains(agent) {
                self.pushed_by.push(agent.clone());
            }
        }
    }

    pub(super) fn shown(&self) -> String {
        match &self.on {
            CiOn::Commit(sha) => sha.chars().take(SHORT_SHA).collect(),
            CiOn::Run(id) => format!("run {}", id_text(id)),
        }
    }

    /// What its wait is kept under, so the next wait on CI in the same repository has an
    /// estimate.
    pub(super) fn waits_on(&self) -> String {
        format!("ci {}", self.repo.id)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CiOn {
    Commit(String),
    Run(Value),
}

/// What `on` names in the checkout `dir`, or else in another repository of its project: a run
/// by its address or id, or else a commit, a branch or `HEAD`; and how the trigger shows it.
pub(super) async fn watch(
    core: &Arc<Core>,
    dir: &str,
    on: &str,
) -> Result<(CiWatch, String), String> {
    let mut first = None;
    for checkout in checkouts(core, dir) {
        match watch_in(core, &checkout, on).await {
            Ok(found) => return Ok(found),
            Err(error) => first = first.or(Some(error)),
        }
    }
    Err(first.unwrap_or_default())
}

/// `dir` first, then the repositories beside it in a project that is a folder of them.
fn checkouts(core: &Core, dir: &str) -> Vec<String> {
    let root = core.workspaces().project_of(Path::new(dir));
    let others = (std::fs::read_dir(&root).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| path.join(".git").exists())
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| path != dir);
    std::iter::once(dir.to_owned()).chain(others).collect()
}

async fn watch_in(core: &Arc<Core>, dir: &str, on: &str) -> Result<(CiWatch, String), String> {
    let repo = codehost::of_project(core, dir).await?;
    let id = repo.id();
    if codehost::run_in(core, on).is_some() && !on.contains(&format!("/{}/{}/", id.owner, id.name))
    {
        return Err(format!("{on} is a run of another repository than {dir}'s"));
    }
    let commit = {
        let (dir, on) = (dir.to_owned(), on.to_owned());
        blocking(move || Ok(commit_of(Path::new(&dir), &on)))
            .await
            .map_err(|error| error.to_string())?
    };
    let run = codehost::run_in(core, on).or_else(|| {
        let id = on.starts_with('{') || on.bytes().all(|byte| byte.is_ascii_digit());
        (id && commit.is_none()).then(|| on.into())
    });
    let (on, shown) = match (run, commit) {
        (Some(run), _) => {
            let shown = format!("run {}", run.as_str().unwrap_or(on));
            (CiOn::Run(run), shown)
        }
        (None, Some(sha)) => {
            let shown = sha.chars().take(SHORT_SHA).collect();
            (CiOn::Commit(sha), shown)
        }
        (None, None) => {
            return Err(format!(
                "ci must be a commit, a branch or a CI run's address or id; {on} is none here"
            ))
        }
    };
    let watch = CiWatch {
        dir: dir.to_owned(),
        fed: crate::server::codehost::ci::is_fed(core, &id),
        repo: repo.watched(),
        on,
        since: epoch_ms(),
        running_since: None,
        unread: 0,
        pushed_by: Vec::new(),
    };
    Ok((watch, shown))
}

/// A watch on the CI of `sha`, which `agents` pushed from `dir`; none where no code host with
/// CI holds the repository.
pub(super) async fn after_push(
    core: &Arc<Core>,
    dir: String,
    agents: Vec<String>,
    sha: String,
) -> Option<CiWatch> {
    if agents.is_empty() {
        return None;
    }
    let repo = codehost::of_project(core, &dir).await.ok()?;
    Some(CiWatch {
        dir,
        fed: crate::server::codehost::ci::is_fed(core, &repo.id()),
        repo: repo.watched(),
        on: CiOn::Commit(sha),
        since: epoch_ms(),
        running_since: None,
        unread: 0,
        pushed_by: agents,
    })
}

fn commit_of(dir: &Path, on: &str) -> Option<String> {
    let spec = format!("{on}^{{commit}}");
    let args = [
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &spec,
    ];
    Git::quick(dir).trimmed(&args).ok()
}

const BLAME: &str =
    "who_wrote with a failed run's address names the commit that broke it, and why.";

/// How the CI it waits on stands.
pub(super) async fn ended(core: &Arc<Core>, watch: &CiWatch) -> Result<Ci, String> {
    let repo = codehost::of_project(core, &watch.dir).await?;
    match &watch.on {
        CiOn::Run(id) => run_ended(&repo, id).await,
        CiOn::Commit(sha) => commit_ended(&repo, sha, watch.since).await,
    }
}

async fn run_ended(repo: &codehost::Repo, id: &Value) -> Result<Ci, String> {
    let detail = codehost::run(repo, id).await?;
    let run = |key: &str| detail.get("run").and_then(|run| run.get(key)?.as_str());
    let conclusion = if run("status") == Some("completed") {
        run("conclusion").unwrap_or("ended")
    } else if a_job_failed(&detail) {
        "failure"
    } else {
        return Ok(Ci::Going { running: true });
    };
    let url = run("url").unwrap_or_default();
    let how = match Conclusion::of(conclusion) {
        Conclusion::Passed => CiEnded::Passed,
        Conclusion::Failed => match codehost::jobs_conclusion(&detail) {
            Conclusion::Failed => CiEnded::Failed,
            Conclusion::Stopped | Conclusion::Passed => CiEnded::Stopped,
        },
        Conclusion::Stopped => CiEnded::Stopped,
    };
    let ended = if how == CiEnded::Stopped {
        "stopped"
    } else {
        conclusion
    };
    let mut said = format!("CI run {} ended {ended}. {url}", id_text(id));
    if how == CiEnded::Failed {
        let failed: Vec<&str> = (detail.get("jobs").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .filter(|job| {
                job.get("conclusion")
                    .and_then(Value::as_str)
                    .map(Conclusion::of)
                    == Some(Conclusion::Failed)
            })
            .filter_map(|job| job.get("name")?.as_str())
            .collect();
        if !failed.is_empty() {
            said.push_str(&format!("\nFailed: {}", failed.join(", ")));
        }
        said.push_str(&format!("\n{BLAME}"));
    }
    Ok(Ci::Ended(said.trim_end().to_owned(), how))
}

fn a_job_failed(detail: &Value) -> bool {
    (detail
        .get("jobs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten())
    .any(|job| {
        job.get("conclusion")
            .and_then(Value::as_str)
            .map(Conclusion::of)
            == Some(Conclusion::Failed)
    })
}

async fn commit_ended(repo: &codehost::Repo, sha: &str, since: u64) -> Result<Ci, String> {
    let short: String = sha.chars().take(SHORT_SHA).collect();
    Ok(match codehost::commit_checks(repo, None, sha, &[]).await? {
        Checks::Running { .. } => Ci::Going { running: true },
        Checks::None if epoch_ms().saturating_sub(since) < CI_START_WAIT_MS => {
            Ci::Going { running: false }
        }
        Checks::None => Ci::Ended(
            format!(
                "No CI run started on {short} within {} minutes of this trigger being set; it may not be pushed, or CI may skip it. A run started by hand or on a schedule doesn't count: wait on its address instead.",
                CI_START_WAIT_MS / 60_000
            ),
            CiEnded::NeverStarted,
        ),
        Checks::Passed => Ci::Ended(format!("CI on {short} passed."), CiEnded::Passed),
        Checks::Failed(lines) => Ci::Ended(
            format!("CI on {short} failed:\n{lines}\n{BLAME}"),
            CiEnded::Failed,
        ),
        Checks::Stopped(lines) => Ci::Ended(
            format!("CI on {short} was stopped:\n{lines}"),
            CiEnded::Stopped,
        ),
    })
}

fn id_text(id: &Value) -> String {
    id.as_str().map_or_else(|| id.to_string(), str::to_owned)
}

/// The repositories CI triggers wait on, which the CI watch keeps checking.
pub(crate) fn waiting_on_ci(core: &Core) -> Vec<Shown> {
    let state = core.triggers().state.locked();
    let waiting = state
        .triggers
        .iter()
        .filter(|trigger| trigger.paused.is_none());
    waiting
        .filter_map(|trigger| trigger.ci.as_ref().map(|watch| watch.repo.clone()))
        .collect()
}

/// The runs on `repo` moved, so the CI triggers waiting on it look again.
pub(crate) fn ci_moved(core: &Arc<Core>, repo: &Shown) {
    let ids: Vec<u64> = {
        let state = core.triggers().state.locked();
        let waiting = state
            .triggers
            .iter()
            .filter(|trigger| trigger.paused.is_none());
        waiting
            .filter(|trigger| trigger.ci.as_ref().is_some_and(|watch| &watch.repo == repo))
            .map(|trigger| trigger.id)
            .collect()
    };
    for id in ids {
        tokio::spawn(look(core.clone(), id));
    }
}

/// Fires the CI trigger `id` with how its CI ended, once it has.
pub(super) async fn look(core: Arc<Core>, id: u64) {
    let watch = {
        let state = core.triggers().state.locked();
        let trigger = state.triggers.iter().find(|trigger| trigger.id == id);
        trigger.and_then(|trigger| trigger.ci.clone())
    };
    let Some(watch) = watch else { return };
    crate::server::codehost::ci::start(&core);
    let (said, how) = match ended(&core, &watch).await {
        Ok(Ci::Ended(said, how)) => (said, how),
        Ok(Ci::Going { .. }) if epoch_ms().saturating_sub(watch.since) >= MAX_WAIT_MS => (
            format!(
                "CI on {} had not ended {} hours after this trigger was set, so this stopped waiting.",
                watch.shown(),
                MAX_WAIT_MS / 3_600_000
            ),
            CiEnded::Stopped,
        ),
        Ok(Ci::Going { running }) => {
            let now = epoch_ms();
            let fed = crate::server::codehost::ci::is_fed(&core, &watch.repo.id);
            if (running && watch.running_since.is_none()) || watch.unread > 0 || fed != watch.fed {
                watched(&core, id, |watch| {
                    watch.unread = 0;
                    watch.fed = fed;
                    if running {
                        watch.running_since.get_or_insert(now);
                    }
                });
            }
            return;
        }
        Err(error) => {
            let unread = watch.unread + 1;
            if unread < MAX_UNREAD {
                eprintln!("sikemux core: CI trigger {id} could not read its CI: {error}");
                watched(&core, id, |watch| watch.unread = unread);
                return;
            }
            (
                format!("Its CI could not be read {MAX_UNREAD} times in a row, so this stopped waiting: {error}"),
                CiEnded::Unread,
            )
        }
    };
    let Some((mut trigger, in_flight)) =
        take_for_firing(&core, |trigger: &Trigger| trigger.id == id)
    else {
        return;
    };
    if let (Some(since), CiEnded::Passed | CiEnded::Failed) = (watch.running_since, how) {
        core.waits()
            .took(&watch.waits_on(), epoch_ms().saturating_sub(since));
    }
    if watch.after_push() {
        tell_pushers(&core, &watch, said, how).await;
        return;
    }
    core.notify().send(Signal::CiEnded {
        agent_id: trigger.agent_id.clone(),
        run: watch.run(),
        what: trigger.title.clone(),
        how,
    });
    trigger.prompt = format!("{}\n\n{said}", trigger.prompt);
    fire(
        core,
        Firing {
            trigger,
            late: None,
            last: true,
            request: None,
            in_flight,
        },
    )
    .await;
}

/// Changes what the CI trigger `id` keeps of its CI, saving it.
fn watched(core: &Core, id: u64, change: impl FnOnce(&mut CiWatch)) {
    core.triggers().change(|state| {
        let watch = (state.triggers.iter_mut())
            .find(|trigger| trigger.id == id)
            .and_then(|trigger| trigger.ci.as_mut());
        if let Some(watch) = watch {
            change(watch);
        }
    });
}

const AFTER_PUSH: &str = "Sikemux watches CI after each push, so this needs no trigger_add.";

/// Tells each agent whose push the watch was for how its CI ended. A failure wakes it, or once
/// it has closed its lead, which hears of the run once for all its closed pushers; any other end
/// is only a notice it reads if it is at work.
async fn tell_pushers(core: &Arc<Core>, watch: &CiWatch, said: String, how: CiEnded) {
    let run = watch.run();
    let mut closed_by_hearer: Vec<(String, Vec<String>)> = Vec::new();
    for agent in &watch.pushed_by {
        if watches_itself(core, agent, &run) {
            continue;
        }
        let open = !core.workspaces().closed(agent);
        let told = match (how, open) {
            (CiEnded::Failed, true) => {
                core.notify().send(Signal::CiEnded {
                    agent_id: agent.clone(),
                    run: run.clone(),
                    what: format!("CI after the push of {}", watch.shown()),
                    how,
                });
                let text = format!("After your push: {said}");
                messages::from_trigger(core, agent.clone(), text, AFTER_PUSH.to_owned())
                    .await
                    .map(drop)
            }
            (CiEnded::Failed, false) => {
                if let Some(to) = crate::server::roster::hears_of(core, agent).agent() {
                    match closed_by_hearer.iter_mut().find(|(hearer, _)| hearer == to) {
                        Some((_, closed)) => closed.push(agent.clone()),
                        None => closed_by_hearer.push((to.to_owned(), vec![agent.clone()])),
                    }
                }
                Ok(())
            }
            (_, true) => {
                messages::from_sikemux(core, agent.clone(), format!("After your push: {said}"))
                    .await
                    .map(drop)
            }
            (_, false) => Ok(()),
        };
        if let Err(error) = told {
            eprintln!("sikemux core: CI after a push did not reach {agent}: {error}");
        }
    }
    for (to, closed) in closed_by_hearer {
        messages::report_ci(core, closed, to, said.clone());
    }
}

/// Whether the agent set a CI trigger of its own on the same run, which tells it already.
fn watches_itself(core: &Core, agent: &str, run: &str) -> bool {
    let state = core.triggers().state.locked();
    (state.triggers.iter()).any(|trigger| {
        trigger.agent_id == agent
            && (trigger.ci.as_ref()).is_some_and(|watch| !watch.after_push() && watch.run() == run)
    })
}
