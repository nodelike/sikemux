//! The queue agents land through. `git_commit` with `land: true` puts an
//! agent's commits in line for the branch its checkout was cut from and
//! returns. One base at a time, the core stacks the waiting lands on the
//! branch's tip in order, checks the stack once, and on a pass moves the branch
//! to it, so the branch stays a straight line of commits. Lands that change no
//! Rust are not built; the others are checked with the crates they change.
//! Before the check, the queue formats the Rust a land changed and runs the
//! fix of each land check it is due, committing what they change on top of
//! that land rather than sending it back. A
//! failed check is traced to the land that broke it, which is sent back while
//! the rest stay in line. Every land ends in a message to its agent naming its
//! commits, landed or sent back; a newer land of the same agent queues behind
//! one being checked rather than taking its place.

use crate::server::changed::{ChangedCore, What};
use crate::server::notify::Signal;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::land::{self, NotPut};
use super::owners::git;
use super::push::{line, Signing};
use crate::server::checks;
use crate::server::connection::blocking;
use crate::server::merge_queue::{self, Lane};
use crate::server::Core;
use crate::tasks::config::LandCheck;

const ATTEMPTS: usize = 4;
/// Files that change how every crate builds.
const WORKSPACE_FILES: [&str; 5] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
    "rustfmt.toml",
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
    /// The agent's own checkout here, which follows its landed commits; none for commits a
    /// worker on another host sent back.
    checkout: Option<PathBuf>,
    /// The commit its checkout stood on when it asked to land.
    head: String,
    /// Where its own commits begin, when its branch starts with commits that are not its own.
    since: Option<String>,
    push: bool,
    queued_at: u64,
    /// A check holding it is running, so a newer land of its agent queues behind it.
    checking: bool,
    /// Its check ran out of time once already, and its agent was told.
    ran_out_of_time: bool,
}

impl Waiting {
    fn is(&self, other: &Waiting) -> bool {
        self.agent == other.agent && self.head == other.head
    }

    fn short(&self) -> &str {
        self.head.get(..9).unwrap_or(&self.head)
    }
}

