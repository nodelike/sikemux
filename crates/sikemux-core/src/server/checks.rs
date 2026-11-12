//! The project's check, run on each commit as committed, in the repository's
//! integration checkout, apart from the work in progress in everyone's own
//! trees, once the files it generates are brought up to date. A failure goes to the agents whose commits it failed after, at
//! once, and to the person once the project has stayed broken a while. It is
//! the board's check; a project without one is not checked. This module owns
//! choosing the checker, the queue, running, and waiting on CI. Commits that land
//! while a check runs are checked together, at the newest, once it ends. A
//! project whose check is its CI is not built here: the code host's runs on
//! the pushed branch tips that hold its commits give the verdict.

mod culprit;
mod report;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use tokio::sync::Notify;

use super::changed::What;
use super::wake::Wake;
use crate::git::{all_said, tail, Git};
use crate::server::codehost::ci::Shown;
use crate::server::codehost::repo::{self as codehost, Checks as CiChecks, CI_START_WAIT_MS};
use crate::server::repo_events::RepoEvent;
use crate::server::Core;
use crate::tasks::config::ProjectCheck;
use culprit::{Hunted, Search};
use sikemux_view::slice::{CheckRun, Verdict};

/// How long a check may run, and how much of what it says is kept.
const RUN_LIMIT: Duration = Duration::from_secs(30 * 60);
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// Checks running at once across projects, one per this many cores.
const CORES_PER_CHECK: usize = 2;
const MIN_RUNNING: usize = 2;
const MAX_RUNNING: usize = 8;
/// The environment variable a check reads to pick a sample that is the same on
/// each of an agent's runs and differs between agents.
const SEED_VARIABLE: &str = "SIKEMUX_CHECK_SEED";
/// A full check waits at least this many times its own length after the last,
/// so it fills at most about a quarter of a project's checking time.
const FULL_GAP_FACTOR: u64 = 3;
const MIN_FULL_GAP_MS: u64 = 5 * 60_000;
const MAX_FULL_GAP_MS: u64 = 60 * 60_000;
/// How long a project stays broken before the person hears of it, so its
/// agents have time to mend it first.
pub(crate) const PERSON_AFTER_MS: u64 = 15 * 60_000;
/// Commits kept as checked per project, so the same one is not run twice.
const MAX_CHECKED: usize = 64;
/// How often the commits waiting on CI are looked at when nothing says their
/// refs or runs moved.
const CI_LOOK: std::time::Duration = std::time::Duration::from_secs(300);
/// How long a commit waiting on CI may be on no branch, as while a rebase
/// runs, before it counts as gone.
const GONE_AFTER_MS: u64 = 10 * 60_000;

#[derive(Clone)]
pub(crate) struct Commit {
    pub sha: String,
    pub subject: String,
    pub agents: Vec<String>,
    /// A slice asked for it to be checked, rather than only its landing.
    pub asked: bool,
}

/// What proves a change sound in a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Checker {
    /// The check its `sikemux.json` names, run on each commit on its branch.
    Named(ProjectCheck),
    /// The test command its build files suggest, for a project that names
    /// none, run only on a slice's work.
    Found(String),
}

impl Checker {
    pub(crate) fn command(&self) -> Option<&str> {
        match self {
            Checker::Named(ProjectCheck::Command(command)) | Checker::Found(command) => {
                Some(command)
            }
            Checker::Named(ProjectCheck::Ci { .. }) => None,
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Checker::Named(check) => check.label(),
            Checker::Found(command) => format!("`{command}`"),
        }
    }
}

#[derive(Default)]
pub(crate) struct ProjectChecks {
    state: Mutex<Checks>,
    /// Starts the checks that wait, whenever one is added or ends.
    pub(crate) wake: Wake,
    /// The test command each project's build files suggest, read once.
    found: Mutex<HashMap<String, Option<String>>>,
    /// Lets one culprit search step run at a time across projects.
    hunt_turn: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Checks {
    /// Told to agents once the lock is let go of.
    notes: Vec<(String, String)>,
    /// The person's list of broken projects changed, so devices are sent the view again.
    publish: bool,
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
    /// The commits whose check waits for the person to trust the project's `sikemux.json`.
    awaiting_trust: HashMap<String, Vec<Commit>>,
    /// How each project's checks have gone, which decides what runs next.
    standing: HashMap<String, Standing>,
    /// The projects searching for the commit that broke them, which take none of the slots.
    hunting: HashSet<String>,
    /// Commits CI failed on, by project, with when: a later passing run that holds one stands
    /// for it too, since the failure may have been another commit's.
    failed_on_ci: HashMap<String, Vec<(Commit, u64)>>,
}

/// Which of a project's two checks a run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    Full,
    Fast,
}

impl Tier {
    fn word(self) -> &'static str {
        match self {
            Tier::Full => "full",
            Tier::Fast => "fast",
        }
    }
}

#[derive(Default)]
struct Standing {
    dir: Option<PathBuf>,
    /// When a check of it last started, so the longest kept waiting goes first.
    started_at: u64,
    /// When its last full check ended, and how long that took.
    full: Option<(u64, u64)>,
    /// The newest commit that passed its full check.
    full_sound: Option<String>,
    /// The newest commit that passed either check.
    sound: Option<String>,
    /// Commits only its fast check has run on, oldest first.
    only_fast: Vec<Commit>,
    /// When to try again a full check that did not run, with no new commit to set it off.
    retry_full_at: u64,
}

