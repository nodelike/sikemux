//! Changes to CI workflows, where the remote knows agents by an identity of
//! their own whose key can't change workflows. The core pushes every such
//! change itself, with a key that can, minted for that one push. A change to a
//! workflow file that mentions secrets or an environment waits for the person,
//! asked once per agent; the rest go out by themselves. Every push to such a remote passes
//! through the core's [`forward`], which says what it may carry.

use std::path::Path;
use std::sync::{Arc, Mutex, Weak};

use sikemux_plugin_api::credential::{Ask, Credential, Reach};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallAsker, CallKind, CallOption, CallOutcome, CallResult, PersonCall};

use super::footer::verified;
use super::owners::git;
use super::push::{self, line};
use crate::git::Git;
use crate::server::person_calls::{self, Raise, HOLD_ALLOW, HOLD_REFUSE};
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{plugged, Core};

pub const FOLDER: &str = ".github/workflows";
/// What a call's key starts with, before its agent, checkout and the changes it lists.
const KEY: &str = "workflows:";
/// The most of the workflow diffs a call shows.
const DIFF_CHARS: usize = 12_000;

/// The address of `remote` of the repository at `dir`, when it knows agents by an identity of
/// their own.
pub fn guarded(core: &Core, dir: &Path, remote: &str) -> Option<String> {
    let url = Git::quick(dir)
        .trimmed(&["remote", "get-url", "--push", remote])
        .ok()?;
    plugged::acts_as(core, &url).map(|_| url)
}

