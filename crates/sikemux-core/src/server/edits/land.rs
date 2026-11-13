//! Moves commits of agents that work in checkouts of their own onto the branch
//! those were cut from, for the land queue. Their commits are made again on
//! that branch's tip, and the branch moves only when the project's own checkout
//! can take them: a file someone has changed there and not committed takes the
//! landed lines merged in, and a clash refuses the landing before anything
//! moves. Each agent's checkout then moves to its landed commits the same way,
//! keeping what it has not committed. Syncing moves the agent's checkout to the
//! branch's tip in that way too, its commits made again on it.

use crate::server::parts::Parts;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::owners::{git, run};
use super::push::{line, replay, Signing, Unpushed};
use crate::server::worktree::AgentWorktree;
use crate::server::Core;

const ATTEMPTS: usize = 4;
const EXECUTABLE: &str = "100755";
const LINK: &str = "120000";

/// One agent's commits made again on top of what lands before them.
pub(super) struct Laid {
    pub listed: Vec<String>,
    /// The last of them.
    pub tip: String,
}

/// Why the branch did not move.
pub(super) enum NotPut {
    /// It moved meanwhile.
    Moved,
    /// The project's checkout has uncommitted changes in these files that
    /// clash with what lands.
    Clash(Vec<PathBuf>),
    Failed(String),
}

#[derive(Serialize)]
pub(crate) struct Synced {
    branch: String,
    /// The branch's commit the checkout now stands on.
    onto: String,
    /// The agent's commits, made again on it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    kept: Vec<String>,
    /// Files with uncommitted changes, which took the branch's lines merged in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    merged: Vec<String>,
}

/// What landing does to one file of the project's checkout.
enum Step {
    Write(Vec<u8>, String),
    Delete,
    /// Its bytes are right already; only the mode may not be.
    Keep(String),
}

/// What moving the project's checkout to new commits does to each file, and
/// whether someone had changed it there.
#[derive(Default)]
pub(super) struct Steps(Vec<(String, Step, bool)>);

/// Carries out `steps` in the project's checkout at `repo`, which its branch
/// just moved from `before` to `after`, and brings its index along. The files
/// someone had changed, which took the new lines merged in.
pub(super) fn apply(repo: &Path, steps: Steps, before: &str, after: &str) -> Vec<String> {
    let mut merged = Vec::new();
    for (path, step, dirty) in steps.0 {
        let file = repo.join(&path);
        let done = match step {
            Step::Write(bytes, mode) => write(&file, &bytes, &mode),
            Step::Delete => remove(&file),
            Step::Keep(mode) => set_executable(&file, &mode),
        };
        if let Err(error) = done {
            eprintln!("sikemux core: {path} did not take the new commits: {error}");
        }
        if dirty {
            merged.push(path);
        }
    }
    share_index(repo, before, after);
    merged
}

pub(super) fn own_checkout(core: &Core, agent_id: &str) -> Option<AgentWorktree> {
    core.workspaces()
        .agent(agent_id)
        .and_then(|agent| agent.worktree)
}

/// Brings the calling agent's own checkout up to the tip of the branch it was
/// cut from, its commits made again on it.
pub(super) fn sync(core: &Core, agent_id: &str) -> Result<Synced, String> {
    let worktree = own_checkout(core, agent_id).ok_or("You work in the project's own checkout, which is on its branch already; there is nothing to sync")?;
    sync_from(&worktree, &Signing::of(core))
}

/// Why `worktree` holds commits its base branch lacks, which a push from the
/// base branch would leave out, when it does.
pub(crate) fn unlanded(worktree: &AgentWorktree) -> Result<Option<String>, String> {
    let Some(base) = worktree.base.as_deref() else {
        return Ok(None);
    };
    let tip = line(git(
        &worktree.repo,
        &["rev-parse", &format!("refs/heads/{base}")],
    )?);
    let head = line(git(&worktree.path, &["rev-parse", "HEAD"])?);
    let commits = own_commits(&worktree.path, &tip, &head)?;
    Ok((!commits.is_empty()).then(|| {
        format!(
            "{} of your commits are only on {}, your checkout's own branch; pass land: true to put them on {base} and push from there",
            commits.len(),
            worktree.branch
        )
    }))
}

/// The commits of `head` that `tip` lacks, by their changes, oldest first.
fn own_commits(tree: &Path, tip: &str, head: &str) -> Result<Vec<Unpushed>, String> {
    let range = format!("{tip}..{head}");
    if git(tree, &["rev-list", "--merges", "--count", &range])?.trim() != "0" {
        return Err("Your commits include a merge; land them by hand".into());
    }
    let listed = git(
        tree,
        &[
            "log",
            "--reverse",
            "--cherry-pick",
            "--right-only",
            "--format=%H%x00%s",
            &format!("{tip}...{head}"),
        ],
    )?;
    Ok(listed
        .lines()
        .filter_map(|entry| entry.split_once('\0'))
        .map(|(id, subject)| Unpushed {
            id: id.to_owned(),
            mine: true,
            by: String::new(),
            subject: subject.to_owned(),
        })
        .collect())
}