impl Standing {
    fn full_due_at(&self) -> u64 {
        self.full.map_or(0, |(ended, took)| {
            ended + (took * FULL_GAP_FACTOR).clamp(MIN_FULL_GAP_MS, MAX_FULL_GAP_MS)
        })
    }

    fn ran(&mut self, tier: Tier, sha: &str, passed: bool, took: u64, commits: &[Commit]) {
        match tier {
            Tier::Full => {
                self.full = Some((epoch_ms(), took));
                if passed {
                    self.full_sound = Some(sha.to_owned());
                }
            }
            Tier::Fast => {
                self.only_fast.extend_from_slice(commits);
                let over = self.only_fast.len().saturating_sub(MAX_CHECKED);
                self.only_fast.drain(..over);
            }
        }
        if passed {
            self.sound = Some(sha.to_owned());
        }
    }
}

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
    /// The one agent mending it.
    #[serde(skip)]
    owner: Option<String>,
    /// Agents told who owns it, so each hears that once.
    #[serde(skip)]
    told: BTreeSet<String>,
    /// Its full check failed, which only a full check can clear.
    #[serde(skip)]
    full: bool,
}

impl Checks {
    /// Queues `commit` of `project` to be checked, unless it was already.
    pub(crate) fn wait(&mut self, project: String, dir: PathBuf, commit: Commit) {
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
    pub(crate) fn wait_again(&mut self, project: String, dir: PathBuf, commits: Vec<Commit>) {
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

    pub(crate) fn ci_repos(&self) -> Vec<Shown> {
        self.ci
            .values()
            .filter_map(|wait| wait.repo.clone())
            .collect()
    }

    /// The runs on `repo` moved, so the CI checks waiting on it look again.
    pub(crate) fn ci_moved(&self, repo: &Shown) {
        for wait in self.ci.values() {
            if wait.repo.as_ref() == Some(repo) {
                wait.wake.notify_one();
            }
        }
    }

    /// `project`'s refs moved, as a push moves them, so its CI check looks again.
    pub(crate) fn refs_moved(&self, project: &str) {
        if let Some(wait) = self.ci.get(project) {
            wait.wake.notify_one();
        }
    }

    /// The projects broken long enough that the person should hear of it.
    pub(crate) fn failing(&self) -> impl Iterator<Item = &Failing> {
        let now = epoch_ms();
        self.failing
            .values()
            .filter(move |failing| failing.since + PERSON_AFTER_MS <= now)
    }

    /// The checks that may start now, taken off the waiting list.
    fn startable(&mut self) -> Vec<(String, PathBuf, Vec<Commit>)> {
        self.queue_due_full_checks();
        let mut started = Vec::new();
        let mut ready: Vec<String> = self
            .waiting
            .keys()
            .filter(|project| !self.running.contains_key(*project))
            .cloned()
            .collect();
        let last_started = |project: &String| {
            self.standing
                .get(project)
                .map_or(0, |standing| standing.started_at)
        };
        ready.sort_by_key(last_started);
        let limit = most_running();
        for project in ready {
            if self.slots_taken() >= limit {
                break;
            }
            if let Some((dir, commits)) = self.waiting.remove(&project) {
                let shas = commits.iter().map(|commit| commit.sha.clone()).collect();
                self.running.insert(project.clone(), shas);
                self.standing.entry(project.clone()).or_default().started_at = epoch_ms();
                started.push((project, dir, commits));
            }
        }
        started
    }

    fn slots_taken(&self) -> usize {
        self.running
            .len()
            .saturating_sub(self.ci.len() + self.hunting.len())
    }

    /// Queues the commits only a fast check has covered, of each project idle now whose
    /// full check is due, so one runs even when no more commits come.
    fn queue_due_full_checks(&mut self) {
        let now = epoch_ms();
        let due: Vec<String> = self
            .standing
            .iter()
            .filter(|(project, standing)| {
                !standing.only_fast.is_empty()
                    && standing.full_due_at().max(standing.retry_full_at) <= now
                    && !self.waiting.contains_key(*project)
                    && !self.running.contains_key(*project)
            })
            .map(|(project, _)| project.clone())
            .collect();
        for project in due {
            let Some(standing) = self.standing.get_mut(&project) else {
                continue;
            };
            let Some(dir) = standing.dir.clone() else {
                continue;
            };
            let commits = std::mem::take(&mut standing.only_fast);
            self.waiting.insert(project, (dir, commits));
        }
    }
}

/// Starts the checks that wait, as far as the limit allows, each on the
/// newest of its commits, and has devices told of a project broken long
/// enough for the person to hear of it, or mended since.
pub(crate) fn settle(core: &Arc<Core>) {
    let mut guard = core.checks.state.locked();
    let state = &mut *guard;
    let listed: HashSet<String> = state
        .failing()
        .map(|failing| failing.project.clone())
        .collect();
    if listed != state.shown {
        state.shown = listed;
        state.publish = true;
    }
    for (project, wait) in &state.ci {
        if state.waiting.contains_key(project) {
            wait.wake.notify_one();
        }
    }
    for (project, dir, mut commits) in state.startable() {
        let (command, named) = match project_check(core, &project) {
            Ok(Checker::Named(ProjectCheck::Command(command))) => (command, true),
            Ok(Checker::Named(ProjectCheck::Ci { ci })) => {
                tokio::spawn(wait_for_ci(core.clone(), project, dir, commits, ci));
                continue;
            }
            Ok(Checker::Found(command)) => {
                commits.retain(|commit| commit.asked);
                (command, false)
            }
            Err(_) => {
                state.running.remove(&project);
                continue;
            }
        };
        if commits.is_empty() {
            state.running.remove(&project);
            continue;
        }
        tokio::spawn(run_batch(
            core.clone(),
            project,
            dir,
            commits,
            command,
            named,
        ));
    }
    let notes = std::mem::take(&mut state.notes);
    let publish = std::mem::take(&mut state.publish);
    drop(guard);
    for (to, text) in notes {
        let core = core.clone();
        tokio::spawn(async move {
            if let Err(error) = super::messages::from_sikemux(&core, to, text).await {
                eprintln!("sikemux core: a note about a check did not reach its agent: {error}");
            }
        });
    }
    if publish {
        core.changed(What::View);
    }
}

/// How many checks may run at once across projects, by the size of the machine.
fn most_running() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    (cores / CORES_PER_CHECK).clamp(MIN_RUNNING, MAX_RUNNING)
}

/// Which check to run on `commits`, with the commits that run takes in: its fast check, unless
/// its full check is due or failing.
fn choose_tier(
    core: &Core,
    project: &str,
    dir: &Path,
    commits: Vec<Commit>,
    full: String,
    fast: Option<String>,
) -> (Tier, String, Vec<Commit>) {
    let mut state = core.checks.state.locked();
    let broken_in_full = state
        .failing
        .get(project)
        .is_some_and(|failing| failing.full);
    let standing = state.standing.entry(project.to_owned()).or_default();
    standing.dir = Some(dir.to_owned());
    match fast {
        Some(fast) if !broken_in_full && standing.full_due_at() > epoch_ms() => {
            (Tier::Fast, fast, commits)
        }
        _ => {
            let mut taken = std::mem::take(&mut standing.only_fast);
            taken.retain(|old| commits.iter().all(|commit| commit.sha != old.sha));
            taken.extend(commits);
            (Tier::Full, full, taken)
        }
    }
}

/// A number stable for `agent` and different between agents, for a check to sample by.
fn sample_seed(agent: &str) -> u32 {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(agent.as_bytes());
    u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]])
}

