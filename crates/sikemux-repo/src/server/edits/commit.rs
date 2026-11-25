//! Commits the lines one agent wrote and nothing else. The commit is built in
//! an index of its own from the last commit plus those lines, and the branch
//! moves only if no one moved it meanwhile. The checkout's files stay as they
//! are, and the shared index takes in only what was committed.

use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::committer;
use super::footer::{agent, attributed, known};
use super::named;
use super::owners::{git, run, traced, Diff, FileOwners, Hunk, Owners};
use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::repo_events::{commit_made, CommitMade};
use crate::server::Core;
use crate::tasks::config::{self, ConfigLoad};
use crate::trailers::Commit;
use sikemux_wire::protocol::PermissionMode;

const ATTEMPTS: usize = 4;
/// Files a commit's answer lists as left out; the rest are counted.
const MAX_LEFT: usize = 12;
/// Files a commit's answer lists as taken; the rest are counted.
const MAX_LISTED: usize = 20;
const INDEX_LOCKED_TRIES: usize = 5;
/// How long a call waits on its push before letting it finish on its own, well
/// inside how long a tool call may take.
const PUSH_ANSWER_WAIT: Duration = Duration::from_secs(40);

#[derive(Serialize)]
pub struct Committed {
    commit: String,
    branch: String,
    files: Vec<Taken>,
    #[serde(rename = "moreFiles", skip_serializing_if = "is_zero")]
    more_files: usize,
    left: Vec<Left>,
    #[serde(rename = "moreLeft", skip_serializing_if = "is_zero")]
    more_left: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

/// What `git_commit` did: the commit, and what it pushed when asked to.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Done {
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    committed: Option<Committed>,
    /// Why nothing was committed before landing or pushing.
    #[serde(skip_serializing_if = "Option::is_none")]
    nothing_committed: Option<String>,
    /// Why the commit made was not synced, landed or pushed.
    #[serde(skip_serializing_if = "Option::is_none")]
    not_synced: Option<String>,
    /// Why the commit made was neither queued to land nor pushed.
    #[serde(skip_serializing_if = "Option::is_none")]
    not_landed: Option<String>,
    /// Why its commits were not pushed.
    #[serde(skip_serializing_if = "Option::is_none")]
    not_pushed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    still_pushing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    synced: Option<super::land::Synced>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    queued: Option<Landing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pushed: Option<super::push::Pushed>,
}

#[derive(Serialize)]
enum Landing {
    #[serde(rename = "queued")]
    Here(super::queue::Queued),
    /// A worker's commits went back to its lead on another host, whose host lands them.
    #[serde(rename = "sentToLead")]
    AtLead(String),
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

#[derive(Serialize)]
struct Taken {
    path: String,
    added: u64,
    removed: u64,
}

/// A file with changes the commit left out, and who made them.
#[derive(Serialize)]
struct Left {
    path: String,
    by: Vec<String>,
    /// Those among `by` still open, who can commit what the others left.
    #[serde(skip)]
    open: Vec<String>,
}

/// Why a commit was not made: none of the uncommitted lines were the agent's,
/// which landing or pushing what it committed before can go past, or anything
/// else.
pub enum Refusal {
    NothingYours(String),
    Other(String),
}

impl From<String> for Refusal {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<Refusal> for String {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::NothingYours(message) | Refusal::Other(message) => message,
        }
    }
}

/// One file as the commit holds it: `content` none when it is deleted.
struct Change {
    path: String,
    mode: String,
    content: Option<Vec<u8>>,
    before: Option<String>,
    added: u64,
    removed: u64,
}

/// `git_commit`: commits the calling agent's lines in its checkout, then
/// when asked brings a checkout of its own up to the branch it was cut from,
/// lands them on that branch, and pushes its unpushed commits.
pub async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = crate::server::caller::agent_of(request, "git_commit")?;
    let flag = |name: &str| request.params.get(name).and_then(Value::as_bool) == Some(true);
    let (sync, land, push, force) = (flag("sync"), flag("land"), flag("push"), flag("force"));
    if force && (land || !push) {
        return Err("force goes with push and without land".into());
    }
    let message = match request.params.get("message") {
        Some(Value::String(message)) => Some(message.clone()),
        None | Some(Value::Null) => None,
        _ => return Err("message must be the commit message".into()),
    };
    let named: Vec<String> = match request.params.get("paths") {
        None | Some(Value::Null) => Vec::new(),
        Some(paths) => {
            serde_json::from_value(paths.clone()).map_err(|_| "paths must be a list of paths")?
        }
    };
    let project = PathBuf::from(&request.project);
    let core = core.clone();
    blocking(move || {
        let (dir, named) = checkout_for(&core, &agent_id, &project, &named)?;
        let message = message.or_else(|| Underway::of(&dir).ok()?.message);
        if message.is_none() && !(sync || land || push) {
            return Err("message must be the commit message".to_owned().into());
        }
        let own = super::land::own_checkout_of(&core, &agent_id, &dir);
        if land && own.is_none() {
            lands_from(&core, &agent_id, &project, &dir)?;
        }
        if land && own.as_ref().is_some_and(|tree| super::push::on_release_line(&tree.path)) {
            return Err("This checkout is on a release line, which never lands on the branch it was cut from; push it with push: true and no land".to_owned().into());
        }
        let (committed, nothing_committed) = match &message {
            Some(message) => match commit(&core, &dir, &agent_id, message, &named) {
                Ok(committed) => (Some(committed), None),
                Err(Refusal::NothingYours(why)) if sync || land || push => (None, Some(why)),
                Err(refusal) => return Err(String::from(refusal).into()),
            },
            None => (None, None),
        };
        let synced = match sync {
            true => match super::land::sync(&core, &agent_id, &dir) {
                Ok(synced) => Some(synced),
                Err(why) if committed.is_some() => {
                    return Ok(Done {
                        committed,
                        nothing_committed,
                        not_synced: Some(why),
                        ..Done::default()
                    })
                }
                Err(why) => return Err(why.into()),
            },
            false => None,
        };
        let ship = || crate::ship::Ship {
            kind: sikemux_view::inbox::ShipKind::Push,
            what: "git push origin HEAD".to_owned(),
            target: None,
        };
        let mut not_pushed = None;
        let queued = match land {
            true => {
                let from = own.as_ref().map_or(&dir, |tree| &tree.repo);
                let push_after = push
                    && match super::super::ship::check(&core, &agent_id, ship(), from) {
                        Ok(_) => true,
                        Err(why) => {
                            not_pushed = Some(why);
                            false
                        }
                    };
                let at_lead = match own {
                    Some(_) => None,
                    None => (core.repo_above().hosts_land_at_lead)(&core, &agent_id, push_after),
                };
                let queued = match at_lead {
                    Some(sent) => sent.map(Landing::AtLead),
                    None => super::queue::enqueue(&core, &agent_id, push_after).map(Landing::Here),
                };
                match queued {
                    Ok(queued) => Some(queued),
                    Err(why) if committed.is_some() || synced.is_some() => {
                        return Ok(Done {
                            committed,
                            nothing_committed,
                            synced,
                            not_landed: Some(why),
                            ..Done::default()
                        })
                    }
                    Err(why) => return Err(why.into()),
                }
            }
            false => None,
        };
        let pushed = match push && !land {
            true => {
                let from = match own {
                    Some(tree)
                        if force
                            || super::push::on_release_line(&tree.path)
                            || super::land::unlanded(&tree)? =>
                    {
                        tree.path
                    }
                    Some(tree) => tree.repo,
                    None => dir,
                };
                not_pushed = super::super::ship::check(&core, &agent_id, ship(), &from).err();
                match not_pushed {
                    None => match push_or_tell(&core, from, &agent_id, force) {
                        Ok(pushed) => pushed,
                        Err(why) if committed.is_some() || synced.is_some() => {
                            not_pushed = Some(why);
                            None
                        }
                        Err(why) => return Err(why.into()),
                    },
                    Some(_) => None,
                }
            }
            false => None,
        };
        let still_pushing = (push && !land && not_pushed.is_none() && pushed.is_none()).then(|| {
            "The push is still running, often the repository's pre-push hook, or goes after another agent's from the same checkout, and has no time limit. End your turn or go on with other work: Sikemux wakes you with how it went and the end of the hook's output once it ends.".to_owned()
        });
        Ok(Done {
            committed,
            nothing_committed,
            not_pushed,
            still_pushing,
            synced,
            queued,
            pushed,
            ..Done::default()
        })
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|done| serde_json::to_value(done).map_err(|error| error.to_string()))
}

