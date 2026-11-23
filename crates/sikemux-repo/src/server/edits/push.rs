//! Pushes the calling agent's own commits, and those of the agents it leads,
//! leaving other agents' unpushed ones behind. A shared branch often holds several agents' commits that are
//! not on the remote yet; pushing the branch would publish all of them. The
//! agent's commits, told by their footers' Agent: lines, are put on the remote's tip
//! and pushed, and the branch is rebuilt on what was pushed with the other
//! commits after it, so its files stay exactly as they are.

use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::committer;
use super::footer::verified;
use super::land;
use super::note;
use super::owners::{git, run};
use super::workflows;
use crate::server::Core;
use crate::trailers::{self, Agent, Commit, COMMITTERS_REF, NOTES_REF};

const ATTEMPTS: usize = 4;
const PUSH_WAIT: Duration = Duration::from_secs(5 * 60);
const PUSH_OUTPUT_LIMIT: usize = 1024 * 1024;
/// A repository's pre-push hook may run its whole test suite, so the push itself has no time
/// limit and room for all a suite prints.
const HOOK_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Serialize)]
pub struct Pushed {
    remote: String,
    branch: String,
    pushed: Vec<String>,
    /// Other agents' commits still waiting to be pushed, with who made them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    kept: Vec<String>,
    /// The caller's commits that wait for the person: changes to CI workflows, and those that
    /// build on them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The end of what the push printed: the repository's pre-push hook, and the remote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip)]
    pub held: Vec<String>,
    /// Commits taken off the branch, as they do not apply onto the remote's newest commits.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sent_back: Vec<String>,
    #[serde(skip)]
    pub blocked: Vec<Blocked>,
    /// The newest commit it sent, when it sent any.
    #[serde(skip)]
    pub tip: Option<String>,
}

/// A commit of the branch that does not apply onto the remote's newest commits, so the push
/// left it out and took it off the branch.
pub struct Blocked {
    pub id: String,
    pub subject: String,
    /// The files it clashes in.
    pub files: Vec<String>,
    /// The chats of the agents who made it.
    pub owners: Vec<String>,
}

/// One unpushed commit: whose it is, and what it is made of.
pub struct Unpushed {
    pub id: String,
    pub mine: bool,
    pub by: String,
    pub subject: String,
    /// It changes CI workflows in a way that waits for the person, who has not passed it.
    pub waits: bool,
}

/// The host's key, which signs footers and notes again for commits made
/// again, and the hosts whose signatures count.
pub struct Signing {
    pub key: Option<iroh::SecretKey>,
    pub hosts: Vec<iroh::PublicKey>,
}

impl Signing {
    pub fn of(core: &Core) -> Self {
        Self {
            key: core.remote().host_key(),
            hosts: core.remote().host_keys(),
        }
    }

    /// The agents `message`'s footer names, to sign again for where the commit now stands. A
    /// host signs only for itself, so a footer a linked host signed is left to it; one whose
    /// signature no longer checks out, after a rebase or a reword, keeps its agents.
    pub fn agents_of(&self, message: &str, was: &Commit) -> Option<Vec<Agent>> {
        if let Some((host, agents)) = trailers::verify(message, was, &self.hosts) {
            return (self.key.as_ref()?.public() == host).then_some(agents);
        }
        let agents = trailers::agents(message);
        (!agents.is_empty()).then_some(agents)
    }
}

/// A land not on its branch yet: the branch stands at `from`, and the land
/// takes it to `to`.
#[derive(Clone, Copy)]
pub struct Landing<'a> {
    pub branch: &'a str,
    pub from: &'a str,
    pub to: &'a str,
}

/// Pushes the unpushed commits on `dir`'s branch made by any of `agents`, or
/// by agents they lead. From the project's own checkout, rather than an
/// agent's, it pushes every one: the land queue or git_commit checked them.
/// So it does from a checkout on a release line, whose cherry-picks carry no
/// note of who made them.
/// Where the remote knows agents by an identity of their own, the caller's
/// changes to CI workflows that can reach secrets wait for the person, who is
/// asked.
pub fn push(core: &Core, dir: &Path, agents: &[String], remote: &str) -> Result<Pushed, String> {
    push_from(core, dir, agents, remote, None)
}

