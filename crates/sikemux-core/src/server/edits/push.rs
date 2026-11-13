//! Pushes the calling agent's own commits, and those of the agents it leads,
//! leaving other agents' unpushed ones behind. A shared branch often holds several agents' commits that are
//! not on the remote yet; pushing the branch would publish all of them. The
//! agent's commits, told by their signed footers, are put on the remote's tip
//! and pushed, and the branch is rebuilt on what was pushed with the other
//! commits after it, so its files stay exactly as they are.

use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::committer;
use super::footer::{self, verified};
use super::land;
use super::note;
use super::owners::{git, run};
use crate::server::Core;
use crate::trailers::{self, Agent, Commit, COMMITTERS_REF, NOTES_REF};

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
            key: core.remote().host_key(),
            hosts: core.remote().host_keys(),
        }
    }

    /// The agents `message`'s footer names, when this host signed it for `was`.
    /// A host signs only for itself, so a linked host's footer is not signed again.
    pub(super) fn agents_of(&self, message: &str, was: &Commit) -> Option<Vec<Agent>> {
        let (host, agents) = trailers::verify(message, was, &self.hosts)?;
        (self.key.as_ref()?.public() == host).then_some(agents)
    }

    /// The agents of the commit `git rebase` made `commit` from, when this host signed its footer.
    fn agents_before_rebase(&self, top: &Path, commit: &str) -> Option<Vec<Agent>> {
        let was = footer::before_rebase(top, commit)?;
        let (tree, parents, message) = footer::read_commit(top, &was).ok()?;
        self.agents_of(
            &message,
            &Commit {
                tree: &tree,
                parents: &parents,
            },
        )
    }
}

/// Pushes the unpushed commits on `dir`'s branch made by any of `agents`, or
/// by agents they lead.
pub(super) fn push(
    core: &Core,
    dir: &Path,
    agents: &[String],
    remote: &str,
) -> Result<Pushed, String> {
    let sessions: Vec<String> = (agents.iter())
        .filter_map(|agent| core.names().last_session(agent))
        .collect();
    let whose = |commit: &str| {
        let is_me = |chat: &str, of: Option<&str>| {
            agents
                .iter()
                .any(|agent| chat == agent || core.names().leads(agent, chat))
                || of.is_some_and(|of| sessions.iter().any(|session| session == of))
        };
        let agents = verified(core, dir, commit)?;
        if agents.is_empty() {
            if let Some(made_by) = committer::of(core, dir, commit) {
                return Ok((
                    is_me(&made_by.chat, made_by.session.as_deref()),
                    crate::server::messages::called(&made_by.name, &made_by.chat),
                ));
            }
        }
        let mine = agents
            .iter()
            .any(|agent| is_me(&agent.chat, agent.session.as_deref()));
        let names: Vec<String> = (agents.iter())
            .map(|agent| crate::server::messages::called(&agent.name, &agent.chat))
            .collect();
        Ok((mine, names.join(", ")))
    };
    push_as(dir, remote, whose, &Signing::of(core))
}

/// Pushes the unpushed commits on `dir`'s branch that `whose` says are the
/// caller's, as `whose` names the writers of each. When the remote has moved
/// on, they go on its tip and the branch takes its new commits; when it moves
/// while pushing, that happens again.
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
    for _ in 0..ATTEMPTS {
        match push_once(&top, remote, &branch, &whose, signing) {
            Err(Attempt::Raced) => continue,
            Err(Attempt::Failed(why)) => return Err(why),
            Ok(pushed) => return Ok(pushed),
        }
    }
    Err(format!(
        "{remote}/{branch} kept moving while pushing; call this again"
    ))
}

enum Attempt {
    Raced,
    Failed(String),
}

impl From<String> for Attempt {
    fn from(why: String) -> Self {
        Self::Failed(why)
    }
}

