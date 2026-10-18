//! The project's check, run in the shared checkout after agents commit, so a
//! commit that breaks what another agent's lines rely on is told at once.
//! It is the board's check command; a project without one is not checked.
//! Commits that land while a check runs are checked together once it ends.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use super::{tell, State};
use crate::server::remote::unix_ms;
use crate::server::Core;

/// Checks running at once, across projects.
const MAX_RUNNING: usize = 2;
/// Failed runs in a row before the person hears of it.
pub(super) const KEEPS_FAILING: u32 = 2;

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
    pub(super) fn wait(&mut self, project: String, dir: PathBuf, commit: Commit) {
        self.waiting
            .entry(project)
            .or_insert_with(|| (dir, Vec::new()))
            .1
            .push(commit);
    }

    pub(super) fn failing(&self) -> impl Iterator<Item = &Failing> {
        self.failing
            .values()
            .filter(|failing| failing.runs >= KEEPS_FAILING)
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

/// Starts the checks that wait, as far as the limit allows.
pub(super) fn start(core: &Arc<Core>, state: &mut State) {
    for (project, dir, commits) in state.checks.startable() {
        let Some(command) = crate::server::board::check_command(core, &project) else {
            state.checks.running.remove(&project);
            continue;
        };
        let core = core.clone();
        tokio::task::spawn_blocking(move || {
            let outcome = crate::server::board::run_check(&dir, &command);
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
    let mut state = collisions.lock();
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
    let was_listed = state
        .checks
        .failing
        .get(project)
        .is_some_and(|failing| failing.runs >= KEEPS_FAILING);
    if passed {
        state.checks.failing.remove(project);
    } else {
        let now = unix_ms();
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
        for agent in &agents {
            if !failing.agents.contains(agent) {
                failing.agents.push((*agent).clone());
            }
        }
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
                    "The project's check (`{command}`) failed after your commit {}. The shared checkout also holds other agents' uncommitted work, so see whether the failure comes from your change before you fix it:\n\n{output}",
                    yours.join(", ")
                ),
            );
        }
    }
    let listed = state
        .checks
        .failing
        .get(project)
        .is_some_and(|failing| failing.runs >= KEEPS_FAILING);
    state.publish |= listed || was_listed;
    drop(state);
    collisions.wake.now();
}
