//! The project's check, run on each commit as committed, in the repository's
//! integration checkout, apart from the work in progress in everyone's own
//! trees, once the files it generates are brought up to date. A failure goes to the agents whose commits it failed after, at
//! once, and to the person once the project has stayed broken a while. It is
//! the board's check command; a project without one is not checked. Commits
//! that land while a check runs are checked together, at the newest, once it
//! ends.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::{tell, State};
use crate::git::Git;
use crate::server::repo_events::RepoEvent;
use crate::server::Core;
use sikemux_wire::slice::Verdict;

/// Checks running at once, across projects.
const MAX_RUNNING: usize = 2;
/// How long a project stays broken before the person hears of it, so its
/// agents have time to mend it first.
pub(super) const PERSON_AFTER_MS: u64 = 15 * 60_000;
/// Commits kept as checked per project, so the same one is not run twice.
const MAX_CHECKED: usize = 64;

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
            if self.running.len() >= MAX_RUNNING {
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
    for (project, dir, commits) in state.checks.startable() {
        let Some(command) = crate::server::board::check_command(core, &project) else {
            state.checks.running.remove(&project);
            continue;
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
                let shas = commits.iter().map(|commit| commit.sha.clone()).collect();
                state.checks.running.insert(project.clone(), shas);
                state.checks.wait_again(project.clone(), dir.clone(), later);
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
            ended(&core, &project, &command, &commits, outcome);
        });
    }
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
    state.checks.running.remove(project);
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
                    "The project's check (`{command}`) failed on {}, built as committed, after your commit {}. Only committed code was built, so the failure is in what the branch holds now: mend it with a commit.\n\n{output}",
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