fn shorts(commits: &[String]) -> String {
    (commits.iter())
        .map(|commit| commit.get(..9).unwrap_or(commit))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The subjects of `commits`, quoted, which tell a lead what landed without commit ids.
fn titles(repo: &Path, commits: &[String]) -> String {
    let mut args = vec!["log", "--no-walk=unsorted", "--format=%s"];
    args.extend(commits.iter().map(String::as_str));
    let subjects = git(repo, &args).unwrap_or_default();
    let quoted: Vec<String> = (subjects.lines())
        .map(|subject| format!("\"{}\"", subject.trim()))
        .collect();
    match quoted.is_empty() {
        true => "its commits".to_owned(),
        false => quoted.join(", "),
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

impl Queued {
    pub fn note(&self) -> &str {
        &self.note
    }
}

/// What a new land of an agent did to the one it had in line already.
enum Replaced {
    Took(String),
    Behind(String),
}

/// Puts the calling agent's commits in line for the branch its checkout was
/// cut from, refusing at once those that clash with the branch.
pub fn enqueue(core: &Core, agent_id: &str, push: bool) -> Result<Queued, String> {
    let worktree = land::own_checkout(core, agent_id).ok_or("You work in the project's own checkout, so your commits are on its branch already; there is nothing to land")?;
    let base = (worktree.base.clone())
        .ok_or("Your checkout was cut from no branch, so there is nowhere to land it")?;
    let head = line(git(&worktree.path, &["rev-parse", "HEAD"])?);
    let sent = Sent {
        repo: worktree.repo,
        base,
        head,
        since: None,
    };
    put_in_line(core, agent_id, sent, Some(worktree.path), push)
}

/// Commits a worker on another host sent back to land: `head` is in `repo` here, and its own
/// commits are those after `since`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Sent {
    pub repo: PathBuf,
    pub base: String,
    pub head: String,
    pub since: Option<String>,
}

/// Puts commits a worker on another host sent back in line for `sent.base`, as its own
/// checkout's would be.
pub fn enqueue_sent(core: &Core, agent_id: &str, sent: Sent, push: bool) -> Result<Queued, String> {
    put_in_line(core, agent_id, sent, None, push)
}

fn put_in_line(
    core: &Core,
    agent_id: &str,
    sent: Sent,
    checkout: Option<PathBuf>,
    push: bool,
) -> Result<Queued, String> {
    let Sent {
        repo,
        base,
        head,
        since,
    } = sent;
    let tip = line(git(&repo, &["rev-parse", &format!("refs/heads/{base}")])?);
    let Some(laid) = land::lay(
        &Signing::of(core),
        &repo,
        &head,
        since.as_deref(),
        &tip,
        &base,
    )?
    else {
        return Ok(Queued {
            note: format!("{base} has every commit of yours already, so nothing waits to land"),
            branch: base,
            commits: Vec::new(),
            ahead: 0,
        });
    };
    let key = (repo, base.clone());
    let waiting = Waiting {
        agent: agent_id.to_owned(),
        checkout,
        head,
        since,
        push,
        queued_at: epoch_ms(),
        checking: false,
        ran_out_of_time: false,
    };
    let short = waiting.short().to_owned();
    let (ahead, replaced) = {
        let mut lines = core.land_queue().0.locked();
        let line = lines.entry(key.clone()).or_default();
        let earlier = (line.waiting.iter()).position(|w| w.agent == agent_id && !w.checking);
        let placed = match earlier {
            Some(at)
                if (line.waiting.get(at)).is_some_and(|earlier| earlier.head == waiting.head) =>
            {
                (at, None)
            }
            Some(at) => {
                let replaced = (line.waiting.get_mut(at)).map(|earlier| {
                    Replaced::Took(std::mem::replace(earlier, waiting).short().to_owned())
                });
                (at, replaced)
            }
            None if (line.waiting.iter()).any(|w| w.is(&waiting)) => {
                return Err(format!(
                    "Your land of {short} is being checked already; you hear how it ends"
                ));
            }
            None => {
                let behind = (line.waiting.iter())
                    .find(|w| w.agent == agent_id)
                    .map(|w| Replaced::Behind(w.short().to_owned()));
                line.waiting.push(waiting);
                (line.waiting.len() - 1, behind)
            }
        };
        save(core, &lines);
        placed
    };
    let (pushed, stays) = match push {
        true => (" and pushed", String::new()),
        false => ("", local_only(&key.0, &base)),
    };
    let stays = match stays.is_empty() {
        true => stays,
        false => format!(" It stays{stays}."),
    };
    core.changed(What::View);
    wake(core, key);
    let replaces = match replaced {
        Some(Replaced::Took(earlier)) => format!(
            " It takes the place of your land of {earlier}, which had not been checked yet."
        ),
        Some(Replaced::Behind(earlier)) => format!(
            " Your land of {earlier} is being checked, so this one goes after it and lands only what it adds."
        ),
        None => String::new(),
    };
    Ok(Queued {
        note: format!(
            "Queued {short}:{replaces} Sikemux checks it with the lands beside it, running rustfmt and cargo check on the crates they change, and puts it on {base}{pushed} when that passes.{stays} Either way a message tells you how it ended, naming {short}; until then your checkout keeps its commits, and you need not run the final checks yourself."
        ),
        branch: base,
        commits: laid.listed,
        ahead,
    })
}

/// Says a land without a push stays in `repo`'s checkout, when `branch` has a remote copy it won't reach.
fn local_only(repo: &Path, branch: &str) -> String {
    match on_remote(repo, branch) {
        true => format!(
            " in this computer's checkout only, not on origin/{branch}; land with push: true to push it too"
        ),
        false => String::new(),
    }
}

fn on_remote(repo: &Path, branch: &str) -> bool {
    let remote = format!("refs/remotes/origin/{branch}");
    git(repo, &["rev-parse", "--verify", "-q", &remote]).is_ok()
}

/// Where the agent's commits wait to land.
pub struct InLine {
    pub branch: String,
    /// 1 for the next to be checked.
    pub place: u32,
    pub since: u64,
}

/// Where the agent's commits wait in the queue, when they do.
pub fn in_line(core: &Core, agent_id: &str) -> Option<InLine> {
    let lines = core.land_queue().0.locked();
    lines.iter().find_map(|((_, branch), line)| {
        let (at, waiting) = (line.waiting.iter().enumerate()).find(|(_, w)| w.agent == agent_id)?;
        Some(InLine {
            branch: branch.clone(),
            place: at as u32 + 1,
            since: waiting.queued_at,
        })
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
        let (repo, branch) = key.clone();
        let to_remote = blocking(move || Ok(on_remote(&repo, &branch)))
            .await
            .unwrap_or(false);
        loop {
            let turn = merge_queue::checkout_turn(to_remote).await;
            let Some(batch) = next(&core, &key) else {
                return;
            };
            let passed = check(&core, &key, batch).await;
            crate::server::worktree::agent::trim_merge_queue(&key.0).await;
            drop(turn);
            if let Some(stacked) = passed {
                put_on(&core, &key, stacked).await;
            }
            with_line(&core, &key, |line| {
                line.waiting.iter_mut().for_each(|w| w.checking = false)
            });
        }
    });
}

/// The lands the next check takes, or none when the queue stops for now.
fn next(core: &Arc<Core>, key: &Key) -> Option<Vec<Waiting>> {
    let mut lines = core.land_queue().0.locked();
    let line = lines.get_mut(key)?;
    let count = line.lane.take(line.waiting.len());
    if count == 0 {
        lines.remove(key);
        save(core, &lines);
        return None;
    }
    let taken = line.waiting.get_mut(..count)?;
    taken.iter_mut().for_each(|w| w.checking = true);
    Some(taken.to_vec())
}

fn with_line<T>(core: &Core, key: &Key, change: impl FnOnce(&mut Line) -> T) -> Option<T> {
    let mut lines = core.land_queue().0.locked();
    let changed = lines.get_mut(key).map(change);
    save(core, &lines);
    changed
}

const SAVED: &str = "land-queue.json";

/// A waiting land as the queue's file keeps it, so a core started again takes it up.
#[derive(Serialize, Deserialize)]
struct Saved {
    agent: String,
    head: String,
    push: bool,
    /// Where commits a worker on another host sent back wait, as its checkout here is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sent: Option<Sent>,
}

fn save(core: &Core, lines: &HashMap<Key, Line>) {
    let Some(path) = core.data_dir().as_ref().map(|dir| dir.join(SAVED)) else {
        return;
    };
    let saved: Vec<Saved> = (lines.iter())
        .flat_map(|((repo, base), line)| line.waiting.iter().map(move |w| (repo, base, w)))
        .map(|(repo, base, w)| Saved {
            agent: w.agent.clone(),
            head: w.head.clone(),
            push: w.push,
            sent: w.checkout.is_none().then(|| Sent {
                repo: repo.clone(),
                base: base.clone(),
                head: w.head.clone(),
                since: w.since.clone(),
            }),
        })
        .collect();
    let written = serde_json::to_vec(&saved)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            sikemux_process::files::write_atomic(&path, &bytes, false)
                .map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        eprintln!("sikemux core: the land queue was not saved: {error}");
    }
}

/// Puts back in line the lands a core before this one had waiting, and starts their queues.
pub fn resume(core: &Arc<Core>) {
    let Some(path) = core.data_dir().as_ref().map(|dir| dir.join(SAVED)) else {
        return;
    };
    let Some(saved) = sikemux_process::files::read_or_set_aside(&path, |bytes| {
        serde_json::from_slice::<Vec<Saved>>(bytes)
    }) else {
        return;
    };
    let mut keys = BTreeSet::new();
    {
        let mut lines = core.land_queue().0.locked();
        for saved in saved {
            let (key, checkout, since) = match saved.sent {
                Some(sent) => ((sent.repo, sent.base), None, sent.since),
                None => {
                    let Some(worktree) = land::own_checkout(core, &saved.agent) else {
                        eprintln!(
                            "sikemux core: a land of {} was dropped, as its agent's checkout is gone",
                            saved.agent
                        );
                        continue;
                    };
                    let Some(base) = worktree.base.clone() else {
                        continue;
                    };
                    ((worktree.repo, base), Some(worktree.path), None)
                }
            };
            lines.entry(key.clone()).or_default().waiting.push(Waiting {
                agent: saved.agent,
                checkout,
                since,
                head: saved.head,
                push: saved.push,
                queued_at: epoch_ms(),
                checking: false,
                ran_out_of_time: false,
            });
            keys.insert(key);
        }
        save(core, &lines);
    }
    for key in keys {
        wake(core, key);
    }
}

fn leave(core: &Core, key: &Key, waiting: &Waiting) {
    with_line(core, key, |line| line.waiting.retain(|w| !w.is(waiting)));
    core.changed(What::View);
}

/// Takes the land out of the queue and tells its agent why.
fn send_back(core: &Arc<Core>, key: &Key, waiting: &Waiting, why: &str) {
    leave(core, key, waiting);
    let text = format!(
        "Your land of {} onto {} was sent back: {why}\n\nYour commits are still in your checkout. Fix it, commit, and land again.",
        waiting.short(),
        key.1
    );
    tell(core, &waiting.agent, text);
    let why = why.lines().next().unwrap_or_default();
    tell_lead(
        core,
        &waiting.agent,
        format!(
            "Its land of {} onto {} was sent back: {why}\n\nIt has been told to fix it and land again.",
            waiting.short(),
            key.1
        ),
        false,
    );
}

/// Tells the agent's lead, when it has one, how its land went: its turn ended before the land did,
/// so no report of that turn says so. A calm note is a land that went in with nothing to fix.
fn tell_lead(core: &Arc<Core>, agent: &str, text: String, calm: bool) {
    (core.repo_above().messages_tell_lead)(core, agent, text, calm);
}

/// Hands the agent work about its land, waking it if need be: a notice would be
/// dropped when it is asleep or has other messages waiting.
fn tell(core: &Arc<Core>, agent: &str, text: String) {
    let (core, agent) = (core.clone(), agent.to_owned());
    tokio::spawn(async move {
        let told =
            (core.repo_above().messages_work_from_sikemux)(&core, agent, text, "about its land");
        if let Err(error) = told.await {
            eprintln!("sikemux core: a note about a land did not reach its agent: {error}");
        }
    });
}

/// Saves the whole output of a failed check as `name`'s log, which the host
/// keeps only for its last two runs, and says where it is.
fn keep_log(core: &Core, name: &str, output: &str) -> String {
    let Some(dir) = core.data_dir().as_ref().map(|dir| dir.join("land-logs")) else {
        return String::new();
    };
    let path = dir.join(format!("{name}.log"));
    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, output)) {
        Ok(()) => format!("\n\nThe whole log is at {}.", path.display()),
        Err(_) => String::new(),
    }
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
        match lay(signing, repo, base, waiting, &on) {
            Ok(Some(laid)) => stacked.laid.push(laid),
            Ok(None) => stacked.done.push(waiting.clone()),
            Err(why) if on == tip => stacked.refused.push((waiting.clone(), why)),
            Err(_) => {}
        }
    }
    Ok(stacked)
}

