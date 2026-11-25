//! Moves commits of agents that work in checkouts of their own onto the branch
//! those were cut from, for the land queue. Their commits are made again on
//! that branch's tip, and the branch moves only when the project's own checkout
//! can take them: a file someone has changed there and not committed takes the
//! landed lines merged in, and a clash refuses the landing before anything
//! moves. Each agent's checkout then moves to its landed commits the same way,
//! keeping what it has not committed. Syncing moves the agent's checkout to the
//! branch's tip in that way too, its commits made again on it. A branch here
//! that is only behind origin's is taken at origin's tip.

use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::owners::{git, run};
use super::push::{is_main_checkout, line, on_release_line, replay, shared, Signing, Unpushed};
use crate::git::Git;
use crate::server::worktree::agent::linked_at;
use crate::server::Core;
use sikemux_view::workspace::AgentWorktree;

const ATTEMPTS: usize = 4;
const FETCH_WAIT: Duration = Duration::from_secs(10);
const EXECUTABLE: &str = "100755";
const LINK: &str = "120000";

/// One agent's commits made again on top of what lands before them.
pub struct Laid {
    pub listed: Vec<String>,
    /// The last of them.
    pub tip: String,
}

/// Why the branch did not move.
pub enum NotPut {
    /// It moved meanwhile.
    Moved,
    /// The project's checkout has uncommitted changes in these files that
    /// clash with what lands.
    Clash(Vec<PathBuf>),
    Failed(String),
}

#[derive(Serialize)]
pub struct Synced {
    checkout: PathBuf,
    /// The branch it was brought up to: the base branch here, or origin's.
    branch: String,
    /// The branch's commit the checkout now stands on.
    onto: String,
    /// The agent's commits, made again on it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    kept: Vec<String>,
    /// Files with uncommitted changes, which took the branch's lines merged in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    merged: Vec<String>,
    /// What origin has, or may have, that the checkout still lacks.
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// What a sync brings a checkout up to.
struct Newest {
    tip: String,
    /// The branch `tip` is on.
    name: String,
    /// How many of origin's commits `tip` lacks, as the two went different ways.
    lacks: usize,
}

/// The newest of `base` in the repository at `repo`: the branch here, or origin's as last
/// fetched when that only adds to it.
fn newest(repo: &Path, base: &str) -> Result<Newest, String> {
    let here = line(git(repo, &["rev-parse", &format!("refs/heads/{base}")])?);
    let of_here = |tip, lacks| Newest {
        tip,
        name: base.to_owned(),
        lacks,
    };
    let Some(theirs) = on_origin(repo, base) else {
        return Ok(of_here(here, 0));
    };
    let lacks = git(repo, &["rev-list", "--count", &format!("{here}..{theirs}")])?;
    let lacks: usize = lacks.trim().parse().map_err(|_| "git counted no commits")?;
    Ok(match lacks > 0 && contains(repo, &theirs, &here) {
        true => Newest {
            tip: theirs,
            name: format!("origin/{base}"),
            lacks: 0,
        },
        false => of_here(here, lacks),
    })
}

/// The commit lands of `heads` are laid on: `here`, the tip of `base` in the repository at
/// `repo`, or the newest of origin's commits past it that one of them was made on.
pub fn laid_on(repo: &Path, base: &str, here: &str, heads: &[&str]) -> String {
    let mut on = here.to_owned();
    let Some(theirs) = on_origin(repo, base) else {
        return on;
    };
    for head in heads {
        let shared = git(repo, &["merge-base", head, &theirs]).map(line);
        if let Some(shared) = shared.ok().filter(|shared| contains(repo, shared, &on)) {
            on = shared;
        }
    }
    on
}

fn on_origin(repo: &Path, base: &str) -> Option<String> {
    let origin = format!("refs/remotes/origin/{base}");
    git(repo, &["rev-parse", "--verify", "-q", &origin])
        .ok()
        .map(line)
}

fn contains(repo: &Path, commit: &str, earlier: &str) -> bool {
    git(repo, &["merge-base", "--is-ancestor", earlier, commit]).is_ok()
}

/// Fetches `base` from origin, when there is an origin that has it. Why origin could not be
/// read, when it could not.
fn fetch_base(repo: &Path, base: &str) -> Option<String> {
    crate::git::origin(repo)?;
    let ssh = sikemux_process::user_environment::var("GIT_SSH_COMMAND")
        .unwrap_or_else(|| "ssh -o BatchMode=yes -o ConnectTimeout=5".into());
    Git::at(repo)
        .timeout(FETCH_WAIT)
        .env("GIT_SSH_COMMAND", &ssh)
        .trimmed(&[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "origin",
            &format!("+refs/heads/{base}:refs/remotes/origin/{base}"),
        ])
        .err()
        .filter(|why| !why.contains("couldn't find remote ref"))
}

fn sync_note(base: &str, at: &Newest, unread: Option<&str>) -> Option<String> {
    let lacks = (at.lacks > 0).then(|| {
        let commits = if at.lacks == 1 { "commit" } else { "commits" };
        format!("origin/{base} has {} {commits} {base} here lacks, and {base} here has ones origin lacks. Sync compared with {base} here, without origin's; to go on from origin's instead, run git rebase origin/{base} in your checkout.", at.lacks)
    });
    let unread = unread.map(|why| {
        format!("origin could not be read, so {base} there may have newer commits your checkout lacks: {why}")
    });
    let said: Vec<String> = lacks.into_iter().chain(unread).collect();
    (!said.is_empty()).then(|| said.join(" "))
}

/// How a sync that clashes with `name` at `tip` says so, and what to do next.
fn clash_with(name: &str, tip: &str, note: Option<&str>) -> String {
    let says = format!(
        "onto {name} ({}), which changed the same lines. Run git rebase {name} in your checkout and settle the clash",
        short(tip)
    );
    note.map_or_else(|| says.clone(), |note| format!("{says}. {note}"))
}

/// What landing does to one file of the project's checkout.
enum Step {
    Write(Vec<u8>, String),
    Delete,
    /// Its bytes are right already; only the mode may not be.
    Keep(String),
}

struct DiskSnap {
    exists: bool,
    len: u64,
    modified: Option<SystemTime>,
    hash: Option<[u8; 32]>,
}

impl DiskSnap {
    fn capture(file: &Path, bytes: Option<&[u8]>) -> Self {
        let meta = std::fs::symlink_metadata(file).ok();
        Self {
            exists: meta.is_some(),
            len: meta.as_ref().map_or(0, std::fs::Metadata::len),
            modified: meta.and_then(|meta| meta.modified().ok()),
            hash: bytes.map(sha256),
        }
    }

