//! The project's check, run on each commit as committed, in the repository's
//! integration checkout, apart from the work in progress in everyone's own
//! trees, once the files it generates are brought up to date. A failure goes to the agents whose commits it failed after, at
//! once, and to the person once the project has stayed broken a while. It is
//! the board's check; a project without one is not checked. Commits that land
//! while a check runs are checked together, at the newest, once it ends. A
//! project whose check is its CI is not built here: the code host's runs on
//! the pushed branch tips that hold its commits give the verdict.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use tokio::sync::Notify;

use super::{tell, State};
use crate::git::Git;
use crate::server::codehost::ci::Shown;
use crate::server::codehost::repo::{self as codehost, Checks as CiChecks, CI_START_WAIT_MS};
use crate::server::repo_events::RepoEvent;
use crate::server::Core;
use crate::tasks::config::ProjectCheck;
use sikemux_wire::slice::{CheckRun, Verdict};

/// Checks running at once, across projects.
const MAX_RUNNING: usize = 2;
/// How long a project stays broken before the person hears of it, so its
/// agents have time to mend it first.
pub(super) const PERSON_AFTER_MS: u64 = 15 * 60_000;
/// Commits kept as checked per project, so the same one is not run twice.
const MAX_CHECKED: usize = 64;
/// How often the commits waiting on CI are looked at when nothing says their
/// refs or runs moved.
const CI_LOOK: std::time::Duration = std::time::Duration::from_secs(300);
/// How long a commit waiting on CI may be on no branch, as while a rebase
/// runs, before it counts as gone.
const GONE_AFTER_MS: u64 = 10 * 60_000;

#[derive(Clone)]
pub(super) struct Commit {
    pub sha: String,
    pub subject: String,
    pub agents: Vec<String>,
}

#[derive(Default)]
pub(super) struct Checks {
    waiting: HashMap<String, (PathBuf, Vec<Commit>)>,
    /// The commits each running check takes in, by project.
    running: HashMap<String, Vec<String>>,
    failing: HashMap<String, Failing>,
    /// The commits each project's check ran on or waits to, newest last.
    checked: HashMap<String, Vec<String>>,
    /// The broken projects the person was told of.
    shown: HashSet<String>,
    /// Each project's CI check that waits, by project.
    ci: HashMap<String, CiWait>,
}

/// A project's CI check while it waits.
struct CiWait {
    /// The code-host repository its runs are read from, once something it
    /// waits on is pushed.
    repo: Option<Shown>,
    /// Wakes it when its commits, refs or runs move.
    wake: Arc<Notify>,
}

/// A project whose check has failed after its latest commits.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Failing {
    pub project: String,
    pub since: u64,
    pub at: u64,
    pub runs: u32,
    /// The agents whose commits it failed after.
    pub agents: Vec<String>,
    pub output: String,
}

impl Checks {
    /// Queues `commit` of `project` to be checked, unless it was already.
    pub(super) fn wait(&mut self, project: String, dir: PathBuf, commit: Commit) {
        let checked = self.checked.entry(project.clone()).or_default();
        if checked.contains(&commit.sha) {
            return;
        }
        if checked.len() >= MAX_CHECKED {
            checked.remove(0);
        }
        checked.push(commit.sha.clone());
        self.waiting
            .entry(project)
            .or_insert_with(|| (dir, Vec::new()))
            .1
            .push(commit);
    }

    /// Queues `commits` of `project` to be checked together, in their order,
    /// even those checked before, unless a running check takes one in.
    pub(super) fn wait_again(&mut self, project: String, dir: PathBuf, commits: Vec<Commit>) {
        for commit in commits {
            let running = self
                .running
                .get(&project)
                .is_some_and(|shas| shas.contains(&commit.sha));
            if running {
                continue;
            }
            if let Some((_, waiting)) = self.waiting.get_mut(&project) {
                waiting.retain(|waiting| waiting.sha != commit.sha);
            }
            if let Some(checked) = self.checked.get_mut(&project) {
                checked.retain(|sha| *sha != commit.sha);
            }
            self.wait(project.clone(), dir.clone(), commit);
        }
    }

    /// The code-host repositories CI checks wait on.
    pub(super) fn ci_repos(&self) -> Vec<Shown> {
        self.ci
            .values()
            .filter_map(|wait| wait.repo.clone())
            .collect()
    }