/// The files under `.github/workflows` that `commit` adds, changes or deletes.
fn files(dir: &Path, commit: &str) -> Vec<String> {
    let args = [
        "diff-tree",
        "-m",
        "--root",
        "--no-renames",
        "--no-commit-id",
        "--name-only",
        "-r",
        commit,
        "--",
        FOLDER,
    ];
    (git(dir, &args).unwrap_or_default().lines())
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether `commit` changes a file under `.github/workflows`.
pub fn touches(dir: &Path, commit: &str) -> bool {
    !files(dir, commit).is_empty()
}

/// Whether every workflow file `commit` changes is plain both before and after it, so it can
/// reach no secret and go without asking.
pub fn plain(dir: &Path, commit: &str) -> bool {
    let parent = git(
        dir,
        &["rev-parse", "--verify", "--quiet", &format!("{commit}^")],
    )
    .ok()
    .map(line);
    let sides: Vec<&str> = parent.as_deref().into_iter().chain([commit]).collect();
    files(dir, commit).iter().all(|path| {
        sides.iter().all(|side| {
            let listed = git(dir, &["ls-tree", side, "--", path]);
            match listed {
                Ok(listed) if listed.trim().is_empty() => true,
                Ok(listed) if !(listed.starts_with("100644 ") || listed.starts_with("100755 ")) => {
                    false
                }
                Ok(_) => Git::quick(dir)
                    .stdout(&["cat-file", "blob", &format!("{side}:{path}")])
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .is_some_and(|text| plain_text(&text)),
                Err(_) => false,
            }
        })
    })
}

/// Whether a workflow file's text, read without case both as written and with its YAML escapes
/// resolved, names no environment, no secret but the run's own `GITHUB_TOKEN` and no workflow of
/// another repository, and keeps its token from writing to the repository.
fn plain_text(text: &str) -> bool {
    let scoped = text.lines().any(|line| line.starts_with("permissions:"));
    let unescaped = unescaped(text);
    scoped
        && ascii(text)
        && ascii(&unescaped)
        && [text.to_lowercase(), unescaped.to_lowercase()]
            .iter()
            .all(|text| reaches_nothing(text))
}

/// Whether `text` is printable ASCII in lines, so no other line break or folded letter can hide a
/// word from the checks.
fn ascii(text: &str) -> bool {
    let bytes = text.as_bytes();
    (bytes.iter().enumerate()).all(|(at, byte)| match byte {
        b' '..=b'~' | b'\t' | b'\n' => true,
        b'\r' => bytes.get(at + 1) == Some(&b'\n'),
        _ => false,
    })
}

fn reaches_nothing(text: &str) -> bool {
    if text.contains("environment") || text.contains("write-all") {
        return false;
    }
    let token = "secrets.github_token";
    let secrets_ok = text.match_indices("secrets").all(|(at, _)| {
        let rest = &text[at..];
        rest.starts_with(token)
            && (rest[token.len()..].chars().next())
                .is_some_and(|after| !after.is_alphanumeric() && after != '_')
    });
    let called = "/.github/workflows/";
    let local_only = (text.match_indices(called)).all(|(at, _)| {
        let before = text[..at].strip_suffix('.');
        before.is_some_and(|before| {
            (before.chars().last())
                .is_none_or(|last| last.is_whitespace() || matches!(last, '"' | '\''))
        })
    });
    let read_only = (text.lines())
        .filter(|line| line.contains("contents"))
        .all(reads_contents);
    secrets_ok && local_only && read_only
}

/// Whether `line` is only `contents: read` or `contents: none` under its indent, with at most a
/// comment after.
fn reads_contents(line: &str) -> bool {
    let line = line.trim_start_matches([' ', '\t']);
    let Some(rest) = line.strip_prefix("contents:") else {
        return false;
    };
    let value = rest.trim_start_matches([' ', '\t']);
    if value.len() == rest.len() {
        return false;
    }
    let Some(after) = value
        .strip_prefix("read")
        .or_else(|| value.strip_prefix("none"))
    else {
        return false;
    };
    let comment = after.trim_start_matches([' ', '\t']);
    comment.trim_end().is_empty() || (comment.starts_with('#') && comment.len() < after.len())
}

/// `text` as YAML's quoted strings would read it: a backslash at a line end joins the next line
/// without its indent, and `\xHH`, `\uHHHH` and `\UHHHHHHHH` become what they spell.
fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let joined = (after
            .strip_prefix("\r\n")
            .or_else(|| after.strip_prefix('\n')))
        .map(|next| next.trim_start_matches([' ', '\t']));
        if let Some(next) = joined {
            rest = next;
            continue;
        }
        let digits = match after.chars().next() {
            Some('x') => 2,
            Some('u') => 4,
            Some('U') => 8,
            _ => 0,
        };
        let spelled = (digits > 0)
            .then(|| after.get(1..=digits))
            .flatten()
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(char::from_u32);
        match spelled {
            Some(spelled) => {
                out.push(spelled);
                rest = &after[1 + digits..];
            }
            None => {
                out.push('\\');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What `commit` changes, by an id that stays when it is made again on another commit.
pub fn patch_id(dir: &Path, commit: &str) -> Option<String> {
    let change = (Git::quick(dir))
        .stdout(&["show", "--diff-merges=first-parent", "--format=", commit])
        .ok()?;
    let id = Git::quick(dir)
        .input(&change)
        .trimmed(&["patch-id", "--stable"])
        .ok()?;
    id.split_whitespace().next().map(str::to_owned)
}

/// Whether `commit` changes a workflow file in a way that waits for the person, who has not
/// passed it.
pub fn waits(dir: &Path, commit: &str) -> bool {
    touches(dir, commit)
        && !plain(dir, commit)
        && !patch_id(dir, commit).is_some_and(|id| PASSED.locked().contains(&id))
}

/// The changes the person passed, by patch id, while the core pushes them.
static PASSED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Lets the changes `patches` name go while this lives.
struct Passing(Vec<String>);

impl Passing {
    fn new(patches: Vec<String>) -> Self {
        PASSED.locked().extend(patches.iter().cloned());
        Self(patches)
    }
}

impl Drop for Passing {
    fn drop(&mut self) {
        let mut passed = PASSED.locked();
        for patch in &self.0 {
            if let Some(at) = passed.iter().position(|kept| kept == patch) {
                passed.remove(at);
            }
        }
    }
}

/// What an agent's push is told when it carries a change that waits for the person.
const HELD: &str = "This push carries a change to CI that waits for the person, so push with git_commit and push: true, which sends your other commits and asks them about it.";

/// What an agent's push is told when a merge carries such a change, which no call could list.
const MERGE: &str = "This push has a merge that changes CI in a way that waits for the person, which they can't pass, so replay it as plain commits and push those with git_commit and push: true.";

/// What the core may forward to `remote` from `outbox`, as the `updates` to its refs, each a
/// full ref name with its old and new commit id, move them: `None` needs only the core's usual key; a grant, while it
/// lives, a key that can change workflows; an error refuses the forward with what to tell the
/// agent.
pub fn forward(
    core: &Core,
    outbox: &Path,
    remote: &str,
    updates: &[(String, String, String)],
) -> Result<Option<Grant>, String> {
    if plugged::acts_as(core, remote).is_none() {
        return Ok(None);
    }
    let carried = carried(outbox, updates)
        .map_err(|why| format!("Sikemux couldn't tell what this push carries: {why}"))?;
    if carried.is_empty() {
        return Ok(None);
    }
    let waiting: Vec<&String> = (carried.iter())
        .filter(|commit| waits(outbox, commit))
        .collect();
    let merge = |commit: &str| {
        git(
            outbox,
            &["rev-parse", "--verify", "--quiet", &format!("{commit}^2")],
        )
        .is_ok()
    };
    match waiting.as_slice() {
        [] => Ok(Some(grant(core, remote))),
        waiting if waiting.iter().any(|commit| merge(commit)) => Err(MERGE.to_owned()),
        _ => Err(HELD.to_owned()),
    }
}

/// The commits that `updates` bring to the remote and that change a workflow file: those the
/// new ids reach and neither the old ids nor any ref of `outbox` they leave alone does.
fn carried(outbox: &Path, updates: &[(String, String, String)]) -> Result<Vec<String>, String> {
    let zero = |id: &str| id.bytes().all(|byte| byte == b'0');
    let news: Vec<&str> = (updates.iter())
        .map(|(_, _, new)| new.as_str())
        .filter(|new| !zero(new))
        .collect();
    if news.is_empty() {
        return Ok(Vec::new());
    }
    let refs = git(
        outbox,
        &["for-each-ref", "--format=%(objectname) %(refname)"],
    )?;
    let updated = |name: &str| updates.iter().any(|(updated, _, _)| updated == name);
    let mut known: Vec<&str> = (refs.lines())
        .filter_map(|entry| entry.split_once(' '))
        .filter(|(_, name)| !updated(name))
        .map(|(tip, _)| tip)
        .collect();
    known.extend(
        (updates.iter())
            .map(|(_, old, _)| old.as_str())
            .filter(|old| !zero(old)),
    );
    let mut args = vec!["rev-list", "--ignore-missing", "--full-history"];
    args.extend(news);
    args.push("--not");
    args.extend(known);
    args.extend(["--", FOLDER]);
    let listed = git(outbox, &args)?;
    Ok(listed
        .lines()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect())
}

/// A key that can change workflows, kept for one push of the core's own while this lives, and
/// revoked once it goes.
pub struct Grant {
    name: String,
}

struct Kept {
    name: String,
    remote: String,
    core: Weak<Core>,
    minted: Option<(Credential, tokio::runtime::Handle)>,
}

/// The keys kept for the core's own pushes.
static GRANTS: Mutex<Vec<Kept>> = Mutex::new(Vec::new());

/// Keeps a key that can change workflows for the core's next push to `remote`. Nothing is
/// minted until that push's git asks for it.
pub fn grant(core: &Core, remote: &str) -> Grant {
    let name = uuid::Uuid::new_v4().simple().to_string();
    GRANTS.locked().push(Kept {
        name: name.clone(),
        remote: remote.to_owned(),
        core: core.arc().as_ref().map(Arc::downgrade).unwrap_or_default(),
        minted: None,
    });
    Grant { name }
}

impl Grant {
    /// The variable the push's git carries, which names this grant to the credential handler.
    pub fn env(&self) -> (String, String) {
        (
            sikemux_process::vars::PUSH_GRANT.to_owned(),
            self.name.clone(),
        )
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        let kept = {
            let mut grants = GRANTS.locked();
            let at = grants.iter().position(|kept| kept.name == self.name);
            at.map(|at| grants.remove(at))
        };
        let Some(Kept {
            remote,
            core,
            minted: Some((credential, runtime)),
            ..
        }) = kept
        else {
            return;
        };
        runtime.spawn(async move {
            let Some(core) = core.upgrade() else {
                return;
            };
            let Some(plugins) = plugged::provided(&core) else {
                return;
            };
            if let Err(failure) = plugins.0.revoke(&remote, credential.secret).await {
                eprintln!(
                    "sikemux core: a key that can change workflows was not revoked, and lapses within the hour: {}",
                    plugged::explain(&failure)
                );
            }
        });
    }
}

/// The key the grant `name` keeps for the core's push to `remote`, minted on the first ask.
/// Only the credential handler calls this, once it knows the asking git is the core's own.
pub async fn granted(core: &Arc<Core>, name: &str, remote: &str) -> Option<Credential> {
    let known = |kept: &Kept| kept.name == name && repository(&kept.remote) == repository(remote);
    let (url, minted) = {
        let grants = GRANTS.locked();
        let kept = grants.iter().find(|kept| known(kept))?;
        let minted = (kept.minted.as_ref()).map(|(credential, _)| credential.clone());
        (kept.remote.clone(), minted)
    };
    if minted.is_some() {
        return minted;
    }
    let ask = Ask {
        remote: url,
        reach: Reach::CoreWorkflows,
    };
    let credential = plugged::credential(core, ask).await.ok()??;
    let kept = (GRANTS.locked().iter_mut().find(|kept| known(kept))).map(|kept| {
        kept.minted = Some((credential.clone(), tokio::runtime::Handle::current()));
    });
    if kept.is_none() {
        if let Some(plugins) = plugged::provided(core) {
            let _ = plugins.0.revoke(remote, credential.secret).await;
        }
        return None;
    }
    Some(credential)
}

/// The host and path a remote names, however it is spelled: `git@github.com:o/r.git` and
/// `https://github.com/o/r` are both `github.com/o/r`.
fn repository(remote: &str) -> String {
    let rest = remote.split_once("://").map_or(remote, |(_, rest)| rest);
    let rest = rest.split_once('@').map_or(rest, |(_, rest)| rest);
    let rest = rest.replacen(':', "/", 1);
    let rest = rest.trim_end_matches('/');
    rest.strip_suffix(".git").unwrap_or(rest).to_lowercase()
}

/// Puts the `held` commits from `dir` for `remote` to the person, one call for each of `agents`
/// that made some, which lists every one of its changes that waits. A call that lists fewer is
/// withdrawn for it, and a list they said no to isn't asked again.
pub fn ask(core: &Core, dir: &Path, remote: &str, agents: &[String], held: &[String]) {
    let Ok(top) = git(dir, &["rev-parse", "--show-toplevel"]).map(line) else {
        return;
    };
    let mut by_agent: Vec<(String, Vec<&String>)> = Vec::new();
    for commit in held {
        let made_by = (verified(core, dir, commit).unwrap_or_default().into_iter())
            .map(|agent| agent.chat)
            .find(|chat| agents.contains(chat));
        let Some(agent) = made_by.or_else(|| agents.first().cloned()) else {
            continue;
        };
        match by_agent.iter_mut().find(|(known, _)| *known == agent) {
            Some((_, commits)) => commits.push(commit),
            None => by_agent.push((agent, vec![commit])),
        }
    }
    for (agent, commits) in by_agent {
        ask_for(core, dir, remote, &top, &agent, &commits);
    }
}

/// What `commits` change, by what each workflow file they touch names: one that names an
/// environment releases or deploys, the rest run.
fn what_changes(dir: &Path, commits: &[&String]) -> String {
    let mut changed: Vec<(String, bool)> = Vec::new();
    for commit in commits {
        for path in files(dir, commit) {
            let text = |side: &str| {
                (Git::quick(dir).stdout(&["cat-file", "blob", &format!("{side}:{path}")]))
                    .map(|bytes| String::from_utf8_lossy(&bytes).to_lowercase())
                    .unwrap_or_default()
            };
            let deploys = [format!("{commit}^"), commit.to_string()]
                .iter()
                .any(|side| text(side).contains("environment"));
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            match changed.iter_mut().find(|(known, _)| *known == name) {
                Some((_, known)) => *known |= deploys,
                None => changed.push((name, deploys)),
            }
        }
    }
    changed.sort();
    let phrases: Vec<String> = (changed.iter())
        .map(|(name, deploys)| match deploys {
            true => format!("how {name} releases or deploys"),
            false => format!("how {name} runs"),
        })
        .collect();
    match phrases.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn ask_for(core: &Core, dir: &Path, remote: &str, top: &str, agent_id: &str, commits: &[&String]) {
    let mut patches: Vec<String> = (commits.iter())
        .filter_map(|commit| patch_id(dir, commit))
        .collect();
    if patches.len() != commits.len() {
        return;
    }
    patches.sort();
    let prefix = format!("{KEY}{agent_id}:{remote}:{top}:");
    let key = format!("{prefix}{}", patches.join(","));
    let project = (core.names().get(agent_id).map(|named| named.project))
        .filter(|project| !project.is_empty())
        .unwrap_or_else(|| core.workspaces().project_of(dir));
    let asked = person_calls::find_key(core, &project, &key).is_some_and(|call| {
        let refused = (call.answer.as_ref())
            .is_some_and(|answer| answer.option.as_deref() != Some(HOLD_ALLOW));
        (call.answer.is_none() && call.outcome.is_none()) || refused
    });
    if asked {
        return;
    }
    for (id, _) in person_calls::open_keyed(core, &project, &prefix) {
        person_calls::set_outcome(core, &id, outcome(CallResult::Withdrawn));
    }
    let subject = |commit: &str| {
        git(dir, &["log", "-1", "--format=%s", commit])
            .map(line)
            .unwrap_or_default()
    };
    let subjects: Vec<String> = commits.iter().map(|commit| subject(commit)).collect();
    let name = core.names().name_or_id(agent_id);
    let (question, listed, them) = match subjects.as_slice() {
        [one] => (format!("@{name} wants to change CI: {one}"), None, "it"),
        many => (
            format!("@{name} wants to change CI in {} commits", many.len()),
            Some(many.join("\n")),
            "them",
        ),
    };
    let why = format!(
        "It changes {}. Changes like this can reach secrets or write to the repository, so agents can't push them: Sikemux pushes {them} once you pass {them}.",
        what_changes(dir, commits)
    );
    let context = match listed {
        Some(listed) => format!("{listed}\n\n{why}"),
        None => why,
    };
    let option = |id: &str, label: &str| CallOption {
        id: id.to_owned(),
        label: label.to_owned(),
        detail: None,
        picture: None,
    };
    let raise = Raise {
        project,
        kind: CallKind::Production,
        asker: CallAsker {
            agent_id: agent_id.to_owned(),
            cli: (core.repo_above().questions_cli_of)(core, agent_id),
            name,
            chief: false,
        },
        question,
        context: Some(context),
        options: vec![
            option(HOLD_ALLOW, "Push the change"),
            option(HOLD_REFUSE, "Don't push it"),
        ],
        pick: None,
        against: None,
        evidence: Vec::new(),
        action: Some(diffs(dir, commits)),
        brief: None,
        place: None,
        key: Some(key),
        yes: Some(HOLD_ALLOW.to_owned()),
    };
    if let Err(why) = person_calls::raise(core, raise) {
        eprintln!("sikemux core: a change to CI could not be put to the person: {why}");
    }
}

/// What `commits` change under `.github/workflows`, each under its subject, cut short when long.
fn diffs(dir: &Path, commits: &[&String]) -> String {
    let show = |commit: &str| {
        git(dir, &["show", "--format=%s%n", commit, "--", FOLDER]).unwrap_or_default()
    };
    let full = (commits.iter())
        .map(|commit| show(commit))
        .collect::<Vec<_>>()
        .join("\n");
    match full.char_indices().nth(DIFF_CHARS) {
        None => full,
        Some((cut, _)) => {
            let more = full[cut..].lines().count();
            format!("{}\n… {more} more lines", &full[..cut])
        }
    }
}

/// Whether a call's key is one of these calls'.
pub fn is_key(key: &str) -> bool {
    key.starts_with(KEY)
}

/// The person answered a call for changes to CI: on a yes the core pushes them, and their agent
/// hears how that went; on a no they stay here and their agent is told to revert them.
pub async fn answered(core: &Arc<Core>, call: &PersonCall, key: &str, typed: Option<String>) {
    let agent = call.asker.agent_id.clone();
    let passed =
        (call.answer.as_ref()).is_some_and(|answer| answer.option.as_deref() == Some(HOLD_ALLOW));
    let parsed = (key.strip_prefix(KEY))
        .and_then(|rest| rest.split_once(':'))
        .and_then(|(_, rest)| rest.split_once(':'))
        .and_then(|(remote, rest)| Some((remote.to_owned(), rest.rsplit_once(':')?)));
    let mut text = match (passed, parsed) {
        (false, _) => {
            person_calls::set_outcome(core, &call.id, outcome(CallResult::NotShipped));
            "The person did not pass your change to CI. It stays in this checkout and is never pushed: revert it with a new commit, and say so in your answer if your work needed it.".to_owned()
        }
        (true, None) => return,
        (true, Some((remote, (top, patches)))) => {
            let passing = Passing::new(patches.split(',').map(str::to_owned).collect());
            let (owner, dir) = (core.clone(), Path::new(top).to_owned());
            let agents = vec![agent.clone()];
            let went = tokio::task::spawn_blocking(move || {
                let _passing = passing;
                push::push(&owner, &dir, &agents, &remote)
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
            match went {
                Ok(pushed) => {
                    person_calls::set_outcome(core, &call.id, outcome(CallResult::Shipped));
                    format!(
                        "The person passed your change to CI, and Sikemux pushed it: {}",
                        serde_json::to_string(&pushed).unwrap_or_default()
                    )
                }
                Err(why) => {
                    person_calls::set_outcome(core, &call.id, outcome(CallResult::Failed));
                    format!(
                        "The person passed your change to CI, but Sikemux couldn't push it: {why}. Once that is settled, push again with git_commit and push: true, and the person is asked again."
                    )
                }
            }
        }
    };
    if let Some(typed) = typed {
        text = format!("{text}\nThey added: {typed}");
    }
    let _ = (core.repo_above().messages_for_person)(core, agent, text).await;
}

fn outcome(result: CallResult) -> CallOutcome {
    CallOutcome {
        result,
        healthy: None,
        roll_back: false,
        why: None,
        at: epoch_ms(),
    }
}
