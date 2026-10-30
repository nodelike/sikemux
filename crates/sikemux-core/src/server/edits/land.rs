//! Lands the commits of an agent that works in a checkout of its own onto the
//! branch it was cut from. Its commits are made again on that branch's tip,
//! and the branch moves only when the project's own checkout can take them:
//! a file someone has changed there and not committed takes the landed lines
//! merged in, and a clash refuses the landing before anything moves. The
//! agent's checkout then moves to the new tip, keeping what it has not
//! committed.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::owners::{git, run};
use super::push::{line, replay, Signing, Unpushed};
use crate::server::worktree::AgentWorktree;
use crate::server::Core;

const ATTEMPTS: usize = 4;

#[derive(Serialize)]
pub(crate) struct Landed {
    branch: String,
    landed: Vec<String>,
    /// Files someone had changed in the project's checkout, which took the
    /// landed lines merged into their change.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    merged: Vec<String>,
    /// The project's checkout, which a push after landing goes from.
    #[serde(skip)]
    pub(super) repo: PathBuf,
}

/// What landing does to one file of the project's checkout.
enum Step {
    Write(Vec<u8>),
    Delete,
    Keep,
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
            Step::Write(bytes) => file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&file, bytes)),
            Step::Delete => match std::fs::remove_file(&file) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                done => done,
            },
            Step::Keep => Ok(()),
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

/// Lands `agent_id`'s commits from its own checkout onto its base branch.
pub(super) fn land(core: &Core, agent_id: &str) -> Result<Landed, String> {
    let worktree = core
        .workspaces
        .agent(agent_id)
        .and_then(|agent| agent.worktree)
        .ok_or("You work in the project's own checkout, so your commits are on its branch already; there is nothing to land")?;
    land_from(&worktree, &Signing::of(core))
}

/// Lands the commits of the checkout `worktree` onto its base branch, signing
/// their footers again with `signing`.
fn land_from(worktree: &AgentWorktree, signing: &Signing) -> Result<Landed, String> {
    let base = worktree
        .base
        .clone()
        .ok_or("Your checkout was cut from no branch, so there is nowhere to land it")?;
    let target = format!("refs/heads/{base}");
    for _ in 0..ATTEMPTS {
        let tip = line(git(&worktree.repo, &["rev-parse", &target])?);
        let head = line(git(&worktree.path, &["rev-parse", "HEAD"])?);
        let range = format!("{tip}..{head}");
        if git(&worktree.path, &["rev-list", "--merges", "--count", &range])?.trim() != "0" {
            return Err("Your commits include a merge; land them by hand".into());
        }
        let listed = git(
            &worktree.path,
            &[
                "log",
                "--reverse",
                "--cherry-pick",
                "--right-only",
                "--format=%H%x00%s",
                &format!("{tip}...{head}"),
            ],
        )?;
        let commits: Vec<Unpushed> = listed
            .lines()
            .filter_map(|entry| entry.split_once('\0'))
            .map(|(id, subject)| Unpushed {
                id: id.to_owned(),
                mine: true,
                by: String::new(),
                subject: subject.to_owned(),
            })
            .collect();
        if commits.is_empty() {
            return Err(format!("Nothing to land: {base} has every commit of yours"));
        }
        let landed = replay(
            signing,
            &worktree.path,
            &tip,
            &commits.iter().collect::<Vec<_>>(),
            &format!("onto {base}, which changed the same lines since your checkout was cut. Run git rebase {base} in your checkout, settle the clash, then land again"),
        )?;
        let checked_out = git(&worktree.repo, &["symbolic-ref", "-q", "HEAD"])
            .is_ok_and(|head| head.trim() == target);
        let steps = if checked_out {
            steps(&worktree.repo, &tip, &landed)?
        } else {
            Steps::default()
        };
        let moved = run(
            &worktree.repo,
            &[
                "update-ref",
                "-m",
                &format!("land: {} commits", commits.len()),
                &target,
                &landed,
                &tip,
            ],
            None,
            None,
        );
        if moved.is_err() {
            continue;
        }
        let merged = match checked_out {
            true => apply(&worktree.repo, steps, &tip, &landed),
            false => Vec::new(),
        };
        if let Err(error) = git(&worktree.path, &["reset", "--keep", "--quiet", &landed]) {
            eprintln!("sikemux core: the landed checkout stays where it was: {error}");
        }
        return Ok(Landed {
            branch: base,
            landed: commits
                .iter()
                .map(|commit| {
                    format!(
                        "{} {}",
                        commit.id.get(..8).unwrap_or(&commit.id),
                        commit.subject
                    )
                })
                .collect(),
            merged,
            repo: worktree.repo.clone(),
        });
    }
    Err("the branch kept moving while landing; call this again".into())
}

/// What each file the landing changes needs in the project's checkout at
/// `repo`, from `before` to `after`, and whether someone had changed it there.
/// A clash with what someone changed refuses the landing.
pub(super) fn steps(repo: &Path, before: &str, after: &str) -> Result<Steps, String> {
    let changed = git(
        repo,
        &["diff", "-z", "--name-only", "--no-renames", before, after],
    )?;
    let mut steps = Vec::new();
    let mut clashes = Vec::new();
    for path in changed.split('\0').filter(|path| !path.is_empty()) {
        let old = blob(repo, before, path);
        let new = blob(repo, after, path);
        let work = std::fs::read(repo.join(path)).ok();
        let old_bytes = old.as_ref().map(|id| contents(repo, id)).transpose()?;
        let new_bytes = new.as_ref().map(|id| contents(repo, id)).transpose()?;
        let step = if work == old_bytes {
            match new_bytes {
                Some(bytes) => (Step::Write(bytes), false),
                None => (Step::Delete, false),
            }
        } else if work == new_bytes {
            (Step::Keep, false)
        } else {
            match (work, old, new) {
                (Some(work), Some(old), Some(new)) => match merged(repo, &work, &old, &new)? {
                    Some(bytes) => (Step::Write(bytes), true),
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
        steps.push((path.to_owned(), step.0, step.1));
    }
    if !clashes.is_empty() {
        return Err(format!(
            "Not landed: the project's checkout has changes nobody committed in {} that clash with yours. Find who made them with who_wrote and agree with them, then land again.",
            clashes.join(", ")
        ));
    }
    Ok(Steps(steps))
}

fn blob(repo: &Path, commit: &str, path: &str) -> Option<String> {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{commit}:{path}"),
        ],
    )
    .ok()
    .map(line)
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
        if staged != blob(repo, before, path) {
            continue;
        }
        match blob(repo, after, path) {
            Some(new) => {
                let mode = git(repo, &["ls-tree", after, "--", path])
                    .ok()
                    .and_then(|entry| entry.split_whitespace().next().map(str::to_owned))
                    .unwrap_or_else(|| "100644".to_owned());
                info.push_str(&format!("{mode} {new}\t{path}\0"));
            }
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

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let mut command = sikemux_process::user_environment::command("git");
        let output = command
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
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
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let landed = land_from(&worktree, &signing).unwrap();
        assert_eq!(landed.merged, ["a.txt"]);
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
}