/// Pushes `dir`'s branch as it stands, replacing the remote's copy as long as it is still what
/// was fetched, as after a rebase. Never the remote's default branch or a release line.
pub fn force_push(
    core: &Core,
    dir: &Path,
    agents: &[String],
    remote: &str,
) -> Result<Pushed, String> {
    let top = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim());
    let branch = line(
        git(&top, &["symbolic-ref", "--short", "-q", "HEAD"])
            .map_err(|_| "force pushes a branch; this checkout is not on one".to_owned())?,
    );
    if protected(&top, remote, &branch) {
        return Err(format!(
            "{branch} is the default branch or a release line, which is never force-pushed; push without force"
        ));
    }
    if !on_remote(&top, remote, &branch)? {
        return push(core, dir, agents, remote);
    }
    let was = fetched_tip(&top, remote, &branch)?;
    let head = line(git(&top, &["rev-parse", "HEAD"])?);
    let listed = git(
        &top,
        &[
            "log",
            "--reverse",
            "--format=%H%x00%s",
            &format!("{was}..{head}"),
        ],
    )?;
    let commits: Vec<(&str, &str)> = listed
        .lines()
        .filter_map(|entry| entry.split_once('\0'))
        .collect();
    if workflows::guarded(core, dir, remote).is_some()
        && commits.iter().any(|(id, _)| workflows::waits(dir, id))
    {
        return Err("Some of these commits change CI workflows, which wait for the person; push without force so Sikemux asks them".to_owned());
    }
    let lease = format!("--force-with-lease=refs/heads/{branch}:{was}");
    let output = match send(&top, remote, &format!("{head}:refs/heads/{branch}"), &[&lease]) {
        Ok(output) => output,
        Err(Attempt::Raced) => {
            return Err(format!(
                "{remote}/{branch} moved since it was fetched, so it was left as it is; look at what it holds before pushing again"
            ))
        }
        Err(Attempt::Failed(why)) => return Err(why),
    };
    follow(&top, remote, &branch)?;
    (core.repo_above().pushed)(core);
    (core.repo_above().triggers_pushed)(core, dir, agents, &head);
    Ok(Pushed {
        remote: remote.to_owned(),
        branch,
        pushed: (commits.iter())
            .map(|(id, subject)| format!("{} {subject}", short(id)))
            .collect(),
        kept: Vec::new(),
        waiting: Vec::new(),
        note: Some(format!(
            "Replaced {remote}'s copy, which stood at {}.",
            short(&was)
        )),
        output,
        held: Vec::new(),
        sent_back: Vec::new(),
        blocked: Vec::new(),
        tip: Some(head),
    })
}

/// Pushes the project's checkout `dir` as it stands with `landing` on it, and
/// moves its branch there only once the remote took it, so a push refused for
/// any reason leaves the branch as it was.
pub fn land_and_push(
    core: &Core,
    dir: &Path,
    agents: &[String],
    remote: &str,
    landing: &Landing,
) -> Result<Pushed, String> {
    push_from(core, dir, agents, remote, Some(landing))
}

fn push_from(
    core: &Core,
    dir: &Path,
    agents: &[String],
    remote: &str,
    landing: Option<&Landing>,
) -> Result<Pushed, String> {
    let whole_branch = is_main_checkout(dir) || on_release_line(dir);
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
        let mine = whole_branch
            || agents
                .iter()
                .any(|agent| is_me(&agent.chat, agent.session.as_deref()));
        let names: Vec<String> = (agents.iter())
            .map(|agent| crate::server::names::called(&agent.name, &agent.chat))
            .collect();
        Ok((mine, names.join(", ")))
    };
    let guarded = workflows::guarded(core, dir, remote).is_some();
    let judge = |commit: &str| guarded && workflows::waits(dir, commit);
    let signing = Signing::of(core);
    let mut pushed = push_holding(dir, remote, whose, judge, whole_branch, &signing, landing)?;
    for blocked in &mut pushed.blocked {
        blocked.owners = (verified(core, dir, &blocked.id)
            .unwrap_or_default()
            .into_iter())
        .map(|agent| agent.chat)
        .collect();
    }
    (core.repo_above().pushed)(core);
    if let Some(tip) = &pushed.tip {
        (core.repo_above().triggers_pushed)(core, dir, agents, tip);
    }
    if !pushed.held.is_empty() {
        workflows::ask(core, dir, remote, agents, &pushed.held);
    }
    Ok(pushed)
}

