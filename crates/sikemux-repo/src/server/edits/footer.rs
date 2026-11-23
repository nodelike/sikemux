//! The signed footer naming the agents whose lines a commit holds, for every
//! way a commit is made here: the git panel, `git_commit`, Auto, and an
//! agent's own `git commit` through the git shim.

use crate::server::chat_state::ChatParts;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::serving::ServeCore;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::committer::Committer;
use super::note::{self, Note};
use super::owners::{git, latest, run, staged_in, Author};
use super::push::{note_again, Signing};
use crate::cli::protocol::HarnessRequest;
use crate::server::callers::{self, CommitGrant};
use crate::server::connection::blocking;
use crate::server::repo_events::CommitMade;
use crate::server::Core;
use crate::trailers::{self, Agent, Commit, PendingNote};

/// Whether commits made in `dir` may name the agents that wrote them. Work
/// repositories turn it off with `git config sikemux.attribution false`.
pub fn attributed(dir: &Path) -> bool {
    git(
        dir,
        &["config", "--type=bool", "--get", "sikemux.attribution"],
    )
    .map_or(true, |value| value.trim() != "false")
}

/// The footer of a commit the core makes by itself for `signers`, such as a land check's fix
/// or brought-up-to-date generated files: none where commits name no agent.
pub fn of_core_commit(
    dir: &Path,
    signers: &[Agent],
    commit: &Commit,
    key: Option<&iroh::SecretKey>,
) -> Vec<String> {
    match attributed(dir) {
        true => trailers::footer(signers, commit, key),
        false => Vec::new(),
    }
}

/// Whether `text` credits Claude, as a Co-Authored-By trailer or a "Generated
/// with Claude Code" line does. The person never wants that on anything an
/// agent writes; Sikemux's footer credits the agent already.
pub fn credits_claude(text: &str) -> bool {
    text.lines().any(is_claude_credit)
}

/// `text` without its lines crediting Claude.
pub fn without_claude_credit(text: &str) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !is_claude_credit(line))
        .collect();
    kept.join("\n").trim_end().to_owned()
}

fn is_claude_credit(line: &str) -> bool {
    let line = line.trim().to_lowercase();
    (line.contains("claude") || line.contains("anthropic"))
        && (line.starts_with("co-authored-by:") || line.contains("generated with"))
}

/// The footer for committing what `dir` has staged, or what `index` holds when
/// git commits from an index of its own: `fallback`, the agent committing, and
/// every agent whose lines it holds for certain, or the agents `named` when
/// that was worked out before. Lines several agents at work together may have
/// written name none of them but the committer. An `amend` keeps the agents of
/// the commit it replaces. Also answers with the note for the commit's hooks to
/// put on it, when it holds lines of several writers. Where commits name no agent
/// that is no footer, and the notes alone say who made it.
fn footer(
    core: &Core,
    dir: &Path,
    named: Option<&[String]>,
    fallback: Option<&str>,
    index: Option<&Path>,
    amend: bool,
    attributed: bool,
) -> Result<(Vec<String>, Option<PendingNote>), String> {
    let (staged, (tree, parents)) = std::thread::scope(|scope| {
        let made_of = scope.spawn(|| made_of(dir, index, amend));
        let staged = staged_in(core, dir, index, amend);
        let made_of = made_of
            .join()
            .unwrap_or_else(|_| Err("git did not run".to_owned()));
        made_of.map(|made_of| (staged, made_of))
    })?;
    let writers: Vec<Agent> = match (named, &staged) {
        (Some(named), _) => named.iter().filter_map(|id| known(core, dir, id)).collect(),
        (None, Ok((owners, _))) => {
            let committers = fallback
                .map(|id| super::owners::standing_for(owners, core, id))
                .unwrap_or_default();
            owners
                .writers()
                .filter(|(at, author)| author.shared.is_empty() && !committers.contains(at))
                .map(|(_, author)| agent(core, author))
                .collect()
        }
        (None, Err(error)) => return Err(error.clone()),
    };
    let mut named: Vec<Agent> = fallback
        .and_then(|id| known(core, dir, id))
        .into_iter()
        .collect();
    if amend {
        named.extend(verified(core, dir, "HEAD")?);
    }
    named.extend(writers);
    let keys: HashMap<&str, &str> = named
        .iter()
        .map(|agent| (agent.chat.as_str(), agent.key()))
        .collect();
    let mut agents: Vec<Agent> = Vec::new();
    for agent in &named {
        if !agents.iter().any(|kept| kept.key() == agent.key()) {
            agents.push(agent.clone());
        }
    }
    let commit = Commit {
        tree: &tree,
        parents: &parents,
    };
    let key = core.remote().host_key();
    let note = staged
        .ok()
        .and_then(|(owners, diff)| note::of(&owners, &diff, &keys, fallback.is_none()))
        .zip(key.as_ref())
        .map(|(note, key)| trailers::signed_note(&note, &commit, key));
    if !attributed {
        let pending = unattributed(&agents, note, tree, parents, key.as_ref());
        return Ok((Vec::new(), pending));
    }
    let footer = trailers::footer(&agents, &commit, key.as_ref());
    let pending = (!footer.is_empty() && (amend || note.is_some())).then_some(PendingNote {
        tree,
        parents,
        note,
        committer: None,
    });
    Ok((footer, pending))
}

