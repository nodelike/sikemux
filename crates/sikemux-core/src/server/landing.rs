//! What counts as landed in a project: committed, pushed or merged. The person sets it on the
//! Playbook's Landing page; until then it is read from how the project's latest changes reached
//! its branch, once an hour at most.

use crate::server::parts::Parts;
pub use sikemux_calls::landing::LandedMeansParams;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::turn::{Landed, LandedMeans, LandingHistory};

use super::connection::blocking;
use super::{Core, CoreResult};
use crate::git::Git;

/// How many of the branch's latest changes the history is read from.
const LOOKED_AT: usize = 50;
const READ_AGAIN_MS: u64 = 60 * 60 * 1000;

/// Each project's history as last read, with when.
#[derive(Default)]
pub(crate) struct Histories(Mutex<HashMap<String, (u64, Option<LandingHistory>)>>);

/// What counts as landed in `project`, reading its history from git when it is not read yet,
/// so only for a caller that may wait on git.
pub(crate) fn means_now(core: &Core, project: &str) -> LandedMeans {
    let history = history(core, project);
    match super::board::landed_set(core, project) {
        Some(means) => LandedMeans {
            means,
            chosen: true,
            history,
        },
        None => LandedMeans {
            means: history
                .as_ref()
                .map_or(Landed::Pushed, |history| history.means),
            chosen: false,
            history,
        },
    }
}

/// What counts as landed in `project`, for the Playbook.
pub(crate) async fn landed_means(core: &Arc<Core>, project: String) -> CoreResult<LandedMeans> {
    let core = core.clone();
    blocking(move || {
        let project = super::board::project_at(&core, &project)?;
        Ok(means_now(&core, &project))
    })
    .await
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLandedMeansParams {
    pub project: String,
    /// None goes back to what the project's history says.
    #[serde(default)]
    pub means: Option<Landed>,
}

/// The person sets what counts as landed in the project.
pub(crate) async fn set_landed_means(
    core: &Arc<Core>,
    params: SetLandedMeansParams,
) -> CoreResult<LandedMeans> {
    super::board::person_settings(
        core,
        super::board::BoardSettingsParams {
            project: params.project.clone(),
            landed: Some(params.means),
            ..Default::default()
        },
    )
    .await?;
    landed_means(core, params.project).await
}

fn history(core: &Core, project: &str) -> Option<LandingHistory> {
    let now = epoch_ms();
    if let Some((at, history)) = core.landing().0.locked().get(project) {
        if now.saturating_sub(*at) < READ_AGAIN_MS {
            return history.clone();
        }
    }
    let history = read(Path::new(project));
    (core.landing().0.locked()).insert(project.to_owned(), (now, history.clone()));
    history
}

/// The branch a project's work lands on, whatever an agent's checkout has out: the one its
/// remote's HEAD names, else `main`, else `master`, else the one the project's own checkout at
/// `repo` is on.
pub(crate) fn project_branch(repo: &Path) -> Option<String> {
    let remote_head = Git::quick(repo)
        .trimmed(&[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .ok();
    if let Some((_, branch)) = remote_head.as_deref().and_then(|head| head.split_once('/')) {
        return Some(branch.to_owned());
    }
    ["main", "master"]
        .into_iter()
        .find(|branch| exists(repo, &format!("refs/heads/{branch}")))
        .map(str::to_owned)
        .or_else(|| {
            Git::quick(repo)
                .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
                .ok()
        })
}

/// Where `branch` stands on its remote: what it pushes to, else origin's copy of it.
pub(crate) fn remote_of(repo: &Path, branch: &str) -> Option<String> {
    let upstream = Git::quick(repo)
        .trimmed(&[
            "rev-parse",
            "--symbolic-full-name",
            &format!("{branch}@{{upstream}}"),
        ])
        .ok()
        .filter(|name| name.starts_with("refs/remotes/"));
    upstream.or_else(|| {
        let origin = format!("refs/remotes/origin/{branch}");
        exists(repo, &origin).then_some(origin)
    })
}

fn exists(repo: &Path, name: &str) -> bool {
    Git::quick(repo)
        .output(&["rev-parse", "--verify", "--quiet", name])
        .is_ok_and(|output| output.status.success())
}

/// How the latest changes reached the project's branch in the repository at `dir`: merged when
/// most came in by a pull request, pushed when most went straight to its remote, and committed
/// when it has no remote.
fn read(dir: &Path) -> Option<LandingHistory> {
    let branch = project_branch(dir)?;
    let remote = remote_of(dir, &branch);
    let local = format!("refs/heads/{branch}");
    let tip = remote.as_deref().unwrap_or(&local);
    let count = format!("-n{LOOKED_AT}");
    let listed = Git::quick(dir)
        .text(&["log", "--first-parent", &count, "--format=%P%x1f%s", tip])
        .ok()?;
    let changes: Vec<bool> = listed.lines().map(by_pull_request).collect();
    let of = changes.len() as u32;
    if of == 0 {
        return None;
    }
    let merged = changes.iter().filter(|merged| **merged).count() as u32;
    let (means, count) = match remote {
        None => (Landed::Committed, of),
        Some(_) if merged * 2 > of => (Landed::Merged, merged),
        Some(_) => (Landed::Pushed, of - merged),
    };
    Some(LandingHistory {
        means,
        count,
        of,
        branch,
    })
}

/// Whether a change came in by a pull request: a merge, or a squash its code host named, such
/// as GitHub's `(#123)` or Bitbucket's `(pull request #123)`.
fn by_pull_request(line: &str) -> bool {
    let (parents, subject) = line.split_once('\x1f').unwrap_or((line, ""));
    let numbered = |rest: &str| {
        rest.rsplit_once('#').is_some_and(|(before, number)| {
            (before.ends_with('(') || before.ends_with("(pull request "))
                && !number.is_empty()
                && number.bytes().all(|b| b.is_ascii_digit())
        })
    };
    let squashed = subject.trim_end().strip_suffix(')').is_some_and(numbered);
    parents.split_whitespace().count() > 1 || squashed
}