/// Pushes from `from` as `agent`, answering within [`PUSH_ANSWER_WAIT`]: none
/// when the push is still going, and the agent is then woken with how it went.
/// A push asked for while one from the same checkout runs goes after it, as its
/// own agent's, rather than running the repository's pre-push hook beside it.
fn push_or_tell(
    core: &Arc<Core>,
    from: PathBuf,
    agent: &str,
    force: bool,
) -> Result<Option<super::push::Pushed>, String> {
    let mut turn = Turn {
        agent: agent.to_owned(),
        force,
    };
    let Some(mut running) = Running::start(&from, &turn) else {
        return Ok(None);
    };
    let (answer, answered) = std::sync::mpsc::channel();
    let runtime = tokio::runtime::Handle::try_current().ok();
    let owner = core.clone();
    std::thread::spawn(move || {
        let _entered = runtime.as_ref().map(tokio::runtime::Handle::enter);
        let mut answer = Some(answer);
        loop {
            let pushed = push_turn(&owner, runtime.as_ref(), &from, &turn);
            let next = running.next();
            let unheard = match answer.take() {
                Some(answer) => answer.send(pushed).err().map(|unheard| unheard.0),
                None => Some(pushed),
            };
            if let (Some(pushed), Some(runtime)) = (unheard, &runtime) {
                tell_pushed(&owner, runtime, turn.agent, &pushed);
            }
            match next {
                Some(next) => turn = next,
                None => return,
            }
        }
    });
    match answered.recv_timeout(PUSH_ANSWER_WAIT) {
        Ok(pushed) => {
            let pushed = pushed?;
            pushed.unsent().map_or(Ok(Some(pushed)), Err)
        }
        Err(_) => Ok(None),
    }
}

fn push_turn(
    core: &Arc<Core>,
    runtime: Option<&tokio::runtime::Handle>,
    from: &Path,
    turn: &Turn,
) -> Result<super::push::Pushed, String> {
    checked_before_push(core, runtime, from, &turn.agent)?;
    let agents = std::slice::from_ref(&turn.agent);
    match turn.force {
        true => super::push::force_push(core, from, agents, "origin"),
        false => super::push::push(core, from, agents, "origin"),
    }
}

/// Wakes `agent` with how its push went.
fn tell_pushed(
    core: &Arc<Core>,
    runtime: &tokio::runtime::Handle,
    agent: String,
    pushed: &Result<super::push::Pushed, String>,
) {
    let text = match pushed {
        Ok(pushed) => {
            super::push::tell_sent_back(core, runtime, pushed);
            let said = pushed
                .unsent()
                .unwrap_or_else(|| serde_json::to_string(pushed).unwrap_or_default());
            format!("Your push ended: {said}")
        }
        Err(why) => format!("Your push failed: {why}"),
    };
    let core = core.clone();
    runtime.spawn(async move {
        let told =
            (core.repo_above().messages_work_from_sikemux)(&core, agent, text, "about its push");
        if let Err(error) = told.await {
            eprintln!("sikemux core: how a push went did not reach its agent: {error}");
        }
    });
}

/// Refuses a land asked for in `dir` when that isn't the checkout a land of this agent takes.
fn lands_from(core: &Core, agent_id: &str, project: &Path, dir: &Path) -> Result<(), String> {
    let home = match super::land::own_checkout(core, agent_id) {
        Some(tree) => tree.path,
        None => core
            .workspaces()
            .agent(agent_id)
            .map_or_else(|| project.to_owned(), |agent| agent.cwd),
    };
    let top = super::land::top(dir)?;
    match super::land::top(&home) {
        Ok(home) if home != top => Err(format!(
            "Nothing was committed or landed: land takes your checkout, {}, and these paths are in {}, which has no branch on record to land on. Push its own branch with push: true and no land.",
            home.display(),
            top.display()
        )),
        _ => Ok(()),
    }
}

/// A push an agent asked for.
#[derive(Clone, PartialEq)]
struct Turn {
    agent: String,
    force: bool,
}

/// The push running from each checkout, then those that go after it.
static RUNNING: std::sync::Mutex<BTreeMap<PathBuf, VecDeque<Turn>>> =
    std::sync::Mutex::new(BTreeMap::new());

/// The pushes from one checkout, ended when dropped.
struct Running(Option<PathBuf>);

impl Running {
    /// None when a push from `from` is already running; `turn` then goes after it.
    fn start(from: &Path, turn: &Turn) -> Option<Self> {
        let mut running = RUNNING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(turns) = running.get_mut(from) {
            if !turns.contains(turn) {
                turns.push_back(turn.clone());
            }
            return None;
        }
        running.insert(from.to_owned(), VecDeque::from([turn.clone()]));
        Some(Self(Some(from.to_owned())))
    }

    /// Ends the running push, and gives the one to run after it.
    fn next(&mut self) -> Option<Turn> {
        let mut running = RUNNING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let turns = running.get_mut(self.0.as_ref()?)?;
        turns.pop_front();
        let next = turns.front().cloned();
        if next.is_none() {
            running.remove(&self.0.take()?);
        }
        next
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(from) = &self.0 {
            let mut running = RUNNING
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            running.remove(from);
        }
    }
}

/// From the project's own checkout a push sends its whole branch, so what it
/// sends passes the land queue's check first, as a land would.
fn checked_before_push(
    core: &Arc<Core>,
    runtime: Option<&tokio::runtime::Handle>,
    from: &Path,
    agent: &str,
) -> Result<(), String> {
    let Some(runtime) = runtime.filter(|_| super::push::is_main_checkout(from)) else {
        return Ok(());
    };
    let top = PathBuf::from(super::push::line(super::owners::git(
        from,
        &["rev-parse", "--show-toplevel"],
    )?));
    let Ok(branch) = super::owners::git(&top, &["symbolic-ref", "--short", "-q", "HEAD"]) else {
        return Ok(());
    };
    let head = super::push::line(super::owners::git(&top, &["rev-parse", "HEAD"])?);
    runtime.block_on(super::queue::check_before_push(
        core,
        &top,
        &super::push::line(branch),
        &head,
        agent,
    ))
}

