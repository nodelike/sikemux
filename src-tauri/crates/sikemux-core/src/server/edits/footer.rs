//! The signed footer naming the agents whose lines a commit holds, for every
//! way a commit is made here: the git panel, `git_commit`, Auto, and an
//! agent's own `git commit` through the git shim.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::note;
use super::owners::{git, latest, run, staged_in, Author};
use crate::cli::protocol::HarnessRequest;
use crate::server::callers::{self, CommitGrant};
use crate::server::connection::blocking;
use crate::server::names::project_of;
use crate::server::saved;
use crate::server::Core;
use crate::trailers::{self, Agent, Commit, PendingNote};

/// Whether commits made in `dir` may name the agents that wrote them. Work
/// repositories turn it off with `git config sikemux.attribution false`.
pub(crate) fn attributed(dir: &Path) -> bool {
    git(
        dir,
        &["config", "--type=bool", "--get", "sikemux.attribution"],
    )
    .map_or(true, |value| value.trim() != "false")
}

/// The footer for committing what `dir` has staged, or what `index` holds when
/// git commits from an index of its own: `fallback`, the agent committing, and
/// every agent whose lines it holds for certain, or the agents `named` when
/// that was worked out before. Lines several agents at work together may have
/// written name none of them but the committer. An `amend` keeps the agents of
/// the commit it replaces. With it, the note for
/// the commit's hooks to put on it, when it holds lines of several writers.
fn footer(
    core: &Core,
    dir: &Path,
    named: Option<&[String]>,
    fallback: Option<&str>,
    index: Option<&Path>,
    amend: bool,
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
                .map(|id| owners.standing_for(core, id))
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
    let key = core.remote.host_key();
    let note = staged
        .ok()
        .and_then(|(owners, diff)| note::of(&owners, &diff, &keys, fallback.is_none()))
        .zip(key.as_ref())
        .map(|(note, key)| trailers::signed_note(&note, &commit, key));
    let footer = trailers::footer(&agents, &commit, key.as_ref());
    let pending = (!footer.is_empty() && (amend || note.is_some())).then_some(PendingNote {
        tree,
        parents,
        note,
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
pub(super) fn agent(core: &Core, author: &Author) -> Agent {
    let chat = core.chats.get(&author.agent_id);
    Agent {
        name: author.name.clone(),
        cli: author.cli.clone(),
        cli_version: chat.as_ref().and_then(|chat| chat.cli_version()),
        model: chat
            .and_then(|chat| chat.current_launch().model)
            .or_else(|| author.model.clone()),
        sikemux: Some(core.build.version.clone()),
        session: core.names.last_session(&author.agent_id),
        chat: author.agent_id.clone(),
    }
}

/// `agent_id` as its latest edit in `dir`'s project names it, else as the
/// chat or terminal it runs in does.
pub(super) fn known(core: &Core, dir: &Path, agent_id: &str) -> Option<Agent> {
    if let Some(author) = latest(core, dir, agent_id) {
        return Some(agent(core, &author));
    }
    let chat = core.chats.get(agent_id);
    let cli = match &chat {
        Some(chat) => chat.provider().to_owned(),
        None => core.all_sessions().iter().find_map(|session| {
            let owner = session.owner();
            (owner.agent_id.as_deref() == Some(agent_id))
                .then(|| owner.agent_type.clone())
                .flatten()
        })?,
    };
    let name = core.names.name(agent_id).unwrap_or_else(|| cli.clone());
    Some(Agent {
        name,
        cli,
        cli_version: chat.as_ref().and_then(|chat| chat.cli_version()),
        model: chat.and_then(|chat| chat.current_launch().model),
        sikemux: Some(core.build.version.clone()),
        session: core.names.last_session(agent_id),
        chat: agent_id.to_owned(),
    })
}

/// The agents `commit`'s footer names, when it checks out: signed by this
/// host or one linked with it, over the commit's own tree and parents, and
/// naming only agents this host knows: a session one of its CLIs keeps, or a
/// chat it recorded. Anything else is text anyone could have written.
pub(crate) fn verified(core: &Core, dir: &Path, commit: &str) -> Result<Vec<Agent>, String> {
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
    let commit = Commit {
        tree: &tree,
        parents: &parents,
    };
    let Some((_, agents)) = trailers::verify(message, &commit, &core.remote.host_keys()) else {
        return Ok(Vec::new());
    };
    let project = project_of(dir, &core.workspaces.project_paths());
    Ok(agents
        .into_iter()
        .filter(|agent| recorded(core, &project, agent))
        .collect())
}

fn recorded(core: &Core, project: &str, agent: &Agent) -> bool {
    let chat = agent.chat.as_str();
    agent
        .session
        .as_deref()
        .is_some_and(|session| core.names.knows_session(session))
        || core.names.name(chat).is_some()
        || core.names.last_session(chat).is_some()
        || !core
            .edits
            .matching(project, |edit| edit.agent_id == chat)
            .is_empty()
        || agent
            .session
            .as_deref()
            .is_some_and(|session| saved::locate(core, session, Some(&agent.cli)).is_ok())
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
pub(crate) async fn call(
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
        if !attributed(&dir) {
            return Ok((Vec::new(), None));
        }
        Ok(footer(
            &core,
            &dir,
            named.as_deref(),
            fallback.as_deref(),
            pending.index.as_deref(),
            pending.amend,
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
pub(crate) fn commit(
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
    let signed = (!staged.is_empty() || fallback.is_some()) && attributed(dir);
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
