//! Pushes the calling agent's own commits, leaving other agents' unpushed
//! ones behind. A shared branch often holds several agents' commits that are
//! not on the remote yet; pushing the branch would publish all of them. The
//! agent's commits, told by their signed footers, are put on the remote's tip
//! and pushed, and the branch is rebuilt on what was pushed with the other
//! commits after it, so its files stay exactly as they are.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::footer::verified;
use super::note;
use super::owners::{git, run};
use crate::server::Core;
use crate::trailers::{self, Commit, NOTES_REF};

const ATTEMPTS: usize = 4;
const PUSH_WAIT: Duration = Duration::from_secs(5 * 60);
const PUSH_OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Serialize)]
pub(crate) struct Pushed {
    remote: String,
    branch: String,
    pushed: Vec<String>,
    /// Other agents' commits still waiting to be pushed, with who made them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    kept: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// One unpushed commit: whose it is, and what it is made of.
pub(super) struct Unpushed {
    pub id: String,
    pub mine: bool,
    pub by: String,
    pub subject: String,
}

/// The host's key, which signs footers and notes again for commits made
/// again, and the hosts whose signatures count.
pub(super) struct Signing {
    pub key: Option<iroh::SecretKey>,
    pub hosts: Vec<iroh::PublicKey>,
}

impl Signing {
    pub(super) fn of(core: &Core) -> Self {
        Self {
            key: core.remote.host_key(),
            hosts: core.remote.host_keys(),
        }
    }
}

/// Pushes `agent_id`'s unpushed commits on `dir`'s branch to `remote`.
pub(super) fn push(
    core: &Core,
    dir: &Path,
    agent_id: &str,
    remote: &str,
) -> Result<Pushed, String> {
    let session = core.names.last_session(agent_id);
    let whose = |commit: &str| {
        let agents = verified(core, dir, commit)?;
        let mine = agents.iter().any(|agent| {
            agent.chat == agent_id
                || session
                    .as_deref()
                    .is_some_and(|session| agent.session.as_deref() == Some(session))
        });
        let names: Vec<&str> = agents.iter().map(|agent| agent.name.as_str()).collect();
        Ok((mine, names.join(", ")))
    };
    push_as(dir, remote, whose, &Signing::of(core))
}

/// Pushes the unpushed commits on `dir`'s branch that `whose` says are the
/// caller's, as `whose` names the writers of each.
fn push_as(
    dir: &Path,
    remote: &str,
    whose: impl Fn(&str) -> Result<(bool, String), String>,
    signing: &Signing,
) -> Result<Pushed, String> {
    let top = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim());
    let branch = git(&top, &["symbolic-ref", "--short", "-q", "HEAD"])
        .map_err(|_| "git_push pushes a branch; this checkout is not on one".to_owned())?
        .trim()
        .to_owned();
    let tracking = format!("refs/remotes/{remote}/{branch}");
    remote_git(
        &top,
        &["fetch", "--quiet", remote, &format!("+{branch}:{tracking}")],
    )?;
    let head = line(git(&top, &["rev-parse", "HEAD"])?);
    let base = line(git(&top, &["rev-parse", &tracking])?);
    let ahead = git(&top, &["rev-list", "--count", &format!("{head}..{base}")])?;
    if ahead.trim() != "0" {
        return Err(format!(
            "{remote}/{branch} has commits this branch lacks; pull them first, then push"
        ));
    }
    let unpushed = unpushed(&top, &base, &head, whose)?;
    if !unpushed.iter().any(|commit| commit.mine) {
        return Err(match unpushed.is_empty() {
            true => "Nothing to push: every commit here is on the remote".to_owned(),
            false => format!(
                "None of the unpushed commits are yours: {}",
                kept(&unpushed).join("; ")
            ),
        });
    }
    let (ours, theirs): (Vec<&Unpushed>, Vec<&Unpushed>) =
        unpushed.iter().partition(|commit| commit.mine);
    let pushed = Pushed {
        remote: remote.to_owned(),
        branch: branch.clone(),
        pushed: ours
            .iter()
            .map(|commit| format!("{} {}", short(&commit.id), commit.subject))
            .collect(),
        kept: kept(&unpushed),
        note: None,
    };
    if theirs.is_empty() {
        remote_git(
            &top,
            &[
                "push",
                "--quiet",
                remote,
                &format!("{head}:refs/heads/{branch}"),
            ],
        )?;
        return Ok(pushed);
    }
    let tip = replay(signing, &top, &base, &ours)?;
    let rebuilt = replay(signing, &top, &tip, &theirs)?;
    if tree(&top, &rebuilt)? != tree(&top, &head)? {
        return Err(format!(
            "Your commits build on other agents' unpushed ones, so they cannot go alone: {}. Ask them to push, or push when the person says to.",
            kept(&unpushed).join("; ")
        ));
    }
    remote_git(
        &top,
        &[
            "push",
            "--quiet",
            remote,
            &format!("{tip}:refs/heads/{branch}"),
        ],
    )?;
    let rebased = rebase_branch(signing, &top, &branch, &head, &rebuilt)?;
    Ok(Pushed {
        note: (!rebased).then(|| format!(
            "{branch} moved while pushing, so it still holds your pushed commits as they were; git pull --rebase drops them"
        )),
        ..pushed
    })
}

