//! The queue agents land through. `git_commit` with `land: true` puts an
//! agent's commits in line for the branch its checkout was cut from and
//! returns. One base at a time, the core stacks the waiting lands on the
//! branch's tip in order, checks the stack once, and on a pass moves the branch
//! to it, so the branch stays a straight line of commits. Lands that change no
//! Rust are not built; the others are checked with the crates they change. A
//! failed check is traced to the land that broke it, which goes back to its
//! agent as a message while the rest stay in line.

use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::land::{self, NotPut};
use super::owners::git;
use super::push::{line, Signing};
use crate::server::checks;
use crate::server::connection::blocking;
use crate::server::merge_queue::{self, Blame, Lane, Next, Waiter};
use crate::server::repo_events::RepoEvent;
use crate::server::Core;
use sikemux_view::workspace::AgentWorktree;

const ATTEMPTS: usize = 4;
/// Files that change how every crate builds.
const WORKSPACE_FILES: [&str; 5] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
    "clippy.toml",
];

/// The repository and the branch its lands go on.
type Key = (PathBuf, String);

#[derive(Default)]
pub struct LandQueue(Mutex<HashMap<Key, Line>>);

#[derive(Default)]
struct Line {
    lane: Lane,
    waiting: Vec<Waiting>,
}

#[derive(Clone)]
struct Waiting {
    agent: String,
    worktree: AgentWorktree,
    /// The commit its checkout stood on when it asked to land.
    head: String,
    push: bool,
    /// The branch's commit it was tried alone on, while that commit failed its own check.
    failed_over: Option<String>,
}