/// Pushes the unpushed commits on `dir`'s branch that `whose` says are the
/// caller's, as `whose` names the writers of each. When the remote has moved
/// on, they go on its tip and the branch takes its new commits; when it moves
/// while pushing, that happens again.
#[cfg(test)]
fn push_as(
    dir: &Path,
    remote: &str,
    whose: impl Fn(&str) -> Result<(bool, String), String>,
    signing: &Signing,
) -> Result<Pushed, String> {
    push_holding(dir, remote, whose, |_| false, false, signing, None)
}

/// [`push_as`], leaving out the caller's commits `judge` says wait for the person, with those
/// that can't go without them.
fn push_holding(
    dir: &Path,
    remote: &str,
    whose: impl Fn(&str) -> Result<(bool, String), String>,
    judge: impl Fn(&str) -> bool,
    drop_clashing: bool,
    signing: &Signing,
    landing: Option<&Landing>,
) -> Result<Pushed, String> {
    let top = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim());
    let branch = match landing {
        Some(landing) => landing.branch.to_owned(),
        None => match release_line(&top) {
            Some(release) => release,
            None => git(&top, &["symbolic-ref", "--short", "-q", "HEAD"])
                .map_err(|_| "git_push pushes a branch; this checkout is not on one".to_owned())?
                .trim()
                .to_owned(),
        },
    };
    for _ in 0..ATTEMPTS {
        let head = match landing {
            Some(_) => String::new(),
            None => line(git(&top, &["rev-parse", "HEAD"])?),
        };
        let landing = landing.copied().unwrap_or(Landing {
            branch: &branch,
            from: &head,
            to: &head,
        });
        match push_once(
            &top,
            remote,
            &landing,
            &whose,
            &judge,
            drop_clashing,
            signing,
        ) {
            Err(Attempt::Raced) => continue,
            Err(Attempt::Failed(why)) => return Err(why),
            Ok(pushed) => {
                follow(&top, remote, &branch)?;
                return Ok(pushed);
            }
        }
    }
    Err(format!(
        "{remote}/{branch} kept moving while pushing; call this again"
    ))
}