/// The checkout to commit in, the one holding the `named` paths, which may lie
/// outside the project's own, with those paths as they lie there. A relative
/// path is looked for in the agent's own folder, then the project's, then among
/// the files the agent changed; from a folder of repositories, one of them holds
/// it. With no paths, the agent's own checkout, else the project's, else the one
/// holding what the agent changed and has not committed.
pub fn checkout_for(
    core: &Core,
    agent_id: &str,
    project: &Path,
    named: &[String],
) -> Result<(PathBuf, Vec<String>), String> {
    let own = core
        .workspaces()
        .agent(agent_id)
        .map_or_else(|| project.to_owned(), |agent| agent.cwd);
    let mut edited: Option<HashSet<String>> = None;
    let mut changed = || -> HashSet<String> {
        edited
            .get_or_insert_with(|| core.edits().paths_of(core, agent_id))
            .clone()
    };
    let named: Vec<String> = named
        .iter()
        .map(|path| placed(path, &own, project, &mut changed))
        .collect();
    let mut tops = BTreeSet::new();
    let mut found = Vec::with_capacity(named.len());
    for path in &named {
        let place = Path::new(path);
        let folder = place
            .ancestors()
            .find(|folder| folder.is_dir())
            .unwrap_or(project);
        let (path, top) = match git(folder, &["rev-parse", "--show-toplevel"]) {
            Ok(top) => (path.clone(), top),
            Err(_) => {
                let repo = repo_below(project, place)?
                    .ok_or_else(|| format!("{path} is not in a git checkout"))?;
                let inside = place.strip_prefix(project).unwrap_or(place);
                let top = git(&repo, &["rev-parse", "--show-toplevel"])?;
                (repo.join(inside).to_string_lossy().into_owned(), top)
            }
        };
        found.push(path);
        tops.insert(PathBuf::from(top.trim()));
    }
    match tops.len() {
        0 => {
            if let Some(folder) = [own.as_path(), project]
                .into_iter()
                .find(|folder| git(folder, &["rev-parse", "--show-toplevel"]).is_ok())
            {
                return Ok((folder.to_owned(), found));
            }
            let mut holding = uncommitted_in(&changed());
            match holding.len() {
                1 => Ok((holding.pop().unwrap_or_default(), found)),
                0 => Err(format!(
                    "{} is not in a git checkout, and nothing you changed waits to be committed in one; name the paths to commit, inside the repository that holds them",
                    own.display()
                )),
                _ => Err(format!(
                    "Nothing was committed: what you changed lies in {} checkouts ({}). Commit each checkout's paths in a call of its own.",
                    holding.len(),
                    holding
                        .iter()
                        .map(|top| top.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
        1 => Ok((tops.into_iter().next().unwrap_or_default(), found)),
        _ => Err(format!(
            "Nothing was committed: the paths lie in {} checkouts ({}). Commit each checkout's paths in a call of its own.",
            tops.len(),
            tops.iter()
                .map(|top| top.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Where the relative `path` an agent in `own` named lies: in its folder, then
/// the project's, then the one file it changed whose path ends so. Absolute
/// paths stay as they are.
fn placed(
    path: &str,
    own: &Path,
    project: &Path,
    changed: &mut impl FnMut() -> HashSet<String>,
) -> String {
    let given = Path::new(path);
    if given.is_absolute() {
        return path.to_owned();
    }
    if let Some(found) = [own, project]
        .into_iter()
        .map(|folder| folder.join(given))
        .find(|place| place.symlink_metadata().is_ok())
    {
        return found.to_string_lossy().into_owned();
    }
    let ending = format!("/{}", path.trim_start_matches("./").trim_end_matches('/'));
    let changed = changed();
    let mut ends = changed.iter().filter(|file| file.ends_with(&ending));
    if let (Some(file), None) = (ends.next(), ends.next()) {
        return file.clone();
    }
    let in_git = own.ancestors().any(|folder| folder.join(".git").exists());
    let folder = if in_git { own } else { project };
    folder.join(given).to_string_lossy().into_owned()
}

/// The checkouts where any of `files` has changes not committed yet.
fn uncommitted_in(files: &HashSet<String>) -> Vec<PathBuf> {
    let mut by_top: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for file in files {
        let file = Path::new(file);
        let Some(top) = file
            .ancestors()
            .skip(1)
            .find(|folder| folder.join(".git").exists())
        else {
            continue;
        };
        if let Ok(inside) = file.strip_prefix(top) {
            by_top
                .entry(top.to_owned())
                .or_default()
                .push(format!(":(top,literal){}", inside.display()));
        }
    }
    by_top
        .into_iter()
        .filter(|(top, files)| {
            let mut args = vec!["status", "--porcelain", "-z", "--"];
            args.extend(files.iter().map(String::as_str));
            git(top, &args).is_ok_and(|status| !status.is_empty())
        })
        .map(|(top, _)| top)
        .collect()
}

/// For a `folder` outside any repository whose repositories lie one level down:
/// the one holding `path`, given from the folder or whole. None when none does.
pub fn repo_below(folder: &Path, path: &Path) -> Result<Option<PathBuf>, String> {
    let Some(inside) = path
        .strip_prefix(folder)
        .ok()
        .or_else(|| path.is_relative().then_some(path))
        .filter(|inside| !inside.as_os_str().is_empty())
    else {
        return Ok(None);
    };
    if git(folder, &["rev-parse", "--show-toplevel"]).is_ok() {
        return Ok(None);
    }
    let committed = format!("HEAD:{}", inside.display());
    let mut holding: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|repo| repo.join(".git").exists())
        .filter(|repo| {
            repo.join(inside).symlink_metadata().is_ok()
                || git(repo, &["cat-file", "-e", &committed]).is_ok()
        })
        .collect();
    holding.sort();
    match holding.len() {
        0 | 1 => Ok(holding.pop()),
        _ => Err(format!(
            "{} is in {} repositories here: {}. Name it from the one you mean.",
            inside.display(),
            holding.len(),
            holding
                .iter()
                .map(|repo| repo.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Commits what `agent_id` wrote in `dir`'s checkout, and every change in the
/// `named` paths that holds no other agent's lines.
pub fn commit(
    core: &Core,
    dir: &Path,
    agent_id: &str,
    message: &str,
    named: &[String],
) -> Result<Committed, Refusal> {
    if message.trim().is_empty() {
        return Err(Refusal::Other(
            "message must say what the commit does".into(),
        ));
    }
    let message = &super::footer::without_claude_credit(message);
    for _ in 0..ATTEMPTS {
        let branch = git(dir, &["symbolic-ref", "-q", "HEAD"])
            .map_or_else(|_| "HEAD".to_owned(), |branch| branch.trim().to_owned());
        let (owners, diff) = traced(core, dir, false)?;
        let named = named
            .iter()
            .map(|path| in_checkout(&diff.top, dir, path))
            .collect::<Result<Vec<_>, _>>()?;
        let underway = Underway::of(&diff.top)?;
        let merging = underway.paths(&diff)?;
        let mine = super::owners::standing_for(&owners, core, agent_id);
        let Plan {
            mut changes,
            left,
            unchanged,
        } = plan(&owners, &diff, mine, &named, &merging)?;
        if changes.is_empty() && !unchanged.is_empty() {
            return Err(Refusal::NothingYours(format!(
                "Nothing was committed: {} {} nothing uncommitted in {}. A file changed in another checkout goes by its absolute path.",
                said(&unchanged),
                if unchanged.len() == 1 { "has" } else { "have" },
                diff.top.display()
            )));
        }
        if changes.is_empty() {
            return Err(Refusal::NothingYours(nothing_yours(&left)));
        }
        let unformatted = format(core, agent_id, &diff.top, &mut changes)?;
        let lacking = unresolved(&diff, &changes)?;
        let warning = (!lacking.is_empty()).then(|| {
            format!(
                "This commit may not build on its own: {}. Commit the missing files too, or check the agent writing them commits soon.",
                lacking.join("; ")
            )
        });
        let me = owners
            .authors
            .iter()
            .find(|author| author.agent_id == agent_id);
        let signer = me
            .map(|me| agent(core, me))
            .or_else(|| known(core, dir, agent_id));
        let key = core.remote().host_key();
        let attributed = attributed(dir);
        let signed = |commit: &Commit| {
            if !attributed {
                return message.to_owned();
            }
            let footer = crate::trailers::footer(
                Vec::from_iter(signer.clone()).as_slice(),
                commit,
                key.as_ref(),
            );
            crate::trailers::append(message, &footer)
        };
        let (commit, message, blobs) = build(&diff, &underway.merged, &changes, signed)?;
        let old = diff
            .base
            .clone()
            .unwrap_or_else(|| "0".repeat(commit.len()));
        let subject = message.lines().next().unwrap_or_default();
        let reflog = format!("commit: {subject}");
        let moved = run(
            &diff.top,
            &["update-ref", "-m", &reflog, &branch, &commit, &old],
            None,
            None,
        );
        match moved {
            Ok(_) => {
                if let Some(signer) = signer.as_ref().filter(|_| !attributed) {
                    committer::record(core, &diff.top, &commit, signer);
                }
                tidy(core, &diff.top, agent_id, &changes, &unformatted);
                underway.finish();
                let resolved = !underway.merged.is_empty();
                let note = share(&diff.top, &changes, &blobs, resolved).err();
                let paths: Vec<String> = changes.iter().map(|change| change.path.clone()).collect();
                commit_made(
                    core,
                    CommitMade {
                        agent: agent_id,
                        top: &diff.top,
                        sha: &commit,
                        paths: &paths,
                        message: &message,
                    },
                );
                return Ok(Committed {
                    commit,
                    branch,
                    more_files: changes.len().saturating_sub(MAX_LISTED),
                    files: changes
                        .into_iter()
                        .take(MAX_LISTED)
                        .map(|change| Taken {
                            path: change.path,
                            added: change.added,
                            removed: change.removed,
                        })
                        .collect(),
                    more_left: left.len().saturating_sub(MAX_LEFT),
                    left: left.into_iter().take(MAX_LEFT).collect(),
                    note,
                    warning,
                });
            }
            Err(error) if crate::git::head(&diff.top) == diff.base => return Err(error.into()),
            Err(_) => {}
        }
    }
    Err(Refusal::Other(
        "the branch kept moving while committing; call this again".into(),
    ))
}

/// Runs each change through the `format` of the land checks its path matches, giving back what
/// each change held before when that changed it. A format that fails refuses the commit. Like
/// a task, it runs unasked for an agent the person let run anything.
fn format(
    core: &Core,
    agent_id: &str,
    top: &Path,
    changes: &mut [Change],
) -> Result<Vec<Option<Vec<u8>>>, Refusal> {
    let mut before = vec![None; changes.len()];
    let formats = |config: &config::ProjectConfig| {
        (config.land_checks.iter()).any(|check| check.format.is_some())
    };
    let ConfigLoad::Valid { config, .. } = config::load(top) else {
        return Ok(before);
    };
    let bypass = (core.workspaces().run_of(agent_id))
        .is_some_and(|run| run.permission_mode == PermissionMode::Bypass);
    let trusted = || bypass || super::generate::trust_needed_for(core, top, top, formats).is_none();
    if !formats(&config) || !trusted() {
        return Ok(before);
    }
    for (change, before) in changes.iter_mut().zip(&mut before) {
        if !change.mode.starts_with("100") {
            continue;
        }
        for check in &config.land_checks {
            let (Some(format), Some(content)) = (&check.format, &change.content) else {
                continue;
            };
            if !check.when.set.is_match(&change.path) {
                continue;
            }
            let args = [change.path.clone()];
            let limit = super::queue::LAND_CHECK_LIMIT;
            let written =
                super::queue::shell(top, format, &check.label, &args, Some(content), limit)
                    .map_err(|why| {
                        Refusal::Other(format!(
                            "Nothing was committed: {} could not format {}:\n\n{}",
                            check.label,
                            change.path,
                            crate::git::tail(&why)
                        ))
                    })?
                    .into_bytes();
            if written != *content {
                before.get_or_insert_with(|| content.clone());
                change.content = Some(written);
            }
        }
    }
    Ok(before)
}

/// Writes the formatted content of each change back to its file, when the file still holds just
/// what was committed before formatting.
fn tidy(
    core: &Core,
    top: &Path,
    agent_id: &str,
    changes: &[Change],
    unformatted: &[Option<Vec<u8>>],
) {
    for (change, before) in changes.iter().zip(unformatted) {
        let (Some(before), Some(content)) = (before, &change.content) else {
            continue;
        };
        let file = top.join(&change.path);
        if std::fs::read(&file).is_ok_and(|now| now == *before)
            && std::fs::write(&file, content).is_ok()
        {
            if let (Ok(before_text), Ok(after_text)) =
                (std::str::from_utf8(before), std::str::from_utf8(content))
            {
                core.edits()
                    .record_formatted(core, top, agent_id, &file, before_text, after_text);
            }
        }
    }
}

fn nothing_yours(left: &[Left]) -> String {
    let mut text = "None of the uncommitted lines here are yours".to_owned();
    if !left.is_empty() {
        let files: Vec<String> = left
            .iter()
            .take(MAX_LEFT)
            .map(|left| format!("{} ({})", left.path, said(&left.by)))
            .collect();
        text.push_str(&format!(
            "; changed by others or by no agent Sikemux knows of: {}. Name paths to commit files your own commands made, which no agent's lines are in",
            files.join(", ")
        ));
    }
    text
}

/// What the lines `changes` add name that the commit made of them lacks.
fn unresolved(diff: &Diff, changes: &[Change]) -> Result<Vec<String>, String> {
    let looked_at: Vec<&Change> = changes
        .iter()
        .filter(|change| change.content.is_some() && named::looks_at(&change.path))
        .collect();
    if looked_at.is_empty() {
        return Ok(Vec::new());
    }
    let mut tree: HashSet<String> = match &diff.base {
        Some(base) => git(&diff.top, &["ls-tree", "-r", "-z", "--name-only", base])?
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect(),
        None => HashSet::new(),
    };
    for change in changes {
        match change.content {
            Some(_) => tree.insert(change.path.clone()),
            None => tree.remove(&change.path),
        };
    }
    let mut lacking = Vec::new();
    for change in looked_at {
        let old = match &change.before {
            Some(blob) => run(&diff.top, &["cat-file", "blob", blob], None, None)?,
            None => Vec::new(),
        };
        let new = change.content.as_deref().unwrap_or_default();
        lacking.extend(named::missing(&change.path, &old, new, &tree, |file| {
            git(&diff.top, &["check-ignore", "-q", "--no-index", file]).is_ok()
        }));
    }
    Ok(lacking)
}

/// `path`, given from `dir` or whole, from the top of the checkout.
pub fn in_checkout(top: &Path, dir: &Path, path: &str) -> Result<String, String> {
    let mut clean = PathBuf::new();
    for part in dir.join(path).components() {
        match part {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            part => clean.push(part),
        }
    }
    let real = sikemux_process::files::real_path(&clean).unwrap_or(clean);
    real.strip_prefix(top)
        .map(|inside| inside.to_string_lossy().into_owned())
        .map_err(|_| format!("{path} is outside this checkout"))
}

pub fn names(paths: &[String], file: &str) -> bool {
    paths.iter().any(|path| {
        path.is_empty()
            || file == path
            || file
                .strip_prefix(path.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// What the commit holds of each changed file, what it leaves out, and the
/// `named` paths with nothing uncommitted, for the committing agent whose
/// authors are `mine`: with `named` paths, only changes in them. A lockfile
/// goes with a manifest beside or below it that the commit changes.
fn plan(
    owners: &Owners,
    diff: &Diff,
    mine: HashSet<usize>,
    named: &[String],
    merging: &[String],
) -> Result<Plan, String> {
    let whole: Vec<String> = named.iter().chain(merging).cloned().collect();
    let mut paths: BTreeSet<&str> = diff
        .changed
        .keys()
        .chain(&diff.untracked)
        .map(String::as_str)
        .collect();
    let deleted = match &diff.base {
        Some(base) if !named.is_empty() => {
            let specs: Vec<String> = named.iter().map(|path| literal(path)).collect();
            let mut args = vec!["diff", "-z", "--name-only", "--no-renames"];
            args.extend(["--diff-filter=D", base, "--"]);
            args.extend(specs.iter().map(String::as_str));
            git(&diff.top, &args)?
        }
        _ => String::new(),
    };
    paths.extend(deleted.split('\0').filter(|path| !path.is_empty()));
    if !named.is_empty() {
        paths.retain(|path| names(&whole, path) || is_lockfile(path));
    }
    let unchanged: Vec<String> = named
        .iter()
        .filter(|path| {
            !paths
                .iter()
                .any(|changed| names(std::slice::from_ref(path), changed))
        })
        .cloned()
        .collect();
    let planner = Planner {
        owners,
        diff,
        mine,
        by_path: owners
            .files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect(),
        before: tree_entries(diff, paths.iter().copied())?,
    };
    let (locks, rest): (Vec<&str>, Vec<&str>) = paths.iter().partition(|path| is_lockfile(path));
    let mut changes = Vec::new();
    let mut left = Vec::new();
    for path in rest {
        planner.take(path, names(&whole, path), &mut changes, &mut left)?;
    }
    for lock in locks {
        let follows = changes.iter().any(|change| locks_for(lock, &change.path));
        if named.is_empty() || follows || names(&whole, lock) {
            planner.take(
                lock,
                follows || names(&whole, lock),
                &mut changes,
                &mut left,
            )?;
        }
    }
    let untaken: Vec<&Left> = whole
        .iter()
        .filter(|path| paths.contains(path.as_str()))
        .filter(|path| !changes.iter().any(|change| change.path == **path))
        .filter_map(|path| left.iter().find(|left| left.path == *path))
        .collect();
    if !untaken.is_empty() {
        let held: Vec<String> = (untaken.iter())
            .map(|left| format!("{} holds only lines {} wrote", left.path, said(&left.by)))
            .collect();
        let open: BTreeSet<&String> = untaken.iter().flat_map(|left| &left.open).collect();
        let open: Vec<String> = open.into_iter().cloned().collect();
        return Err(format!(
            "Nothing was committed: {}. Ask {} with agent_send, or leave those paths out.",
            held.join("; "),
            said(&open)
        ));
    }
    Ok(Plan {
        changes,
        left,
        unchanged,
    })
}

struct Plan {
    changes: Vec<Change>,
    left: Vec<Left>,
    unchanged: Vec<String>,
}

/// Names as one would say them: "Otter", "Otter and Briar".
fn said(names: &[String]) -> String {
    match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

const LOCKFILES: [(&str, &[&str]); 7] = [
    ("Cargo.lock", &["Cargo.toml"]),
    ("pnpm-lock.yaml", &["package.json", "pnpm-workspace.yaml"]),
    ("package-lock.json", &["package.json"]),
    ("yarn.lock", &["package.json"]),
    ("bun.lock", &["package.json"]),
    ("uv.lock", &["pyproject.toml"]),
    ("go.sum", &["go.mod"]),
];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn is_lockfile(path: &str) -> bool {
    LOCKFILES.iter().any(|(lock, _)| file_name(path) == *lock)
}

/// Whether the lockfile at `lock` records what the manifest at `manifest` asks for.
fn locks_for(lock: &str, manifest: &str) -> bool {
    let at = folder(lock);
    let below = at.is_empty() || manifest == at || manifest.starts_with(&format!("{at}/"));
    below
        && LOCKFILES.iter().any(|(name, manifests)| {
            file_name(lock) == *name && manifests.contains(&file_name(manifest))
        })
}

/// What a commit of one agent's lines is planned from.
struct Planner<'a> {
    owners: &'a Owners,
    diff: &'a Diff,
    /// The authors that stand for the committing agent, with agents at work
    /// together that include it.
    mine: HashSet<usize>,
    by_path: HashMap<&'a str, &'a FileOwners>,
    before: HashMap<String, (String, String)>,
}

impl Planner<'_> {
    /// Whether `author` is another agent still open, for certain, not agents
    /// at work together that may include the committing one.
    fn theirs(&self, author: usize) -> bool {
        !self.mine.contains(&author)
            && (self.owners.authors.get(author))
                .is_some_and(|writer| writer.shared.is_empty() && writer.open)
    }

    fn name(&self, author: Option<usize>) -> String {
        (author.and_then(|at| self.owners.authors.get(at)))
            .map_or_else(|| "no agent".to_owned(), |writer| writer.name.clone())
    }

    /// Whether `author` is the committing agent, or agents at work together
    /// that include it when the commit names the file and so claims them.
    fn is_mine(&self, author: usize, claimed: bool) -> bool {
        self.mine.contains(&author)
            && (claimed
                || (self.owners.authors.get(author)).is_some_and(|writer| writer.shared.is_empty()))
    }

    /// Plans what the commit takes of `path`: all of it when `may_whole` and
    /// no other agent wrote there for certain, else the committing agent's
    /// lines, with those others may have written only when `may_whole`.
    fn take(
        &self,
        path: &str,
        may_whole: bool,
        changes: &mut Vec<Change>,
        left: &mut Vec<Left>,
    ) -> Result<(), String> {
        let top = &self.diff.top;
        let file = self.by_path.get(path);
        let owner_of: HashMap<u64, usize> = file
            .into_iter()
            .flat_map(|file| &file.lines)
            .flat_map(|run| (run.start..run.start + run.count).map(|line| (line, run.author)))
            .collect();
        let removed_by: HashMap<u64, usize> = file
            .into_iter()
            .flat_map(|file| &file.removed)
            .map(|removal| (removal.at, removal.author))
            .collect();
        let authors: BTreeSet<usize> = owner_of
            .values()
            .chain(removed_by.values())
            .copied()
            .collect();
        let uncertain_theirs: Vec<usize> = self
            .diff
            .uncertain
            .get(path)
            .into_iter()
            .flatten()
            .copied()
            .filter(|author| self.theirs(*author))
            .collect();
        let uncertain_other = !uncertain_theirs.is_empty();
        let whole =
            may_whole && !uncertain_other && !authors.iter().any(|author| self.theirs(*author));
        let leave = |by: BTreeSet<Option<usize>>, left: &mut Vec<Left>| {
            let open: Vec<String> = (by.iter().flatten())
                .filter(|at| self.theirs(**at))
                .filter_map(|at| self.owners.authors.get(*at))
                .map(|writer| crate::server::names::called(&writer.name, &writer.agent_id))
                .collect();
            let mut by: BTreeSet<String> = by.into_iter().map(|at| self.name(at)).collect();
            if by.is_empty() {
                by.insert(self.name(None));
            }
            left.push(Left {
                path: path.to_owned(),
                by: by.into_iter().collect(),
                open,
            });
        };
        if !whole
            && !authors
                .iter()
                .any(|author| self.is_mine(*author, may_whole))
        {
            leave(
                authors
                    .iter()
                    .chain(&uncertain_theirs)
                    .map(|at| Some(*at))
                    .collect(),
                left,
            );
            return Ok(());
        }
        let entry = self.before.get(path);
        let old = match entry {
            Some((_, blob)) => run(top, &["cat-file", "blob", blob], None, None)?,
            None => Vec::new(),
        };
        let Ok(meta) = std::fs::symlink_metadata(top.join(path)) else {
            if let Some((_, blob)) = entry {
                changes.push(Change {
                    path: path.to_owned(),
                    mode: "0".into(),
                    content: None,
                    before: Some(blob.clone()),
                    added: 0,
                    removed: lines(&old).len() as u64,
                });
            }
            return Ok(());
        };
        if !meta.is_file() {
            return Ok(());
        }
        let new = std::fs::read(top.join(path)).map_err(|error| format!("{path}: {error}"))?;
        let mode = match entry {
            Some((mode, _)) => mode.clone(),
            None if crate::os::is_executable(&meta) => "100755".into(),
            None => "100644".into(),
        };
        let hunks = match self.diff.changed.get(path) {
            Some(hunks) if hunks.is_empty() => {
                if !whole {
                    leave(BTreeSet::new(), left);
                    return Ok(());
                }
                changes.push(Change {
                    path: path.to_owned(),
                    mode,
                    added: lines(&new).len() as u64,
                    removed: lines(&old).len() as u64,
                    content: Some(new),
                    before: entry.map(|(_, blob)| blob.clone()),
                });
                return Ok(());
            }
            Some(hunks) => hunks.clone(),
            None => vec![Hunk {
                start: 1,
                count: lines(&new).len() as u64,
                removed: 0,
            }],
        };
        let owner = |line| owner_of.get(&line).copied();
        let removal = |line| removed_by.get(&line).copied();
        let mine = |author: Option<usize>| {
            whole || author.is_some_and(|author| self.is_mine(author, may_whole))
        };
        let theirs = |author: Option<usize>| {
            !whole
                && match author {
                    Some(at) => self.theirs(at),
                    None => uncertain_other,
                }
        };
        let mut tally = Tally::default();
        let content = splice(&old, &new, &hunks, &mut tally, owner, removal, mine, theirs)
            .ok_or_else(|| format!("{path} changed while it was read; call this again"))?;
        if !tally.left.is_empty() {
            if uncertain_other && tally.left.contains(&None) {
                tally
                    .left
                    .extend(uncertain_theirs.iter().copied().map(Some));
            }
            leave(std::mem::take(&mut tally.left), left);
        }
        if tally.added + tally.removed == 0 {
            return Ok(());
        }
        changes.push(Change {
            path: path.to_owned(),
            mode,
            content: Some(content),
            before: entry.map(|(_, blob)| blob.clone()),
            added: tally.added,
            removed: tally.removed,
        });
        Ok(())
    }
}

#[derive(Default)]
struct Tally {
    added: u64,
    removed: u64,
    left: BTreeSet<Option<usize>>,
}

/// `old` with the parts of `new`'s hunks that are `mine`. A run of changed
/// lines holding some of mine and none certainly `theirs` goes in whole, so a
/// line the record lost track of does not split it. Otherwise a hunk that both
/// removes and adds lines goes in whole or not at all, since its lines cannot
/// be told apart from the ones they replaced.
#[allow(clippy::too_many_arguments)]
fn splice(
    old: &[u8],
    new: &[u8],
    hunks: &[Hunk],
    tally: &mut Tally,
    owner: impl Fn(u64) -> Option<usize>,
    removal: impl Fn(u64) -> Option<usize>,
    mine: impl Fn(Option<usize>) -> bool,
    theirs: impl Fn(Option<usize>) -> bool,
) -> Option<Vec<u8>> {
    let old = lines(old);
    let new = lines(new);
    let mut out = Vec::new();
    let (mut at, mut shift) = (0usize, 0i64);
    for hunk in hunks {
        let start = usize::try_from(hunk.start as i64 - shift - 1).ok()?;
        let end = start + hunk.removed as usize;
        out.extend(old.get(at..start)?.concat());
        let added = hunk.start..hunk.start + hunk.count;
        let some_mine = added.clone().any(|line| mine(owner(line)));
        let none_theirs = !added.clone().any(|line| theirs(owner(line)));
        let picked: Vec<u64> = match (hunk.count, hunk.removed) {
            (0, _) => Vec::new(),
            _ if some_mine && none_theirs => added.clone().collect(),
            (_, 0) => added.clone().filter(|line| mine(owner(*line))).collect(),
            _ => Vec::new(),
        };
        let replaced = match hunk.count {
            0 => mine(removal(hunk.start)),
            _ => hunk.removed > 0 && !picked.is_empty(),
        };
        if replaced {
            tally.removed += hunk.removed;
        } else {
            out.extend(old.get(start..end)?.concat());
            if hunk.count == 0 {
                tally.left.insert(removal(hunk.start));
            }
        }
        for line in added {
            if picked.contains(&line) {
                out.extend_from_slice(new.get(line as usize - 1)?);
                tally.added += 1;
            } else {
                tally.left.insert(owner(line));
            }
        }
        at = end;
        shift += hunk.count as i64 - hunk.removed as i64;
    }
    out.extend(old.get(at..)?.concat());
    Some(out)
}

fn lines(text: &[u8]) -> Vec<&[u8]> {
    text.split_inclusive(|byte| *byte == b'\n').collect()
}

fn literal(path: &str) -> String {
    format!(":(top,literal){path}")
}

/// Each path's mode and blob in the last commit.
fn tree_entries<'a>(
    diff: &Diff,
    paths: impl Iterator<Item = &'a str>,
) -> Result<HashMap<String, (String, String)>, String> {
    let specs: Vec<String> = paths.map(literal).collect();
    let Some(base) = diff.base.as_deref().filter(|_| !specs.is_empty()) else {
        return Ok(HashMap::new());
    };
    let mut args = vec!["ls-tree", "-z", base, "--"];
    args.extend(specs.iter().map(String::as_str));
    let listed = git(&diff.top, &args)?;
    Ok(listed
        .split('\0')
        .filter_map(|entry| {
            let (info, path) = entry.split_once('\t')?;
            let mut info = info.split(' ');
            let (mode, kind, blob) = (info.next()?, info.next()?, info.next()?);
            (kind == "blob").then(|| (path.to_owned(), (mode.to_owned(), blob.to_owned())))
        })
        .collect())
}

/// The commit of `changes` on top of the last one, built in an index of its
/// own with the message `signed` gives for its tree and parents, and the blob
/// each change wrote.
fn build(
    diff: &Diff,
    merged: &[String],
    changes: &[Change],
    signed: impl FnOnce(&Commit) -> String,
) -> Result<(String, String, Vec<Option<String>>), String> {
    let git_dir = git(&diff.top, &["rev-parse", "--absolute-git-dir"])?;
    let index = Scratch(
        Path::new(git_dir.trim()).join(format!("sikemux-commit-{}.index", Uuid::new_v4().simple())),
    );
    let index = Some(index.0.as_path());
    match &diff.base {
        Some(base) => run(&diff.top, &["read-tree", base], index, None)?,
        None => run(&diff.top, &["read-tree", "--empty"], index, None)?,
    };
    let mut blobs = Vec::new();
    for change in changes {
        let blob = match &change.content {
            Some(content) => {
                let args = ["hash-object", "-w", "--path", &change.path, "--stdin"];
                let blob = run(&diff.top, &args, None, Some(content))?;
                Some(String::from_utf8_lossy(&blob).trim().to_owned())
            }
            None => None,
        };
        blobs.push(blob);
    }
    let info = entries(changes.iter().zip(&blobs));
    run(
        &diff.top,
        &["update-index", "-z", "--index-info"],
        index,
        Some(&info),
    )?;
    let tree = git_in(&diff.top, &["write-tree"], index)?;
    let parents: Vec<String> = diff.base.iter().chain(merged).cloned().collect();
    let message = signed(&Commit {
        tree: &tree,
        parents: &parents,
    });
    let mut args = vec!["commit-tree", tree.as_str()];
    for parent in &parents {
        args.extend(["-p", parent.as_str()]);
    }
    args.extend(["-F", "-"]);
    let commit = super::owners::run_committing(&diff.top, &args, message.as_bytes())
        .map_err(|error| no_identity(&diff.top, error))?;
    Ok((
        String::from_utf8_lossy(&commit).trim().to_owned(),
        message,
        blobs,
    ))
}

/// `error`, or what to do about it when git has no name and email to commit with.
fn no_identity(top: &Path, error: String) -> String {
    if !error.contains("identity unknown") {
        return error;
    }
    format!(
        "Nothing was committed: git has no name and email to commit with in {}. Ask the person which to use, set them with git config user.name and user.email, then commit again.",
        top.display()
    )
}

fn git_in(top: &Path, args: &[&str], index: Option<&Path>) -> Result<String, String> {
    run(top, args, index, None).map(|out| String::from_utf8_lossy(&out).trim().to_owned())
}

/// `update-index --index-info` lines setting each path to its blob, or
/// removing it.
fn entries<'a>(set: impl Iterator<Item = (&'a Change, &'a Option<String>)>) -> Vec<u8> {
    let mut info = String::new();
    for (change, blob) in set {
        match blob {
            Some(blob) => info.push_str(&format!("{} {blob}\t{}\0", change.mode, change.path)),
            None => info.push_str(&format!("0 {}\t{}\0", "0".repeat(40), change.path)),
        }
    }
    info.into_bytes()
}

/// A merge, cherry-pick or revert git stopped partway through in a checkout, which the next
/// commit finishes as `git commit` would.
struct Underway {
    git_dir: PathBuf,
    /// The commits a merge brings in, which become parents beside HEAD.
    merged: Vec<String>,
    /// MERGE_MSG without its comment lines.
    message: Option<String>,
}

impl Underway {
    fn of(dir: &Path) -> Result<Self, String> {
        let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"])?.trim());
        let merged = std::fs::read_to_string(git_dir.join("MERGE_HEAD"))
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let message = std::fs::read_to_string(git_dir.join("MERGE_MSG"))
            .ok()
            .map(|text| {
                let kept: Vec<&str> = text.lines().filter(|line| !line.starts_with('#')).collect();
                kept.join("\n").trim().to_owned()
            })
            .filter(|message| !message.is_empty());
        Ok(Self {
            git_dir,
            merged,
            message,
        })
    }

    /// The paths a merge changed, staged or still in conflict, which its commit takes whole.
    fn paths(&self, diff: &Diff) -> Result<Vec<String>, String> {
        let Some(base) = diff.base.as_deref().filter(|_| !self.merged.is_empty()) else {
            return Ok(Vec::new());
        };
        let listed = git(
            &diff.top,
            &[
                "diff-index",
                "--cached",
                "-z",
                "--name-only",
                "--no-renames",
                base,
            ],
        )?;
        Ok(listed
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Forgets the merge, cherry-pick or revert, leaving the steps of a longer sequence for
    /// `git cherry-pick --continue`.
    fn finish(&self) {
        for file in [
            "MERGE_HEAD",
            "MERGE_MSG",
            "MERGE_MODE",
            "AUTO_MERGE",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
        ] {
            let _ = std::fs::remove_file(self.git_dir.join(file));
        }
    }
}

/// A private index file, gone once the commit is built.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Brings the committed changes into the shared index, so what others had
/// staged stays staged and nothing shows as undoing the commit. A merge's
/// commit settles the paths still in conflict there.
fn share(
    top: &Path,
    changes: &[Change],
    blobs: &[Option<String>],
    resolved: bool,
) -> Result<(), String> {
    let specs: Vec<String> = changes.iter().map(|change| literal(&change.path)).collect();
    let mut args = vec!["ls-files", "-s", "-z", "--"];
    args.extend(specs.iter().map(String::as_str));
    let listed = git(top, &args)?;
    let mut staged: HashMap<&str, Option<&str>> = HashMap::new();
    for entry in listed.split('\0') {
        let Some((info, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut info = info.split(' ').skip(1);
        let (blob, stage) = (info.next(), info.next());
        let unmerged = stage != Some("0") || staged.contains_key(path);
        staged.insert(path, blob.filter(|_| !unmerged));
    }
    let mut kept = Vec::new();
    let mut updated = Vec::new();
    for (change, blob) in changes.iter().zip(blobs) {
        let shared = staged.get(change.path.as_str()).copied();
        let before = change.before.as_deref();
        let merged = match (shared, before, blob.as_deref()) {
            (Some(Some(shared)), _, Some(blob)) if shared == blob => continue,
            (Some(None), _, blob) if resolved => Some(blob.map(str::to_owned)),
            (None, None, Some(blob)) => Some(Some(blob.to_owned())),
            (Some(Some(shared)), Some(before), _) if shared == before => Some(blob.clone()),
            (Some(Some(shared)), before, Some(blob)) => merge(top, shared, before, blob).map(Some),
            _ => None,
        };
        match merged {
            Some(merged) => updated.push((change, merged)),
            None => kept.push(change.path.as_str()),
        }
    }
    if !updated.is_empty() {
        let info = entries(updated.iter().map(|(change, blob)| (*change, blob)));
        let args = ["update-index", "-z", "--index-info"];
        let mut tries = 1;
        while let Err(error) = run(top, &args, None, Some(&info)) {
            if tries == INDEX_LOCKED_TRIES || !error.contains("index.lock") {
                return Err(format!(
                    "The commit went in, but the shared index did not take it, so git status may show it undone: {error}"
                ));
            }
            tries += 1;
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if kept.is_empty() {
        return Ok(());
    }
    Err(format!(
        "What was staged of {} clashes with the commit and was left as it was; check git diff --cached",
        kept.join(", ")
    ))
}

/// What is staged with the commit's change merged in, unless they clash.
fn merge(top: &Path, shared: &str, before: Option<&str>, committed: &str) -> Option<String> {
    let empty;
    let before = match before {
        Some(before) => before,
        None => {
            let blob = run(top, &["hash-object", "-w", "--stdin"], None, Some(b"")).ok()?;
            empty = String::from_utf8_lossy(&blob).trim().to_owned();
            empty.as_str()
        }
    };
    let args = ["merge-file", "--object-id", shared, before, committed];
    git_in(top, &args, None).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_others_may_have_written_does_not_split_an_agents_change() {
        let old = b".a {\n  color: red;\n}\n";
        let new = b".a {\n  color: red;\n}\n.b {\n  color: blue;\n}\n";
        let hunks = [Hunk {
            start: 4,
            count: 3,
            removed: 0,
        }];
        let (me, unsure) = (0, 1);
        let owner = |line| Some(if line == 6 { unsure } else { me });
        let mut tally = Tally::default();
        let content = splice(
            old,
            new,
            &hunks,
            &mut tally,
            owner,
            |_| None,
            |author| author == Some(me),
            |_| false,
        );
        assert_eq!(content.as_deref(), Some(&new[..]));
        assert!(tally.left.is_empty());
    }

    /// 13 `git_commit` calls from ~/proj/swishx, whose repositories lie one level down, failed with
    /// ".../lib/ads/ads.types.ts is not in a git checkout".
    fn author(agent_id: &str, shared: &[&str]) -> super::super::owners::Author {
        super::super::owners::Author {
            agent_id: agent_id.to_owned(),
            name: agent_id.to_owned(),
            cli: "claude".to_owned(),
            model: None,
            feed: String::new(),
            shared: shared.iter().map(|id| (*id).to_owned()).collect(),
            open: true,
        }
    }

    fn appended(path: &str, author: usize) -> FileOwners {
        FileOwners {
            path: path.to_owned(),
            lines: vec![super::super::owners::Run {
                start: 2,
                count: 1,
                author,
                over: None,
            }],
            removed: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn named_paths_limit_the_commit_and_take_the_lockfile_along() {
        let top = tempfile::tempdir().unwrap();
        let top = sikemux_process::files::real_path(top.path()).unwrap();
        let files = ["Cargo.toml", "Cargo.lock", "a.rs", "b.rs"];
        for file in files {
            std::fs::write(top.join(file), "x\n").unwrap();
        }
        let git = |args: &[&str]| run(&top, args, None, None).unwrap();
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "base",
        ]);
        for file in files {
            std::fs::write(top.join(file), "x\ny\n").unwrap();
        }
        let owners = Owners {
            authors: vec![author("me", &[]), author("", &["birch", "quartz"])],
            files: vec![
                appended("Cargo.lock", 1),
                appended("Cargo.toml", 0),
                appended("a.rs", 0),
                appended("b.rs", 0),
            ],
        };
        let diff = super::super::owners::read_diff(&top, false, false, None, false).unwrap();
        let mine = HashSet::from([0]);
        let changes = plan(&owners, &diff, mine, &["Cargo.toml".to_owned()], &[])
            .unwrap()
            .changes;
        let taken: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
        assert_eq!(taken, ["Cargo.toml", "Cargo.lock"]);
    }

    #[test]
    fn lines_others_may_have_written_go_in_only_from_a_named_file() {
        let top = tempfile::tempdir().unwrap();
        let top = sikemux_process::files::real_path(top.path()).unwrap();
        let files = ["seen.rs", "protocol.rs"];
        for file in files {
            std::fs::write(top.join(file), "x\n").unwrap();
        }
        let git = |args: &[&str]| run(&top, args, None, None).unwrap();
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "base",
        ]);
        for file in files {
            std::fs::write(top.join(file), "x\ny\n").unwrap();
        }
        let owners = Owners {
            authors: vec![author("towhee", &[]), author("", &["towhee", "dolphin"])],
            files: vec![appended("protocol.rs", 1), appended("seen.rs", 0)],
        };
        let diff = super::super::owners::read_diff(&top, false, false, None, false).unwrap();
        let taken = |named: &[&str]| {
            let named: Vec<String> = named.iter().map(|path| (*path).to_owned()).collect();
            let changes = plan(&owners, &diff, HashSet::from([0, 1]), &named, &[])
                .unwrap()
                .changes;
            changes
                .into_iter()
                .map(|change| change.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(taken(&[]), ["seen.rs"]);
        assert_eq!(taken(&["protocol.rs"]), ["protocol.rs"]);
    }

    /// @Xiaolan merged origin/main into her branch, settled the clash and committed with
    /// git_commit, which gave the commit HEAD alone as parent: the PR listed 119 of main's files.
    #[test]
    fn committing_a_merge_keeps_what_it_merged_as_a_parent() {
        use super::super::tests_git::git_in;
        let top = tempfile::tempdir().unwrap();
        let top = sikemux_process::files::real_path(top.path()).unwrap();
        git_in(&top, &["init", "-q", "-b", "main"]);
        git_in(&top, &["config", "user.name", "t"]);
        git_in(&top, &["config", "user.email", "t@t"]);
        std::fs::write(top.join("a.txt"), "one\n").unwrap();
        git_in(&top, &["add", "."]);
        git_in(&top, &["commit", "-qm", "base"]);
        git_in(&top, &["checkout", "-qb", "feature"]);
        std::fs::write(top.join("a.txt"), "one, the feature's\n").unwrap();
        git_in(&top, &["commit", "-qam", "feature"]);
        git_in(&top, &["checkout", "-q", "main"]);
        std::fs::write(top.join("a.txt"), "one, main's\n").unwrap();
        std::fs::write(top.join("b.txt"), "only on main\n").unwrap();
        git_in(&top, &["add", "."]);
        git_in(&top, &["commit", "-qm", "main"]);
        let main = git_in(&top, &["rev-parse", "main"]);
        git_in(&top, &["checkout", "-q", "feature"]);
        assert!(run(&top, &["merge", "main"], None, None).is_err());
        std::fs::write(top.join("a.txt"), "one, both\n").unwrap();

        let diff = super::super::owners::read_diff(&top, false, false, None, false).unwrap();
        let underway = Underway::of(&top).unwrap();
        assert_eq!(
            underway.message.as_deref(),
            Some("Merge branch 'main' into feature")
        );
        let merging = underway.paths(&diff).unwrap();
        assert_eq!(merging, ["a.txt", "b.txt"]);
        let nobody = Owners {
            authors: Vec::new(),
            files: Vec::new(),
        };
        let changes = plan(&nobody, &diff, HashSet::new(), &[], &merging)
            .unwrap()
            .changes;
        let message = underway.message.clone().unwrap();
        let (commit, _, blobs) = build(&diff, &underway.merged, &changes, |_| message).unwrap();
        let head = diff.base.clone().unwrap();
        git_in(&top, &["update-ref", "refs/heads/feature", &commit, &head]);
        underway.finish();
        share(&top, &changes, &blobs, true).unwrap();

        assert_eq!(
            git_in(&top, &["rev-list", "--parents", "-1", "HEAD"]),
            format!("{commit} {head} {main}")
        );
        assert_eq!(git_in(&top, &["status", "--porcelain"]), "");
        assert!(!top.join(".git/MERGE_HEAD").exists());
        assert_eq!(
            git_in(&top, &["diff", "--name-only", "main", "HEAD"]),
            "a.txt"
        );
    }
}