/// Moves `branch` from `head` to `rebuilt`, the same files with the pushed
/// commits first, taking along commits others made on it meanwhile. Whether
/// it moved.
fn rebase_branch(
    signing: &Signing,
    top: &Path,
    branch: &str,
    head: &str,
    rebuilt: &str,
) -> Result<bool, String> {
    let reflog = "push: other agents' unpushed commits go after the pushed ones";
    let target = format!("refs/heads/{branch}");
    let mut from = head.to_owned();
    let mut onto = rebuilt.to_owned();
    for _ in 0..ATTEMPTS {
        if run(
            top,
            &["update-ref", "-m", reflog, &target, &onto, &from],
            None,
            None,
        )
        .is_ok()
        {
            return Ok(true);
        }
        let now = line(git(top, &["rev-parse", &target])?);
        let later = git(top, &["rev-list", "--reverse", &format!("{from}..{now}")])?;
        let later: Vec<Unpushed> = later
            .lines()
            .map(|id| Unpushed {
                id: id.to_owned(),
                mine: false,
                by: String::new(),
                subject: String::new(),
            })
            .collect();
        let caught_up = replay(signing, top, &onto, &later.iter().collect::<Vec<_>>())?;
        if tree(top, &caught_up)? != tree(top, &now)? {
            return Ok(false);
        }
        (from, onto) = (now, caught_up);
    }
    Ok(false)
}