/// What a commit naming no agent carries for its hooks to put on it: the note of who made it,
/// the first of `agents`, and whose lines it holds, with the note of which lines each wrote.
fn unattributed(
    agents: &[Agent],
    note: Option<String>,
    tree: String,
    parents: Vec<String>,
    key: Option<&iroh::SecretKey>,
) -> Option<PendingNote> {
    let made_of = Commit {
        tree: &tree,
        parents: &parents,
    };
    let committer = key
        .zip(Committer::of_agents(agents))
        .and_then(|(key, committer)| committer.signed(&made_of, key));
    (note.is_some() || committer.is_some()).then_some(PendingNote {
        tree,
        parents,
        note,
        committer,
    })
}

/// The commit a rebase or cherry-pick is making again, which still carries a
/// footer and notes signed for where that commit stood.
struct Remaking {
    from: String,
    /// A fixup or squash folds it into `HEAD` rather than adding a commit.
    into_head: bool,
}

fn remaking(dir: &Path) -> Option<Remaking> {
    let done = git(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "rebase-merge/done",
        ],
    )
    .ok()?;
    let Ok(done) = std::fs::read_to_string(done.trim()) else {
        let from = git(
            dir,
            &["rev-parse", "--verify", "--quiet", "CHERRY_PICK_HEAD"],
        )
        .ok()?;
        return Some(Remaking {
            from: from.trim().to_owned(),
            into_head: false,
        });
    };
    let mut step = done.lines().last()?.split_whitespace();
    let into_head = match step.next()? {
        "pick" | "p" | "reword" | "r" => false,
        "fixup" | "f" | "squash" | "s" => true,
        _ => return None,
    };
    let from = step.find(|word| !word.starts_with('-'))?.to_owned();
    Some(Remaking { from, into_head })
}

/// The footer and notes for the commit `remaking` makes: the agents and notes
/// of the commit it comes from, whoever is rebasing, signed for where it now
/// stands. A commit with no footer of this host's stays as it was, so this can
/// never put a name on one.
fn remade(
    core: &Core,
    dir: &Path,
    index: Option<&Path>,
    remaking: &Remaking,
) -> Result<(Vec<String>, Option<PendingNote>), String> {
    let signing = Signing::of(core);
    let Some(key) = signing.key.as_ref() else {
        return Ok((Vec::new(), None));
    };
    let (tree, parents) = made_of(dir, index, remaking.into_head)?;
    let made = Commit {
        tree: &tree,
        parents: &parents,
    };
    let (from_tree, from_parents, message) = read_commit(dir, &remaking.from)?;
    let was = Commit {
        tree: &from_tree,
        parents: &from_parents,
    };
    let mut agents = Vec::new();
    if remaking.into_head {
        let (head_tree, head_parents, head_message) = read_commit(dir, "HEAD")?;
        let head = Commit {
            tree: &head_tree,
            parents: &head_parents,
        };
        agents.extend(signing.agents_of(&head_message, &head).unwrap_or_default());
    }
    for agent in signing.agents_of(&message, &was).unwrap_or_default() {
        if !agents.iter().any(|kept: &Agent| kept.key() == agent.key()) {
            agents.push(agent);
        }
    }
    let footer = match attributed(dir) {
        true => trailers::footer(&agents, &made, Some(key)),
        false => Vec::new(),
    };
    let note = (!footer.is_empty())
        .then(|| {
            note_again::<Note>(
                dir,
                trailers::NOTES_REF,
                &remaking.from,
                &was,
                &made,
                key,
                &signing.hosts,
            )
        })
        .flatten();
    let committer = note_again::<Committer>(
        dir,
        trailers::COMMITTERS_REF,
        &remaking.from,
        &was,
        &made,
        key,
        &signing.hosts,
    );
    let pending = (note.is_some() || committer.is_some()).then_some(PendingNote {
        tree,
        parents,
        note,
        committer,
    });
    Ok((footer, pending))
}