    fn verify(&self, file: &Path) -> Result<(), Option<Vec<u8>>> {
        let meta = std::fs::symlink_metadata(file).ok();
        match (self.exists, meta) {
            (false, None) => return Ok(()),
            (true, Some(meta))
                if meta.len() == self.len && meta.modified().ok() == self.modified =>
            {
                return Ok(());
            }
            _ => {}
        }
        let current = on_disk(file);
        if current.as_deref().map(sha256) == self.hash {
            Ok(())
        } else {
            Err(current)
        }
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

struct StepEntry {
    path: String,
    step: Step,
    dirty: bool,
    snap: DiskSnap,
    old_blob: Option<String>,
    new_blob: Option<String>,
    new_bytes: Option<Vec<u8>>,
    new_mode: String,
}

/// What moving the project's checkout to new commits does to each file, and
/// whether someone had changed it there.
#[derive(Default)]
pub struct Steps(Vec<StepEntry>);

/// Carries out `steps` in the project's checkout at `repo`, which its branch
/// just moved from `before` to `after`, and brings its index along. The files
/// someone had changed, which took the new lines merged in.
pub fn apply(repo: &Path, steps: Steps, before: &str, after: &str) -> Vec<String> {
    let mut merged_paths = Vec::new();
    for entry in steps.0 {
        let file = repo.join(&entry.path);
        let (done, was_merged) = match entry.snap.verify(&file) {
            Ok(()) => {
                let done = match entry.step {
                    Step::Write(bytes, mode) => write(&file, &bytes, &mode),
                    Step::Delete => remove(&file),
                    Step::Keep(mode) => set_executable(&file, &mode),
                };
                (done, entry.dirty)
            }
            Err(current) => {
                if matches!(entry.step, Step::Keep(_)) || current == entry.new_bytes {
                    let done = if current.is_some() {
                        set_executable(&file, &entry.new_mode)
                    } else {
                        Ok(())
                    };
                    (done, false)
                } else if let (Some(now), Some(old_blob), Some(new_blob)) = (
                    current.as_deref(),
                    entry.old_blob.as_deref(),
                    entry.new_blob.as_deref(),
                ) {
                    match merged(repo, now, old_blob, new_blob) {
                        Ok(Some(bytes)) => (write(&file, &bytes, &entry.new_mode), true),
                        _ => {
                            eprintln!(
                                "sikemux core: {} changed while landing; left live edits on disk",
                                entry.path
                            );
                            (Ok(()), false)
                        }
                    }
                } else {
                    eprintln!(
                        "sikemux core: {} changed while landing; left live edits on disk",
                        entry.path
                    );
                    (Ok(()), false)
                }
            }
        };
        if let Err(error) = done {
            eprintln!(
                "sikemux core: {} did not take the new commits: {error}",
                entry.path
            );
        }
        if was_merged {
            merged_paths.push(entry.path);
        }
    }
    share_index(repo, before, after);
    merged_paths
}

pub fn own_checkout(core: &Core, agent_id: &str) -> Option<AgentWorktree> {
    core.workspaces()
        .agent(agent_id)
        .and_then(|agent| agent.worktree)
}

/// [`own_checkout`], when `dir` is in it.
pub fn own_checkout_of(core: &Core, agent_id: &str, dir: &Path) -> Option<AgentWorktree> {
    let own = own_checkout(core, agent_id)?;
    (top(&own.path).ok()? == top(dir).ok()?).then_some(own)
}

pub fn top(dir: &Path) -> Result<PathBuf, String> {
    Ok(PathBuf::from(line(git(
        dir,
        &["rev-parse", "--show-toplevel"],
    )?)))
}

/// Brings the worktree `dir` is in up to the tip of the branch it was cut from, here or on
/// origin, its commits made again on it.
pub fn sync(core: &Core, agent_id: &str, dir: &Path) -> Result<Synced, String> {
    let worktree = match own_checkout_of(core, agent_id, dir) {
        Some(own) => own,
        None => worktree_at(core, dir)?,
    };
    sync_from(&worktree, &Signing::of(core))
}

/// The worktree `dir` is in, when no agent has it as its own and the branch it was cut from is
/// its repository's shared one.
fn worktree_at(core: &Core, dir: &Path) -> Result<AgentWorktree, String> {
    let top = top(dir)?;
    let shown = top.display();
    if is_main_checkout(&top) {
        let held = (core.repo_above().harness_running_in)(core, &top).map_or_else(String::new, |process| {
            format!(" It is held back now: {process} runs in it. Stop that process, or pull by hand.")
        });
        return Err(format!("{shown} is its repository's own checkout. Sync is for an agent's own checkout; Sikemux fast-forwards this one by itself every ten minutes while it is clean, on its default branch and has no commits of its own, and no process started in Sikemux runs in it.{held} To sync a worktree, name a path in it in paths."));
    }
    let agents = core.workspaces().view().agents;
    let owner = agents.iter().find(|agent| {
        let own = agent.worktree.as_ref();
        own.is_some_and(|tree| self::top(&tree.path).is_ok_and(|own| own == top))
    });
    if let Some(owner) = owner {
        return Err(format!(
            "{shown} is the own checkout of {}, which alone can sync it; ask it with agent_send",
            crate::server::names::called_now(core, &owner.id)
        ));
    }
    let shares = |branch: &str| shared(&top, "origin", branch);
    linked_at(&top)
        .filter(|tree| tree.base.as_deref().is_some_and(shares))
        .filter(|tree| !shares(&tree.branch) && !on_release_line(&top))
        .ok_or_else(|| format!("Not synced: which branch {shown} was cut from can't be told. Run git rebase onto that branch there."))
}

/// Whether `worktree` holds commits its base branch lacks.
pub fn unlanded(worktree: &AgentWorktree) -> Result<bool, String> {
    let Some(base) = worktree.base.as_deref() else {
        return Ok(false);
    };
    let tip = line(git(
        &worktree.repo,
        &["rev-parse", &format!("refs/heads/{base}")],
    )?);
    let head = line(git(&worktree.path, &["rev-parse", "HEAD"])?);
    Ok(!own_commits(&worktree.path, base, &tip, &head, None)?.is_empty())
}

/// The commits of `head` that `tip` lacks, by their changes, oldest first, after `since` when
/// given. Commits the remote holds on `base` or its main are never the caller's, however far
/// behind the branch here is.
fn own_commits(
    tree: &Path,
    base: &str,
    tip: &str,
    head: &str,
    since: Option<&str>,
) -> Result<Vec<Unpushed>, String> {
    let not_since = since.map(|since| format!("^{since}"));
    let not_mine: Vec<String> = (not_since.iter().cloned())
        .chain(
            on_remote(tree, base)
                .iter()
                .map(|commit| format!("^{commit}")),
        )
        .collect();
    let range = format!("{tip}..{head}");
    let mine = git(
        tree,
        &[
            &["rev-list", "--parents", range.as_str()][..],
            &not_mine.iter().map(String::as_str).collect::<Vec<_>>(),
        ]
        .concat(),
    )?;
    let landed = |merged: &str| git(tree, &["merge-base", "--is-ancestor", merged, tip]).is_ok();
    let brings_unlanded = |commit: &str| commit.split(' ').skip(2).any(|merged| !landed(merged));
    if mine.lines().any(brings_unlanded) {
        return Err("Your commits merge in commits the branch they land on lacks, which the land queue doesn't take; rebase them onto that branch so they form one line, then land again".into());
    }
    let mine: HashSet<&str> = mine
        .lines()
        .filter_map(|commit| commit.split(' ').next())
        .collect();
    let listed = git(
        tree,
        &[
            "log",
            "--reverse",
            "--cherry-pick",
            "--right-only",
            "--format=%H%x00%s",
            &format!("{tip}...{head}"),
        ]
        .into_iter()
        .chain(not_since.as_deref())
        .collect::<Vec<_>>(),
    )?;
    Ok(listed
        .lines()
        .filter_map(|entry| entry.split_once('\0'))
        .filter(|(id, _)| mine.contains(id))
        .map(|(id, subject)| Unpushed {
            id: id.to_owned(),
            mine: true,
            by: String::new(),
            subject: subject.to_owned(),
            waits: false,
        })
        .collect())
}

/// What `base` is on the remote, by its upstream or its namesake there, and the remote's main.
fn on_remote(tree: &Path, base: &str) -> Vec<String> {
    let named = [
        format!("{base}@{{upstream}}"),
        format!("refs/remotes/origin/{base}"),
        "refs/remotes/origin/HEAD".to_owned(),
    ];
    let mut found: Vec<String> = (named.iter())
        .filter_map(|name| {
            git(
                tree,
                &["rev-parse", "--verify", "-q", &format!("{name}^{{commit}}")],
            )
            .ok()
        })
        .map(line)
        .collect();
    found.sort();
    found.dedup();
    found
}

pub fn listed(commits: &[Unpushed]) -> Vec<String> {
    commits
        .iter()
        .map(|commit| format!("{} {}", short(&commit.id), commit.subject))
        .collect()
}

fn short(id: &str) -> String {
    id.get(..8).unwrap_or(id).to_owned()
}

fn sync_from(worktree: &AgentWorktree, signing: &Signing) -> Result<Synced, String> {
    let base = worktree
        .base
        .clone()
        .ok_or("Your checkout was cut from no branch, so there is nothing to sync with")?;
    let unread = fetch_base(&worktree.repo, &base);
    for _ in 0..ATTEMPTS {
        let at = newest(&worktree.repo, &base)?;
        let note = sync_note(&base, &at, unread.as_deref());
        let (tip, name) = (at.tip, at.name);
        let head = line(git(&worktree.path, &["rev-parse", "HEAD"])?);
        let commits = own_commits(&worktree.path, &base, &tip, &head, None)?;
        let synced = replay(
            signing,
            &worktree.path,
            &tip,
            &commits.iter().collect::<Vec<_>>(),
            &clash_with(&name, &tip, note.as_deref()),
        )?;
        if synced == head {
            return Ok(Synced {
                checkout: worktree.path.clone(),
                branch: name,
                onto: short(&tip),
                kept: Vec::new(),
                merged: Vec::new(),
                note,
            });
        }
        let (steps, clashes) = plan(&worktree.path, &head, &synced)?;
        if !clashes.is_empty() {
            return Err(format!(
                "Not synced: what you have not committed in {} clashes with {name}. Commit it, then sync again.",
                clashes.join(", ")
            ));
        }
        let reflog = format!("sync: onto {name}");
        if run(
            &worktree.path,
            &["update-ref", "-m", &reflog, "HEAD", &synced, &head],
            None,
            None,
        )
        .is_err()
        {
            continue;
        }
        return Ok(Synced {
            checkout: worktree.path.clone(),
            branch: name,
            onto: short(&tip),
            kept: listed(&commits),
            merged: apply(&worktree.path, steps, &head, &synced),
            note,
        });
    }
    Err("your checkout kept moving while syncing; call this again".into())
}

pub struct Followed {
    pub stays: Option<String>,
    pub rebased: HashMap<String, String>,
}

/// Moves the agent's checkout at `tree` from `before` to `after`, as landing
/// moves the project's, keeping what it has not committed and rebasing any
/// follow-up commits made on top of `before`.
pub fn follow(signing: &Signing, tree: &Path, before: &str, after: &str) -> Followed {
    let stays = |why: String| {
        Followed {
        stays: Some(format!(
            "Your checkout stays on your old commits: {why}. Then call git_commit with sync: true to bring it up to the landed ones"
        )),
        rebased: HashMap::new(),
    }
    };
    for _ in 0..ATTEMPTS {
        let head = match git(tree, &["rev-parse", "HEAD"]) {
            Ok(head) => line(head),
            Err(error) => return stays(error),
        };
        if head == after {
            return Followed {
                stays: None,
                rebased: HashMap::new(),
            };
        }
        let (target, rebased) = if head == before {
            (after.to_owned(), HashMap::new())
        } else if contains(tree, &head, before) {
            let range = format!("{before}..{head}");
            let has_merges = git(tree, &["rev-list", "--merges", "--count", &range])
                .ok()
                .is_some_and(|count| count.trim() != "0");
            if has_merges {
                return stays("it has merge commits on top of the landed ones".into());
            }
            let listed = match git(tree, &["log", "--reverse", "--format=%H%x00%s", &range]) {
                Ok(listed) => listed,
                Err(error) => return stays(error),
            };
            let commits: Vec<Unpushed> = listed
                .lines()
                .filter_map(|entry| entry.split_once('\0'))
                .map(|(id, subject)| Unpushed {
                    id: id.to_owned(),
                    mine: true,
                    by: String::new(),
                    subject: subject.to_owned(),
                    waits: false,
                })
                .collect();
            let mut onto = after.to_owned();
            let mut map = HashMap::new();
            for commit in &commits {
                match replay(signing, tree, &onto, &[commit], "onto the landed commits") {
                    Ok(next) => {
                        map.insert(commit.id.clone(), next.clone());
                        onto = next;
                    }
                    Err(why) => return stays(why),
                }
            }
            (onto, map)
        } else {
            return stays("it moved while landing".into());
        };
        let (steps, clashes) = match plan(tree, &head, &target) {
            Ok(planned) => planned,
            Err(error) => return stays(error),
        };
        if !clashes.is_empty() {
            return stays(format!(
                "what you have not committed in {} clashes with them; commit it",
                clashes.join(", ")
            ));
        }
        let reflog = "land: follow the landed commits";
        if run(
            tree,
            &["update-ref", "-m", reflog, "HEAD", &target, &head],
            None,
            None,
        )
        .is_ok()
        {
            apply(tree, steps, &head, &target);
            return Followed {
                stays: None,
                rebased,
            };
        }
    }
    stays("it moved while landing".into())
}

/// The commits of `head` that `onto` lacks, after `since` when given, made again on it in the
/// repository at `repo`, their footers signed again with `signing`. None when `onto` holds
/// every one of their changes, even as commits made again elsewhere.
pub fn lay(
    signing: &Signing,
    repo: &Path,
    head: &str,
    since: Option<&str>,
    onto: &str,
    base: &str,
) -> Result<Option<Laid>, String> {
    let commits = own_commits(repo, base, onto, head, since)?;
    if commits.is_empty() {
        return Ok(None);
    }
    let tip = replay(
        signing,
        repo,
        onto,
        &commits.iter().collect::<Vec<_>>(),
        &format!("onto {base} ({}), which changed the same lines since your checkout was cut. Run git_commit with sync: true, settle the clash, then land again", short(onto)),
    )?;
    Ok((tip != onto).then(|| Laid {
        listed: listed(&commits),
        tip,
    }))
}

/// Moves `base` in the repository at `repo` from `from` to `to`, bringing the
/// project's checkout along when it has `base` out. The files someone had
/// changed there, which took the new lines merged in.
pub fn put(repo: &Path, base: &str, from: &str, to: &str) -> Result<Vec<String>, NotPut> {
    let steps = planned(repo, base, from, to)?;
    let target = format!("refs/heads/{base}");
    run(
        repo,
        &[
            "update-ref",
            "-m",
            "land: the land queue",
            &target,
            to,
            from,
        ],
        None,
        None,
    )
    .map_err(|_| NotPut::Moved)?;
    Ok(match steps {
        Some(steps) => apply(repo, steps, from, to),
        None => Vec::new(),
    })
}

/// What moving `base` from `from` to `to` does to the project's checkout at
/// `repo`, none when it does not have `base` out. A clash with what someone
/// changed there refuses it.
pub fn planned(repo: &Path, base: &str, from: &str, to: &str) -> Result<Option<Steps>, NotPut> {
    let target = format!("refs/heads/{base}");
    if !git(repo, &["symbolic-ref", "-q", "HEAD"]).is_ok_and(|head| head.trim() == target) {
        return Ok(None);
    }
    let (steps, clashes) = plan(repo, from, to).map_err(NotPut::Failed)?;
    if !clashes.is_empty() {
        return Err(NotPut::Clash(
            clashes.iter().map(|path| repo.join(path)).collect(),
        ));
    }
    Ok(Some(steps))
}

/// [`planned`], with a clash said in words.
pub fn steps(repo: &Path, base: &str, before: &str, after: &str) -> Result<Option<Steps>, String> {
    planned(repo, base, before, after).map_err(|not| match not {
        NotPut::Clash(paths) => format!(
            "Not landed: the project's checkout has changes nobody committed in {} that clash with yours. Find who made them with who_wrote on those paths and agree with them, then land again.",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        NotPut::Failed(why) => why,
        NotPut::Moved => format!("{base} moved meanwhile"),
    })
}

fn commit_tree<'a>(git_repo: &'a git2::Repository, rev: &str) -> Result<git2::Tree<'a>, String> {
    git_repo
        .revparse_single(rev)
        .and_then(|obj| obj.peel_to_commit())
        .and_then(|commit| commit.tree())
        .map_err(|error| format!("git tree {rev}: {error}"))
}

fn format_mode(mode: git2::FileMode) -> String {
    format!("{:06o}", u32::from(mode))
}

/// What each file changed from `before` to `after` needs in the checkout at
/// `repo`, and the files whose uncommitted changes there clash with it.
fn plan(repo: &Path, before: &str, after: &str) -> Result<(Steps, Vec<String>), String> {
    let git_repo = git2::Repository::open(repo).map_err(|error| format!("git open: {error}"))?;
    let before_tree = commit_tree(&git_repo, before)?;
    let after_tree = commit_tree(&git_repo, after)?;
    let mut opts = git2::DiffOptions::new();
    opts.include_typechange(true);
    let diff = git_repo
        .diff_tree_to_tree(Some(&before_tree), Some(&after_tree), Some(&mut opts))
        .map_err(|error| format!("git diff: {error}"))?;
    let mut steps = Vec::new();
    let mut clashes = Vec::new();
    for delta in diff.deltas() {
        let old_file = delta.old_file();
        let new_file = delta.new_file();
        let Some(path) = new_file
            .path()
            .or_else(|| old_file.path())
            .and_then(|p| p.to_str())
        else {
            continue;
        };
        if old_file.mode() == git2::FileMode::Commit || new_file.mode() == git2::FileMode::Commit {
            continue;
        }
        let old_oid = (!old_file.id().is_zero()).then_some(old_file.id());
        let new_oid = (!new_file.id().is_zero()).then_some(new_file.id());
        let old_bytes = old_oid
            .map(|oid| {
                git_repo
                    .find_blob(oid)
                    .map(|blob| blob.content().to_vec())
                    .map_err(|error| format!("git blob {oid}: {error}"))
            })
            .transpose()?;
        let new_bytes = new_oid
            .map(|oid| {
                git_repo
                    .find_blob(oid)
                    .map(|blob| blob.content().to_vec())
                    .map_err(|error| format!("git blob {oid}: {error}"))
            })
            .transpose()?;
        let file_path = repo.join(path);
        let work = on_disk(&file_path);
        let snap = DiskSnap::capture(&file_path, work.as_deref());
        let old_blob = old_oid.map(|oid| oid.to_string());
        let new_blob = new_oid.map(|oid| oid.to_string());
        let new_mode = new_oid
            .map(|_| format_mode(new_file.mode()))
            .unwrap_or_default();
        let (step, dirty) = if work == old_bytes {
            match new_bytes.clone() {
                Some(bytes) => (Step::Write(bytes, new_mode.clone()), false),
                None => (Step::Delete, false),
            }
        } else if work == new_bytes {
            (Step::Keep(new_mode.clone()), false)
        } else {
            match (work, old_blob.as_deref(), new_blob.as_deref()) {
                (Some(work), Some(old), Some(new)) => match merged(repo, &work, old, new)? {
                    Some(bytes) => (Step::Write(bytes, new_mode.clone()), true),
                    None => {
                        clashes.push(path.to_owned());
                        continue;
                    }
                },
                _ => {
                    clashes.push(path.to_owned());
                    continue;
                }
            }
        };
        steps.push(StepEntry {
            path: path.to_owned(),
            step,
            dirty,
            snap,
            old_blob,
            new_blob,
            new_bytes,
            new_mode,
        });
    }
    Ok((Steps(steps), clashes))
}

/// What the checkout holds at `file`, in the terms a commit does: a link is
/// the path it points at.
fn on_disk(file: &Path) -> Option<Vec<u8>> {
    match std::fs::read_link(file) {
        Ok(target) => Some(crate::os::link_target_bytes(&target)),
        Err(_) => std::fs::read(file).ok(),
    }
}

fn write(file: &Path, bytes: &[u8], mode: &str) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if mode == LINK || std::fs::read_link(file).is_ok() {
        remove(file)?;
    }
    if mode == LINK {
        return crate::os::write_link(bytes, file);
    }
    std::fs::write(file, bytes)?;
    set_executable(file, mode)
}

fn remove(file: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        done => done,
    }
}

