//! Commits the lines one agent wrote and nothing else. The commit is built in
//! an index of its own from the last commit plus those lines, and the branch
//! moves only if no one moved it meanwhile. The checkout's files stay as they
//! are, and the shared index takes in only what was committed.

use crate::server::parts::Parts;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
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
use crate::trailers::Commit;

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
pub(crate) struct Committed {
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
    /// Why its commits were not pushed: a push waits on the person.
    #[serde(skip_serializing_if = "Option::is_none")]
    not_pushed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    still_pushing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    synced: Option<super::land::Synced>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued: Option<super::queue::Queued>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pushed: Option<super::push::Pushed>,
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
pub(crate) enum Refusal {
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
pub(crate) async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = crate::server::caller::agent_of(request, "git_commit")?;
    let flag = |name: &str| request.params.get(name).and_then(Value::as_bool) == Some(true);
    let (sync, land, push) = (flag("sync"), flag("land"), flag("push"));
    let message = match request.params.get("message") {
        Some(Value::String(message)) => Some(message.clone()),
        None | Some(Value::Null) if sync || land || push => None,
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
        let (committed, nothing_committed) = match &message {
            Some(message) => match commit(&core, &dir, &agent_id, message, &named) {
                Ok(committed) => (Some(committed), None),
                Err(Refusal::NothingYours(why)) if sync || land || push => (None, Some(why)),
                Err(refusal) => return Err(String::from(refusal).into()),
            },
            None => (None, None),
        };
        let synced = match sync {
            true => match super::land::sync(&core, &agent_id) {
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
        let own =
            super::land::own_checkout(&core, &agent_id).filter(|tree| dir.starts_with(&tree.path));
        let ship = || crate::ship::Ship {
            kind: sikemux_view::inbox::ShipKind::Push,
            what: "git_commit push".to_owned(),
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
                match super::queue::enqueue(&core, &agent_id, push_after) {
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
                let (from, unlanded) = match own {
                    Some(tree) => (tree.repo.clone(), super::land::unlanded(&tree)?),
                    None => (dir.clone(), None),
                };
                not_pushed = unlanded
                    .or_else(|| super::super::ship::check(&core, &agent_id, ship(), &from).err());
                match not_pushed {
                    None => push_or_tell(&core, from, &agent_id)?,
                    Some(_) => None,
                }
            }
            false => None,
        };
        let still_pushing = (push && !land && not_pushed.is_none() && pushed.is_none()).then(|| {
            "The push is taking a while, so it goes on without you; a message says how it went once it ends".to_owned()
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
/// when the push is still going, and the agent then hears how it went.
fn push_or_tell(
    core: &Arc<Core>,
    from: PathBuf,
    agent: &str,
) -> Result<Option<super::push::Pushed>, String> {
    let (answer, answered) = std::sync::mpsc::channel();
    let runtime = tokio::runtime::Handle::try_current().ok();
    let (owner, agent) = (core.clone(), agent.to_owned());
    std::thread::spawn(move || {
        let pushed = super::push::push(&owner, &from, std::slice::from_ref(&agent), "origin");
        let Err(std::sync::mpsc::SendError(pushed)) = answer.send(pushed) else {
            return;
        };
        let text = match pushed {
            Ok(pushed) => format!(
                "Your push ended: {}",
                serde_json::to_string(&pushed).unwrap_or_default()
            ),
            Err(why) => format!("Your push failed: {why}"),
        };
        if let Some(runtime) = runtime {
            runtime.spawn(async move {
                let _ = crate::server::messages::from_sikemux(&owner, agent, text).await;
            });
        }
    });
    match answered.recv_timeout(PUSH_ANSWER_WAIT) {
        Ok(pushed) => pushed.map(Some),
        Err(_) => Ok(None),
    }
}

/// The checkout to commit in, the one holding the `named` paths, which may lie
/// outside the project's own, with those paths as they lie there. A relative
/// path is looked for in the agent's own folder, then the project's, then among
/// the files the agent changed; from a folder of repositories, one of them holds
/// it. With no paths, the agent's own checkout, else the project's, else the one
/// holding what the agent changed and has not committed.
fn checkout_for(
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
pub(crate) fn repo_below(folder: &Path, path: &Path) -> Result<Option<PathBuf>, String> {
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
pub(crate) fn commit(
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
    if super::footer::credits_claude(message) {
        return Err(Refusal::Other(
            "Nothing was committed: leave out the line crediting Claude, such as a Co-Authored-By trailer or \"Generated with Claude Code\". Sikemux's footer credits the agent already.".into(),
        ));
    }
    for _ in 0..ATTEMPTS {
        let branch = git(dir, &["symbolic-ref", "-q", "HEAD"])
            .map_or_else(|_| "HEAD".to_owned(), |branch| branch.trim().to_owned());
        let (owners, diff) = traced(core, dir, false)?;
        let named = named
            .iter()
            .map(|path| in_checkout(&diff.top, dir, path))
            .collect::<Result<Vec<_>, _>>()?;
        let mine = super::owners::standing_for(&owners, core, agent_id);
        let Plan {
            changes,
            left,
            unchanged,
        } = plan(&owners, &diff, mine, &named)?;
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
        let lacking = unresolved(&diff, &changes)?;
        if !lacking.is_empty() {
            return Err(Refusal::Other(format!(
                "Nothing was committed, since the commit would not build on its own: {}. Name the missing files in paths too, or wait until the agent writing them commits.",
                lacking.join("; ")
            )));
        }
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
        let (commit, message, blobs) = build(&diff, &changes, signed)?;
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
                let note = share(&diff.top, &changes, &blobs).err();
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
        lacking.extend(named::missing(&change.path, &old, new, &tree));
    }
    Ok(lacking)
}

/// `path`, given from `dir` or whole, from the top of the checkout.
pub(super) fn in_checkout(top: &Path, dir: &Path, path: &str) -> Result<String, String> {
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
    let real = std::fs::canonicalize(&clean).unwrap_or(clean);
    real.strip_prefix(top)
        .map(|inside| inside.to_string_lossy().into_owned())
        .map_err(|_| format!("{path} is outside this checkout"))
}

pub(super) fn names(paths: &[String], file: &str) -> bool {
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
) -> Result<Plan, String> {
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
        paths.retain(|path| names(named, path) || is_lockfile(path));
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
        planner.take(path, names(named, path), &mut changes, &mut left)?;
    }
    for lock in locks {
        let follows = changes.iter().any(|change| locks_for(lock, &change.path));
        if named.is_empty() || follows || names(named, lock) {
            planner.take(lock, follows || names(named, lock), &mut changes, &mut left)?;
        }
    }
    let untaken: Vec<&Left> = named
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
        let writer = &self.owners.authors[author];
        !self.mine.contains(&author) && writer.shared.is_empty() && writer.open
    }

    fn name(&self, author: Option<usize>) -> String {
        author.map_or_else(
            || "no agent".to_owned(),
            |at| self.owners.authors[at].name.clone(),
        )
    }

    /// Whether `author` is the committing agent, or agents at work together
    /// that include it when the commit names the file and so claims them.
    fn is_mine(&self, author: usize, claimed: bool) -> bool {
        self.mine.contains(&author) && (claimed || self.owners.authors[author].shared.is_empty())
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
        let whole = may_whole && !authors.iter().any(|author| self.theirs(*author));
        let leave = |by: BTreeSet<Option<usize>>, left: &mut Vec<Left>| {
            let open: Vec<String> = (by.iter().flatten())
                .filter(|at| self.theirs(**at))
                .map(|at| {
                    let writer = &self.owners.authors[*at];
                    crate::server::messages::called(&writer.name, &writer.agent_id)
                })
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
            leave(authors.iter().map(|at| Some(*at)).collect(), left);
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
            None if meta.permissions().mode() & 0o111 != 0 => "100755".into(),
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
        let theirs = |author: Option<usize>| !whole && author.is_some_and(|at| self.theirs(at));
        let mut tally = Tally::default();
        let content = splice(&old, &new, &hunks, &mut tally, owner, removal, mine, theirs)
            .ok_or_else(|| format!("{path} changed while it was read; call this again"))?;
        if !tally.left.is_empty() {
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
    let parents = Vec::from_iter(diff.base.clone());
    let message = signed(&Commit {
        tree: &tree,
        parents: &parents,
    });
    let mut args = vec!["commit-tree", tree.as_str()];
    for parent in &parents {
        args.extend(["-p", parent.as_str()]);
    }
    args.extend(["-F", "-"]);
    let commit = run(&diff.top, &args, None, Some(message.as_bytes()))
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

/// A private index file, gone once the commit is built.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Brings the committed changes into the shared index, so what others had
/// staged stays staged and nothing shows as undoing the commit.
fn share(top: &Path, changes: &[Change], blobs: &[Option<String>]) -> Result<(), String> {
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
    use crate::server::edits::tests_git::git_in;
    use crate::server::ServerCore;

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
    #[test]
    fn a_path_in_a_folder_of_repositories_is_found_one_level_down() {
        let folder = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(folder.path()).unwrap();
        for repo in ["api", "web"] {
            let repo = folder.join(repo);
            std::fs::create_dir_all(repo.join("lib")).unwrap();
            git_in(&repo, &["init", "-q"]);
            std::fs::write(repo.join("shared.ts"), "").unwrap();
        }
        std::fs::write(folder.join("api/lib/only.ts"), "").unwrap();
        let named = |path: &str| vec![folder.join(path).to_string_lossy().into_owned()];
        let core = Core::new(Default::default(), None).unwrap();
        let checkout_for = |named: &[String]| checkout_for(&core, "agent", &folder, named);

        let (dir, paths) = checkout_for(&named("lib/only.ts")).unwrap();
        assert_eq!(dir, folder.join("api"));
        assert_eq!(paths, named("api/lib/only.ts"));

        let several = checkout_for(&named("shared.ts")).unwrap_err();
        assert!(
            several.contains("api") && several.contains("web"),
            "{several}"
        );

        let nowhere = checkout_for(&named("missing.ts")).unwrap_err();
        assert!(nowhere.contains("is not in a git checkout"), "{nowhere}");
    }

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
        let top = std::fs::canonicalize(top.path()).unwrap();
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
        let changes = plan(&owners, &diff, mine, &["Cargo.toml".to_owned()])
            .unwrap()
            .changes;
        let taken: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
        assert_eq!(taken, ["Cargo.toml", "Cargo.lock"]);
    }

    #[test]
    fn lines_others_may_have_written_go_in_only_from_a_named_file() {
        let top = tempfile::tempdir().unwrap();
        let top = std::fs::canonicalize(top.path()).unwrap();
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
            let changes = plan(&owners, &diff, HashSet::from([0, 1]), &named)
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
}