pub(super) fn listed(commits: &[Unpushed]) -> Vec<String> {
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
    let target = format!("refs/heads/{base}");
    for _ in 0..ATTEMPTS {
        let tip = line(git(&worktree.repo, &["rev-parse", &target])?);
        let head = line(git(&worktree.path, &["rev-parse", "HEAD"])?);
        let commits = own_commits(&worktree.path, &tip, &head)?;
        let synced = replay(
            signing,
            &worktree.path,
            &tip,
            &commits.iter().collect::<Vec<_>>(),
            &format!("onto {base}, which changed the same lines. Run git rebase {base} in your checkout and settle the clash"),
        )?;
        if synced == head {
            return Ok(Synced {
                branch: base,
                onto: short(&tip),
                kept: Vec::new(),
                merged: Vec::new(),
            });
        }
        let (steps, clashes) = plan(&worktree.path, &head, &synced)?;
        if !clashes.is_empty() {
            return Err(format!(
                "Not synced: what you have not committed in {} clashes with {base}. Commit it, then sync again.",
                clashes.join(", ")
            ));
        }
        let reflog = format!("sync: onto {base}");
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
            branch: base,
            onto: short(&tip),
            kept: listed(&commits),
            merged: apply(&worktree.path, steps, &head, &synced),
        });
    }
    Err("your checkout kept moving while syncing; call this again".into())
}

/// Moves the agent's checkout at `tree` from `before` to `after`, as landing
/// moves the project's, keeping what it has not committed. Why it stayed
/// where it was, when it did.
pub(super) fn follow(tree: &Path, before: &str, after: &str) -> Option<String> {
    let stays = |why: String| {
        Some(format!(
            "Your checkout stays on your old commits: {why}. Then call git_commit with sync: true to bring it up to the landed ones"
        ))
    };
    let (steps, clashes) = match plan(tree, before, after) {
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
        &["update-ref", "-m", reflog, "HEAD", after, before],
        None,
        None,
    )
    .is_err()
    {
        return stays("it moved while landing".into());
    }
    apply(tree, steps, before, after);
    None
}

/// The commits of `head` that `onto` lacks, made again on it in the repository
/// at `repo`, their footers signed again with `signing`. None when `onto` holds
/// every one of their changes, even as commits made again elsewhere.
pub(super) fn lay(
    signing: &Signing,
    repo: &Path,
    head: &str,
    onto: &str,
    base: &str,
) -> Result<Option<Laid>, String> {
    let commits = own_commits(repo, onto, head)?;
    if commits.is_empty() {
        return Ok(None);
    }
    let tip = replay(
        signing,
        repo,
        onto,
        &commits.iter().collect::<Vec<_>>(),
        &format!("onto {base}, which changed the same lines since your checkout was cut. Run git_commit with sync: true, settle the clash, then land again"),
    )?;
    Ok((tip != onto).then(|| Laid {
        listed: listed(&commits),
        tip,
    }))
}

/// Moves `base` in the repository at `repo` from `from` to `to`, bringing the
/// project's checkout along when it has `base` out. The files someone had
/// changed there, which took the new lines merged in.
pub(super) fn put(repo: &Path, base: &str, from: &str, to: &str) -> Result<Vec<String>, NotPut> {
    let target = format!("refs/heads/{base}");
    let checked_out =
        git(repo, &["symbolic-ref", "-q", "HEAD"]).is_ok_and(|head| head.trim() == target);
    let steps = match checked_out {
        true => {
            let (steps, clashes) = plan(repo, from, to).map_err(NotPut::Failed)?;
            if !clashes.is_empty() {
                return Err(NotPut::Clash(
                    clashes.iter().map(|path| repo.join(path)).collect(),
                ));
            }
            steps
        }
        false => Steps::default(),
    };
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
    Ok(match checked_out {
        true => apply(repo, steps, from, to),
        false => Vec::new(),
    })
}