/// Makes `branch` track its namesake on `remote`, unless it already tracks something.
fn follow(top: &Path, remote: &str, branch: &str) -> Result<(), String> {
    let merge = format!("branch.{branch}.merge");
    let tracking = format!("refs/remotes/{remote}/{branch}");
    if git(top, &["config", "--get", &merge]).is_ok()
        || git(top, &["rev-parse", "--verify", "-q", &tracking]).is_err()
    {
        return Ok(());
    }
    git(top, &["config", &format!("branch.{branch}.remote"), remote])?;
    git(top, &["config", &merge, &format!("refs/heads/{branch}")])?;
    Ok(())
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
    landing: &Landing,
    whose: &impl Fn(&str) -> Result<(bool, String), String>,
    judge: &impl Fn(&str) -> bool,
    drop_clashing: bool,
    signing: &Signing,
) -> Result<Pushed, Attempt> {
    let (branch, at, head) = (
        landing.branch,
        landing.from.to_owned(),
        landing.to.to_owned(),
    );
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
    if let (true, Some(tip)) = (moved, &base) {
        let rewritten = git(
            top,
            &[
                "log",
                "--cherry-pick",
                "--left-only",
                "--format=%H",
                &format!("{tip}...{head}"),
            ],
        )?;
        if rewritten.trim().is_empty() && !protected(top, remote, branch) {
            return Err(Attempt::Failed(format!(
                "{remote}/{branch} holds only older copies of this branch's commits, as after a rebase; push again with force: true to replace it"
            )));
        }
    }
    let range = fork
        .as_deref()
        .map_or_else(|| head.clone(), |fork| format!("{fork}..{head}"));
    let merges = git(top, &["rev-list", "--merges", "--count", &range])?.trim() != "0";
    if merges && protected(top, remote, branch) {
        return Err(Attempt::Failed(MERGES_STAY_OFF.into()));
    }
    let unpushed = unpushed(top, fork.as_deref(), base.as_deref(), &head, whose, judge)?;
    if !unpushed.iter().any(|commit| commit.mine) {
        let rebased = at == head || rebase_branch(signing, top, branch, &at, &head, true, LANDED)?;
        return Ok(Pushed {
            remote: remote.to_owned(),
            branch: branch.to_owned(),
            pushed: Vec::new(),
            kept: kept(&unpushed),
            waiting: Vec::new(),
            note: Some(match (rebased, unpushed.is_empty()) {
                (false, _) => not_landed_here(remote, branch),
                (true, true) => "Every commit here is on the remote already.".to_owned(),
                (true, false) => "Nothing of yours waits to be pushed here. The rest go out with their makers' pushes, or with git_commit push from the project's own checkout, which pushes all it holds.".to_owned(),
            }),
            held: Vec::new(),
            sent_back: Vec::new(),
            blocked: Vec::new(),
            output: None,
            tip: None,
        });
    }
    let shown = |commits: &[&Unpushed]| -> Vec<String> {
        (commits.iter())
            .map(|commit| format!("{} {}", short(&commit.id), commit.subject))
            .collect()
    };
    let held_ids: Vec<String> = (unpushed.iter())
        .filter(|commit| commit.mine && commit.waits)
        .map(|commit| commit.id.clone())
        .collect();
    let waits = |waiting: &[&Unpushed]| (!waiting.is_empty()).then(|| WAITS_FOR_PERSON.to_owned());
    let (ours, theirs): (Vec<&Unpushed>, Vec<&Unpushed>) =
        unpushed.iter().partition(|commit| commit.mine);
    let as_is = held_ids.is_empty() && theirs.is_empty() && !moved;
    if merges && !as_is {
        return Err(Attempt::Failed(MERGES_STAY_OFF.into()));
    }
    if as_is {
        land::steps(top, branch, &at, &head)?;
        let output = send(top, remote, &format!("{head}:refs/heads/{branch}"), &[])?;
        let rebased = at == head || rebase_branch(signing, top, branch, &at, &head, true, LANDED)?;
        return Ok(Pushed {
            remote: remote.to_owned(),
            branch: branch.to_owned(),
            pushed: shown(&ours),
            kept: Vec::new(),
            waiting: Vec::new(),
            held: Vec::new(),
            sent_back: Vec::new(),
            blocked: Vec::new(),
            note: (!rebased).then(|| not_landed_here(remote, branch)),
            output,
            tip: Some(head),
        });
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
    let mut tip = base;
    let mut going: Vec<&Unpushed> = Vec::new();
    let mut waiting: Vec<&Unpushed> = Vec::new();
    let mut blocked: Vec<Blocked> = Vec::new();
    for (at, commit) in unpushed.iter().enumerate() {
        if !commit.mine {
            continue;
        }
        if commit.waits {
            waiting.push(commit);
            continue;
        }
        match replay(signing, top, &tip, &[commit], clash) {
            Ok(next) => {
                tip = next;
                going.push(commit);
            }
            Err(_) if unpushed.iter().take(at).any(|before| before.mine && before.waits) => {
                waiting.push(commit)
            }
            Err(_) if unpushed.iter().take(at).any(|before| before.waits) => {
                return Err(Attempt::Failed(format!(
                    "Your commits build on a change to CI that waits for the person, so they go once it is passed and pushed: {}. Go on with other work, and push again after.",
                    unpushed.iter().take(at)
                        .filter(|before| before.waits)
                        .map(|before| format!("{} {} by {}", short(&before.id), before.subject, before.by))
                        .collect::<Vec<_>>()
                        .join("; ")
                )))
            }
            Err(_) if moved && drop_clashing => blocked.push(Blocked {
                id: commit.id.clone(),
                subject: commit.subject.clone(),
                files: clashes(top, &tip, &commit.id),
                owners: Vec::new(),
            }),
            Err(why) => {
                return Err(match moved {
                    true => Attempt::Failed(format!("{why}; pull and settle it, then push again")),
                    false => cannot_go_alone(),
                })
            }
        }
    }
    let left: Vec<&Unpushed> = (unpushed.iter())
        .filter(|commit| !going.iter().any(|gone| gone.id == commit.id))
        .filter(|commit| !blocked.iter().any(|off| off.id == commit.id))
        .collect();
    let pushed = Pushed {
        remote: remote.to_owned(),
        branch: branch.to_owned(),
        pushed: shown(&going),
        kept: kept(&unpushed),
        waiting: shown(&waiting),
        note: waits(&waiting),
        held: held_ids,
        sent_back: (blocked.iter())
            .map(|off| {
                format!(
                    "{} {} clashes in {}",
                    short(&off.id),
                    off.subject,
                    off.files.join(", ")
                )
            })
            .collect(),
        blocked,
        output: None,
        tip: None,
    };
    let landing = at != head;
    if going.is_empty() && pushed.blocked.is_empty() {
        let rebased = !landing || rebase_branch(signing, top, branch, &at, &head, true, LANDED)?;
        return Ok(Pushed {
            note: match rebased {
                true => pushed.note.clone(),
                false => Some(not_landed_here(remote, branch)),
            },
            ..pushed
        });
    }
    let rebuilt = replay(signing, top, &tip, &left, clash).map_err(|_| cannot_go_alone())?;
    if !moved && tree(top, &rebuilt)? != tree(top, &head)? {
        return Err(cannot_go_alone());
    }
    land::steps(top, branch, &at, &rebuilt)?;
    let output = send(top, remote, &format!("{tip}:refs/heads/{branch}"), &[])?;
    let rebased = rebase_branch(
        signing,
        top,
        branch,
        &at,
        &rebuilt,
        moved || landing,
        match landing {
            true => LANDED,
            false => "push: other agents' unpushed commits go after the pushed ones",
        },
    )?;
    let moving = (!rebased).then(|| match landing {
        true => not_landed_here(remote, branch),
        false => format!(
            "{branch} moved while pushing, so it still holds your pushed commits as they were; run git pull --rebase in this checkout to drop them"
        ),
    });
    Ok(Pushed {
        tip: Some(tip),
        output,
        note: match (moving, pushed.note.clone()) {
            (Some(moving), Some(waits)) => Some(format!("{waits} {moving}")),
            (moving, waits) => moving.or(waits),
        },
        ..pushed
    })
}

/// The files `commit` clashes in when made again on `onto`.
fn clashes(top: &Path, onto: &str, commit: &str) -> Vec<String> {
    let mut command = sikemux_process::user_environment::command("git");
    command
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            &format!("--merge-base={commit}^"),
            onto,
            commit,
        ])
        .current_dir(top);
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    let mut files: Vec<String> = (String::from_utf8_lossy(&output.stdout).lines())
        .skip(1)
        .take_while(|file| !file.is_empty())
        .map(str::to_owned)
        .collect();
    files.dedup();
    files
}