fn push_once(
    top: &Path,
    remote: &str,
    branch: &str,
    whose: &impl Fn(&str) -> Result<(bool, String), String>,
    signing: &Signing,
) -> Result<Pushed, Attempt> {
    let head = line(git(top, &["rev-parse", "HEAD"])?);
    let (base, fork) = match on_remote(top, remote, branch)? {
        true => {
            let tip = fetched_tip(top, remote, branch)?;
            let fork = line(git(top, &["merge-base", &head, &tip])?);
            (Some(tip), Some(fork))
        }
        false => {
            let fork = last_on_remote(top, remote, &head)?;
            (fork.clone(), fork)
        }
    };
    let moved = base != fork;
    let unpushed = unpushed(top, fork.as_deref(), base.as_deref(), &head, whose)?;
    if !unpushed.iter().any(|commit| commit.mine) {
        return Err(Attempt::Failed(match unpushed.is_empty() {
            true => "Nothing to push: every commit here is on the remote".to_owned(),
            false => format!(
                "None of the unpushed commits are yours: {}",
                kept(&unpushed).join("; ")
            ),
        }));
    }
    let (ours, theirs): (Vec<&Unpushed>, Vec<&Unpushed>) =
        unpushed.iter().partition(|commit| commit.mine);
    let pushed = Pushed {
        remote: remote.to_owned(),
        branch: branch.to_owned(),
        pushed: ours
            .iter()
            .map(|commit| format!("{} {}", short(&commit.id), commit.subject))
            .collect(),
        kept: kept(&unpushed),
        note: None,
    };
    if theirs.is_empty() && !moved {
        send(top, remote, branch, &head)?;
        return Ok(pushed);
    }
    let cannot_go_alone = || {
        Attempt::Failed(format!(
            "Your commits build on other agents' unpushed ones, so they cannot go alone: {}. Ask them to push theirs with agent_send, then push again.",
            kept(&unpushed).join("; ")
        ))
    };
    let clash = match moved {
        true => "onto the remote's newest commits",
        false => WITHOUT_THEIRS,
    };
    let base = base.ok_or_else(cannot_go_alone)?;
    let tip = replay(signing, top, &base, &ours, clash).map_err(|why| match moved {
        true => Attempt::Failed(format!("{why}; pull and settle it, then push again")),
        false => cannot_go_alone(),
    })?;
    let rebuilt = replay(signing, top, &tip, &theirs, clash).map_err(|_| cannot_go_alone())?;
    match moved {
        true => {
            land::steps(top, &head, &rebuilt)?;
        }
        false if tree(top, &rebuilt)? != tree(top, &head)? => return Err(cannot_go_alone()),
        false => {}
    }
    send(top, remote, branch, &tip)?;
    let rebased = rebase_branch(signing, top, branch, &head, &rebuilt, moved)?;
    Ok(Pushed {
        note: (!rebased).then(|| format!(
            "{branch} moved while pushing, so it still holds your pushed commits as they were; git pull --rebase drops them"
        )),
        ..pushed
    })
}

/// Pushes `commit` as the remote's `branch`; a remote that moved on since it
/// was fetched is a race to try again.
fn send(top: &Path, remote: &str, branch: &str, commit: &str) -> Result<(), Attempt> {
    remote_git(
        top,
        &[
            "push",
            "--quiet",
            remote,
            &format!("{commit}:refs/heads/{branch}"),
        ],
    )
    .map_err(|why| {
        let raced = [
            "non-fast-forward",
            "fetch first",
            "stale info",
            "cannot lock ref",
        ]
        .iter()
        .any(|sign| why.contains(sign));
        match raced {
            true => Attempt::Raced,
            false => Attempt::Failed(why),
        }
    })
}

fn on_remote(top: &Path, remote: &str, branch: &str) -> Result<bool, String> {
    let found = git_remote_output(
        top,
        &[
            "ls-remote",
            "--heads",
            remote,
            &format!("refs/heads/{branch}"),
        ],
    )?;
    Ok(!found.trim().is_empty())
}

/// The tip of `branch` on the remote, fetched.
fn fetched_tip(top: &Path, remote: &str, branch: &str) -> Result<String, String> {
    let tracking = format!("refs/remotes/{remote}/{branch}");
    remote_git(
        top,
        &["fetch", "--quiet", remote, &format!("+{branch}:{tracking}")],
    )?;
    Ok(line(git(top, &["rev-parse", &tracking])?))
}