/// What each file the landing changes needs in the project's checkout at
/// `repo`, from `before` to `after`, and whether someone had changed it there.
/// A clash with what someone changed refuses the landing.
pub(super) fn steps(repo: &Path, before: &str, after: &str) -> Result<Steps, String> {
    let (steps, clashes) = plan(repo, before, after)?;
    if !clashes.is_empty() {
        return Err(format!(
            "Not landed: the project's checkout has changes nobody committed in {} that clash with yours. Find who made them with who_wrote on those paths and agree with them, then land again.",
            clashes
                .iter()
                .map(|path| repo.join(path).display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(steps)
}

/// What each file changed from `before` to `after` needs in the checkout at
/// `repo`, and the files whose uncommitted changes there clash with it.
fn plan(repo: &Path, before: &str, after: &str) -> Result<(Steps, Vec<String>), String> {
    let changed = git(
        repo,
        &["diff", "-z", "--name-only", "--no-renames", before, after],
    )?;
    let mut steps = Vec::new();
    let mut clashes = Vec::new();
    for path in changed.split('\0').filter(|path| !path.is_empty()) {
        let old = entry(repo, before, path);
        let new = entry(repo, after, path);
        let work = on_disk(&repo.join(path));
        let old_bytes = old
            .as_ref()
            .map(|old| contents(repo, &old.blob))
            .transpose()?;
        let new_bytes = new
            .as_ref()
            .map(|new| contents(repo, &new.blob))
            .transpose()?;
        let new_mode = new.as_ref().map(|new| new.mode.clone()).unwrap_or_default();
        let step = if work == old_bytes {
            match new_bytes {
                Some(bytes) => (Step::Write(bytes, new_mode), false),
                None => (Step::Delete, false),
            }
        } else if work == new_bytes {
            (Step::Keep(new_mode), false)
        } else {
            match (work, old, new) {
                (Some(work), Some(old), Some(new)) => {
                    match merged(repo, &work, &old.blob, &new.blob)? {
                        Some(bytes) => (Step::Write(bytes, new_mode), true),
                        None => {
                            clashes.push(path.to_owned());
                            continue;
                        }
                    }
                }
                _ => {
                    clashes.push(path.to_owned());
                    continue;
                }
            }
        };
        steps.push((path.to_owned(), step.0, step.1));
    }
    Ok((Steps(steps), clashes))
}

/// A file as a commit holds it.
struct Entry {
    mode: String,
    blob: String,
}

fn entry(repo: &Path, commit: &str, path: &str) -> Option<Entry> {
    let listed = git(
        repo,
        &["ls-tree", commit, "--", &format!(":(top,literal){path}")],
    )
    .ok()?;
    let mut info = listed.split_whitespace();
    let mode = info.next()?.to_owned();
    let blob = info.nth(1)?.to_owned();
    Some(Entry { mode, blob })
}

/// What the checkout holds at `file`, in the terms a commit does: a link is
/// the path it points at.
fn on_disk(file: &Path) -> Option<Vec<u8>> {
    match std::fs::read_link(file) {
        Ok(target) => Some(target.as_os_str().as_bytes().to_vec()),
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
        return std::os::unix::fs::symlink(OsStr::from_bytes(bytes), file);
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
    let now = meta.permissions().mode();
    let wanted = match mode == EXECUTABLE {
        true => now | (now & 0o444) >> 2,
        false => now & !0o111,
    };
    if wanted == now {
        return Ok(());
    }
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(wanted))
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
    let Ok(changed) = git(
        repo,
        &["diff", "-z", "--name-only", "--no-renames", before, after],
    ) else {
        return;
    };
    let mut info = String::new();
    for path in changed.split('\0').filter(|path| !path.is_empty()) {
        let staged = git(
            repo,
            &["ls-files", "-s", "--", &format!(":(top,literal){path}")],
        )
        .ok()
        .and_then(|entry| entry.split_whitespace().nth(1).map(str::to_owned));
        if staged != entry(repo, before, path).map(|old| old.blob) {
            continue;
        }
        match entry(repo, after, path) {
            Some(new) => info.push_str(&format!("{} {}\t{path}\0", new.mode, new.blob)),
            None => info.push_str(&format!("0 {}\t{path}\0", "0".repeat(40))),
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
        let laid = lay(signing, &worktree.repo, &head, &tip, base)?.ok_or("nothing to land")?;
        let merged = put(&worktree.repo, base, &tip, &laid.tip).map_err(|not| match not {
            NotPut::Clash(paths) => format!("{paths:?}"),
            NotPut::Moved => "moved".to_owned(),
            NotPut::Failed(why) => why,
        })?;
        follow(&worktree.path, &head, &laid.tip);
        Ok(merged)
    }

    #[test]
    fn landing_merges_into_files_others_changed_and_refuses_a_clash() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
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
        let root = std::fs::canonicalize(root.path()).unwrap();
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
        assert!(lay(&signing, &main, &head, &before, "main")
            .unwrap()
            .is_none());
        assert!(land_from(&worktree, &signing).is_err());
        assert_eq!(git_in(&main, &["rev-parse", "main"]), before);
    }

    #[test]
    fn landing_gives_each_file_the_mode_its_commit_holds() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
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
}