/// Tells each agent whose commit a push took off its branch why, and what to do.
pub fn tell_sent_back(core: &Arc<Core>, runtime: &tokio::runtime::Handle, pushed: &Pushed) {
    for blocked in &pushed.blocked {
        let text = format!(
            "Your commit {} {} was on {} but does not apply onto {}/{}'s newest commits: it clashes in {}. Sikemux took it off {} so everyone else's commits could push. Bring your checkout up with git_commit sync, settle the clash, and land it again.",
            short(&blocked.id),
            blocked.subject,
            pushed.branch,
            pushed.remote,
            pushed.branch,
            blocked.files.join(", "),
            pushed.branch,
        );
        for owner in &blocked.owners {
            let (core, owner, text) = (core.clone(), owner.clone(), text.clone());
            runtime.spawn(async move {
                let told = (core.repo_above().messages_work_from_sikemux)(&core, owner, text, "about its push");
                if let Err(error) = told.await {
                    eprintln!("sikemux core: a note about a commit taken off a branch did not reach its agent: {error}");
                }
            });
        }
    }
}

/// What a push says of the caller's commits it left for the person.
const WAITS_FOR_PERSON: &str = "The commits under waiting change CI workflows, or build on such a change, so they wait for the person: Sikemux asked them, and pushes those commits itself once they pass. Do not push them another way, and go on with other work.";

