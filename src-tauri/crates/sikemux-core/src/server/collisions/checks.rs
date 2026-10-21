//! The project's check, run on each commit as committed, in the repository's
//! integration checkout, apart from the work in progress in everyone's own
//! trees, once the files it generates are brought up to date. A failure goes to the agents whose commits it failed after, at
//! once, and to the person once the project has stayed broken a while. It is
//! the board's check command; a project without one is not checked. Commits
//! that land while a check runs are checked together, at the newest, once it
//! ends.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::{tell, State};
use crate::server::Core;

/// Checks running at once, across projects.
const MAX_RUNNING: usize = 2;
/// How long a project stays broken before the person hears of it, so its
/// agents have time to mend it first.
pub(super) const PERSON_AFTER_MS: u64 = 15 * 60_000;
/// Commits kept as checked per project, so the same one is not run twice.
const MAX_CHECKED: usize = 64;

pub(super) struct Commit {
    pub sha: String,
    pub subject: String,
    pub agents: Vec<String>,
}

#[derive(Default)]
pub(super) struct Checks {
    waiting: HashMap<String, (PathBuf, Vec<Commit>)>,
    running: HashSet<String>,
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
            .filter(|project| !self.running.contains(*project))
            .cloned()
            .collect();
        for project in ready {
            if self.running.len() >= MAX_RUNNING {
                break;
            }
            if let Some((dir, commits)) = self.waiting.remove(&project) {
                self.running.insert(project.clone());
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
        let (Some(command), Some(newest)) = (
            crate::server::board::check_command(core, &project),
            commits.last().map(|commit| commit.sha.clone()),
        ) else {
            state.checks.running.remove(&project);
            continue;
        };
        let core = core.clone();
        tokio::spawn(async move {
            let agents: Vec<String> = commits
                .iter()
                .flat_map(|commit| commit.agents.iter().cloned())
                .collect();
            let outcome = match crate::server::sandbox::integration(&core, &dir, &newest).await {
                Ok(checkout) => {
                    let (owner, command) = (core.clone(), command.clone());
                    tokio::task::spawn_blocking(move || {
                        use crate::server::edits::generate::regenerate;
                        match regenerate(&owner, &dir, &checkout, &newest, &agents) {
                            Ok(Some(_)) => Ok(None),
                            Ok(None) => {
                                crate::server::board::run_check(&checkout, &command).map(Some)
                            }
                            Err(error) => Ok(Some((false, error))),
                        }
                    })
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
                }
                Err(error) => Err(format!("the integration checkout is not ready: {error}")),
            };
            let outcome = match outcome {
                Ok(Some(outcome)) => Ok(outcome),
                Ok(None) => {
                    core.collisions
                        .state
                        .locked()
                        .checks
                        .running
                        .remove(&project);
                    core.collisions.wake.now();
                    return;
                }
                Err(error) => Err(error),
            };
            ended(&core, &project, &command, &commits, outcome);
        });
    }
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
    let (passed, output) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("sikemux core: the check after a commit did not run: {error}");
            drop(state);
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