fn lay(
    signing: &Signing,
    repo: &Path,
    base: &str,
    waiting: &Waiting,
    on: &str,
) -> Result<Option<Laid>, String> {
    let since = waiting.since.as_deref();
    let Some(laid) = land::lay(signing, repo, &waiting.head, since, on, base)? else {
        return Ok(None);
    };
    let commits = git(
        repo,
        &["rev-list", "--reverse", &format!("{on}..{}", laid.tip)],
    )?;
    Ok(Some(Laid {
        waiting: waiting.clone(),
        commits: commits.lines().map(str::to_owned).collect(),
        tip: laid.tip,
    }))
}

/// Formats the Rust each land of `stacked` changed and runs the fix of each land check it is
/// due, in the merge-queue checkout at the land, committing what they changed on top of it. A
/// land after one that gained a commit is laid again on it, or waits for the next pass when it
/// no longer lays cleanly.
async fn tidy(core: &Arc<Core>, key: &Key, stacked: Stack) -> Stack {
    let Stack {
        tip,
        laid,
        refused,
        done,
    } = stacked;
    let mut was = tip.clone();
    let mut tidied = Stack {
        tip,
        laid: Vec::new(),
        refused,
        done,
    };
    let signing = Arc::new(Signing::of(core));
    for laid in laid {
        let on = tidied.top().to_owned();
        let from = std::mem::replace(&mut was, laid.tip.clone());
        let laid = match on == from {
            true => laid,
            false => {
                let (signing, key, on) = (signing.clone(), key.clone(), on.clone());
                let again = blocking(move || {
                    lay(&signing, &key.0, &key.1, &laid.waiting, &on).map_err(Into::into)
                })
                .await;
                match again {
                    Ok(Some(laid)) => laid,
                    _ => continue,
                }
            }
        };
        tidied.laid.push(match fixed(core, key, &on, &laid).await {
            Some(commit) => Laid {
                commits: [laid.commits, vec![commit.clone()]].concat(),
                tip: commit,
                waiting: laid.waiting,
            },
            None => laid,
        });
    }
    tidied
}