/// Pushes `refspec`, for as long as the repository's pre-push hook runs, with the project's
/// `hookEnv`; the end of what it printed, when it printed anything. A remote that moved on
/// since it was fetched is a race to try again.
fn send(
    top: &Path,
    remote: &str,
    refspec: &str,
    flags: &[&str],
) -> Result<Option<String>, Attempt> {
    let mut push = crate::git::Git::at(top)
        .timeout(Duration::MAX)
        .limit(HOOK_OUTPUT_LIMIT);
    for (key, value) in crate::tasks::config::hook_env_near(top) {
        push = push.env(&key, value);
    }
    let mut args = vec!["push", "--quiet"];
    args.extend(flags);
    args.extend([remote, refspec]);
    let output = push.output(&args)?;
    let said = crate::git::said(&output);
    if output.status.success() {
        return Ok((!said.is_empty()).then_some(said));
    }
    let raced = [
        "non-fast-forward",
        "fetch first",
        "stale info",
        "cannot lock ref",
    ]
    .iter()
    .any(|sign| said.contains(sign));
    match raced {
        true => Err(Attempt::Raced),
        false => Err(Attempt::Failed(format!("git push failed: {said}"))),
    }
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
    reflog: &str,
) -> Result<bool, String> {
    let target = format!("refs/heads/{branch}");
    let mut from = head.to_owned();
    let mut onto = rebuilt.to_owned();
    for _ in 0..ATTEMPTS {
        let Ok(steps) = land::steps(top, branch, &from, &onto) else {
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
            if let Some(steps) = steps {
                land::apply(top, steps, &from, &onto);
            }
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
                waits: false,
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

/// What a push says when the remote took a land that its branch here could not take after.
fn not_landed_here(remote: &str, branch: &str) -> String {
    format!(
        "{branch} moved in the project's checkout while pushing, so your commits are on {remote}/{branch} but not yet on {branch} there; run git pull --rebase in it to bring them"
    )
}

const LANDED: &str = "land: the land queue, once the remote took it";

const WITHOUT_THEIRS: &str = "without other agents' unpushed commits";

pub fn line(text: String) -> String {
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
    judge: impl Fn(&str) -> bool,
) -> Result<Vec<Unpushed>, String> {
    let range = fork.map_or_else(|| head.to_owned(), |fork| format!("{fork}..{head}"));
    let mut args = vec!["log", "--reverse", "--first-parent", "--format=%H%x00%s"];
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
                waits: judge(id),
            })
        })
        .collect()
}