/// For a branch the remote lacks: the newest commit of `head` that any of the
/// remote's branches holds, none when the remote shares nothing with it.
fn last_on_remote(top: &Path, remote: &str, head: &str) -> Result<Option<String>, String> {
    remote_git(top, &["fetch", "--quiet", remote])?;
    let not_on_remote = git(
        top,
        &[
            "rev-list",
            "--reverse",
            "--topo-order",
            head,
            "--not",
            &format!("--remotes={remote}"),
        ],
    )?;
    let Some(oldest) = not_on_remote.lines().next() else {
        return Ok(Some(head.to_owned()));
    };
    Ok(git(
        top,
        &["rev-parse", "--verify", "--quiet", &format!("{oldest}^")],
    )
    .ok()
    .map(line))
}

/// Moves `branch` from `head` to `rebuilt`, the pushed commits first and the
/// others after them, taking along commits others made on it meanwhile. With
/// the remote `moved` on, the checkout's files take its new commits as landing
/// does; otherwise they stay exactly as they are. Whether it moved.
fn rebase_branch(
    signing: &Signing,
    top: &Path,
    branch: &str,
    head: &str,
    rebuilt: &str,
    moved: bool,
) -> Result<bool, String> {
    let reflog = "push: other agents' unpushed commits go after the pushed ones";
    let target = format!("refs/heads/{branch}");
    let mut from = head.to_owned();
    let mut onto = rebuilt.to_owned();
    for _ in 0..ATTEMPTS {
        let Ok(steps) = land::steps(top, &from, &onto) else {
            return Ok(false);
        };
        if run(
            top,
            &["update-ref", "-m", reflog, &target, &onto, &from],
            None,
            None,
        )
        .is_ok()
        {
            land::apply(top, steps, &from, &onto);
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
        let later: Vec<&Unpushed> = later.iter().collect();
        let Ok(caught_up) = replay(signing, top, &onto, &later, WITHOUT_THEIRS) else {
            return Ok(false);
        };
        if !moved && tree(top, &caught_up)? != tree(top, &now)? {
            return Ok(false);
        }
        (from, onto) = (now, caught_up);
    }
    Ok(false)
}

const WITHOUT_THEIRS: &str = "without other agents' unpushed commits";

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

/// The commits on `head` that `fork` lacks, all of them with no `fork`, oldest
/// first, leaving out those `tip` holds already as other commits, each with
/// whether it is the caller's and who wrote it, as `whose` says.
fn unpushed(
    top: &Path,
    fork: Option<&str>,
    tip: Option<&str>,
    head: &str,
    whose: impl Fn(&str) -> Result<(bool, String), String>,
) -> Result<Vec<Unpushed>, String> {
    let range = fork.map_or_else(|| head.to_owned(), |fork| format!("{fork}..{head}"));
    let merges = git(top, &["rev-list", "--merges", "--count", &range])?;
    if merges.trim() != "0" {
        return Err("The unpushed commits include a merge; push those by hand".into());
    }
    let mut args = vec!["log", "--reverse", "--format=%H%x00%s"];
    let beside = tip.map(|tip| format!("{tip}...{head}"));
    match (&beside, tip != fork) {
        (Some(beside), true) => args.extend(["--cherry-pick", "--right-only", beside.as_str()]),
        _ => args.push(&range),
    }
    let listed = git(top, &args)?;
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
/// now stand. A commit whose change `onto` already holds is left out rather
/// than made again empty. The last one made, or `onto` when there are none.
/// `clash` says what a commit that does not apply there lacks.
pub(super) fn replay(
    signing: &Signing,
    top: &Path,
    onto: &str,
    commits: &[&Unpushed],
    clash: &str,
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
            .map_err(|_| format!("{} {} does not apply {clash}", short(id), commit.subject))?;
            merged.lines().next().unwrap_or_default().to_owned()
        };
        let was_tree = line(git(top, &["rev-parse", &format!("{id}^{{tree}}")])?);
        let changed_something =
            was_tree != line(git(top, &["rev-parse", &format!("{parent}^{{tree}}")])?);
        if changed_something && tree == line(git(top, &["rev-parse", &format!("{onto}^{{tree}}")])?)
        {
            continue;
        }
        let object = git(top, &["cat-file", "commit", id])?;
        let (header, message) = object.split_once("\n\n").unwrap_or((&object, ""));
        let parents = vec![onto.clone()];
        let made_of = Commit {
            tree: &tree,
            parents: &parents,
        };
        let old_parents = vec![parent.clone()];
        let was = Commit {
            tree: &was_tree,
            parents: &old_parents,
        };
        let agents = signing
            .agents_of(message, &was)
            .or_else(|| signing.agents_before_rebase(top, id));
        let message = match agents {
            Some(agents) => {
                trailers::append(message, &trailers::footer(&agents, &made_of, key.as_ref()))
            }
            None => message.to_owned(),
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
        carry_note::<note::Note>(
            top,
            NOTES_REF,
            id,
            &was,
            &made,
            &made_of,
            key.as_ref(),
            hosts,
        );
        carry_note::<committer::Committer>(
            top,
            COMMITTERS_REF,
            id,
            &was,
            &made,
            &made_of,
            key.as_ref(),
            hosts,
        );
        onto = made;
    }
    Ok(onto)
}

/// Puts `from`'s note under `notes` on `to`, signed for it.
#[allow(clippy::too_many_arguments)]
fn carry_note<Holds: DeserializeOwned>(
    top: &Path,
    notes: &str,
    from: &str,
    was: &Commit,
    to: &str,
    made_of: &Commit,
    key: Option<&iroh::SecretKey>,
    hosts: &[iroh::PublicKey],
) {
    let Some(signed) =
        key.and_then(|key| note_again::<Holds>(top, notes, from, was, made_of, key, hosts))
    else {
        return;
    };
    let notes = format!("--ref={notes}");
    let _ = run(
        top,
        &["notes", &notes, "add", "-f", "-F", "-", to],
        None,
        Some(signed.as_bytes()),
    );
}

/// `from`'s note under `notes`, when it checks out, signed for `made_of`.
pub(super) fn note_again<Holds: DeserializeOwned>(
    top: &Path,
    notes: &str,
    from: &str,
    was: &Commit,
    made_of: &Commit,
    key: &iroh::SecretKey,
    hosts: &[iroh::PublicKey],
) -> Option<String> {
    let blob = git(top, &["notes", &format!("--ref={notes}"), "show", from]).ok()?;
    let text = trailers::verify_note(&blob, was, hosts)?;
    serde_json::from_str::<Holds>(text).ok()?;
    Some(trailers::signed_note(text, made_of, key))
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
    git_remote_output(top, args).map(drop)
}

fn git_remote_output(top: &Path, args: &[&str]) -> Result<String, String> {
    crate::git::Git::at(top)
        .timeout(PUSH_WAIT)
        .limit(PUSH_OUTPUT_LIMIT)
        .stdout(args)
        .map(|out| String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::edits::tests_git::git_in;

    fn commit(dir: &Path, file: &str, text: &str, subject: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        git_in(dir, &["add", file]);
        git_in(dir, &["commit", "-qm", subject]);
    }

    /// `git_commit` with push failed with "couldn't find remote ref" on
    /// bench/compare-compilers and others, which were not on the remote yet.
    #[test]
    fn a_branch_the_remote_lacks_is_published_with_only_the_callers_commits() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let remote = root.join("remote.git");
        let work = root.join("work");
        git_in(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        git_in(&root, &["clone", "-q", &remote.to_string_lossy(), "work"]);
        git_in(&work, &["checkout", "-q", "-b", "main"]);
        commit(&work, "base.txt", "base\n", "base");
        git_in(&work, &["push", "-q", "origin", "main"]);
        git_in(&work, &["checkout", "-q", "-b", "fresh"]);
        commit(&work, "a.txt", "a\n", "mine: a");
        commit(&work, "b.txt", "b\n", "theirs: b");
        let whose = |commit: &str| {
            let subject = git_in(&work, &["log", "-1", "--format=%s", commit]);
            Ok((subject.starts_with("mine"), "someone".to_owned()))
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let pushed = push_as(&work, "origin", whose, &signing).unwrap();
        assert_eq!(pushed.pushed.len(), 1);
        assert_eq!(pushed.kept.len(), 1);
        assert_eq!(
            git_in(&remote, &["log", "--format=%s", "fresh"]),
            "mine: a\nbase"
        );

        git_in(
            &work,
            &["remote", "set-url", "origin", "/nowhere/remote.git"],
        );
        assert!(push_as(&work, "origin", whose, &signing).is_err());
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