/// The commit made on top of `laid` with its land checks' fixes run, when they changed anything.
async fn fixed(core: &Arc<Core>, key: &Key, from: &str, laid: &Laid) -> Option<String> {
    let (repo, from, to) = (key.0.clone(), from.to_owned(), laid.tip.clone());
    let fixes = blocking(move || {
        Ok((land_checks_due(&repo, &from, &to).into_iter())
            .filter(|(check, _)| check.fix.is_some())
            .collect::<Vec<_>>())
    })
    .await
    .ok()?;
    if fixes.is_empty() {
        return None;
    }
    let checkout = crate::server::worktree::agent::merge_queue(core, &key.0, &laid.tip)
        .await
        .ok()?;
    let signers: Vec<_> = super::footer::known(core, &key.0, &laid.waiting.agent)
        .into_iter()
        .collect();
    let (owner, project, tip) = (core.clone(), key.0.clone(), laid.tip.clone());
    let host_key = Signing::of(core).key;
    blocking(move || {
        let mut ran = Vec::new();
        let trusted = super::generate::trust_needed_for(&owner, &project, &checkout, |config| {
            !config.land_checks.is_empty()
        })
        .is_none();
        for (check, files) in fixes.iter().filter(|_| trusted) {
            let fix = check.fix.as_deref().unwrap_or_default();
            if shell(&checkout, fix, &check.label, files, None, LAND_CHECK_LIMIT).is_ok() {
                ran.push(format!("the fix of {}", check.label));
            }
        }
        git(&checkout, &["add", "-u"])?;
        let tree = line(git(&checkout, &["write-tree"])?);
        if ran.is_empty()
            || tree == line(git(&checkout, &["rev-parse", &format!("{tip}^{{tree}}")])?)
        {
            return Ok(None);
        }
        let parents = vec![tip.clone()];
        let footer = super::footer::of_core_commit(
            &checkout,
            &signers,
            &crate::trailers::Commit {
                tree: &tree,
                parents: &parents,
            },
            host_key.as_ref(),
        );
        let subject = format!("chore(land): {}", ran.join(" and "));
        let message = crate::trailers::append(&subject, &footer);
        let made = super::owners::run(
            &checkout,
            &["commit-tree", &tree, "-p", &tip, "-F", "-"],
            None,
            Some(message.as_bytes()),
        )?;
        let made = line(String::from_utf8_lossy(&made).into_owned());
        if let (true, Some(signer), Some(key)) = (footer.is_empty(), signers.first(), &host_key) {
            super::committer::record_with(&checkout, &made, signer, key);
        }
        Ok(Some(made))
    })
    .await
    .ok()
    .flatten()
}