/// The tree `index` makes, and the parents a commit of it takes: those of
/// `HEAD` when amending it, else `HEAD` and those of a merge underway.
fn made_of(dir: &Path, index: Option<&Path>, amend: bool) -> Result<(String, Vec<String>), String> {
    let text = |out: Vec<u8>| String::from_utf8_lossy(&out).trim().to_owned();
    let tree = text(run(dir, &["write-tree"], index, None)?);
    let parents = if amend {
        git(dir, &["rev-parse", "HEAD^@"])?
            .lines()
            .map(str::to_owned)
            .collect()
    } else {
        let merging = git(
            dir,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "MERGE_HEAD",
            ],
        )?;
        let head = git(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok();
        let merged = std::fs::read_to_string(merging.trim()).unwrap_or_default();
        head.iter()
            .map(|head| head.trim().to_owned())
            .chain(merged.split_whitespace().map(str::to_owned))
            .collect()
    };
    Ok((tree, parents))
}

/// `author` as it is now: the CLI's version and the model of its open chat,
/// else the model of its latest edit, and the Sikemux it runs in.
pub fn agent(core: &Core, author: &Author) -> Agent {
    let chat = core.chats().get(&author.agent_id);
    Agent {
        name: author.name.clone(),
        cli: author.cli.clone(),
        cli_version: chat.as_ref().and_then(|chat| chat.cli_version()),
        model: chat
            .and_then(|chat| chat.current_launch().model)
            .or_else(|| author.model.clone()),
        sikemux: Some(core.build().version.clone()),
        session: core.names().last_session(&author.agent_id),
        chat: author.agent_id.clone(),
    }
}

/// `agent_id` as its latest edit in `dir`'s project names it, else as the
/// chat or terminal it runs in does.
pub fn known(core: &Core, dir: &Path, agent_id: &str) -> Option<Agent> {
    if let Some(author) = latest(core, dir, agent_id) {
        return Some(agent(core, &author));
    }
    let chat = core.chats().get(agent_id);
    let cli = match &chat {
        Some(chat) => chat.provider().to_owned(),
        None => core.all_sessions().iter().find_map(|session| {
            let owner = session.owner();
            (owner.agent_id.as_deref() == Some(agent_id))
                .then(|| owner.agent_type.clone())
                .flatten()
        })?,
    };
    let name = core.names().name(agent_id).unwrap_or_else(|| cli.clone());
    Some(Agent {
        name,
        cli,
        cli_version: chat.as_ref().and_then(|chat| chat.cli_version()),
        model: chat.and_then(|chat| chat.current_launch().model),
        sikemux: Some(core.build().version.clone()),
        session: core.names().last_session(agent_id),
        chat: agent_id.to_owned(),
    })
}

/// The agents `commit`'s footer names that this host knows: a session one of its CLIs keeps, or
/// a chat it recorded. The footer's signature is not asked for, so a commit keeps its agents
/// through a rebase, a reword or an amend. A commit without a footer has the agents its
/// signed note of who made it names.
pub fn verified(core: &Core, dir: &Path, commit: &str) -> Result<Vec<Agent>, String> {
    let (_, _, message) = read_commit(dir, commit)?;
    let project = core.workspaces().project_of(dir);
    let named = trailers::agents(&message);
    if named.is_empty() {
        return Ok(super::committer::of(core, dir, commit)
            .map(Committer::agents)
            .unwrap_or_default());
    }
    Ok(named
        .into_iter()
        .filter(|agent| recorded(core, &project, agent))
        .collect())
}

/// `commit`'s tree, parents and message.
pub fn read_commit(dir: &Path, commit: &str) -> Result<(String, Vec<String>, String), String> {
    let object = git(dir, &["cat-file", "commit", commit])?;
    let (header, message) = object.split_once("\n\n").unwrap_or((&object, ""));
    let mut tree = String::new();
    let mut parents = Vec::new();
    for line in header.lines() {
        match line.split_once(' ') {
            Some(("tree", id)) => tree = id.to_owned(),
            Some(("parent", id)) => parents.push(id.to_owned()),
            _ => {}
        }
    }
    Ok((tree, parents, message.to_owned()))
}