    /// The runs on `repo` moved, so the CI checks waiting on it look again.
    pub(super) fn ci_moved(&self, repo: &Shown) {
        for wait in self.ci.values() {
            if wait.repo.as_ref() == Some(repo) {
                wait.wake.notify_one();
            }
        }
    }

    /// `project`'s refs moved, as a push moves them, so its CI check looks again.
    pub(super) fn refs_moved(&self, project: &str) {
        if let Some(wait) = self.ci.get(project) {
            wait.wake.notify_one();
        }
    }

    /// The projects broken long enough that the person should hear of it.
    pub(super) fn failing(&self) -> impl Iterator<Item = &Failing> {
        let now = epoch_ms();
        self.failing
            .values()
            .filter(move |failing| failing.since + PERSON_AFTER_MS <= now)
    }

    /// The checks that may start now, taken off the waiting list.
    fn startable(&mut self) -> Vec<(String, PathBuf, Vec<Commit>)> {
        let mut started = Vec::new();
        let ready: Vec<String> = self
            .waiting
            .keys()
            .filter(|project| !self.running.contains_key(*project))
            .cloned()
            .collect();
        for project in ready {
            if self.running.len().saturating_sub(self.ci.len()) >= MAX_RUNNING {
                break;
            }
            if let Some((dir, commits)) = self.waiting.remove(&project) {
                let shas = commits.iter().map(|commit| commit.sha.clone()).collect();
                self.running.insert(project.clone(), shas);
                started.push((project, dir, commits));
            }
        }
        started
    }
}