/// Stacks the lands in `batch` and checks them together: the stack to land when it passed.
/// Landing comes after the queues' checkout is free again, so a push whose pre-push hook
/// runs for long holds up only the lands onto its own branch.
async fn check(core: &Arc<Core>, key: &Key, batch: Vec<Waiting>) -> Option<Stack> {
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
            return None;
        }
    };
    for waiting in &stacked.done {
        leave(core, key, waiting);
        let text = format!(
            "Your land of {} had nothing left to put on {}: it has every change of those commits already.",
            waiting.short(),
            key.1
        );
        tell(core, &waiting.agent, text);
    }
    for (waiting, why) in &stacked.refused {
        send_back(core, key, waiting, why);
    }
    if stacked.laid.is_empty() {
        return None;
    }
    let stacked = tidy(core, key, stacked).await;
    let first = stacked.laid.first()?;
    let ranges: Vec<(String, String)> = (stacked.laid.iter())
        .scan(stacked.tip.clone(), |from, laid| {
            Some((std::mem::replace(from, laid.tip.clone()), laid.tip.clone()))
        })
        .collect();
    if let Some((laid, why)) = failing_land_check(core, key, &stacked, &ranges).await {
        send_back(core, key, &laid.waiting, &why);
        return None;
    }
    let to = stacked.top().to_owned();
    let command = match crate::server::worktree::agent::merge_queue(core, &key.0, &to).await {
        Ok(checkout) => {
            let (repo, base, tip) = (key.0.clone(), key.1.clone(), stacked.tip.clone());
            blocking(move || {
                let from = checked_below(&repo, &base, &tip);
                rust_check(&repo, &checkout, &from, &to).map_err(Into::into)
            })
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
            return None;
        }
    };
    let Some((label, command)) = command else {
        mark_checked(key, stacked.top());
        return Some(stacked);
    };
    let front = first.waiting.agent.clone();
    let ran = checks::run_in_queue(core, &key.0, stacked.top(), &command, &front).await;
    let ran = match ran {
        Ok(ran) if ran.passed => {
            mark_checked(key, stacked.top());
            return Some(stacked);
        }
        Ok(ran) => ran,
        Err(checks::NotRun::OutOfTime(why))
            if stacked
                .laid
                .iter()
                .all(|laid| !laid.waiting.ran_out_of_time) =>
        {
            for laid in &stacked.laid {
                out_of_time(core, key, &laid.waiting, &label, &why);
            }
            return None;
        }
        Err(checks::NotRun::OutOfTime(why) | checks::NotRun::Other(why)) => {
            for laid in &stacked.laid {
                send_back(
                    core,
                    key,
                    &laid.waiting,
                    &format!("its check did not run: {why}"),
                );
            }
            return None;
        }
    };
    let lands: Vec<&str> = (stacked.laid.iter())
        .map(|laid| laid.waiting.short())
        .collect();
    eprintln!(
        "sikemux core: {label} failed on {} lands for {} ({}) after {} s.{}",
        lands.len(),
        key.1,
        lands.join(", "),
        ran.took_ms / 1000,
        keep_log(core, &batch_log_name(&key.1), &ran.output),
    );
    let [only] = stacked.laid.as_slice() else {
        with_line(core, key, |line| line.lane.one_at_a_time = true);
        return None;
    };
    let why = format!(
        "{label} failed on top of the latest {}:\n\n{}{}",
        key.1,
        crate::git::tail(&ran.output),
        keep_log(core, &only.waiting.agent, &ran.output)
    );
    send_back(core, key, &only.waiting, &why);
    None
}

/// The name a failed check of `branch`'s lands keeps its whole log under.
fn batch_log_name(branch: &str) -> String {
    format!("lands-onto-{}", branch.replace('/', "-"))
}

/// Its check ran out of time on a busy host, which says nothing of its commits, so it keeps its
/// place and the check runs once more, carrying on from what the last run built. A second run
/// out of time sends it back, so a check that hangs cannot hold the line for good.
fn out_of_time(core: &Arc<Core>, key: &Key, waiting: &Waiting, label: &str, why: &str) {
    let newly = with_line(core, key, |line| {
        let w = line.waiting.iter_mut().find(|w| w.is(waiting))?;
        Some(!std::mem::replace(&mut w.ran_out_of_time, true))
    });
    if newly.flatten() == Some(true) {
        let text = format!(
            "Your land of {} onto {} is still waiting: {label} ran out of time on this busy computer ({why}). That is not your commits' doing, so it keeps its place and the check runs once more. Nothing is yours to do.",
            waiting.short(),
            key.1
        );
        tell(core, &waiting.agent, text);
    }
}

/// Moves the branch to the checked stack, made again on its tip should that
/// have moved since, and brings each agent's checkout along. A stack with a
/// land to push goes to the remote first and moves the branch only once it took it.
async fn put_on(core: &Arc<Core>, key: &Key, stacked: Stack) {
    let owner = core.clone();
    let key_now = key.clone();
    let put = blocking(move || Ok(put_stack(&owner, &key_now, stacked))).await;
    let Ok(Put {
        landed,
        refused,
        pushed,
    }) = put
    else {
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
            .retain(|w| !landed.iter().any(|(laid, _, _)| laid.is(w)));
    });
    core.changed(What::View);
    if let Some(pushed) = &pushed {
        super::push::tell_sent_back(core, &tokio::runtime::Handle::current(), pushed);
    }
    let sent_back = |agent: &str| {
        (pushed.iter().flat_map(|pushed| &pushed.blocked))
            .any(|blocked| blocked.owners.iter().any(|owner| owner == agent))
    };
    for (waiting, commits, stays) in &landed {
        let stays = stays
            .as_deref()
            .map(|stays| format!(" {stays}"))
            .unwrap_or_default();
        core.notify().send(Signal::Landed {
            agent_id: waiting.agent.clone(),
        });
        if !waiting.push {
            let local = local_only(&key.0, &key.1);
            let on = format!(
                "{} is on {} as {}{local}",
                waiting.short(),
                key.1,
                shorts(commits)
            );
            tell(core, &waiting.agent, format!("Your land of {on}.{stays}"));
            let landed = format!("Landed {} on {}{local}.", titles(&key.0, commits), key.1);
            tell_lead(core, &waiting.agent, landed, true);
        } else if sent_back(&waiting.agent) {
            tell_lead(
                core,
                &waiting.agent,
                format!(
                    "Landed {} on {}, but it clashed with the remote's newest and came off it again. It has been told to settle it and land again.",
                    titles(&key.0, commits),
                    key.1
                ),
                false,
            );
        } else {
            let said = (pushed.iter().filter_map(|pushed| pushed.note.as_deref()))
                .map(|note| format!(" {note}"))
                .collect::<String>();
            let on = format!(
                "{} is on {} and origin/{} as {}.{said}",
                waiting.short(),
                key.1,
                key.1,
                shorts(commits)
            );
            tell(core, &waiting.agent, format!("Your land of {on}{stays}"));
            let landed = format!(
                "Landed {} on {} and origin/{}.{said}",
                titles(&key.0, commits),
                key.1,
                key.1
            );
            tell_lead(core, &waiting.agent, landed, true);
        }
    }
}