/// Turns the execute bits on or off to match a commit's `mode`, as git does,
/// leaving the rest of the file's permissions as they are.
fn set_executable(file: &Path, mode: &str) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(file)?;
    if !meta.is_file() {
        return Ok(());
    }
    crate::os::set_executable(file, &meta, mode == EXECUTABLE)
}

fn contents(repo: &Path, blob: &str) -> Result<Vec<u8>, String> {
    run(repo, &["cat-file", "blob", blob], None, None)
}

/// `work` with the change from blob `old` to blob `new` merged in, or none
/// where they clash.
fn merged(repo: &Path, work: &[u8], old: &str, new: &str) -> Result<Option<Vec<u8>>, String> {
    let work = line(
        String::from_utf8_lossy(&run(
            repo,
            &["hash-object", "-w", "--stdin"],
            None,
            Some(work),
        )?)
        .into_owned(),
    );
    match git(repo, &["merge-file", "--object-id", &work, old, new]) {
        Ok(id) => contents(repo, id.trim()).map(Some),
        Err(_) => Ok(None),
    }
}

/// Brings the project's index up to the landed commits wherever it held the
/// branch's old copy of a file, so nothing shows as undoing the landing, and
/// leaves alone whatever someone staged.
fn share_index(repo: &Path, before: &str, after: &str) {
    let Ok(git_repo) = git2::Repository::open(repo) else {
        return;
    };
    let (Ok(before_tree), Ok(after_tree), Ok(index)) = (
        commit_tree(&git_repo, before),
        commit_tree(&git_repo, after),
        git_repo.index(),
    ) else {
        return;
    };
    let mut opts = git2::DiffOptions::new();
    opts.include_typechange(true);
    let Ok(diff) =
        git_repo.diff_tree_to_tree(Some(&before_tree), Some(&after_tree), Some(&mut opts))
    else {
        return;
    };
    let mut info = String::new();
    for delta in diff.deltas() {
        let old_file = delta.old_file();
        let new_file = delta.new_file();
        let Some(path) = new_file
            .path()
            .or_else(|| old_file.path())
            .and_then(|p| p.to_str())
        else {
            continue;
        };
        let path_ref = Path::new(path);
        if index.get_path(path_ref, 1).is_some() {
            continue;
        }
        let staged = index.get_path(path_ref, 0).map(|entry| entry.id);
        let old_oid = (!old_file.id().is_zero()).then_some(old_file.id());
        if staged != old_oid {
            continue;
        }
        if !new_file.id().is_zero() {
            info.push_str(&format!(
                "{} {}\t{path}\0",
                format_mode(new_file.mode()),
                new_file.id()
            ));
        } else {
            info.push_str(&format!("0 {}\t{path}\0", "0".repeat(40)));
        }
    }
    if info.is_empty() {
        return;
    }
    if let Err(error) = run(
        repo,
        &["update-index", "-z", "--index-info"],
        None,
        Some(info.as_bytes()),
    ) {
        eprintln!("sikemux core: the index did not take the landed commits: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::edits::tests_git::git_in;

    /// Lands `worktree` by itself, as the land queue lands a batch of one: the
    /// files that took the landed lines merged in.
    fn land_from(worktree: &AgentWorktree, signing: &Signing) -> Result<Vec<String>, String> {
        let base = worktree.base.as_deref().unwrap_or_default();
        let tip = line(git(&worktree.repo, &["rev-parse", base])?);
        let head = line(git(&worktree.path, &["rev-parse", "HEAD"])?);
        let laid =
            lay(signing, &worktree.repo, &head, None, &tip, base)?.ok_or("nothing to land")?;
        let merged = put(&worktree.repo, base, &tip, &laid.tip).map_err(|not| match not {
            NotPut::Clash(paths) => format!("{paths:?}"),
            NotPut::Moved => "moved".to_owned(),
            NotPut::Failed(why) => why,
        })?;
        follow(signing, &worktree.path, &head, &laid.tip);
        Ok(merged)
    }

    #[test]
    fn landing_merges_into_files_others_changed_and_refuses_a_clash() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let main = root.join("project");
        std::fs::create_dir(&main).unwrap();
        git_in(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
        std::fs::write(main.join("b.txt"), "b\n").unwrap();
        git_in(&main, &["add", "."]);
        git_in(&main, &["commit", "-qm", "base"]);
        let start_sha = git_in(&main, &["rev-parse", "HEAD"]);
        let tree = root.join("tree");
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "sikemux/work",
                &tree.to_string_lossy(),
            ],
        );
        std::fs::write(tree.join("a.txt"), "ONE\ntwo\nthree\nfour\nfive\n").unwrap();
        git_in(&tree, &["commit", "-qam", "worker: first line"]);
        std::fs::write(main.join("a.txt"), "one\ntwo\nthree\nfour\nFIVE\n").unwrap();
        std::fs::write(main.join("b.txt"), "b changed by someone\n").unwrap();
        let worktree = AgentWorktree {
            repo: main.clone(),
            path: tree.clone(),
            branch: "sikemux/work".into(),
            base: Some("main".into()),
            start_sha,
            name: String::new(),
            first_port: None,
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let landed = land_from(&worktree, &signing).unwrap();
        assert_eq!(landed, ["a.txt"]);
        assert_eq!(
            git_in(&main, &["log", "-1", "--format=%s", "main"]),
            "worker: first line"
        );
        let a = std::fs::read_to_string(main.join("a.txt")).unwrap();
        assert_eq!(a, "ONE\ntwo\nthree\nfour\nFIVE\n");
        assert_eq!(
            std::fs::read_to_string(main.join("b.txt")).unwrap(),
            "b changed by someone\n"
        );
        assert_eq!(git_in(&main, &["diff", "--cached", "--name-only"]), "");
        assert_eq!(
            git_in(&tree, &["rev-parse", "HEAD"]),
            git_in(&main, &["rev-parse", "main"])
        );

        std::fs::write(
            tree.join("a.txt"),
            "ONE\ntwo\nthree\nfour\nfive, the worker's\n",
        )
        .unwrap();
        git_in(&tree, &["commit", "-qam", "worker: last line"]);
        let before = git_in(&main, &["rev-parse", "main"]);
        assert!(land_from(&worktree, &signing)
            .err()
            .unwrap_or_default()
            .contains("a.txt"));
        assert_eq!(git_in(&main, &["rev-parse", "main"]), before);
    }

    #[test]
    fn landing_a_commit_a_push_already_made_again_adds_no_empty_copy() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let main = root.join("project");
        std::fs::create_dir(&main).unwrap();
        git_in(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "one\ntwo\nthree\nfour\nfive\nsix\n").unwrap();
        git_in(&main, &["add", "."]);
        git_in(&main, &["commit", "-qm", "base"]);
        let start_sha = git_in(&main, &["rev-parse", "HEAD"]);
        let tree = root.join("tree");
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "sikemux/work",
                &tree.to_string_lossy(),
            ],
        );
        std::fs::write(tree.join("a.txt"), "one\ntwo\nthree\nFOUR\nfive\nsix\n").unwrap();
        git_in(&tree, &["commit", "-qam", "worker: fourth line"]);
        std::fs::write(main.join("a.txt"), "ONE\ntwo\nthree\nfour\nfive\nsix\n").unwrap();
        git_in(&main, &["commit", "-qam", "pushed first: first line"]);
        std::fs::write(main.join("a.txt"), "ONE\ntwo\nthree\nFOUR\nfive\nsix\n").unwrap();
        git_in(&main, &["commit", "-qam", "worker: fourth line"]);
        let before = git_in(&main, &["rev-parse", "main"]);
        let worktree = AgentWorktree {
            repo: main.clone(),
            path: tree.clone(),
            branch: "sikemux/work".into(),
            base: Some("main".into()),
            start_sha,
            name: String::new(),
            first_port: None,
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let head = git_in(&tree, &["rev-parse", "HEAD"]);
        assert!(lay(&signing, &main, &head, None, &before, "main")
            .unwrap()
            .is_none());
        assert!(land_from(&worktree, &signing).is_err());
        assert_eq!(git_in(&main, &["rev-parse", "main"]), before);
    }

    #[cfg(unix)]
    #[test]
    fn landing_gives_each_file_the_mode_its_commit_holds() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let main = root.join("project");
        std::fs::create_dir(&main).unwrap();
        git_in(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("becomes-executable.sh"), "echo\n").unwrap();
        std::fs::write(main.join("edited.sh"), "one\n").unwrap();
        std::fs::set_permissions(
            main.join("edited.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        git_in(&main, &["add", "."]);
        git_in(&main, &["commit", "-qm", "base"]);
        let start_sha = git_in(&main, &["rev-parse", "HEAD"]);
        let tree = root.join("tree");
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "sikemux/work",
                &tree.to_string_lossy(),
            ],
        );
        std::fs::write(tree.join("new.sh"), "echo\n").unwrap();
        std::fs::set_permissions(tree.join("new.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::set_permissions(
            tree.join("becomes-executable.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::write(tree.join("edited.sh"), "two\n").unwrap();
        std::os::unix::fs::symlink("new.sh", tree.join("link")).unwrap();
        git_in(&tree, &["add", "."]);
        git_in(&tree, &["commit", "-qm", "worker"]);
        let worktree = AgentWorktree {
            repo: main.clone(),
            path: tree,
            branch: "sikemux/work".into(),
            base: Some("main".into()),
            start_sha,
            name: String::new(),
            first_port: None,
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        land_from(&worktree, &signing).unwrap();
        let executable = |name: &str| {
            let mode = std::fs::metadata(main.join(name)).unwrap().permissions();
            mode.mode() & 0o111 != 0
        };
        assert!(executable("new.sh"));
        assert!(executable("becomes-executable.sh"));
        assert!(executable("edited.sh"));
        assert_eq!(
            std::fs::read_link(main.join("link")).unwrap(),
            Path::new("new.sh")
        );
        assert_eq!(git_in(&main, &["status", "--porcelain"]), "");
    }

    #[test]
    fn a_nested_repos_own_checkout_lands_and_pushes_while_its_checkout_is_mid_rebase() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let remote = root.join("remote.git");
        git_in(
            &root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                &remote.to_string_lossy(),
            ],
        );
        let project = root.join("project");
        let web = project.join("web");
        std::fs::create_dir_all(&web).unwrap();
        git_in(&web, &["init", "-q", "-b", "main"]);
        git_in(&web, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git_in(&web, &["commit", "-q", "--allow-empty", "-m", "second"]);
        git_in(
            &web,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        git_in(&web, &["push", "-q", "-u", "origin", "main"]);
        git_in(&web, &["remote", "set-head", "origin", "main"]);
        let stopped = sikemux_process::user_environment::command("git")
            .args(["rebase", "-q", "--exec", "false", "HEAD~1"])
            .current_dir(&web)
            .output()
            .unwrap();
        assert!(!stopped.status.success());

        let (worktree, _) =
            crate::server::worktree::agent::cut_for(&web, &project, "fix/client").unwrap();
        assert_eq!(worktree.base.as_deref(), Some("main"));
        std::fs::write(worktree.path.join("c.txt"), "c\n").unwrap();
        git_in(&worktree.path, &["add", "c.txt"]);
        git_in(&worktree.path, &["commit", "-qm", "worker: c"]);
        assert!(unlanded(&worktree).unwrap());

        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        land_from(&worktree, &signing).unwrap();
        assert_eq!(
            git_in(&web, &["log", "-1", "--format=%s", "main"]),
            "worker: c"
        );
    }

    #[test]
    fn merges_the_remote_holds_are_not_yours_when_the_branch_here_split_from_it() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let remote = root.join("remote.git");
        git_in(
            &root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                &remote.to_string_lossy(),
            ],
        );
        let main = root.join("project");
        std::fs::create_dir(&main).unwrap();
        git_in(&main, &["init", "-q", "-b", "main"]);
        git_in(&main, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git_in(
            &main,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        git_in(&main, &["push", "-q", "-u", "origin", "main"]);
        for squashed in ["l1", "l2"] {
            std::fs::write(main.join(format!("{squashed}.txt")), "x\n").unwrap();
            git_in(&main, &["add", "."]);
            git_in(&main, &["commit", "-qm", squashed]);
        }
        let stale = git_in(&main, &["rev-parse", "HEAD"]);

        let other = root.join("other");
        git_in(
            &root,
            &[
                "clone",
                "-q",
                &remote.to_string_lossy(),
                &other.to_string_lossy(),
            ],
        );
        for squashed in ["l1", "l2"] {
            std::fs::write(other.join(format!("{squashed}.txt")), "x\n").unwrap();
        }
        git_in(&other, &["add", "."]);
        git_in(&other, &["commit", "-qm", "l1 and l2, squashed"]);
        for merged in ["one", "two"] {
            git_in(&other, &["checkout", "-q", "-b", merged]);
            std::fs::write(other.join(format!("{merged}.txt")), "x\n").unwrap();
            git_in(&other, &["add", "."]);
            git_in(&other, &["commit", "-qm", merged]);
            git_in(&other, &["checkout", "-q", "main"]);
            git_in(
                &other,
                &[
                    "merge",
                    "-q",
                    "--no-ff",
                    "-m",
                    &format!("Merge {merged}"),
                    merged,
                ],
            );
        }
        git_in(&other, &["push", "-q", "origin", "main"]);
        git_in(&main, &["fetch", "-q", "origin"]);
        assert_eq!(git_in(&main, &["rev-parse", "main"]), stale);

        let tree = root.join("tree");
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "sikemux/work",
                &tree.to_string_lossy(),
                "origin/main",
            ],
        );
        for file in ["a", "b", "c"] {
            std::fs::write(tree.join(format!("{file}.txt")), "x\n").unwrap();
            git_in(&tree, &["add", "."]);
            git_in(&tree, &["commit", "-qm", &format!("worker: {file}")]);
        }
        let worktree = AgentWorktree {
            repo: main.clone(),
            path: tree.clone(),
            branch: "sikemux/work".into(),
            base: Some("main".into()),
            start_sha: stale.clone(),
            name: String::new(),
            first_port: None,
        };
        assert!(unlanded(&worktree).unwrap());
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let head = git_in(&tree, &["rev-parse", "HEAD"]);
        let laid = lay(&signing, &main, &head, None, &stale, "main")
            .unwrap()
            .unwrap();
        assert_eq!(laid.listed.len(), 3);
        assert!(laid.listed.iter().all(|commit| commit.contains("worker: ")));
    }

    #[test]
    fn syncing_brings_what_origin_has_and_the_base_here_lacks() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let remote = root.join("remote.git");
        let remote_path = remote.to_string_lossy().into_owned();
        git_in(&root, &["init", "-q", "--bare", "-b", "main", &remote_path]);
        let main = root.join("project");
        std::fs::create_dir(&main).unwrap();
        git_in(&main, &["init", "-q", "-b", "main"]);
        git_in(&main, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git_in(&main, &["remote", "add", "origin", &remote_path]);
        git_in(&main, &["push", "-q", "-u", "origin", "main"]);
        let start_sha = git_in(&main, &["rev-parse", "HEAD"]);
        let tree = root.join("tree");
        let tree_path = tree.to_string_lossy().into_owned();
        git_in(&main, &["worktree", "add", "-q", "-b", "work", &tree_path]);
        let other = root.join("other");
        git_in(
            &root,
            &["clone", "-q", &remote_path, &other.to_string_lossy()],
        );
        let commit = |dir: &Path, file: &str| {
            std::fs::write(dir.join(file), "x\n").unwrap();
            git_in(dir, &["add", file]);
            git_in(dir, &["commit", "-qm", file]);
        };
        commit(&other, "from-origin.txt");
        git_in(&other, &["push", "-q", "origin", "main"]);
        commit(&tree, "worker.txt");
        let worktree = AgentWorktree {
            repo: main.clone(),
            path: tree.clone(),
            branch: "work".into(),
            base: Some("main".into()),
            start_sha,
            name: String::new(),
            first_port: None,
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };

        let synced = sync_from(&worktree, &signing).unwrap();
        assert!(tree.join("from-origin.txt").exists());
        assert!(tree.join("worker.txt").exists());
        assert_eq!(synced.branch, "origin/main");

        commit(&main, "landed-unpushed.txt");
        let synced = sync_from(&worktree, &signing).unwrap();
        assert!(tree.join("landed-unpushed.txt").exists());
        assert!(tree.join("worker.txt").exists());
        assert!(synced.note.unwrap().contains("origin/main has 1 commit"));

        std::fs::rename(&remote, root.join("gone.git")).unwrap();
        let synced = sync_from(&worktree, &signing).unwrap();
        assert!(synced.note.unwrap().contains("could not be read"));
    }

    /// A stale base here refused commits made on origin's newer ones, as they clashed with it.
    #[test]
    fn syncing_goes_onto_origin_when_the_base_here_is_behind_commits_the_worker_builds_on() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let remote = root.join("remote.git");
        let remote_path = remote.to_string_lossy().into_owned();
        git_in(&root, &["init", "-q", "--bare", "-b", "main", &remote_path]);
        let main = root.join("project");
        std::fs::create_dir(&main).unwrap();
        git_in(&main, &["init", "-q", "-b", "main"]);
        let write = |dir: &Path, text: &str, subject: &str| {
            std::fs::write(dir.join("shared.txt"), text).unwrap();
            git_in(dir, &["add", "."]);
            git_in(dir, &["commit", "-qm", subject]);
        };
        write(&main, "one\n", "base");
        git_in(&main, &["remote", "add", "origin", &remote_path]);
        git_in(&main, &["push", "-q", "-u", "origin", "main"]);
        let start_sha = git_in(&main, &["rev-parse", "HEAD"]);
        let other = root.join("other");
        git_in(
            &root,
            &["clone", "-q", &remote_path, &other.to_string_lossy()],
        );
        write(&other, "two\n", "newer on origin");
        git_in(&other, &["push", "-q", "origin", "main"]);
        git_in(&main, &["fetch", "-q", "origin"]);
        let tree = root.join("tree");
        let tree_path = tree.to_string_lossy().into_owned();
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "work",
                &tree_path,
                "origin/main",
            ],
        );
        write(&tree, "three\n", "worker");
        let worktree = AgentWorktree {
            repo: main.clone(),
            path: tree.clone(),
            branch: "work".into(),
            base: Some("main".into()),
            start_sha: start_sha.clone(),
            name: String::new(),
            first_port: None,
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };

        assert_eq!(
            sync_from(&worktree, &signing).unwrap().branch,
            "origin/main"
        );
        assert_eq!(git_in(&main, &["rev-parse", "main"]), start_sha);

        let head = git_in(&tree, &["rev-parse", "HEAD"]);
        assert_eq!(
            laid_on(&main, "main", &start_sha, &[&head]),
            git_in(&main, &["rev-parse", "origin/main"])
        );

        git_in(&main, &["commit", "-q", "--allow-empty", "-m", "only here"]);
        let why = sync_from(&worktree, &signing).err().unwrap();
        assert!(why.contains("git rebase origin/main"), "{why}");
    }
}