/// Starts the checks that wait, as far as the limit allows, each on the
/// newest of its commits, and has devices told of a project broken long
/// enough for the person to hear of it, or mended since.
pub(super) fn start(core: &Arc<Core>, state: &mut State) {
    let listed: HashSet<String> = state
        .checks
        .failing()
        .map(|failing| failing.project.clone())
        .collect();
    if listed != state.checks.shown {
        state.checks.shown = listed;
        state.publish = true;
    }
    for (project, wait) in &state.checks.ci {
        if state.checks.waiting.contains_key(project) {
            wait.wake.notify_one();
        }
    }
    for (project, dir, commits) in state.checks.startable() {
        let command = match crate::server::board::project_check(core, &project) {
            Some(ProjectCheck::Command(command)) => command,
            Some(ProjectCheck::Ci { ci }) => {
                tokio::spawn(wait_for_ci(core.clone(), project, dir, commits, ci));
                continue;
            }
            None => {
                state.checks.running.remove(&project);
                continue;
            }
        };
        let core = core.clone();
        tokio::spawn(async move {
            let ordering = dir.clone();
            let (commits, later) =
                tokio::task::spawn_blocking(move || newest_with(&ordering, commits))
                    .await
                    .unwrap_or_default();
            let Some(newest) = commits.last().map(|commit| commit.sha.clone()) else {
                core.collisions
                    .state
                    .locked()
                    .checks
                    .running
                    .remove(&project);
                core.collisions.wake.now();
                return;
            };
            {
                let mut state = core.collisions.state.locked();
                let shas: Vec<String> = commits.iter().map(|commit| commit.sha.clone()).collect();
                state.checks.running.insert(project.clone(), shas.clone());
                state.checks.wait_again(project.clone(), dir.clone(), later);
                core.repo_events.tell(RepoEvent::CheckStarted {
                    project: project.clone(),
                    covered: shas,
                    run: CheckRun::Local,
                });
            }
            let agents: Vec<String> = commits
                .iter()
                .flat_map(|commit| commit.agents.iter().cloned())
                .collect();
            let outcome = match crate::server::worktree::agent::integration(&core, &dir, &newest)
                .await
            {
                Ok(checkout) => {
                    let (owner, command) = (core.clone(), command.clone());
                    let (dir, agents) = (dir.clone(), agents.clone());
                    tokio::task::spawn_blocking(move || {
                            use crate::server::edits::generate::regenerate;
                            match regenerate(&owner, &dir, &checkout, &newest, &agents) {
                                Ok(Some(generated)) => Ok(Err(generated)),
                                Ok(None) => {
                                    crate::server::board::git::check(&checkout, &command).map(Ok)
                                }
                                Err(error) => Err(format!(
                                    "the files the project generates could not be brought up to date: {error}"
                                )),
                            }
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                }
                Err(error) => Err(format!("the integration checkout is not ready: {error}")),
            };
            let outcome = match outcome {
                Ok(Ok(outcome)) => Ok(outcome),
                Ok(Err(generated)) => {
                    let mut state = core.collisions.state.locked();
                    state.checks.running.remove(&project);
                    let mut again = commits;
                    again.push(Commit {
                        sha: generated,
                        subject: "the files the project generates".to_owned(),
                        agents,
                    });
                    state.checks.wait_again(project, dir, again);
                    drop(state);
                    core.collisions.wake.now();
                    return;
                }
                Err(error) => Err(error),
            };
            core.collisions
                .state
                .locked()
                .checks
                .running
                .remove(&project);
            ended(&core, &project, &command, &commits, outcome);
        });
    }
}

/// What git says of the commits a CI check waits on.
#[derive(Default)]
pub(crate) struct Looked {
    /// The tip of the remote branch the project's branch pushes to, when it
    /// holds some of them, with those it holds.
    pub tip: Option<(String, Vec<String>)>,
    /// Those no branch holds.
    pub gone: Vec<String>,
}

/// Waiting commits a CI check ruled on: under `tip`, or with none when no
/// branch holds them any more.
struct Ruled {
    held: Vec<String>,
    tip: Option<String>,
    outcome: Result<(bool, String), String>,
}

/// Reads which of `waiting` the project's pushed branch holds, and which no
/// branch does. Other remote branches, such as work in progress, decide nothing.
pub(crate) fn look(dir: &Path, waiting: &[String]) -> Looked {
    let upstream = crate::git::upstream(dir);
    let tip = upstream
        .as_deref()
        .and_then(|upstream| Git::quick(dir).trimmed(&["rev-parse", upstream]).ok());
    let mut looked = Looked::default();
    for sha in waiting {
        let refs = Git::quick(dir)
            .trimmed(&[
                "for-each-ref",
                "--contains",
                sha,
                "--format=%(refname)",
                "refs/heads",
                "refs/remotes",
            ])
            .unwrap_or_default();
        if refs.is_empty() {
            looked.gone.push(sha.clone());
            continue;
        }
        let (Some(upstream), Some(tip)) = (&upstream, &tip) else {
            continue;
        };
        if refs.lines().any(|name| name == upstream) {
            looked
                .tip
                .get_or_insert_with(|| (tip.clone(), Vec::new()))
                .1
                .push(sha.clone());
        }
    }
    looked
}

/// Waits for the code host's CI to rule on `commits` and those of the
/// project's that come meanwhile. CI runs on the pushed tip of the project's
/// branch, so that tip's verdict goes to every waiting commit it holds: once every workflow named
/// has run there, all must have passed. A commit nobody pushes waits for it;
/// one on no branch for a while, as after a rebase, ends with no verdict, as
/// do those under a tip that no run starts on or whose runs were cancelled.
/// Each verdict on the tip goes to the board too, which files CI failing
/// the same way twice in a row as a card.
async fn wait_for_ci(
    core: Arc<Core>,
    project: String,
    dir: PathBuf,
    mut commits: Vec<Commit>,
    workflows: Vec<String>,
) {
    let label = ProjectCheck::Ci {
        ci: workflows.clone(),
    }
    .label();
    let wake = Arc::new(Notify::new());
    core.collisions.state.locked().checks.ci.insert(
        project.clone(),
        CiWait {
            repo: None,
            wake: wake.clone(),
        },
    );
    let mut repo = None;
    let mut gone_since: HashMap<String, u64> = HashMap::new();
    let mut tips_since: HashMap<String, u64> = HashMap::new();
    let mut started: HashSet<String> = HashSet::new();
    loop {
        {
            let mut state = core.collisions.state.locked();
            if let Some((_, more)) = state.checks.waiting.remove(&project) {
                for commit in more {
                    if commits.iter().all(|kept| kept.sha != commit.sha) {
                        commits.push(commit);
                    }
                }
            }
            let shas = commits.iter().map(|commit| commit.sha.clone()).collect();
            state.checks.running.insert(project.clone(), shas);
        }
        if commits.is_empty() {
            break;
        }
        let (at, waiting) = (
            dir.clone(),
            commits
                .iter()
                .map(|commit| commit.sha.clone())
                .collect::<Vec<_>>(),
        );
        let looked = tokio::task::spawn_blocking(move || look(&at, &waiting))
            .await
            .unwrap_or_default();
        let now = epoch_ms();
        gone_since.retain(|sha, _| looked.gone.contains(sha));
        let mut ruled: Vec<Ruled> = Vec::new();
        for sha in &looked.gone {
            let since = *gone_since.entry(sha.clone()).or_insert(now);
            if now.saturating_sub(since) >= GONE_AFTER_MS {
                let why = format!(
                    "{} is on no branch any more, so CI will never check it",
                    sha.get(..7).unwrap_or(sha)
                );
                ruled.push(Ruled {
                    held: vec![sha.clone()],
                    tip: None,
                    outcome: Err(why),
                });
            }
        }
        let listed = |tip: &String| looked.tip.as_ref().is_some_and(|(known, _)| known == tip);
        tips_since.retain(|tip, _| listed(tip));
        started.retain(|tip| listed(tip));
        if looked.tip.is_some() && repo.is_none() {
            match codehost::of_project(&core, &project).await {
                Ok(found) => {
                    if let Some(wait) = core.collisions.state.locked().checks.ci.get_mut(&project) {
                        wait.repo = Some(found.watched());
                    }
                    crate::server::codehost::ci::start(&core);
                    repo = Some(found);
                }
                Err(error) => {
                    let why = format!("its CI can't be read: {}", String::from(error));
                    if let Some((tip, held)) = &looked.tip {
                        ruled.push(Ruled {
                            held: held.clone(),
                            tip: Some(tip.clone()),
                            outcome: Err(why),
                        });
                    }
                }
            }
        }
        let mut credited: HashSet<String> =
            ruled.iter().flat_map(|ruled| ruled.held.clone()).collect();
        let unruled = looked.tip.as_ref().and_then(|(tip, held)| {
            let held: Vec<String> = held
                .iter()
                .filter(|sha| !credited.contains(*sha))
                .cloned()
                .collect();
            let repo = repo.as_ref().filter(|_| !held.is_empty())?;
            Some((tip, held, repo))
        });
        if let Some((tip, held, repo)) = unruled {
            let since = *tips_since.entry(tip.clone()).or_insert(now);
            let overdue = now.saturating_sub(since) >= CI_START_WAIT_MS;
            let checks = codehost::commit_checks(repo, None, tip, &workflows).await;
            match &checks {
                Ok(CiChecks::Passed) => crate::server::board::ci_ruled(&core, &project, tip, &[]),
                Ok(CiChecks::Failed(_)) => {
                    let failed = codehost::failed_jobs(repo, tip, &workflows).await;
                    crate::server::board::ci_ruled(&core, &project, tip, &failed);
                }
                _ => {}
            }
            let outcome = match checks {
                Ok(CiChecks::Passed) => Some(Ok((true, format!("{label} passed on {tip}")))),
                Ok(CiChecks::Failed(what)) => Some(Ok((false, what))),
                Ok(CiChecks::Stopped(why)) => Some(Err(format!(
                    "its CI runs were cancelled or ended without a verdict:\n{why}"
                ))),
                Ok(CiChecks::Running { workflow, url }) => {
                    if started.insert(tip.clone()) {
                        let mut covered = held.clone();
                        covered.push(tip.clone());
                        core.repo_events.tell(RepoEvent::CheckStarted {
                            project: project.clone(),
                            covered,
                            run: CheckRun::Ci { workflow, url },
                        });
                    }
                    None
                }
                Ok(CiChecks::None) if overdue => Some(Err(format!(
                    "no run of {} started on {}",
                    workflows.join(", "),
                    tip.get(..7).unwrap_or(tip)
                ))),
                Err(error) if overdue => Some(Err(format!("its CI can't be read: {error}"))),
                Ok(CiChecks::None) | Err(_) => None,
            };
            if let Some(outcome) = outcome {
                credited.extend(held.iter().cloned());
                ruled.push(Ruled {
                    held,
                    tip: Some(tip.clone()),
                    outcome,
                });
            }
        }
        for Ruled { held, tip, outcome } in ruled {
            if tip.is_none() {
                let mut state = core.collisions.state.locked();
                for commit in commits.iter().filter(|commit| held.contains(&commit.sha)) {
                    for agent in &commit.agents {
                        tell(
                            &mut state,
                            agent,
                            format!(
                                "Your commit {} ({}) is on no branch any more, as after a rebase or an amend, so CI will never check it. Commit and push the work again to have it checked.",
                                commit.sha.get(..7).unwrap_or(&commit.sha),
                                commit.subject
                            ),
                        );
                    }
                }
            }
            let mut checked: Vec<Commit> = commits
                .iter()
                .filter(|commit| held.contains(&commit.sha))
                .cloned()
                .collect();
            if let Some(tip) = tip.filter(|tip| !held.contains(tip)) {
                checked.push(Commit {
                    sha: tip,
                    subject: String::new(),
                    agents: Vec::new(),
                });
            }
            ended(&core, &project, &label, &checked, outcome);
        }
        commits.retain(|commit| !credited.contains(&commit.sha));
        if commits.is_empty() {
            continue;
        }
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep(CI_LOOK) => {}
        }
    }
    let mut state = core.collisions.state.locked();
    state.checks.ci.remove(&project);
    state.checks.running.remove(&project);
    drop(state);
    core.collisions.wake.now();
}

/// The newest of `commits` by ancestry, last, with those of them it holds,
/// which its check is for; and the others, which wait for a check of their own.
fn newest_with(dir: &Path, commits: Vec<Commit>) -> (Vec<Commit>, Vec<Commit>) {
    let holds = |newer: &str, commit: &str| {
        commit == newer
            || Git::quick(dir)
                .output(&["merge-base", "--is-ancestor", commit, newer])
                .is_ok_and(|output| output.status.success())
    };
    let Some(mut newest) = commits.first().map(|commit| commit.sha.clone()) else {
        return (Vec::new(), Vec::new());
    };
    for commit in &commits[1..] {
        if holds(&commit.sha, &newest) {
            newest.clone_from(&commit.sha);
        }
    }
    let (mut held, others): (Vec<Commit>, Vec<Commit>) = commits
        .into_iter()
        .partition(|commit| holds(&newest, &commit.sha));
    held.sort_by_key(|commit| commit.sha == newest);
    (held, others)
}

fn ended(
    core: &Arc<Core>,
    project: &str,
    command: &str,
    commits: &[Commit],
    outcome: Result<(bool, String), String>,
) {
    let collisions = &core.collisions;
    let agents: BTreeSet<&String> = commits.iter().flat_map(|commit| &commit.agents).collect();
    let mut state = collisions.state.locked();
    let checked = |verdict, output| RepoEvent::Checked {
        project: project.to_owned(),
        command: command.to_owned(),
        sha: commits.last().map(|commit| commit.sha.clone()),
        covered: commits.iter().map(|commit| commit.sha.clone()).collect(),
        verdict,
        output,
    };
    let (passed, output) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("sikemux core: the check after a commit did not run: {error}");
            drop(state);
            core.repo_events.tell(checked(None, error));
            collisions.wake.now();
            return;
        }
    };
    let mut person_at = None;
    if passed {
        state.checks.failing.remove(project);
    } else {
        let now = epoch_ms();
        let failing = state
            .checks
            .failing
            .entry(project.to_owned())
            .or_insert_with(|| Failing {
                project: project.to_owned(),
                since: now,
                at: now,
                runs: 0,
                agents: Vec::new(),
                output: String::new(),
            });
        failing.runs += 1;
        failing.at = now;
        failing.output = output.clone();
        person_at = Some(failing.since + PERSON_AFTER_MS);
        for agent in &agents {
            if !failing.agents.contains(agent) {
                failing.agents.push((*agent).clone());
            }
        }
        let newest = commits.last().map_or("", |commit| commit.sha.as_str());
        for agent in &agents {
            let yours: Vec<String> = commits
                .iter()
                .filter(|commit| commit.agents.contains(agent))
                .map(|commit| {
                    format!(
                        "{} ({})",
                        commit.sha.get(..7).unwrap_or(&commit.sha),
                        commit.subject
                    )
                })
                .collect();
            tell(
                &mut state,
                agent,
                format!(
                    "The project's check, {command}, failed on {} after your commit {}. It checked only what was committed, so the failure is in what the branch holds now: mend it with a commit.\n\n{output}",
                    newest.get(..7).unwrap_or(newest),
                    yours.join(", ")
                ),
            );
        }
    }
    drop(state);
    let verdict = if passed {
        Verdict::Passed
    } else {
        Verdict::Failed
    };
    core.repo_events.tell(checked(Some(verdict), output));
    core.harness.note_repository(
        project,
        if passed {
            "check.passed"
        } else {
            "check.failed"
        },
    );
    collisions.wake.now();
    if let Some(at) = person_at.filter(|at| *at > epoch_ms()) {
        collisions.wake.at(at);
    }
}