type Landed = Vec<(Waiting, Vec<String>, Option<String>)>;

/// The lands put on the branch, each with the commits made for it and why its
/// checkout stayed behind when it did, those refused with why, and what was
/// pushed when any of them pushes.
struct Put {
    landed: Landed,
    refused: Vec<(Waiting, String)>,
    pushed: Option<super::push::Pushed>,
}

fn put_stack(core: &Core, key: &Key, stacked: Stack) -> Put {
    let (repo, base) = key;
    let signing = Signing::of(core);
    let mut stacked = stacked;
    let mut put = Put {
        landed: Vec::new(),
        refused: Vec::new(),
        pushed: None,
    };
    for _ in 0..ATTEMPTS {
        let tip = match git(repo, &["rev-parse", &format!("refs/heads/{base}")]) {
            Ok(tip) => line(tip),
            Err(_) => break,
        };
        if tip != stacked.tip {
            if checked_below(repo, base, &tip) != tip {
                return put;
            }
            let batch: Vec<Waiting> = (stacked.laid.iter())
                .map(|laid| laid.waiting.clone())
                .collect();
            stacked = match stack(&signing, key, &batch) {
                Ok(again) => again,
                Err(_) => break,
            };
            put.refused.append(&mut stacked.refused);
        }
        let Some(top) = stacked.laid.last().map(|laid| laid.tip.clone()) else {
            break;
        };
        let pushers: Vec<String> = (stacked.laid.iter())
            .filter(|laid| laid.waiting.push)
            .map(|laid| laid.waiting.agent.clone())
            .collect();
        let moved = match pushers.is_empty() {
            true => land::put(repo, base, &stacked.tip, &top).map(drop),
            false => land::planned(repo, base, &stacked.tip, &top).map(drop),
        };
        match moved {
            Ok(()) if pushers.is_empty() => {
                put.landed = landed(core, repo, &stacked);
                return put;
            }
            Ok(()) => {
                let landing = super::push::Landing {
                    branch: base,
                    from: &stacked.tip,
                    to: &top,
                };
                match super::push::land_and_push(core, repo, &pushers, "origin", &landing) {
                    Ok(pushed) => {
                        put.landed = landed(core, repo, &stacked);
                        put.pushed = Some(pushed);
                    }
                    Err(why) => {
                        let why = format!("pushing it failed, so nothing landed: {why}");
                        put.refused.extend(
                            (stacked.laid.into_iter())
                                .filter(|laid| laid.waiting.push)
                                .map(|laid| (laid.waiting, why.clone())),
                        );
                    }
                }
                return put;
            }
            Err(NotPut::Moved) => continue,
            Err(NotPut::Clash(paths)) => {
                let why = format!(
                    "the project's checkout has changes nobody committed in {} that clash with it. Find who made them with who_wrote on those paths and agree with them, then land again.",
                    paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let mut clashing = clashing(repo, &stacked, &paths);
                if clashing.is_empty() {
                    clashing = stacked.laid.into_iter().map(|laid| laid.waiting).collect();
                }
                (put.refused).extend(clashing.into_iter().map(|waiting| (waiting, why.clone())));
                return put;
            }
            Err(NotPut::Failed(why)) => {
                for laid in stacked.laid {
                    put.refused.push((laid.waiting, why.clone()));
                }
                return put;
            }
        }
    }
    put
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
        let stays = (waiting.checkout.as_deref())
            .and_then(|checkout| land::follow(checkout, &waiting.head, &laid.tip));
        core.build_outputs().landed(&waiting.agent);
        (core.repo_above().turn_state_committed)(core, &waiting.agent);
        let made = format!("{onto}..{}", laid.tip);
        let commits: Vec<(String, String)> =
            git(repo, &["log", "--reverse", "--format=%H %s", &made])
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.split_once(' '))
                .map(|(sha, subject)| (sha.to_owned(), subject.to_owned()))
                .collect();
        (core.repo_above().board_committed)(core, &project, &waiting.agent, &commits, true);
        onto = laid.tip.clone();
        landed.push((waiting.clone(), laid.commits.clone(), stays));
    }
    landed
}

/// Runs the land check on the commits of `base` up to `head` that no passing
/// check covered and the remote does not have, as the queue runs it on a land,
/// so commits made in the project's own checkout pass it before they push.
pub async fn check_before_push(
    core: &Arc<Core>,
    repo: &Path,
    base: &str,
    head: &str,
    agent: &str,
) -> Result<(), String> {
    let key: Key = (repo.to_owned(), base.to_owned());
    let from = {
        let (repo, base, head) = (repo.to_owned(), base.to_owned(), head.to_owned());
        blocking(move || Ok(checked_below(&repo, &base, &head)))
            .await
            .map_err(|error| error.to_string())?
    };
    if from == head {
        return Ok(());
    }
    let _turn = merge_queue::checkout_turn(true).await;
    let checked = check_from(core, &key, &from, head, agent).await;
    crate::server::worktree::agent::trim_merge_queue(repo).await;
    checked
}