/// `commits` made again one after another on `onto`, with the same changes,
/// authors and messages, their footers and notes signed again for where they
/// now stand. A commit whose change `onto` already holds is left out rather
/// than made again empty. The last one made, or `onto` when there are none.
/// `clash` says what a commit that does not apply there lacks.
pub fn replay(
    signing: &Signing,
    top: &Path,
    onto: &str,
    commits: &[&Unpushed],
    clash: &str,
) -> Result<String, String> {
    let (key, hosts) = (&signing.key, &signing.hosts);
    let acting = crate::git_guard::identity::actor_of(top);
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
        let agents = signing.agents_of(message, &was);
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
                    let (person, mail) = match &acting {
                        Some(actor) if field == "committer " => {
                            (actor.name.as_str(), actor.email.as_str())
                        }
                        _ => (person, mail),
                    };
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
pub fn note_again<Holds: DeserializeOwned>(
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

/// Whether `dir` is in the repository's own checkout rather than a worktree of it.
pub(super) fn is_main_checkout(dir: &Path) -> bool {
    let path = |what| git(dir, &["rev-parse", "--path-format=absolute", what]).map(line);
    matches!((path("--git-dir"), path("--git-common-dir")), (Ok(own), Ok(common)) if own == common)
}

const RELEASE_LINE: &str = "release/";

/// The `release/*` branch `dir` stands on, or follows from a branch of its own.
fn release_line(dir: &Path) -> Option<String> {
    let named = |args: &[&str]| git(dir, args).map(line).ok();
    let branch = named(&["symbolic-ref", "--short", "-q", "HEAD"])?;
    let upstream = named(&["rev-parse", "--abbrev-ref", "-q", "@{upstream}"]);
    let followed = upstream.and_then(|upstream| Some(upstream.split_once('/')?.1.to_owned()));
    [Some(branch), followed]
        .into_iter()
        .flatten()
        .find(|branch| branch.starts_with(RELEASE_LINE))
}

const MERGES_STAY_OFF: &str = "The unpushed commits include a merge, which git_commit pushes only as it is onto a feature branch nobody else moved; rebase them onto the remote's branch in this checkout so they form one line, then push again";

/// Whether `branch` is never to be force-pushed: `remote`'s default branch, `main` or
/// `master`, or a release line.
fn protected(top: &Path, remote: &str, branch: &str) -> bool {
    let default = git(
        top,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            &format!("refs/remotes/{remote}/HEAD"),
        ],
    )
    .ok()
    .and_then(|head| Some(line(head).strip_prefix(&format!("{remote}/"))?.to_owned()));
    branch.starts_with(RELEASE_LINE)
        || ["main", "master"].contains(&branch)
        || default.as_deref() == Some(branch)
        || release_line(top).is_some()
}

pub(super) fn on_release_line(dir: &Path) -> bool {
    release_line(dir).is_some()
}

fn tree(top: &Path, commit: &str) -> Result<String, String> {
    Ok(line(git(
        top,
        &["rev-parse", &format!("{commit}^{{tree}}")],
    )?))
}

/// Runs git against a remote, which may ask over the network, so it gets longer than git run
/// locally.
fn remote_git(top: &Path, args: &[&str]) -> Result<(), String> {
    git_remote_output(top, args).map(drop)
}

fn git_remote_output(top: &Path, args: &[&str]) -> Result<String, String> {
    let output = crate::git::Git::at(top)
        .timeout(PUSH_WAIT)
        .limit(PUSH_OUTPUT_LIMIT)
        .output(args)?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            crate::git::said(&output)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
        let root = sikemux_process::files::real_path(root.path()).unwrap();
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
        assert_eq!(
            git_in(&work, &["rev-parse", "--abbrev-ref", "fresh@{upstream}"]).trim(),
            "origin/fresh"
        );

        git_in(
            &work,
            &["remote", "set-url", "origin", "/nowhere/remote.git"],
        );
        assert!(push_as(&work, "origin", whose, &signing).is_err());
    }

    /// A push Bitbucket's branch rules refused said only "failed to push some refs".
    #[test]
    fn a_refused_push_says_what_the_remote_said() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let remote = root.join("remote.git");
        let work = root.join("work");
        git_in(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        git_in(&root, &["clone", "-q", &remote.to_string_lossy(), "work"]);
        git_in(&work, &["checkout", "-q", "-b", "main"]);
        commit(&work, "base.txt", "base\n", "mine: base");
        let hook = |at: PathBuf, said: &str| {
            std::fs::write(&at, format!("#!/bin/sh\necho '{said}'\nexit 1\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        };
        hook(
            work.join(".git/hooks/pre-push"),
            "typecheck failed in app/page.tsx",
        );
        let whose = |_: &str| Ok((true, "someone".to_owned()));
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let Err(why) = push_as(&work, "origin", whose, &signing) else {
            panic!("the remote refuses every push");
        };
        assert!(why.contains("typecheck failed in app/page.tsx"), "{why}");
        std::fs::remove_file(work.join(".git/hooks/pre-push")).unwrap();
        hook(
            remote.join("hooks/pre-receive"),
            "Branch main only takes pull requests.",
        );
        let Err(why) = push_as(&work, "origin", whose, &signing) else {
            panic!("the remote refuses every push");
        };
        assert!(
            why.contains("Branch main only takes pull requests."),
            "{why}"
        );
    }

    /// On 2026-11-12 a land moved the project's main onto stale commits, and only then did its
    /// push refuse one whose authorship record was lost; main had to be put back by hand.
    #[test]
    fn a_refused_landing_push_leaves_main_as_it_was() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let remote = root.join("remote.git");
        let work = root.join("work");
        git_in(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        git_in(&root, &["clone", "-q", &remote.to_string_lossy(), "work"]);
        git_in(&work, &["checkout", "-q", "-b", "main"]);
        commit(&work, "base.txt", "base\n", "base");
        git_in(&work, &["push", "-q", "-u", "origin", "main"]);
        let from = line(git_in(&work, &["rev-parse", "HEAD"]));
        git_in(&work, &["checkout", "-q", "-b", "agent"]);
        commit(&work, "landed.txt", "landed\n", "lost its authorship");
        let to = line(git_in(&work, &["rev-parse", "HEAD"]));
        git_in(&work, &["checkout", "-q", "main"]);
        let landing = Landing {
            branch: "main",
            from: &from,
            to: &to,
        };
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let unknown = |_: &str| Err("no authorship record".to_owned());
        let refused = push_holding(
            &work,
            "origin",
            unknown,
            |_| false,
            true,
            &signing,
            Some(&landing),
        );
        assert!(refused.is_err());
        let as_it_was = || {
            assert_eq!(line(git_in(&work, &["rev-parse", "main"])), from);
            assert!(!work.join("landed.txt").exists());
            assert_eq!(line(git_in(&remote, &["rev-parse", "main"])), from);
        };
        as_it_was();

        let hook = remote.join("hooks/pre-receive");
        std::fs::write(&hook, "#!/bin/sh\necho 'main is protected'\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let known = |_: &str| Ok((true, "someone".to_owned()));
        let refused = push_holding(
            &work,
            "origin",
            known,
            |_| false,
            true,
            &signing,
            Some(&landing),
        );
        assert!(refused.is_err_and(|why| why.contains("main is protected")));
        as_it_was();

        std::fs::remove_file(&hook).unwrap();
        push_holding(
            &work,
            "origin",
            known,
            |_| false,
            true,
            &signing,
            Some(&landing),
        )
        .unwrap();
        assert_eq!(line(git_in(&work, &["rev-parse", "main"])), to);
        assert_eq!(line(git_in(&remote, &["rev-parse", "main"])), to);
        assert!(work.join("landed.txt").exists());
    }

    #[test]
    fn only_the_callers_commits_are_pushed_and_the_branch_keeps_its_files() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
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

    /// Main stopped pushing on 2026-11-15: GitHub took a rustfmt pass while a landed commit
    /// changed one of its lines, and that one commit failed every push after it.
    #[test]
    fn a_landed_commit_that_clashes_with_the_remote_comes_off_and_the_rest_push() {
        let root = tempfile::tempdir().unwrap();
        let root = sikemux_process::files::real_path(root.path()).unwrap();
        let remote = root.join("remote.git");
        let (work, other) = (root.join("work"), root.join("other"));
        git_in(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        git_in(&root, &["clone", "-q", &remote.to_string_lossy(), "work"]);
        git_in(&work, &["checkout", "-q", "-b", "main"]);
        commit(&work, "x.txt", "one\n", "base");
        git_in(&work, &["push", "-q", "origin", "main"]);
        git_in(&root, &["clone", "-q", &remote.to_string_lossy(), "other"]);
        commit(&other, "x.txt", "formatted\n", "fmt");
        git_in(&other, &["push", "-q", "origin", "main"]);
        commit(&work, "x.txt", "changed\n", "clashes");
        commit(&work, "y.txt", "y\n", "fine");
        let signing = Signing {
            key: None,
            hosts: Vec::new(),
        };
        let whose = |_: &str| Ok((true, "someone".to_owned()));
        let pushed = push_holding(&work, "origin", whose, |_| false, true, &signing, None).unwrap();
        let subjects = |dir: &Path, rev: &str| git_in(dir, &["log", "--format=%s", rev]);
        assert_eq!(subjects(&remote, "main"), "fine\nfmt\nbase");
        assert_eq!(subjects(&work, "main"), "fine\nfmt\nbase");
        assert_eq!(pushed.blocked.len(), 1);
        assert_eq!(pushed.blocked[0].subject, "clashes");
        assert_eq!(pushed.blocked[0].files, ["x.txt"]);
    }
}