pub(super) fn line(text: String) -> String {
    text.trim().to_owned()
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn kept(unpushed: &[Unpushed]) -> Vec<String> {
    unpushed
        .iter()
        .filter(|commit| !commit.mine)
        .map(|commit| format!("{} {} ({})", short(&commit.id), commit.subject, commit.by))
        .collect()
}

/// The commits on `head` that `base` lacks, oldest first, each with whether
/// it is the caller's and who wrote it, as `whose` says.
fn unpushed(
    top: &Path,
    base: &str,
    head: &str,
    whose: impl Fn(&str) -> Result<(bool, String), String>,
) -> Result<Vec<Unpushed>, String> {
    let range = format!("{base}..{head}");
    let merges = git(top, &["rev-list", "--merges", "--count", &range])?;
    if merges.trim() != "0" {
        return Err("The unpushed commits include a merge; push those by hand".into());
    }
    let listed = git(top, &["log", "--reverse", "--format=%H%x00%s", &range])?;
    listed
        .lines()
        .filter_map(|entry| entry.split_once('\0'))
        .map(|(id, subject)| {
            let (mine, by) = whose(id)?;
            Ok(Unpushed {
                id: id.to_owned(),
                mine,
                by: if by.is_empty() {
                    "no agent".to_owned()
                } else {
                    by
                },
                subject: subject.to_owned(),
            })
        })
        .collect()
}

/// `commits` made again one after another on `onto`, with the same changes,
/// authors and messages, their footers and notes signed again for where they
/// now stand. The last one made, or `onto` when there are none.
pub(super) fn replay(
    signing: &Signing,
    top: &Path,
    onto: &str,
    commits: &[&Unpushed],
) -> Result<String, String> {
    let (key, hosts) = (&signing.key, &signing.hosts);
    let mut onto = onto.to_owned();
    for commit in commits {
        let id = commit.id.as_str();
        let parent = line(git(top, &["rev-parse", &format!("{id}^")])?);
        let tree = if parent == onto {
            tree(top, id)?
        } else {
            let merged = git(
                top,
                &[
                    "merge-tree",
                    "--write-tree",
                    "--no-messages",
                    &format!("--merge-base={parent}"),
                    &onto,
                    id,
                ],
            )
            .map_err(|_| {
                format!(
                    "{} {} does not apply without other agents' unpushed commits",
                    short(id),
                    commit.subject
                )
            })?;
            merged.lines().next().unwrap_or_default().to_owned()
        };
        let object = git(top, &["cat-file", "commit", id])?;
        let (header, message) = object.split_once("\n\n").unwrap_or((&object, ""));
        let parents = vec![onto.clone()];
        let made_of = Commit {
            tree: &tree,
            parents: &parents,
        };
        let old_parents = vec![parent.clone()];
        let was = Commit {
            tree: &line(git(top, &["rev-parse", &format!("{id}^{{tree}}")])?),
            parents: &old_parents,
        };
        let ours = |host: &iroh::PublicKey| key.as_ref().is_some_and(|key| key.public() == *host);
        let message = match (trailers::verify(message, &was, hosts), key) {
            (Some((host, agents)), Some(key)) if ours(&host) => {
                trailers::append(message, &trailers::footer(&agents, &made_of, Some(key)))
            }
            _ => message.to_owned(),
        };
        let mut command = sikemux_process::user_environment::command("git");
        command
            .args([
                "-c",
                "core.fsmonitor=false",
                "commit-tree",
                &tree,
                "-p",
                &onto,
                "-F",
                "-",
            ])
            .current_dir(top)
            .env("GIT_TERMINAL_PROMPT", "0");
        for (field, name, email, date) in [
            (
                "author ",
                "GIT_AUTHOR_NAME",
                "GIT_AUTHOR_EMAIL",
                "GIT_AUTHOR_DATE",
            ),
            (
                "committer ",
                "GIT_COMMITTER_NAME",
                "GIT_COMMITTER_EMAIL",
                "GIT_COMMITTER_DATE",
            ),
        ] {
            if let Some((who, when)) = header
                .lines()
                .find_map(|line| line.strip_prefix(field))
                .and_then(|value| value.rsplit_once("> "))
            {
                if let Some((person, mail)) = who.split_once(" <") {
                    command
                        .env(name, person)
                        .env(email, mail)
                        .env(date, format!("@{when}"));
                }
            }
        }
        let output = sikemux_process::run(
            &mut command,
            Some(message.as_bytes()),
            PUSH_WAIT,
            PUSH_OUTPUT_LIMIT,
            None,
        )
        .map_err(|error| format!("git did not run: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git commit-tree failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let made = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        carry_note(top, id, &was, &made, &made_of, key.as_ref(), hosts);
        onto = made;
    }
    Ok(onto)
}

/// Puts `from`'s note saying whose lines it holds on `to`, signed for it.
fn carry_note(
    top: &Path,
    from: &str,
    was: &Commit,
    to: &str,
    made_of: &Commit,
    key: Option<&iroh::SecretKey>,
    hosts: &[iroh::PublicKey],
) {
    let Some(key) = key else { return };
    let Ok(blob) = git(top, &["notes", &format!("--ref={NOTES_REF}"), "show", from]) else {
        return;
    };
    let Some(text) = trailers::verify_note(&blob, was, hosts) else {
        return;
    };
    if serde_json::from_str::<note::Note>(text).is_err() {
        return;
    }
    let signed = trailers::signed_note(text, made_of, key);
    let _ = run(
        top,
        &[
            "notes",
            &format!("--ref={NOTES_REF}"),
            "add",
            "-f",
            "-F",
            "-",
            to,
        ],
        None,
        Some(signed.as_bytes()),
    );
}

fn tree(top: &Path, commit: &str) -> Result<String, String> {
    Ok(line(git(
        top,
        &["rev-parse", &format!("{commit}^{{tree}}")],
    )?))
}

/// Runs git against a remote, which may ask over the network and run the
/// repository's push hooks, so it gets longer than git run locally.
fn remote_git(top: &Path, args: &[&str]) -> Result<(), String> {
    crate::git::Git::at(top)
        .timeout(PUSH_WAIT)
        .limit(PUSH_OUTPUT_LIMIT)
        .stdout(args)
        .map(drop)
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

    fn commit(dir: &Path, file: &str, text: &str, subject: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        git_in(dir, &["add", file]);
        git_in(dir, &["commit", "-qm", subject]);
    }

    #[test]
    fn only_the_callers_commits_are_pushed_and_the_branch_keeps_its_files() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let remote = root.join("remote.git");
        let work = root.join("work");
        git_in(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        git_in(&root, &["clone", "-q", &remote.to_string_lossy(), "work"]);
        git_in(&work, &["checkout", "-q", "-b", "main"]);
        commit(&work, "base.txt", "base\n", "base");
        git_in(&work, &["push", "-q", "origin", "main"]);
        commit(&work, "a.txt", "a\n", "mine: a");
        commit(&work, "b.txt", "b\n", "theirs: b");
        commit(&work, "c.txt", "c\n", "mine: c");
        let files = git_in(&work, &["rev-parse", "HEAD^{tree}"]);
        let whose = |commit: &str| {
            let subject = git_in(&work, &["log", "-1", "--format=%s", commit]);
            Ok((subject.starts_with("mine"), "someone".to_owned()))
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let pushed = push_as(&work, "origin", whose, &signing).unwrap();
        assert_eq!(pushed.pushed.len(), 2);
        assert_eq!(pushed.kept.len(), 1);
        assert!(pushed.note.is_none());
        let subjects = |dir: &Path, rev: &str| git_in(dir, &["log", "--format=%s", rev]);
        assert_eq!(subjects(&remote, "main"), "mine: c\nmine: a\nbase");
        assert_eq!(subjects(&work, "main"), "theirs: b\nmine: c\nmine: a\nbase");
        assert_eq!(git_in(&work, &["rev-parse", "HEAD^{tree}"]), files);
        assert_eq!(
            git_in(&work, &["rev-parse", "HEAD^"]),
            git_in(&remote, &["rev-parse", "main"])
        );

        commit(&work, "b.txt", "b and more\n", "mine: builds on b");
        let whose = |commit: &str| {
            let subject = git_in(&work, &["log", "-1", "--format=%s", commit]);
            Ok((subject.starts_with("mine"), "someone".to_owned()))
        };
        assert!(push_as(&work, "origin", whose, &signing).is_err());
        assert_eq!(subjects(&remote, "main"), "mine: c\nmine: a\nbase");
    }
}