/// [`check_before_push`] of the commits from `from` to `head`, while it holds the checkout.
async fn check_from(
    core: &Arc<Core>,
    key: &Key,
    from: &str,
    head: &str,
    agent: &str,
) -> Result<(), String> {
    let repo = &key.0;
    let due = {
        let (repo, from, head) = (repo.to_owned(), from.to_owned(), head.to_owned());
        blocking(move || Ok(land_checks_due(&repo, &from, &head)))
            .await
            .unwrap_or_default()
    };
    if let Some((check, files, why)) = land_checks_at(core, key, head, due).await {
        if land_checks_at(core, key, from, vec![(check, files)])
            .await
            .is_none()
        {
            return Err(why);
        }
    }
    let checkout = crate::server::worktree::agent::merge_queue(core, repo, head).await?;
    let command = {
        let (repo, from, head) = (repo.to_owned(), from.to_owned(), head.to_owned());
        blocking(move || rust_check(&repo, &checkout, &from, &head).map_err(Into::into))
            .await
            .map_err(|error| error.to_string())?
    };
    if let Some((label, command)) = command {
        let ran = checks::run_in_queue(core, repo, head, &command, agent).await?;
        if !ran.passed {
            return Err(format!(
                "{label} failed on the commits it would push, so they stay unpushed until a fix passes it:\n\n{}{}",
                crate::git::tail(&ran.output),
                keep_log(core, agent, &ran.output)
            ));
        }
    }
    mark_checked(key, head);
    Ok(())
}

fn checked_ref(base: &str) -> String {
    format!("refs/sikemux/checked/{base}")
}

/// Records that `commit` passed the land check for the branch.
fn mark_checked((repo, base): &Key, commit: &str) {
    let _ = git(repo, &["update-ref", &checked_ref(base), commit]);
}

/// The newest commit at or below `at` that passed the land check or is on the
/// remote, so a check from there covers commits put on the branch without one;
/// `at` itself when neither is known.
fn checked_below(repo: &Path, base: &str, at: &str) -> String {
    let is_below =
        |commit: &str, of: &str| git(repo, &["merge-base", "--is-ancestor", commit, of]).is_ok();
    [checked_ref(base), format!("refs/remotes/origin/{base}")]
        .iter()
        .filter_map(|name| {
            let commit = line(
                git(
                    repo,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("{name}^{{commit}}"),
                    ],
                )
                .ok()?,
            );
            is_below(&commit, at).then_some(commit)
        })
        .reduce(|older, newer| match is_below(&older, &newer) {
            true => newer,
            false => older,
        })
        .unwrap_or_else(|| at.to_owned())
}

