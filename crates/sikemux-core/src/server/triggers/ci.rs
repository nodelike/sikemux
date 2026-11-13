//! Triggers that fire once CI on a commit or a run ends, so an agent ends its turn rather than
//! sleeping and asking the code host again. The core's one CI watch looks at the repository and
//! says when its runs move; a trigger also looks on its own now and then, in case a move was missed.

use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::serving::ServeParts;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::{fire, remove_where, Firing, Trigger};
use crate::git::Git;
use crate::server::codehost::ci::Shown;
use crate::server::codehost::repo::{self as codehost, Checks, Conclusion, CI_START_WAIT_MS};
use crate::server::connection::blocking;
use crate::server::notify::{CiEnded, Signal};
use crate::server::Core;

/// How long a CI trigger goes without looking, when the CI watch says nothing moved.
pub(super) const LOOK_AGAIN_MS: u64 = 5 * 60 * 1000;
/// How many looks in a row may fail to read the CI before its wait ends as unread: an hour of
/// looking again.
const MAX_UNREAD: u32 = 12;
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

    /// The run or commit it waits on, which every agent waiting on it shares.
    fn run(&self) -> String {
        let on = match &self.on {
            CiOn::Commit(sha) => sha.clone(),
            CiOn::Run(id) => id_text(id),
        };
        format!("{} {on}", self.repo.id)
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

/// What `on` names in the checkout `dir`: a run by its address or id, or else a commit, a
/// branch or `HEAD`; and how the trigger shows it.
pub(super) async fn watch(
    core: &Arc<Core>,
    dir: &str,
    on: &str,
) -> Result<(CiWatch, String), String> {
    let repo = codehost::of_project(core, dir).await?;
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
        repo: repo.watched(),
        on,
        since: epoch_ms(),
        running_since: None,
        unread: 0,
    };
    Ok((watch, shown))
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
    let run = &detail["run"];
    if run["status"] != "completed" {
        return Ok(Ci::Going { running: true });
    }
    let conclusion = run["conclusion"].as_str().unwrap_or("ended");
    let url = run["url"].as_str().unwrap_or_default();
    let mut said = format!("CI run {} ended {conclusion}. {url}", id_text(id));
    let how = match Conclusion::of(conclusion) {
        Conclusion::Passed => CiEnded::Passed,
        Conclusion::Failed => CiEnded::Failed,
        Conclusion::Stopped => CiEnded::Stopped,
    };
    if how == CiEnded::Failed {
        let failed: Vec<&str> = detail["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|job| {
                job["conclusion"].as_str().map(Conclusion::of) == Some(Conclusion::Failed)
            })
            .filter_map(|job| job["name"].as_str())
            .collect();
        if !failed.is_empty() {
            said.push_str(&format!("\nFailed: {}", failed.join(", ")));
        }
        said.push_str(&format!("\n{BLAME}"));
    }
    Ok(Ci::Ended(said.trim_end().to_owned(), how))
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
                "No CI run started on {short} within {} minutes of this trigger being set; it may not be pushed, or CI may skip it.",
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
        Ok(Ci::Going { running }) => {
            let now = epoch_ms();
            if (running && watch.running_since.is_none()) || watch.unread > 0 {
                watched(&core, id, |watch| {
                    watch.unread = 0;
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
    let Some(mut trigger) = remove_where(&core, |trigger: &Trigger| trigger.id == id) else {
        return;
    };
    if let (Some(since), CiEnded::Passed | CiEnded::Failed) = (watch.running_since, how) {
        core.waits()
            .took(&watch.waits_on(), epoch_ms().saturating_sub(since));
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
