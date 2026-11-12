//! What counts as landed in a project: committed, pushed or merged. The person sets it on the
//! Playbook's Landing page; until then it is read from how the project's latest changes reached
//! its branch, once an hour at most.

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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LandedMeansParams {
    pub project: String,
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
    if let Some((at, history)) = core.landing.0.locked().get(project) {
        if now.saturating_sub(*at) < READ_AGAIN_MS {
            return history.clone();
        }
    }
    let history = read(Path::new(project));
    (core.landing.0.locked()).insert(project.to_owned(), (now, history.clone()));
    history
}

/// How the latest changes reached the branch the checkout at `dir` is on: merged when most came
/// in by a pull request, pushed when most went straight to its remote, and committed when it
/// has no remote.
fn read(dir: &Path) -> Option<LandingHistory> {
    let branch = Git::quick(dir)
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()?;
    let upstream = Git::quick(dir)
        .trimmed(&[
            "rev-parse",
            "--symbolic-full-name",
            &format!("{branch}@{{upstream}}"),
        ])
        .ok()
        .filter(|name| name.starts_with("refs/remotes/"));
    let tip = upstream.as_deref().unwrap_or(&branch);
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
    let (means, count) = match upstream {
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

/// Whether a change came in by a pull request: a merge, or a squash its code host named
/// `(#123)`.
fn by_pull_request(line: &str) -> bool {
    let (parents, subject) = line.split_once('\x1f').unwrap_or((line, ""));
    let squashed = (subject.trim_end().strip_suffix(')'))
        .and_then(|rest| rest.rsplit_once("(#"))
        .is_some_and(|(_, number)| {
            !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
        });
    parents.split_whitespace().count() > 1 || squashed
}