/// What came of a batch's check in the integration checkout.
enum BatchRun {
    Ran(Ran),
    Generated(String),
}

async fn run_batch(
    core: Arc<Core>,
    project: String,
    dir: PathBuf,
    commits: Vec<Commit>,
    full: String,
    named: bool,
) {
    let ordering = dir.clone();
    let (commits, later) = tokio::task::spawn_blocking(move || newest_with(&ordering, commits))
        .await
        .unwrap_or_default();
    if commits.is_empty() {
        core.checks.state.locked().running.remove(&project);
        core.checks.wake.now();
        return;
    }
    let fast = named.then(|| fast_check(&project)).flatten();
    let (tier, command, commits) = choose_tier(&core, &project, &dir, commits, full, fast);
    let Some(newest) = commits.last().map(|commit| commit.sha.clone()) else {
        core.checks.state.locked().running.remove(&project);
        core.checks.wake.now();
        return;
    };
    let (sound, seed) = {
        let mut state = core.checks.state.locked();
        let shas: Vec<String> = commits.iter().map(|commit| commit.sha.clone()).collect();
        state.running.insert(project.clone(), shas.clone());
        state.wait_again(project.clone(), dir.clone(), later);
        core.repo_events.tell(RepoEvent::CheckStarted {
            project: project.clone(),
            covered: shas,
            run: CheckRun::Local,
        });
        let standing = state.standing.entry(project.clone()).or_default();
        let sound = match tier {
            Tier::Full => standing.full_sound.clone(),
            Tier::Fast => standing.sound.clone(),
        };
        let author = commits
            .last()
            .and_then(|commit| commit.agents.first())
            .map_or("", String::as_str);
        (sound, sample_seed(author))
    };
    let agents: Vec<String> = commits
        .iter()
        .flat_map(|commit| commit.agents.iter().cloned())
        .collect();
    let outcome = match crate::server::worktree::agent::integration(&core, &dir, &newest).await {
        Ok(checkout) if needs_trust(&core, &project, &dir, &checkout, &commits, named).await => {
            Err("sikemux.json is not trusted yet, so the check did not run. The person is asked to trust it, and the check runs again if they do".to_owned())
        }
        Ok(checkout) => {
            let (owner, command, newest, dir) =
                (core.clone(), command.clone(), newest.clone(), dir.clone());
            let agents = agents.clone();
            tokio::task::spawn_blocking(move || {
                use crate::server::edits::generate::regenerate;
                match regenerate(&owner, &dir, &checkout, &newest, &agents) {
                    Ok(Some(generated)) => Ok(BatchRun::Generated(generated)),
                    Ok(None) => run_check(&checkout, &command, seed, RUN_LIMIT).map(BatchRun::Ran),
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
    let ran = match outcome {
        Ok(BatchRun::Generated(generated)) => {
            let mut state = core.checks.state.locked();
            state.running.remove(&project);
            let mut again = commits;
            again.push(Commit {
                sha: generated,
                subject: "the files the project generates".to_owned(),
                agents,
                asked: true,
            });
            state.wait_again(project, dir, again);
            drop(state);
            core.checks.wake.now();
            return;
        }
        Ok(BatchRun::Ran(ran)) => ran,
        Err(error) => {
            {
                let mut state = core.checks.state.locked();
                state.running.remove(&project);
                if tier == Tier::Full {
                    let standing = state.standing.entry(project.clone()).or_default();
                    standing.retry_full_at = epoch_ms() + MIN_FULL_GAP_MS;
                    standing.only_fast = commits.clone();
                }
            }
            ended(&core, &project, &command, &commits, Err(error), named, tier);
            return;
        }
    };
    let was_broken = {
        let mut state = core.checks.state.locked();
        state.standing.entry(project.clone()).or_default().ran(
            tier,
            &newest,
            ran.passed,
            ran.took_ms,
            &commits,
        );
        state.failing.contains_key(&project)
    };
    if ran.passed || !named {
        core.checks.state.locked().running.remove(&project);
        crate::server::board::record_check(
            &core,
            &project,
            &check_ran(&commits, &ran, &Hunted::NotTried),
        );
        ended(
            &core,
            &project,
            &command,
            &commits,
            Ok((ran.passed, tail(&ran.text))),
            named,
            tier,
        );
        if tier == Tier::Fast {
            let due =
                (core.checks.state.locked().standing.get(&project)).map(Standing::full_due_at);
            if let Some(at) = due {
                core.checks.wake.at(at);
            }
        }
        return;
    }
    let hunts = !was_broken;
    {
        let mut state = core.checks.state.locked();
        state.running.insert(project.clone(), Vec::new());
        if hunts {
            state.hunting.insert(project.clone());
        }
    }
    ended(
        &core,
        &project,
        &command,
        &commits,
        Ok((false, tail(&ran.text))),
        true,
        tier,
    );
    let hunted = match hunts {
        true => {
            culprit::hunt(
                &core,
                Search {
                    dir: dir.clone(),
                    command: command.clone(),
                    seed,
                    sound,
                    commits: commits.clone(),
                    failing_took: ran.took_ms,
                    in_queue: false,
                },
            )
            .await
        }
        false => Hunted::NotTried,
    };
    let (keeping, sha, tier_word, text) =
        (dir.clone(), newest.clone(), tier.word(), ran.text.clone());
    let log =
        tokio::task::spawn_blocking(move || report::keep_log(&keeping, &sha, tier_word, &text))
            .await
            .ok()
            .flatten();
    let digest = report::digest(&ran.text, log.as_deref());
    blame(&core, &project, &command, &commits, &digest, &hunted).await;
    crate::server::board::record_check(&core, &project, &check_ran(&commits, &ran, &hunted));
    {
        let mut state = core.checks.state.locked();
        state.running.remove(&project);
        state.hunting.remove(&project);
    }
    core.checks.wake.now();
}

fn check_ran<'a>(
    commits: &'a [Commit],
    ran: &'a Ran,
    hunted: &'a Hunted,
) -> crate::server::board::CheckRan<'a> {
    let (culprit_agents, found, steps): (&[String], bool, u32) = match hunted {
        Hunted::Found { agents, steps, .. } => (agents, true, *steps),
        Hunted::Unknown { steps, .. } => (&[], false, *steps),
        Hunted::NotTried => (&[], false, 0),
    };
    crate::server::board::CheckRan {
        took_ms: ran.took_ms,
        passed: ran.passed,
        authors: commits
            .iter()
            .flat_map(|commit| commit.agents.iter())
            .collect(),
        culprit_agents,
        found,
        searched: !matches!(hunted, Hunted::NotTried),
        steps,
        text: &ran.text,
    }
}

/// Whether the check of `commits` stops at a `sikemux.json` the person has not trusted: the
/// project's, for the check it names, or `checkout`'s, for its generate command. They are
/// asked, once at a time, and those commits are checked again if they trust it.
async fn needs_trust(
    core: &Arc<Core>,
    project: &str,
    dir: &Path,
    checkout: &Path,
    commits: &[Commit],
    named: bool,
) -> bool {
    let reading = {
        let (owner, dir, checkout) = (core.clone(), dir.to_owned(), checkout.to_owned());
        tokio::task::spawn_blocking(move || {
            let check = named
                .then(|| {
                    let load = crate::tasks::config::load(&dir);
                    super::tasks::config_needs(&owner, &load)
                })
                .flatten();
            check.or_else(|| crate::server::edits::generate::trust_needed(&owner, &dir, &checkout))
        })
        .await
    };
    let Ok(Some(ask)) = reading else {
        return false;
    };
    let first = {
        let mut state = core.checks.state.locked();
        let waiting = state.awaiting_trust.entry(project.to_owned()).or_default();
        waiting.extend_from_slice(commits);
        waiting.len() == commits.len()
    };
    if first {
        let (core, project, dir) = (core.clone(), project.to_owned(), dir.to_owned());
        tokio::spawn(async move {
            let trusted = super::tasks::ask_person(&core, &project, ask, None).await;
            let mut state = core.checks.state.locked();
            let commits = state.awaiting_trust.remove(&project).unwrap_or_default();
            if trusted {
                state.wait_again(project, dir, commits);
            }
            drop(state);
            core.checks.wake.now();
        });
    }
    true
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
/// Whether the project's pushed branch has moved past `tip`. Its CI run may then have been
/// cancelled only because a newer push took its place, and the newer tip's run rules instead.
async fn tip_moved(dir: &Path, tip: &str) -> bool {
    let (dir, tip) = (dir.to_owned(), tip.to_owned());
    tokio::task::spawn_blocking(move || {
        let Some(upstream) = crate::git::upstream(&dir) else {
            return false;
        };
        Git::quick(&dir)
            .trimmed(&["rev-parse", &upstream])
            .is_ok_and(|now| now != tip)
    })
    .await
    .unwrap_or(false)
}

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
    core.checks.state.locked().ci.insert(
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
            let mut state = core.checks.state.locked();
            if let Some((_, more)) = state.waiting.remove(&project) {
                for commit in more {
                    if commits.iter().all(|kept| kept.sha != commit.sha) {
                        commits.push(commit);
                    }
                }
            }
            let shas = commits.iter().map(|commit| commit.sha.clone()).collect();
            state.running.insert(project.clone(), shas);
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
                    crate::git::short(sha)
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
                    if let Some(wait) = core.checks.state.locked().ci.get_mut(&project) {
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
                    if let Some(failed) = codehost::failed_jobs(repo, tip, &workflows).await {
                        crate::server::board::ci_ruled(&core, &project, tip, &failed);
                    }
                }
                _ => {}
            }
            let outcome = match checks {
                Ok(CiChecks::Passed) => Some(Ok((true, format!("{label} passed on {tip}")))),
                Ok(CiChecks::Failed(what)) => Some(Ok((false, what))),
                Ok(CiChecks::Stopped(_)) if tip_moved(&dir, tip).await => None,
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
                Ok(CiChecks::None) if overdue && !tip_moved(&dir, tip).await => Some(Err(format!(
                    "no run of {} started on {}",
                    workflows.join(", "),
                    crate::git::short(tip)
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
                let mut state = core.checks.state.locked();
                for commit in commits.iter().filter(|commit| held.contains(&commit.sha)) {
                    for agent in &commit.agents {
                        tell(
                &mut state,
                agent,
                            format!(
                                "Your commit {} ({}) is on no branch any more, as after a rebase or an amend, so CI will never check it. Commit and push the work again to have it checked.",
                                crate::git::short(&commit.sha),
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
            match (&outcome, &tip) {
                (Ok((false, _)), _) => keep_failed(&core, &project, &checked),
                (Ok((true, _)), Some(tip)) => {
                    let mut vouched = vouched_by(&core, &project, &dir, tip).await;
                    for head in slices_held_by(&core, &project, &dir, tip).await {
                        if vouched.iter().all(|kept| kept.sha != head.sha) {
                            vouched.push(head);
                        }
                    }
                    vouched.retain(|old| checked.iter().all(|commit| commit.sha != old.sha));
                    checked.splice(0..0, vouched);
                }
                _ => {}
            }
            if let Some(tip) = tip.filter(|tip| !held.contains(tip)) {
                checked.push(Commit {
                    sha: tip,
                    subject: String::new(),
                    agents: Vec::new(),
                    asked: false,
                });
            }
            let failed_output = match &outcome {
                Ok((false, output)) => Some(report::digest(output, None)),
                _ => None,
            };
            ended(&core, &project, &label, &checked, outcome, true, Tier::Full);
            if let Some(digest) = failed_output {
                blame(
                    &core,
                    &project,
                    &label,
                    &checked,
                    &digest,
                    &Hunted::NotTried,
                )
                .await;
            }
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
    let mut state = core.checks.state.locked();
    state.ci.remove(&project);
    state.running.remove(&project);
    drop(state);
    core.checks.wake.now();
}

/// How long a commit CI failed on waits for a later passing run to stand for it.
const FAILED_KEPT_MS: u64 = 24 * 60 * 60_000;

fn keep_failed(core: &Core, project: &str, commits: &[Commit]) {
    let now = epoch_ms();
    let mut state = core.checks.state.locked();
    let kept = state.failed_on_ci.entry(project.to_owned()).or_default();
    kept.retain(|(_, at)| now.saturating_sub(*at) < FAILED_KEPT_MS);
    kept.extend(commits.iter().cloned().map(|commit| (commit, now)));
}

/// The commits CI failed on that `tip` holds, which its passing run stands for.
async fn vouched_by(core: &Core, project: &str, dir: &Path, tip: &str) -> Vec<Commit> {
    let kept = (core.checks.state.locked().failed_on_ci.remove(project)).unwrap_or_default();
    if kept.is_empty() {
        return Vec::new();
    }
    let (dir, tip) = (dir.to_owned(), tip.to_owned());
    let (held, others): (Vec<_>, Vec<_>) = tokio::task::spawn_blocking(move || {
        kept.into_iter().partition(|(commit, _)| {
            Git::quick(&dir)
                .output(&["merge-base", "--is-ancestor", &commit.sha, &tip])
                .is_ok_and(|output| output.status.success())
        })
    })
    .await
    .unwrap_or_default();
    if !others.is_empty() {
        let mut state = core.checks.state.locked();
        state
            .failed_on_ci
            .entry(project.to_owned())
            .or_default()
            .extend(others);
    }
    held.into_iter().map(|(commit, _)| commit).collect()
}

/// The latest commits of the board's slices that still wait for the check and that `tip` holds,
/// which its passing run stands for, as most commits get no CI run of their own.
async fn slices_held_by(core: &Core, project: &str, dir: &Path, tip: &str) -> Vec<Commit> {
    let heads = crate::server::board::unbuilt_heads(core, project);
    if heads.is_empty() {
        return Vec::new();
    }
    let (dir, tip) = (dir.to_owned(), tip.to_owned());
    tokio::task::spawn_blocking(move || {
        (heads.into_iter())
            .filter(|head| {
                Git::quick(&dir)
                    .output(&["merge-base", "--is-ancestor", head, &tip])
                    .is_ok_and(|output| output.status.success())
            })
            .map(|sha| Commit {
                sha,
                subject: String::new(),
                agents: Vec::new(),
                asked: false,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
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

/// A check ended. Only a check the project named marks the project broken;
/// the board hears of every one.
fn ended(
    core: &Arc<Core>,
    project: &str,
    command: &str,
    commits: &[Commit],
    outcome: Result<(bool, String), String>,
    named: bool,
    tier: Tier,
) {
    let checks = &core.checks;
    let agents: BTreeSet<&String> = commits.iter().flat_map(|commit| &commit.agents).collect();
    let mut state = checks.state.locked();
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
            checks.wake.now();
            return;
        }
    };
    let mut person_at = None;
    if passed {
        let stays_broken = tier == Tier::Fast
            && state
                .failing
                .get(project)
                .is_some_and(|failing| failing.full);
        if !stays_broken {
            state.failing.remove(project);
        }
    } else if named {
        let now = epoch_ms();
        let failing = state
            .failing
            .entry(project.to_owned())
            .or_insert_with(|| Failing {
                project: project.to_owned(),
                since: now,
                at: now,
                runs: 0,
                agents: Vec::new(),
                output: String::new(),
                owner: None,
                told: BTreeSet::new(),
                full: false,
            });
        failing.runs += 1;
        failing.at = now;
        failing.output = output.clone();
        failing.full = tier == Tier::Full;
        person_at = Some(failing.since + PERSON_AFTER_MS);
        for agent in &agents {
            if !failing.agents.contains(agent) {
                failing.agents.push((*agent).clone());
            }
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
    checks.wake.now();
    if let Some(at) = person_at.filter(|at| *at > epoch_ms()) {
        checks.wake.at(at);
    }
}

/// Whether `agent` should be told of `project`'s failing check, the first time, once only.
fn newly_told(core: &Core, project: &str, agent: &str) -> bool {
    core.checks
        .state
        .locked()
        .failing
        .get_mut(project)
        .is_some_and(|failing| failing.told.insert(agent.to_owned()))
}

fn someone(core: &Core, agent: &str) -> String {
    format!("@{}", super::messages::called_now(core, agent))
}

/// Tells who should mend a failing check: the agent whose commit broke it when that is known,
/// else one agent of those it failed after, and the others one line saying who has it.
async fn blame(
    core: &Arc<Core>,
    project: &str,
    command: &str,
    commits: &[Commit],
    digest: &str,
    hunted: &Hunted,
) {
    let mut authors: Vec<String> = Vec::new();
    for commit in commits.iter().rev() {
        for agent in &commit.agents {
            if !authors.contains(agent) {
                authors.push(agent.clone());
            }
        }
    }
    let newest = commits.last().map_or("", |commit| commit.sha.as_str());
    let failed = format!(
        "The project's check, {command}, failed on {}.",
        crate::git::short(newest)
    );
    if let Some(card) = crate::server::board::ci_card(core, project) {
        let holder = card.owner.as_deref().map(|owner| someone(core, owner));
        let who = holder.unwrap_or_else(|| format!("card {}", card.label));
        for agent in authors
            .iter()
            .filter(|agent| card.owner.as_ref() != Some(agent))
        {
            if newly_told(core, project, agent) {
                let text = format!(
                    "{failed} {who} owns mending it, on card {}; leave it to them.",
                    card.label
                );
                send_note(core, agent.clone(), text).await;
            }
        }
        return;
    }
    let owner = core
        .checks
        .state
        .locked()
        .failing
        .get(project)
        .and_then(|failing| failing.owner.clone());
    let owner = match owner {
        Some(owner) if crate::server::messages::reachable(core, &owner).await => Some(owner),
        _ => None,
    };
    if let Some(owner) = owner {
        if authors.contains(&owner) {
            let text = format!(
                "{failed} It is still yours to mend, as your commit did not fix it.\n{digest}"
            );
            send_work(core, owner.clone(), text).await;
        }
        tell_others(core, project, &failed, &owner, &authors).await;
        return;
    }
    let culprit_agents: &[String] = match hunted {
        Hunted::Found { agents, .. } => agents,
        _ => &[],
    };
    let mut candidates: Vec<&String> = culprit_agents.iter().collect();
    candidates.extend(
        authors
            .iter()
            .filter(|agent| !culprit_agents.contains(agent)),
    );
    let mut elected = None;
    for candidate in candidates {
        if crate::server::messages::reachable(core, candidate).await {
            elected = Some(candidate.clone());
            break;
        }
    }
    let Some(owner) = elected else {
        return;
    };
    {
        let mut state = core.checks.state.locked();
        if let Some(failing) = state.failing.get_mut(project) {
            failing.owner = Some(owner.clone());
            failing.told.insert(owner.clone());
        }
    }
    let why = match hunted {
        Hunted::Found { sha, subject, agents, .. } if agents.contains(&owner) => format!(
            "Your commit {} ({subject}) broke it: the commit before it passes. Mend it with a commit.",
            crate::git::short(sha)
        ),
        Hunted::Found { sha, subject, .. } => format!(
            "Commit {} ({subject}) broke it and whoever wrote it can't be reached, so it is yours to mend with a commit.",
            crate::git::short(sha)
        ),
        Hunted::Unknown { why, .. } => format!(
            "It came after your commits, and no one commit could be found to have broken it ({why}), so it is yours to mend with a commit."
        ),
        Hunted::NotTried => "It came after your commits, so it is yours to mend with a commit.".to_owned(),
    };
    let text = format!("{failed} {why}\n{digest}");
    send_work(core, owner.clone(), text).await;
    if !matches!(hunted, Hunted::Found { agents, .. } if agents.contains(&owner)) {
        tell_others(core, project, &failed, &owner, &authors).await;
    }
}

async fn tell_others(
    core: &Arc<Core>,
    project: &str,
    failed: &str,
    owner: &str,
    authors: &[String],
) {
    let name = someone(core, owner);
    for agent in authors.iter().filter(|agent| agent.as_str() != owner) {
        if newly_told(core, project, agent) {
            let text = format!("{failed} {name} owns mending it; leave it to them.");
            send_note(core, agent.clone(), text).await;
        }
    }
}

async fn send_work(core: &Arc<Core>, to: String, text: String) {
    if let Err(error) =
        super::messages::work_from_sikemux(core, to, text, "over the project's check").await
    {
        eprintln!("sikemux core: a failed check did not reach the agent mending it: {error}");
    }
}

async fn send_note(core: &Arc<Core>, to: String, text: String) {
    if let Err(error) = super::messages::from_sikemux(core, to, text).await {
        eprintln!("sikemux core: a note about a check did not reach its agent: {error}");
    }
}

/// Why a project has no check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NoCheck {
    /// Its `sikemux.json` names none or turns it off, and its build files suggest none.
    Unnamed,
    /// Its `sikemux.json` is invalid, so whatever check it names is unread; says what is wrong.
    Invalid(String),
}

/// What proves a change sound in `project`: the `check` its `sikemux.json`
/// names, which runs once the person trusts that file, else the test command
/// its build files suggest, unless the file turns that off with `"check": false`.
pub(crate) fn project_check(core: &Core, project: &str) -> Result<Checker, NoCheck> {
    use crate::tasks::config::{ConfigLoad, Stated};
    let stated = match crate::tasks::config::load(Path::new(project)) {
        ConfigLoad::Valid { config, .. } => config.check,
        ConfigLoad::Absent { .. } => None,
        ConfigLoad::Invalid { errors, .. } => {
            return Err(NoCheck::Invalid(crate::tasks::config::invalid_words(
                &errors,
            )))
        }
    };
    match stated {
        Some(Stated::Off) => Err(NoCheck::Unnamed),
        Some(Stated::Given(check)) => Ok(Checker::Named(check)),
        None => found_check(core, project)
            .map(Checker::Found)
            .ok_or(NoCheck::Unnamed),
    }
}

fn found_check(core: &Core, project: &str) -> Option<String> {
    if let Some(found) = core.checks.found.locked().get(project) {
        return found.clone();
    }
    let found = crate::tasks::detect::test_check(Path::new(project));
    core.checks
        .found
        .locked()
        .insert(project.to_owned(), found.clone());
    found
}

/// `fastCheck` of `project`'s `sikemux.json`, when the fast cadence is on.
fn fast_check(project: &str) -> Option<String> {
    use crate::tasks::config::ConfigLoad;
    match crate::tasks::config::load(Path::new(project)) {
        ConfigLoad::Valid { config, .. } => config.fast_check,
        _ => None,
    }
}

/// How one run of a check went.
pub(super) struct Ran {
    passed: bool,
    text: String,
    took_ms: u64,
}

/// Runs `command` in `cwd` at low priority, with its output in the order written, for `limit`
/// at most.
fn run_check(cwd: &Path, command: &str, seed: u32, limit: Duration) -> Result<Ran, String> {
    let mut shell = sikemux_process::user_environment::background_shell();
    shell
        .args(["-c", &format!("exec 2>&1\n{command}")])
        .env(SEED_VARIABLE, seed.to_string())
        .current_dir(cwd);
    let started = Instant::now();
    let output = sikemux_process::run(&mut shell, None, limit, OUTPUT_LIMIT, None)
        .map_err(|error| format!("it did not run: {error}"))?;
    Ok(Ran {
        passed: output.status.success(),
        text: all_said(&output),
        took_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// How the check went on a commit of the merge queue's checkout.
pub(crate) struct QueueRun {
    pub passed: bool,
    pub output: String,
    pub took_ms: u64,
}

/// Runs `command` on `commit` in the merge queue's checkout, the way the
/// project's own check runs.
pub(crate) async fn run_in_queue(
    core: &Arc<Core>,
    dir: &Path,
    commit: &str,
    command: &str,
    agent: &str,
) -> Result<QueueRun, String> {
    use crate::server::edits::generate::refresh;
    let checkout = crate::server::worktree::agent::merge_queue(core, dir, commit).await?;
    let (owner, dir, command) = (core.clone(), dir.to_owned(), command.to_owned());
    let seed = sample_seed(agent);
    let ran = tokio::task::spawn_blocking(move || {
        refresh(&owner, &dir, &checkout)?;
        run_check(&checkout, &command, seed, RUN_LIMIT)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(QueueRun {
        passed: ran.passed,
        output: ran.text,
        took_ms: ran.took_ms,
    })
}

/// The first of `commits`, oldest first on top of `sound`, that fails
/// `command` in the merge queue's checkout, or why none could be named.
pub(crate) async fn first_failing_in_queue(
    core: &Arc<Core>,
    dir: &Path,
    commits: Vec<String>,
    sound: &str,
    command: &str,
    agent: &str,
    failing_took: u64,
) -> Result<String, String> {
    let commits = commits
        .into_iter()
        .map(|sha| Commit {
            sha,
            subject: String::new(),
            agents: vec![agent.to_owned()],
            asked: false,
        })
        .collect();
    let search = Search {
        dir: dir.to_owned(),
        command: command.to_owned(),
        seed: sample_seed(agent),
        sound: Some(sound.to_owned()),
        commits,
        failing_took,
        in_queue: true,
    };
    match culprit::hunt(core, search).await {
        Hunted::Found { sha, .. } => Ok(sha),
        Hunted::Unknown { why, .. } => Err(why),
        Hunted::NotTried => Err("nothing was searched".to_owned()),
    }
}

/// Runs a check or gate's command in the worktree. Whether it passed, and the
/// end of what it wrote.
pub(crate) fn run_command(cwd: &Path, command: &str) -> Result<(bool, String), String> {
    let mut shell = sikemux_process::user_environment::background_shell();
    shell.args(["-c", command]).current_dir(cwd);
    let output = sikemux_process::run(&mut shell, None, RUN_LIMIT, OUTPUT_LIMIT, None)
        .map_err(|error| format!("it did not run: {error}"))?;
    Ok((output.status.success(), tail(&all_said(&output))))
}

fn tell(state: &mut Checks, to: &str, text: String) {
    state.notes.push((to.to_owned(), text));
}

impl ProjectChecks {
    /// `commit` of `project` was made on its branch, to be checked with the agents it names.
    pub(crate) fn wait_for(&self, project: String, dir: PathBuf, commit: Commit) {
        self.state.locked().wait(project, dir, commit);
        self.wake.now();
    }

    /// Has the check run on `sha` of `project` again, for `agent`, unless a running check takes
    /// it in: for work whose earlier check did not run or was not heard.
    pub(crate) fn check_again(&self, project: &str, sha: &str, subject: String, agent: &str) {
        self.state.locked().wait_again(
            project.to_owned(),
            PathBuf::from(project),
            vec![Commit {
                sha: sha.to_owned(),
                subject,
                agents: vec![agent.to_owned()],
                asked: true,
            }],
        );
        self.wake.now();
    }

    /// The agent mending `project`'s failing check, when one was given it.
    pub(crate) fn failure_owner(&self, project: &str) -> Option<String> {
        self.state.locked().failing.get(project)?.owner.clone()
    }

    /// Whether a check of `project` waits to take in `sha`, or is taking it in.
    pub(crate) fn queued(&self, project: &str, sha: &str) -> bool {
        let state = self.state.locked();
        let waiting = (state.waiting.get(project))
            .is_some_and(|(_, commits)| commits.iter().any(|commit| commit.sha == sha));
        waiting
            || (state.running.get(project))
                .is_some_and(|shas| shas.iter().any(|running| running == sha))
    }

    /// The code-host repositories whose CI a project's check waits on, which the CI watch keeps
    /// checking.
    pub(crate) fn waiting_on_ci(&self) -> Vec<Shown> {
        self.state.locked().ci_repos()
    }

    pub(crate) fn ci_moved(&self, repo: &Shown) {
        self.state.locked().ci_moved(repo);
    }

    pub(crate) fn refs_moved(&self, project: &str) {
        self.state.locked().refs_moved(project);
    }

    /// The projects broken long enough that the person should hear of it.
    pub(crate) fn failing(&self) -> Vec<Failing> {
        self.state.locked().failing().cloned().collect()
    }
}

impl super::part::Part for ProjectChecks {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![super::part::on_wake(core, |core| &core.checks.wake, settle)]
    }
}

/// `project`'s branch is at another commit, by whatever commit: its newest waits for the check,
/// with the agents its footer names, in a project that checks each commit.
pub(crate) fn head_moved(core: &Arc<Core>, project: &str, dir: &Path) {
    if !matches!(project_check(core, project), Ok(Checker::Named(_))) {
        return;
    }
    let Ok(head) = super::edits::git(dir, &["log", "-1", "--format=%H%x00%s"]) else {
        return;
    };
    let Some((sha, subject)) = head.trim_end().split_once('\0') else {
        return;
    };
    let agents = super::edits::footer::verified(core, dir, sha)
        .unwrap_or_default()
        .into_iter()
        .map(|agent| agent.chat)
        .collect();
    let commit = Commit {
        sha: sha.to_owned(),
        subject: subject.to_owned(),
        agents,
        asked: false,
    };
    core.checks
        .wait_for(project.to_owned(), dir.to_owned(), commit);
}