/// The first land of `stacked` that fails one of the `landChecks` its `sikemux.json` names for
/// what it changes, with why. Each runs in the merge-queue checkout at that land.
async fn failing_land_check<'a>(
    core: &Arc<Core>,
    key: &Key,
    stacked: &'a Stack,
    ranges: &[(String, String)],
) -> Option<(&'a Laid, String)> {
    for (laid, (from, to)) in stacked.laid.iter().zip(ranges) {
        let (repo, range) = (key.0.clone(), (from.clone(), to.clone()));
        let due = blocking(move || Ok(land_checks_due(&repo, &range.0, &range.1)))
            .await
            .unwrap_or_default();
        if due.is_empty() {
            continue;
        }
        let Some((check, files, why)) = land_checks_at(core, key, to, due).await else {
            continue;
        };
        let (repo, at) = (key.0.clone(), from.clone());
        let older = blocking(move || {
            Ok((files.into_iter())
                .filter(|file| git(&repo, &["cat-file", "-e", &format!("{at}:{file}")]).is_ok())
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
        if older.is_empty() {
            return Some((laid, why));
        }
        if land_checks_at(core, key, from, vec![(check, older)])
            .await
            .is_none()
        {
            return Some((laid, why));
        }
    }
    None
}

/// The first of `due` that fails in the merge-queue checkout at `commit`, with its files and why.
async fn land_checks_at(
    core: &Arc<Core>,
    key: &Key,
    commit: &str,
    due: Vec<(LandCheck, Vec<String>)>,
) -> Option<(LandCheck, Vec<String>, String)> {
    let checkout = crate::server::worktree::agent::merge_queue(core, &key.0, commit)
        .await
        .ok()?;
    let (owner, project) = (core.clone(), key.0.clone());
    blocking(move || {
        let untrusted = super::generate::trust_needed_for(&owner, &project, &checkout, |config| {
            !config.land_checks.is_empty()
        });
        Ok(match untrusted {
            Some(_) => None,
            None => due.into_iter().find_map(|(check, files)| {
                let why = run_land_check(&checkout, &check, &files)?;
                Some((check, files, why))
            }),
        })
    })
    .await
    .ok()
    .flatten()
}

/// The `landChecks` of `sikemux.json` at `to` whose paths the change from `from` touches, each
/// with the files it touches.
fn land_checks_due(repo: &Path, from: &str, to: &str) -> Vec<(LandCheck, Vec<String>)> {
    let Ok(source) = git(
        repo,
        &[
            "show",
            &format!("{to}:{}", crate::tasks::config::CONFIG_FILE),
        ],
    ) else {
        return Vec::new();
    };
    let Ok(config) = serde_json::from_str(&source)
        .map_err(|_| ())
        .and_then(|value| crate::tasks::config::validate(&value).map_err(|_| ()))
        .map(|(config, _)| config)
    else {
        return Vec::new();
    };
    let Ok(changed) = git(
        repo,
        &["diff", "-z", "--name-only", "--diff-filter=AMR", from, to],
    ) else {
        return Vec::new();
    };
    let changed: Vec<&str> = changed
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    (config.land_checks.into_iter())
        .filter_map(|check| {
            let files: Vec<String> = (changed.iter())
                .filter(|path| check.when.set.is_match(path))
                .map(|path| (*path).to_owned())
                .collect();
            (!files.is_empty()).then_some((check, files))
        })
        .collect()
}

/// Why `check` fails in `checkout` on `files`, or none when it passes.
fn run_land_check(checkout: &Path, check: &LandCheck, files: &[String]) -> Option<String> {
    let output = shell(
        checkout,
        &check.command,
        &check.label,
        files,
        None,
        LAND_CHECK_LIMIT,
    )
    .err()?;
    Some(format!(
        "{} fails on it:\n\n{}\nFix that and land again.",
        check.label,
        crate::git::tail(&output)
    ))
}

/// Runs `script` in `dir` with `args` and `input` on standard input, its standard output on
/// success and all it wrote when it fails.
pub(super) fn shell(
    dir: &Path,
    script: &str,
    name: &str,
    args: &[String],
    input: Option<&[u8]>,
    limit: std::time::Duration,
) -> Result<String, String> {
    let mut shell = sikemux_process::user_environment::background_shell()?;
    shell
        .args(["-c", script, name])
        .args(args)
        .envs(crate::git_guard::shim::environment(None))
        // A land check's heavy commands start at once, so its time limit counts only its run.
        .env(sikemux_process::vars::BUILD_TURN, "1")
        .current_dir(dir);
    match sikemux_process::run(&mut shell, input, limit, METADATA_OUTPUT_LIMIT, None) {
        Ok(ran) if ran.status.success() => Ok(String::from_utf8_lossy(&ran.stdout).into_owned()),
        Ok(ran) => Err(format!(
            "{}{}",
            String::from_utf8_lossy(&ran.stdout),
            String::from_utf8_lossy(&ran.stderr)
        )),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) const LAND_CHECK_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The check for what changed from `from` to `to`, read in `checkout` at `to`,
/// with what to call it: rustfmt and `cargo check` of the crates it changes or whose
/// files they read, of every crate when it changes how they all build, none when it
/// changes no Rust. Clippy, the tests and the crates that use the changed ones are
/// left to CI, so a land takes minutes and a red main is fixed forward.
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
    let mut changed_crates = BTreeSet::new();
    for path in changed {
        let readers: Vec<&str> = (reads.iter())
            .filter(|(read, _)| path == read || path.starts_with(&format!("{read}/")))
            .map(|(_, name)| name.as_str())
            .collect();
        if !readers.is_empty() {
            changed_crates.extend(readers.into_iter().map(str::to_owned));
        } else if !path.ends_with(".md") && !path.ends_with(".mdx") {
            changed_crates.extend(owner(&members, path).map(|member| member.name.clone()));
        }
    }
    if !every && changed_crates.is_empty() {
        return Ok(None);
    }
    let check = |scope: &str| format!("cargo check {scope} --all-targets --all-features");
    Ok(Some(match every {
        true => (
            "rustfmt and cargo check on every crate".to_owned(),
            format!("cargo fmt --all --check && {}", check("--workspace")),
        ),
        false => {
            let flags = (changed_crates.iter())
                .map(|name| format!("-p {name}"))
                .collect::<Vec<_>>()
                .join(" ");
            let listed = changed_crates
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            (
                format!("rustfmt and cargo check on {listed}"),
                format!("cargo fmt --check {flags} && {}", check(&flags)),
            )
        }
    }))
}

const METADATA_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);
const METADATA_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

/// A crate of the workspace.
struct Member {
    name: String,
    /// Its folder from the workspace's root, empty for the root itself.
    dir: String,
}

/// The workspace's crates, as cargo reads them in `checkout`.
fn members(checkout: &Path) -> Result<Vec<Member>, String> {
    let mut shell = sikemux_process::user_environment::background_shell()?;
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
    let root = PathBuf::from(
        (metadata
            .get("workspace_root")
            .and_then(serde_json::Value::as_str))
        .unwrap_or_default(),
    );
    let in_workspace: BTreeSet<&str> = (metadata.get("workspace_members"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let packages = (metadata
        .get("packages")
        .and_then(serde_json::Value::as_array))
    .cloned()
    .unwrap_or_default();
    Ok(packages
        .iter()
        .filter(|package| (package["id"].as_str()).is_some_and(|id| in_workspace.contains(id)))
        .map(|package| {
            let manifest = PathBuf::from(package["manifest_path"].as_str().unwrap_or_default());
            let dir = (manifest.parent())
                .and_then(|dir| dir.strip_prefix(&root).ok())
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default();
            Member {
                name: package["name"].as_str().unwrap_or_default().to_owned(),
                dir,
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