fn recorded(core: &Core, project: &str, agent: &Agent) -> bool {
    let chat = agent.chat.as_str();
    agent
        .session
        .as_deref()
        .is_some_and(|session| core.names().knows_session(session))
        || core.names().name(chat).is_some()
        || core.names().last_session(chat).is_some()
        || !core
            .edits()
            .matching(project, |edit| edit.agent_id == chat)
            .is_empty()
        || agent.session.as_deref().is_some_and(|session| {
            (core.repo_above().saved_locate)(core, session, Some(&agent.cli)).is_ok()
        })
}

#[derive(Deserialize)]
struct Pending {
    /// The index git commits from, when not the checkout's own.
    index: Option<PathBuf>,
    #[serde(default)]
    amend: bool,
}

/// The footer for the commit an agent's `git commit`, or one the core runs
/// under `grant`, is making, which its hooks ask for.
pub async fn call(
    core: &Arc<Core>,
    request: &HarnessRequest,
    grant: Option<CommitGrant>,
) -> Result<Value, String> {
    let (named, fallback) = match grant {
        Some(grant) => (Some(grant.agents), grant.fallback),
        None => (
            None,
            Some(
                request
                    .agent_id
                    .clone()
                    .ok_or("only an agent's commits carry a footer")?,
            ),
        ),
    };
    let pending: Pending =
        serde_json::from_value(request.params.clone()).map_err(|error| error.to_string())?;
    let (core, dir) = (core.clone(), PathBuf::from(&request.project));
    let (footer, note) = blocking(move || {
        if let Some(remaking) = (!pending.amend).then(|| remaking(&dir)).flatten() {
            return Ok(remade(&core, &dir, pending.index.as_deref(), &remaking)?);
        }
        Ok(footer(
            &core,
            &dir,
            named.as_deref(),
            fallback.as_deref(),
            pending.index.as_deref(),
            pending.amend,
            attributed(&dir),
        )?)
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(json!({ "footer": footer, "note": note }))
}

/// How long a commit the core runs may take, hooks and all.
const COMMIT_WAIT: Duration = Duration::from_secs(5 * 60);
const COMMIT_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

/// Commits what `dir` has staged with `message`, plus `args`, through the
/// hooks an agent's own commit runs: once the repository's hooks are done,
/// ours put on the footer naming the agents whose lines were staged as it
/// began, else `fallback`. Only this git may ask for that footer, and only
/// once. Whose lines they are is worked out first, since a hook that reformats
/// the files leaves the record unable to say. A commit naming nobody runs
/// without our hooks.
pub fn commit(
    core: &Core,
    dir: &Path,
    args: &[&str],
    message: &str,
    fallback: Option<&str>,
) -> Result<Output, String> {
    let staged: Vec<String> = staged_in(core, dir, None, false)
        .map(|(owners, _)| {
            owners
                .writers()
                .filter(|(_, author)| author.shared.is_empty())
                .map(|(_, author)| author.agent_id.clone())
                .collect()
        })
        .unwrap_or_default();
    let signed = !staged.is_empty() || fallback.is_some();
    let mut command = crate::git::git_command(dir);
    command.args(["commit", "-F", "-"]).args(args);
    if signed {
        crate::git_guard::shim::sign_commit(&mut command, dir);
    }
    let git = Cell::new(None);
    let output = sikemux_process::run_with_pid(
        &mut command,
        Some(message.as_bytes()),
        COMMIT_WAIT,
        COMMIT_OUTPUT_LIMIT,
        None,
        |pid| {
            if signed {
                callers::grant_commit(pid, staged, fallback, COMMIT_WAIT);
                git.set(Some(pid));
            }
        },
    );
    if let Some(pid) = git.get() {
        callers::end_commit(pid);
    }
    output.map_err(|error| format!("git did not run: {error}"))
}

/// Tells the core `agent` just made the commit at `HEAD` of the checkout at `dir`.
pub fn head_made(core: &Core, agent: &str, dir: &Path) -> Result<(), String> {
    let top = git(dir, &["rev-parse", "--show-toplevel"])?;
    let top = Path::new(top.trim());
    let head = git(top, &["log", "-1", "--format=%H%x00%B"])?;
    let (sha, message) = head.split_once('\0').ok_or("git read no commit")?;
    let paths = git(
        top,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            "HEAD",
        ],
    )?;
    let paths: Vec<String> = paths.lines().map(str::to_owned).collect();
    crate::server::repo_events::commit_made(
        core,
        CommitMade {
            agent,
            top,
            sha,
            paths: &paths,
            message,
        },
    );
    Ok(())
}

/// An agent's own `git commit` made `HEAD`, as its hook says.
pub async fn made(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = (request.agent_id.clone()).ok_or("only an agent's commits are told")?;
    let (core, dir) = (core.clone(), PathBuf::from(&request.project));
    blocking(move || {
        head_made(&core, &agent, &dir)?;
        Ok(Value::Null)
    })
    .await
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::edits::tests_git::git_in;

    /// On 2026-11-23 a person whose commits must not credit an agent still got "Agent:" lines:
    /// the commits the land queue makes by itself put the footer on whatever the setting said.
    #[test]
    fn a_commit_the_core_makes_names_no_agent_where_attribution_is_off() {
        let root = tempfile::tempdir().unwrap();
        let dir = sikemux_process::files::real_path(root.path()).unwrap();
        git_in(&dir, &["init", "-q", "-b", "main"]);
        let agent = Agent {
            name: "Otter".to_owned(),
            cli: "claude".to_owned(),
            cli_version: None,
            model: None,
            sikemux: None,
            session: None,
            chat: "chat".to_owned(),
        };
        let parents = vec!["0".repeat(40)];
        let made_of = Commit {
            tree: "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
            parents: &parents,
        };
        let key = iroh::SecretKey::generate();
        let footer = || of_core_commit(&dir, std::slice::from_ref(&agent), &made_of, Some(&key));
        git_in(&dir, &["config", "sikemux.attribution", "true"]);
        assert!(footer().iter().any(|line| line.starts_with("Agent: Otter")));

        git_in(&dir, &["config", "sikemux.attribution", "false"]);
        assert_eq!(footer(), Vec::<String>::new());
    }

    /// With attribution off, an agent's own `git commit` got no footer and no notes, so who
    /// wrote which of its lines was lost.
    #[test]
    fn a_commit_naming_no_agent_keeps_who_made_it_and_whose_lines_it_holds() {
        let root = tempfile::tempdir().unwrap();
        let dir = sikemux_process::files::real_path(root.path()).unwrap();
        git_in(&dir, &["init", "-q", "-b", "main"]);
        git_in(&dir, &["config", "user.name", "t"]);
        git_in(&dir, &["config", "user.email", "t@t"]);
        git_in(&dir, &["config", "sikemux.attribution", "false"]);
        std::fs::write(dir.join("both.txt"), "otter\nheron\n").unwrap();
        git_in(&dir, &["add", "both.txt"]);
        git_in(&dir, &["commit", "-qm", "both"]);
        let sha = git_in(&dir, &["rev-parse", "HEAD"]);
        let (tree, parents, _) = read_commit(&dir, &sha).unwrap();
        let agent = |name: &str| Agent {
            name: name.to_owned(),
            cli: "claude".to_owned(),
            cli_version: None,
            model: None,
            sikemux: None,
            session: Some(format!("{name}-session")),
            chat: format!("{name}-chat"),
        };
        let agents = [agent("Otter"), agent("Heron")];
        let lines =
            r#"{"Heron-session":{"both.txt":[[2,2]]},"Otter-session":{"both.txt":[[1,1]]}}"#;
        let key = iroh::SecretKey::generate();
        let made_of = Commit {
            tree: &tree,
            parents: &parents,
        };
        let note = trailers::signed_note(lines, &made_of, &key);
        let pending = unattributed(
            &agents,
            Some(note),
            tree.clone(),
            parents.clone(),
            Some(&key),
        )
        .expect("notes to put on the commit");
        let put = |notes: &str, text: &str| {
            let notes = format!("--ref={notes}");
            run(
                &dir,
                &["notes", &notes, "add", "-f", "-F", "-", &sha],
                None,
                Some(text.as_bytes()),
            )
            .unwrap();
        };
        put(trailers::NOTES_REF, pending.note.as_deref().unwrap());
        put(
            trailers::COMMITTERS_REF,
            pending.committer.as_deref().unwrap(),
        );

        let hosts = [key.public()];
        let read = |notes: &str| {
            let blob = git(&dir, &["notes", &format!("--ref={notes}"), "show", &sha]).unwrap();
            trailers::verify_note(&blob, &made_of, &hosts)
                .unwrap()
                .to_owned()
        };
        let named = serde_json::from_str::<Committer>(&read(trailers::COMMITTERS_REF)).unwrap();
        let keys: Vec<String> = (named.agents().iter())
            .map(|agent| agent.key().to_owned())
            .collect();
        assert_eq!(keys, ["Otter-session", "Heron-session"]);
        let wrote = note::read(&dir, &[sha.as_str()], &hosts);
        let wrote = &wrote[&sha];
        assert!(keys.iter().all(|key| wrote.contains_key(key)));
    }
}