impl Waiting {
    fn is(&self, other: &Waiting) -> bool {
        self.agent == other.agent && self.head == other.head
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Queued {
    branch: String,
    commits: Vec<String>,
    /// Lands ahead of it in the queue.
    ahead: usize,
    note: String,
}

/// Puts the calling agent's commits in line for the branch its checkout was
/// cut from, refusing at once those that clash with the branch.
pub fn enqueue(core: &Core, agent_id: &str, push: bool) -> Result<Queued, String> {
    let worktree = land::own_checkout(core, agent_id).ok_or("You work in the project's own checkout, so your commits are on its branch already; there is nothing to land")?;
    let base = (worktree.base.clone())
        .ok_or("Your checkout was cut from no branch, so there is nowhere to land it")?;
    let tip = line(git(
        &worktree.repo,
        &["rev-parse", &format!("refs/heads/{base}")],
    )?);
    let head = line(git(&worktree.path, &["rev-parse", "HEAD"])?);
    let laid = land::lay(&Signing::of(core), &worktree.repo, &head, &tip, &base)?
        .ok_or_else(|| format!("Nothing to land: {base} has every commit of yours"))?;
    let key = (worktree.repo.clone(), base.clone());
    let waiting = Waiting {
        agent: agent_id.to_owned(),
        worktree,
        head,
        push,
        failed_over: None,
    };
    let ahead = {
        let mut lines = core.land_queue().0.locked();
        let line = lines.entry(key.clone()).or_default();
        match (line.waiting.iter()).position(|w| w.agent == agent_id) {
            Some(at) => {
                line.waiting[at] = waiting;
                at
            }
            None => {
                line.waiting.push(waiting);
                line.waiting.len() - 1
            }
        }
    };
    wake(core, key);
    let pushed = match push {
        true => " and pushed",
        false => "",
    };
    Ok(Queued {
        note: format!("Queued: Sikemux checks it with the lands beside it, building only the crates they change, and puts it on {base}{pushed} when that passes. If the check fails because of it, it comes back to you as a message; until then your checkout keeps its commits, and you need not run the final checks yourself."),
        branch: base,
        commits: laid.listed,
        ahead,
    })
}

/// Starts the queue for `key` when it is not running.
fn wake(core: &Core, key: Key) {
    let Some(core) = core.arc() else {
        return;
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    {
        let mut lines = core.land_queue().0.locked();
        let Some(line) = lines.get_mut(&key) else {
            return;
        };
        if line.lane.running {
            return;
        }
        line.lane.running = true;
    }
    runtime.spawn(async move {
        while let Some(batch) = next(&core, &key) {
            check_and_land(&core, &key, batch).await;
        }
    });
}

/// The lands the next check takes, or none when the queue stops for now.
fn next(core: &Arc<Core>, key: &Key) -> Option<Vec<Waiting>> {
    let mut lines = core.land_queue().0.locked();
    let line = lines.get_mut(key)?;
    let waiters: Vec<Waiter> = (line.waiting.iter())
        .map(|w| Waiter {
            failed_over: w.failed_over.as_deref(),
            alone: false,
        })
        .collect();
    match line.lane.next(&waiters) {
        Some((at, Next::Take(count))) => Some(line.waiting[at..at + count].to_vec()),
        Some((_, Next::LookAt(at))) => {
            line.lane.running = false;
            let (core, key) = (core.clone(), key.clone());
            tokio::spawn(async move {
                let wait = at.saturating_sub(epoch_ms());
                tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                wake(&core, key);
            });
            None
        }
        None => {
            lines.remove(key);
            None
        }
    }
}

fn with_line<T>(core: &Core, key: &Key, change: impl FnOnce(&mut Line) -> T) -> Option<T> {
    core.land_queue().0.locked().get_mut(key).map(change)
}

fn leave(core: &Core, key: &Key, waiting: &Waiting) {
    with_line(core, key, |line| line.waiting.retain(|w| !w.is(waiting)));
}

/// Takes the land out of the queue and tells its agent why.
fn send_back(core: &Arc<Core>, key: &Key, waiting: &Waiting, why: &str) {
    leave(core, key, waiting);
    let text = format!(
        "Your land onto {} was sent back: {why}\n\nYour commits are still in your checkout. Fix it, commit, and land again.",
        key.1
    );
    tell(core, &waiting.agent, text);
}

fn tell(core: &Arc<Core>, agent: &str, text: String) {
    let (core, agent) = (core.clone(), agent.to_owned());
    tokio::spawn(async move {
        if let Err(error) = (core.repo_above().messages_from_sikemux)(&core, agent, text).await {
            eprintln!("sikemux core: a note about a land did not reach its agent: {error}");
        }
    });
}

/// One land made again on top of the branch and the lands before it.
struct Laid {
    waiting: Waiting,
    /// The commits made for it, oldest first.
    commits: Vec<String>,
    tip: String,
}

struct Stack {
    /// The branch's commit the lands were made again on.
    tip: String,
    laid: Vec<Laid>,
    /// Lands that clash with the branch itself, and why.
    refused: Vec<(Waiting, String)>,
    /// Lands whose commits are all on the branch already.
    done: Vec<Waiting>,
}

impl Stack {
    fn top(&self) -> &str {
        self.laid.last().map_or(&self.tip, |laid| &laid.tip)
    }
}

/// Lays each land of `batch` on the branch's tip and the ones before it. A land
/// that only clashes with another queued land waits for that one to land.
fn stack(signing: &Signing, (repo, base): &Key, batch: &[Waiting]) -> Result<Stack, String> {
    let tip = line(git(repo, &["rev-parse", &format!("refs/heads/{base}")])?);
    let mut stacked = Stack {
        tip: tip.clone(),
        laid: Vec::new(),
        refused: Vec::new(),
        done: Vec::new(),
    };
    for waiting in batch {
        let on = stacked.top().to_owned();
        match land::lay(signing, repo, &waiting.head, &on, base) {
            Ok(Some(laid)) => {
                let range = format!("{on}..{}", laid.tip);
                let commits = git(repo, &["rev-list", "--reverse", &range])?;
                stacked.laid.push(Laid {
                    waiting: waiting.clone(),
                    commits: commits.lines().map(str::to_owned).collect(),
                    tip: laid.tip,
                });
            }
            Ok(None) => stacked.done.push(waiting.clone()),
            Err(why) if on == tip => stacked.refused.push((waiting.clone(), why)),
            Err(_) => {}
        }
    }
    Ok(stacked)
}

/// Stacks the lands in `batch`, checks them together and lands them on a pass.
async fn check_and_land(core: &Arc<Core>, key: &Key, batch: Vec<Waiting>) {
    let stacked = {
        let (signing, key, batch) = (Signing::of(core), key.clone(), batch.clone());
        blocking(move || stack(&signing, &key, &batch).map_err(Into::into)).await
    };
    let stacked = match stacked {
        Ok(stacked) => stacked,
        Err(error) => {
            for waiting in &batch {
                send_back(core, key, waiting, &format!("stacking it failed: {error}"));
            }
            return;
        }
    };
    for waiting in &stacked.done {
        leave(core, key, waiting);
    }
    for (waiting, why) in &stacked.refused {
        send_back(core, key, waiting, why);
    }
    if stacked.laid.is_empty() {
        return;
    }
    let tip = stacked.tip.clone();
    let red = with_line(core, key, |line| {
        let red = line.lane.red_tip().map(str::to_owned);
        if red.as_deref().is_some_and(|red| red != tip) {
            line.lane.clear_red();
        }
        red
    })
    .flatten();
    let repairing = red.as_deref() == Some(tip.as_str());
    let _turn = merge_queue::checkout_turn().await;
    let (from, to) = (tip.clone(), stacked.top().to_owned());
    let command = match crate::server::worktree::agent::merge_queue(core, &key.0, &to).await {
        Ok(checkout) => {
            let repo = key.0.clone();
            blocking(move || rust_check(&repo, &checkout, &from, &to).map_err(Into::into))
                .await
                .map_err(|error| error.to_string())
        }
        Err(why) => Err(why),
    };
    let command = match command {
        Ok(command) => command,
        Err(error) => {
            for laid in &stacked.laid {
                let why = format!("reading which crates it changes failed: {error}");
                send_back(core, key, &laid.waiting, &why);
            }
            return;
        }
    };
    let Some((label, command)) = command else {
        return put_on(core, key, stacked).await;
    };
    if repairing && stacked.laid[0].waiting.failed_over.as_deref() == Some(tip.as_str()) {
        with_line(core, key, |line| line.lane.mark_red(&tip));
        return;
    }
    let front = stacked.laid[0].waiting.agent.clone();
    let ran = checks::run_in_queue(core, &key.0, stacked.top(), &command, &front).await;
    let ran = match ran {
        Ok(ran) if ran.passed => return put_on(core, key, stacked).await,
        Ok(ran) => ran,
        Err(why) => {
            for laid in &stacked.laid {
                send_back(
                    core,
                    key,
                    &laid.waiting,
                    &format!("its check did not run: {why}"),
                );
            }
            return;
        }
    };
    if repairing {
        return failed_over(core, key, &stacked.laid[0].waiting, &tip);
    }
    let changes: Vec<Vec<String>> = stacked
        .laid
        .iter()
        .map(|laid| laid.commits.clone())
        .collect();
    let blamed =
        merge_queue::blame(core, &key.0, &tip, &changes, &command, &front, ran.took_ms).await;
    let failed = |laid: &Laid| {
        let ahead = match stacked.laid[0].waiting.is(&laid.waiting) {
            true => "",
            false => ", with the lands ahead of it in the queue",
        };
        format!(
            "{label} failed on top of the latest {}{ahead}:\n\n{}",
            key.1,
            crate::git::tail(&ran.output)
        )
    };
    match blamed {
        Ok(Blame::Broke(at)) => {
            with_line(core, key, |line| line.lane.one_at_a_time = false);
            let laid = &stacked.laid[at];
            send_back(core, key, &laid.waiting, &failed(laid));
        }
        Ok(Blame::BaseRed) => {
            with_line(core, key, |line| line.lane.mark_red(&tip));
            if let [only] = stacked.laid.as_slice() {
                failed_over(core, key, &only.waiting, &tip);
            }
        }
        Ok(Blame::Unknown) | Err(_) if stacked.laid.len() > 1 => {
            with_line(core, key, |line| line.lane.one_at_a_time = true);
        }
        Ok(Blame::Unknown) | Err(_) => {
            let laid = &stacked.laid[0];
            send_back(core, key, &laid.waiting, &failed(laid));
        }
    }
}

/// It was tried alone on a branch that fails its own check and did not mend
/// it. That is not its fault, so it keeps its place until the branch moves.
fn failed_over(core: &Core, key: &Key, waiting: &Waiting, tip: &str) {
    with_line(core, key, |line| {
        if let Some(w) = line.waiting.iter_mut().find(|w| w.is(waiting)) {
            w.failed_over = Some(tip.to_owned());
        }
    });
}

/// Moves the branch to the checked stack, made again on its tip should that
/// have moved since, then brings each agent's checkout along and pushes.
async fn put_on(core: &Arc<Core>, key: &Key, stacked: Stack) {
    let owner = core.clone();
    let key_now = key.clone();
    let put = blocking(move || Ok(put_stack(&owner, &key_now, stacked))).await;
    let Ok((landed, refused)) = put else {
        return;
    };
    for (waiting, why) in &refused {
        send_back(core, key, waiting, why);
    }
    if landed.is_empty() {
        return;
    }
    with_line(core, key, |line| {
        line.lane.one_at_a_time = false;
        line.waiting
            .retain(|w| !landed.iter().any(|(laid, _)| laid.is(w)));
    });
    for (waiting, stays) in &landed {
        if let Some(stays) = stays {
            tell(
                core,
                &waiting.agent,
                format!("Your commits landed on {}. {stays}", key.1),
            );
        }
    }
    let pushers: Vec<String> = (landed.iter())
        .filter(|(waiting, _)| waiting.push)
        .map(|(waiting, _)| waiting.agent.clone())
        .collect();
    if pushers.is_empty() {
        return;
    }
    let (owner, repo, agents) = (core.clone(), key.0.clone(), pushers.clone());
    let pushed =
        blocking(move || super::push::push(&owner, &repo, &agents, "origin").map_err(Into::into))
            .await;
    if let Err(error) = pushed {
        for agent in &pushers {
            tell(
                core,
                agent,
                format!("Your commits landed on {} but did not push: {error}", key.1),
            );
        }
    }
}

type Landed = Vec<(Waiting, Option<String>)>;

/// The lands put on the branch, each with why its checkout stayed behind
/// when it did, and those refused with why.
fn put_stack(core: &Core, key: &Key, stacked: Stack) -> (Landed, Vec<(Waiting, String)>) {
    let (repo, base) = key;
    let signing = Signing::of(core);
    let mut stacked = stacked;
    let mut refused = Vec::new();
    for _ in 0..ATTEMPTS {
        let tip = match git(repo, &["rev-parse", &format!("refs/heads/{base}")]) {
            Ok(tip) => line(tip),
            Err(_) => break,
        };
        if tip != stacked.tip {
            let batch: Vec<Waiting> = (stacked.laid.iter())
                .map(|laid| laid.waiting.clone())
                .collect();
            stacked = match stack(&signing, key, &batch) {
                Ok(again) => again,
                Err(_) => break,
            };
            refused.append(&mut stacked.refused);
        }
        let Some(top) = stacked.laid.last().map(|laid| laid.tip.clone()) else {
            break;
        };
        match land::put(repo, base, &stacked.tip, &top) {
            Ok(_) => return (landed(core, repo, &stacked), refused),
            Err(NotPut::Moved) => continue,
            Err(NotPut::Clash(paths)) => {
                let why = format!(
                    "the project's checkout has changes nobody committed in {} that clash with it. Find who made them with who_wrote on those paths and agree with them, then land again.",
                    paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
                );
                let mut clashing = clashing(repo, &stacked, &paths);
                if clashing.is_empty() {
                    clashing = stacked.laid.into_iter().map(|laid| laid.waiting).collect();
                }
                refused.extend(clashing.into_iter().map(|waiting| (waiting, why.clone())));
                return (Vec::new(), refused);
            }
            Err(NotPut::Failed(why)) => {
                for laid in stacked.laid {
                    refused.push((laid.waiting, why.clone()));
                }
                return (Vec::new(), refused);
            }
        }
    }
    (Vec::new(), refused)
}

/// The lands of `stacked` that change any of `paths`.
fn clashing(repo: &Path, stacked: &Stack, paths: &[PathBuf]) -> Vec<Waiting> {
    let mut before = stacked.tip.clone();
    let mut found = Vec::new();
    for laid in &stacked.laid {
        let changed =
            git(repo, &["diff", "-z", "--name-only", &before, &laid.tip]).unwrap_or_default();
        if changed
            .split('\0')
            .any(|path| !path.is_empty() && paths.contains(&repo.join(path)))
        {
            found.push(laid.waiting.clone());
        }
        before = laid.tip.clone();
    }
    found
}

/// Brings each landed agent's checkout up to its landed commits and says they landed.
fn landed(core: &Core, repo: &Path, stacked: &Stack) -> Landed {
    let project = core.workspaces().project_of(repo);
    let mut onto = stacked.tip.clone();
    let mut landed = Vec::with_capacity(stacked.laid.len());
    for laid in &stacked.laid {
        let waiting = &laid.waiting;
        let stays = land::follow(&waiting.worktree.path, &waiting.head, &laid.tip);
        core.build_outputs().landed(&waiting.agent);
        let subject = git(repo, &["log", "-1", "--format=%s", &laid.tip]).unwrap_or_default();
        (core.repo_above().chief_landed)(core, &project, &waiting.agent, &subject);
        core.repo_events().tell(RepoEvent::Landed {
            project: project.clone(),
            agent: waiting.agent.clone(),
            onto: onto.clone(),
            tip: laid.tip.clone(),
        });
        onto = laid.tip.clone();
        landed.push((waiting.clone(), stays));
    }
    landed
}

/// The check for what changed from `from` to `to`, read in `checkout` at `to`,
/// with what to call it: Clippy and the tests of the crates it changes or whose
/// files they read, and of every crate that depends on those; of every crate
/// when it changes how they all build; none when it changes no Rust.
fn rust_check(
    repo: &Path,
    checkout: &Path,
    from: &str,
    to: &str,
) -> Result<Option<(String, String)>, String> {
    if !checkout.join("Cargo.toml").is_file() {
        return Ok(None);
    }
    let changed = git(
        repo,
        &["diff", "-z", "--name-only", "--no-renames", from, to],
    )?;
    let changed: Vec<&str> = changed
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    let every =
        (changed.iter()).any(|path| WORKSPACE_FILES.contains(path) || path.starts_with(".cargo/"));
    let members = members(checkout)?;
    let reads = reads(repo, to, &members);
    let mut names = BTreeSet::new();
    for path in changed {
        let readers: Vec<&str> = (reads.iter())
            .filter(|(read, _)| path == read || path.starts_with(&format!("{read}/")))
            .map(|(_, name)| name.as_str())
            .collect();
        if !readers.is_empty() {
            names.extend(readers.into_iter().map(str::to_owned));
        } else if !path.ends_with(".md") && !path.ends_with(".mdx") {
            names.extend(owner(&members, path).map(|member| member.name.clone()));
        }
    }
    if !every && names.is_empty() {
        return Ok(None);
    }
    let mut grew = true;
    while grew {
        grew = false;
        for member in &members {
            if !names.contains(&member.name) && member.deps.iter().any(|dep| names.contains(dep)) {
                names.insert(member.name.clone());
                grew = true;
            }
        }
    }
    let (scope, of) = match every {
        true => ("--workspace".to_owned(), "every crate".to_owned()),
        false => (
            (names.iter())
                .map(|name| format!("-p {name}"))
                .collect::<Vec<_>>()
                .join(" "),
            names.iter().cloned().collect::<Vec<_>>().join(", "),
        ),
    };
    Ok(Some((
        format!("Clippy and the tests of {of}"),
        format!("cargo clippy {scope} --all-targets --all-features -- -D warnings && cargo test {scope} --all-features"),
    )))
}

const METADATA_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);
const METADATA_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

/// A crate of the workspace.
struct Member {
    name: String,
    /// Its folder from the workspace's root, empty for the root itself.
    dir: String,
    /// The workspace's crates it depends on.
    deps: Vec<String>,
}

/// The workspace's crates, as cargo reads them in `checkout`.
fn members(checkout: &Path) -> Result<Vec<Member>, String> {
    let mut shell = sikemux_process::user_environment::background_shell();
    shell
        .args(["-c", "cargo metadata --no-deps --format-version 1"])
        .envs(crate::git_guard::shim::environment(None))
        .current_dir(checkout);
    let output = sikemux_process::run(
        &mut shell,
        None,
        METADATA_LIMIT,
        METADATA_OUTPUT_LIMIT,
        None,
    )
    .map_err(|error| format!("cargo metadata did not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            crate::git::tail(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata did not read: {error}"))?;
    let root = PathBuf::from(metadata["workspace_root"].as_str().unwrap_or_default());
    let in_workspace: BTreeSet<&str> = (metadata["workspace_members"].as_array())
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let packages = metadata["packages"].as_array().cloned().unwrap_or_default();
    let names: BTreeSet<&str> = (packages.iter())
        .filter(|package| (package["id"].as_str()).is_some_and(|id| in_workspace.contains(id)))
        .filter_map(|package| package["name"].as_str())
        .collect();
    Ok(packages
        .iter()
        .filter(|package| (package["id"].as_str()).is_some_and(|id| in_workspace.contains(id)))
        .map(|package| {
            let manifest = PathBuf::from(package["manifest_path"].as_str().unwrap_or_default());
            let dir = (manifest.parent())
                .and_then(|dir| dir.strip_prefix(&root).ok())
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default();
            let deps = (package["dependencies"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|dep| dep["name"].as_str())
                .filter(|dep| names.contains(dep))
                .map(str::to_owned)
                .collect();
            Member {
                name: package["name"].as_str().unwrap_or_default().to_owned(),
                dir,
                deps,
            }
        })
        .collect())
}

/// The crate whose folder holds `path`, the innermost when they nest.
fn owner<'a>(members: &'a [Member], path: &str) -> Option<&'a Member> {
    (members.iter())
        .filter(|member| member.dir.is_empty() || path.starts_with(&format!("{}/", member.dir)))
        .max_by_key(|member| member.dir.len())
}

/// The files and folders each crate reads at `at` as it builds, by the paths
/// its sources name in `include_str!` and `include_bytes!` and its build script
/// in `rerun-if-changed`.
fn reads(repo: &Path, at: &str, members: &[Member]) -> Vec<(String, String)> {
    let named = git(
        repo,
        &[
            "grep",
            "-I",
            "-E",
            r#"include_(str|bytes)!\("|rerun-if-changed=[^{"]"#,
            at,
            "--",
            "*.rs",
        ],
    )
    .unwrap_or_default();
    let mut reads = Vec::new();
    for found in named.lines() {
        let Some((file, text)) = found
            .strip_prefix(&format!("{at}:"))
            .and_then(|rest| rest.split_once(':'))
        else {
            continue;
        };
        let Some(member) = owner(members, file) else {
            continue;
        };
        let source_dir = Path::new(file).parent().unwrap_or(Path::new(""));
        for (marker, from) in [
            ("include_str!(\"", source_dir),
            ("include_bytes!(\"", source_dir),
            ("rerun-if-changed=", Path::new(&member.dir)),
        ] {
            for named in text.split(marker).skip(1) {
                let Some(literal) = named
                    .split('"')
                    .next()
                    .filter(|literal| !literal.is_empty())
                else {
                    continue;
                };
                if let Some(path) = within_repo(&from.join(literal)) {
                    reads.push((path, member.name.clone()));
                }
            }
        }
    }
    reads
}

/// `path` with its `..` steps taken, when it stays inside the repository.
fn within_repo(path: &Path) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                parts.pop()?;
            }
            std::path::Component::Normal(name) => parts.push(name.to_str()?),
            _ => {}
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}
