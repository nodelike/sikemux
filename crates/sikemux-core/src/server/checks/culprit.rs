//! Finds the commit that broke a project's check by testing commits between
//! the last sound one and the failing one in the integration checkout, for a
//! bounded time, one project's step at a time.

use crate::server::parts::Parts;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{run_check, Commit, Ran};
use crate::git::Git;
use crate::server::edits::generate::refresh;
use crate::server::Core;

/// How long a search for the commit may take, so a broken project still gets
/// its next verdict soon.
const HUNT_LIMIT: Duration = Duration::from_secs(15 * 60);
/// More commits than this are searched by nobody; one agent owns the failure.
const MAX_COMMITS: usize = 256;

pub(super) enum Hunted {
    /// The commit that broke it, with the steps the search took.
    Found {
        sha: String,
        subject: String,
        agents: Vec<String>,
        steps: u32,
    },
    Unknown {
        why: String,
        steps: u32,
    },
    /// Nothing was searched.
    NotTried,
}

#[derive(Clone)]
pub(super) struct Search {
    pub dir: PathBuf,
    pub command: String,
    pub seed: u32,
    /// The newest commit known to pass this check.
    pub sound: Option<String>,
    /// The commits of the failed run, oldest first.
    pub commits: Vec<Commit>,
    pub failing_took: u64,
    /// Search in the merge queue's checkout, not the integration checkout.
    pub in_queue: bool,
}

impl Search {
    fn newest(&self) -> &str {
        self.commits.last().map_or("", |commit| commit.sha.as_str())
    }

    fn oldest(&self) -> &str {
        self.commits
            .first()
            .map_or("", |commit| commit.sha.as_str())
    }

    /// The agents of `sha`, by the run's own list, or else by the commit's signed footer.
    fn agents_of(&self, core: &Core, sha: &str) -> Vec<String> {
        if let Some(commit) = self.commits.iter().find(|commit| commit.sha == sha) {
            return commit.agents.clone();
        }
        crate::server::edits::footer::verified(core, &self.dir, sha)
            .unwrap_or_default()
            .into_iter()
            .map(|agent| agent.chat)
            .collect()
    }
}

/// Commits to search, oldest first, and whether the commit before them is known sound.
fn commits_after(search: &Search) -> Result<(String, bool, Vec<String>), String> {
    let git = || Git::quick(&search.dir);
    let (base, known) = match &search.sound {
        Some(sound)
            if git()
                .output(&["merge-base", "--is-ancestor", sound, search.newest()])
                .is_ok_and(|output| output.status.success()) =>
        {
            (sound.clone(), true)
        }
        _ => (
            git()
                .trimmed(&["rev-parse", &format!("{}^", search.oldest())])
                .map_err(|_| "its oldest commit has no parent".to_owned())?,
            false,
        ),
    };
    let listed = git()
        .trimmed(&[
            "rev-list",
            "--reverse",
            "--first-parent",
            &format!("{base}..{}", search.newest()),
        ])
        .map_err(|error| error.to_string())?;
    Ok((
        base,
        known,
        listed.lines().map(str::to_owned).collect::<Vec<_>>(),
    ))
}

fn budgeted(started: Instant, step_took: Duration) -> bool {
    started.elapsed() + step_took <= HUNT_LIMIT
}

/// Whether the check passes on `sha`, in the integration checkout.
async fn passes(
    core: &Arc<Core>,
    search: &Search,
    sha: &str,
    started: Instant,
) -> Result<Ran, String> {
    let _turn = core.checks().hunt_turn.lock().await;
    let Some(left) = HUNT_LIMIT.checked_sub(started.elapsed()) else {
        return Err("it ran out of time".to_owned());
    };
    let checkout = match search.in_queue {
        true => crate::server::worktree::agent::merge_queue(core, &search.dir, sha).await?,
        false => crate::server::worktree::agent::integration(core, &search.dir, sha).await?,
    };
    let env = match search.in_queue {
        true => (search.commits.iter())
            .find_map(|commit| commit.agents.first())
            .map(|agent| super::before_commit(agent))
            .unwrap_or_default(),
        false => Vec::new(),
    };
    let (owner, search) = (core.clone(), search.clone());
    tokio::task::spawn_blocking(move || {
        refresh(&owner, &search.dir, &checkout)?;
        run_check(&checkout, &search.command, search.seed, left, env)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Which commit broke the check. A step that can't run, a commit before them that fails too,
/// and the time running out each end the search with none.
pub(super) async fn hunt(core: &Arc<Core>, search: Search) -> Hunted {
    let listing = search.clone();
    let found = tokio::task::spawn_blocking(move || commits_after(&listing))
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
    let (base, known, commits) = match found {
        Ok(found) => found,
        Err(why) => return Hunted::Unknown { why, steps: 0 },
    };
    if commits.is_empty() {
        return Hunted::Unknown {
            why: "no commits lie between the last sound one and this".to_owned(),
            steps: 0,
        };
    }
    if commits.len() > MAX_COMMITS {
        return Hunted::Unknown {
            why: format!("{} commits are too many to search", commits.len()),
            steps: 0,
        };
    }
    let started = Instant::now();
    let mut step = Duration::from_millis(search.failing_took);
    let mut steps = 0;
    let unknown = |why: String, steps| Hunted::Unknown { why, steps };
    if !known {
        if !budgeted(started, step) {
            return unknown("each run takes too long to search".to_owned(), steps);
        }
        steps += 1;
        match passes(core, &search, &base, started).await {
            Ok(ran) if ran.passed => step = Duration::from_millis(ran.took_ms),
            Ok(_) => {
                return unknown(
                    "the commit before them fails too, so it is older than these".to_owned(),
                    steps,
                )
            }
            Err(why) => return unknown(why, steps),
        }
    }
    let mut good: isize = -1;
    let mut bad = commits.len() as isize - 1;
    while bad - good > 1 {
        if !budgeted(started, step) {
            return unknown("it ran out of time".to_owned(), steps);
        }
        let middle = good + (bad - good) / 2;
        steps += 1;
        match passes(core, &search, &commits[middle as usize], started).await {
            Ok(ran) => {
                step = Duration::from_millis(ran.took_ms);
                if ran.passed {
                    good = middle;
                } else {
                    bad = middle;
                }
            }
            Err(why) => return unknown(why, steps),
        }
    }
    let sha = commits[bad as usize].clone();
    let (owner, reading, found_sha) = (core.clone(), search.clone(), sha.clone());
    let (subject, agents) = tokio::task::spawn_blocking(move || {
        let subject = Git::quick(&reading.dir)
            .trimmed(&["log", "-1", "--format=%s", &found_sha])
            .unwrap_or_default();
        (subject, reading.agents_of(&owner, &found_sha))
    })
    .await
    .unwrap_or_default();
    Hunted::Found {
        sha,
        subject,
        agents,
        steps,
    }
}
