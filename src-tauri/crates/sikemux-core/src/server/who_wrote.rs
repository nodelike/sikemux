//! Who wrote a commit, some lines of a file, or a test, and how to reach them:
//! the agent sessions a commit's signed footer names, else the person git
//! names as its author and the pull request it came in through.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::cli::protocol::HarnessRequest;
use crate::trailers::{self, Agent};

use super::board::codehost;
use super::connection::blocking;
use super::edits::note::{self, Note, Wrote};
use super::edits::{commit_lines, CommitLines};
use super::edits::{footer, git};
use super::messages::{self, Listed, Reach};
use super::{saved, Core, CoreResult};

mod ci;

/// Commits whose footers are remembered, and sessions whose titles are.
const MAX_REMEMBERED: usize = 4096;
/// Commits the app may ask about at once.
const MAX_ASKED: usize = 200;
/// Commits one answer names.
const MAX_COMMITS: usize = 8;
const LATEST_PER_FILE: usize = 3;
const MAX_TEST_FILES: usize = 3;
const MAX_BLAMED_LINES: u64 = 5000;
const PULL_BUDGET: Duration = Duration::from_secs(5);
const UNCOMMITTED: &str = "0000000000000000000000000000000000000000";

/// An agent a commit's footer names, as it stands here now.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentOwner {
    pub name: String,
    pub cli: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The version of Sikemux the agent ran in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sikemux: Option<String>,
    /// The agent the footer names, which stands for the session until it is known.
    pub chat: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The agent that has its session open now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    /// The CLI's own session, which resuming the agent needs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The project its session ran in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The lines it wrote in the commit, as the commit's note says, when the
    /// commit holds lines of others too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Wrote>,
}

fn remember<T: Clone>(map: &Mutex<HashMap<String, T>>, key: &str, value: &T) {
    let mut map = map.locked();
    if map.len() >= MAX_REMEMBERED {
        map.clear();
    }
    map.insert(key.to_owned(), value.clone());
}

/// The agents `commit`'s footer names that check out, read once per commit.
fn verified(core: &Core, dir: &Path, commit: &str) -> Vec<Agent> {
    static VERIFIED: OnceLock<Mutex<HashMap<String, Vec<Agent>>>> = OnceLock::new();
    let cache = VERIFIED.get_or_init(Default::default);
    if let Some(agents) = cache.locked().get(commit) {
        return agents.clone();
    }
    let Ok(agents) = footer::verified(core, dir, commit) else {
        return Vec::new();
    };
    remember(cache, commit, &agents);
    agents
}

type TitleAndProject = (String, String);

/// What a saved session is called and the project it ran in, looked up once
/// per session.
fn saved_session(core: &Core, cli: &str, session_id: &str) -> Option<TitleAndProject> {
    static SAVED: OnceLock<Mutex<HashMap<String, Option<TitleAndProject>>>> = OnceLock::new();
    let cache = SAVED.get_or_init(Default::default);
    let key = format!("{cli}\0{session_id}");
    if let Some(saved) = cache.locked().get(&key) {
        return saved.clone();
    }
    let saved = saved::locate(core, session_id, Some(cli))
        .ok()
        .map(|saved| (saved.title, saved.project.to_string_lossy().into_owned()));
    remember(cache, &key, &saved);
    saved
}

/// Each of `agents` with its session, its title, the agent that has it open
/// now, if any, and the lines the commit's `note` gives it.
fn owners(
    core: &Core,
    agents: Vec<Agent>,
    listed: &[Listed],
    mut note: Option<Note>,
) -> Vec<AgentOwner> {
    agents
        .into_iter()
        .map(|agent| {
            let lines = note.as_mut().and_then(|note| note.remove(agent.key()));
            let session_id = agent
                .session
                .clone()
                .or_else(|| core.names.last_session(&agent.chat));
            let open = listed.iter().filter(|open| open.reach != Reach::Stopped);
            let running = open
                .clone()
                .find(|open| open.agent_id == agent.chat)
                .or_else(|| {
                    let session_id = session_id.as_ref()?;
                    open.clone().find(|other| {
                        core.names.session_of(&other.agent_id).as_ref() == Some(session_id)
                    })
                });
            let saved = session_id
                .as_deref()
                .filter(|_| running.is_none())
                .and_then(|session_id| saved_session(core, &agent.cli, session_id));
            let title = match running {
                Some(running) => running.title.clone(),
                None => saved.as_ref().map(|(title, _)| title.clone()),
            };
            let project = saved
                .map(|(_, project)| project)
                .or_else(|| core.names.ran_session(&agent.chat).map(|ran| ran.project));
            AgentOwner {
                title,
                open: running.map(|running| running.agent_id.clone()),
                session_id,
                project,
                name: agent.name,
                cli: agent.cli,
                cli_version: agent.cli_version,
                model: agent.model,
                sikemux: agent.sikemux,
                chat: agent.chat,
                lines,
            }
        })
        .collect()
}

/// What the notes on `commits` say each one's verified agents wrote, for the
/// commits that have one this host or one linked with it signed. An agent a
/// note names that the footer does not counts for nothing.
fn noted<'a>(
    core: &Core,
    dir: &Path,
    commits: impl Iterator<Item = (&'a str, &'a [Agent])>,
) -> HashMap<String, Note> {
    let commits: HashMap<&str, &[Agent]> =
        commits.filter(|(_, agents)| !agents.is_empty()).collect();
    let hashes: Vec<&str> = commits.keys().copied().collect();
    let mut notes = note::read(dir, &hashes, &core.remote.host_keys());
    notes.retain(|commit, note| {
        let agents = commits.get(commit.as_str()).copied().unwrap_or_default();
        note.retain(|key, _| agents.iter().any(|agent| agent.key() == key));
        !note.is_empty()
    });
    notes
}

#[derive(Deserialize)]
pub(crate) struct CommitsParams {
    dir: PathBuf,
    commits: Vec<String>,
}

/// The `commitOwners` call: the agents each of `commits` has a verified
/// footer for, by commit. Commits without one are left out.
pub(crate) async fn call(
    core: &Arc<Core>,
    params: CommitsParams,
) -> CoreResult<BTreeMap<String, Vec<AgentOwner>>> {
    let core = core.clone();
    blocking(move || {
        let mut found = Vec::new();
        for commit in params.commits.iter().take(MAX_ASKED) {
            if !is_hash(commit) {
                continue;
            }
            let agents = verified(&core, &params.dir, commit);
            if !agents.is_empty() {
                found.push((commit.clone(), agents));
            }
        }
        if found.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut notes = noted(
            &core,
            &params.dir,
            found
                .iter()
                .map(|(commit, agents)| (commit.as_str(), agents.as_slice())),
        );
        let listed = messages::listed(&core);
        Ok(found
            .into_iter()
            .map(|(commit, agents)| {
                let owners = owners(&core, agents, &listed, notes.remove(&commit));
                (commit, owners)
            })
            .collect())
    })
    .await
}

#[derive(Deserialize)]
pub(crate) struct CommitParams {
    dir: PathBuf,
    commit: String,
}

/// The `commitLines` call: which agent wrote each line `commit` changed, as its
/// note says, with the notes to show beside them. A commit whose note gives no
/// lines, because one agent wrote all of it or none did, has none.
pub(crate) async fn lines(core: &Arc<Core>, params: CommitParams) -> CoreResult<CommitLines> {
    let core = core.clone();
    blocking(move || {
        let CommitParams { dir, commit } = params;
        if !is_hash(&commit) {
            return Ok(CommitLines::default());
        }
        let agents = verified(&core, &dir, &commit);
        let Some(mut note) =
            noted(&core, &dir, std::iter::once((commit.as_str(), &agents[..]))).remove(&commit)
        else {
            return Ok(CommitLines::default());
        };
        let wrote: Vec<(String, Wrote)> = agents
            .iter()
            .filter_map(|agent| Some((agent.name.clone(), note.remove(agent.key())?)))
            .collect();
        let wrote: Vec<(&str, &Wrote)> = wrote
            .iter()
            .map(|(name, lines)| (name.as_str(), lines))
            .collect();
        Ok(commit_lines(&dir, &commit, &wrote)?)
    })
    .await
}

fn is_hash(text: &str) -> bool {
    (7..=64).contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Runs of line numbers, first and last.
type Lines = Vec<(u64, u64)>;

/// A line of the file asked about, `at`, as the commit that last changed it
/// had it: line `orig` of `path`.
struct Blamed {
    path: String,
    orig: u64,
    at: u64,
}

/// What the asker named, worked out to commits.
struct Found {
    kind: &'static str,
    /// Each commit with the lines of the file it last changed, when blamed.
    commits: Vec<(String, Vec<Blamed>)>,
    path: Option<String>,
    uncommitted: Lines,
    /// Picked by matching a test's name, which may have found the wrong file.
    guess: Option<String>,
    /// Why a commit was picked, by commit, when it is not plain.
    why: HashMap<String, String>,
    /// How many commits there were to pick from, when more than were picked.
    total: Option<usize>,
    notes: Vec<String>,
}

impl Found {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            commits: Vec::new(),
            path: None,
            uncommitted: Vec::new(),
            guess: None,
            why: HashMap::new(),
            total: None,
            notes: Vec::new(),
        }
    }
}

fn line_range(spec: &str) -> Option<(u64, u64)> {
    let (start, end) = match spec.split_once(['-', ',']) {
        Some((start, end)) => (start.trim().parse().ok()?, end.trim().parse().ok()?),
        None => {
            let line = spec.trim().parse().ok()?;
            (line, line)
        }
    };
    (start >= 1 && end >= start).then_some((start, end.min(start + MAX_BLAMED_LINES - 1)))
}

/// A file of the checkout `top` that `path` names, relative to `top`.
fn file_in(top: &Path, dir: &Path, path: &str) -> Option<String> {
    let path = Path::new(path.trim());
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        dir.join(path)
    };
    let full = std::fs::canonicalize(full).ok()?;
    if !full.is_file() {
        return None;
    }
    Some(full.strip_prefix(top).ok()?.to_string_lossy().into_owned())
}

fn resolve(dir: &Path, top: &Path, target: &str) -> Result<Found, String> {
    if let Some((path, range)) = target
        .rsplit_once(':')
        .and_then(|(path, spec)| Some((path, line_range(spec)?)))
    {
        let file = file_in(top, dir, path)
            .ok_or_else(|| format!("{path} is not a file in this checkout"))?;
        let mut found = blame(top, &file, range)?;
        found.path = Some(file);
        return Ok(found);
    }
    if let Some(path) = file_in(top, dir, target) {
        let mut found = Found::new("file");
        found.commits = latest(top, &path, MAX_COMMITS)?;
        found.path = Some(path);
        return Ok(found);
    }
    if !target.contains(char::is_whitespace) {
        let spec = format!("{target}^{{commit}}");
        if let Ok(hash) = git(
            top,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &spec,
            ],
        ) {
            let mut found = Found::new("commit");
            found.commits.push((hash.trim().to_owned(), Vec::new()));
            return Ok(found);
        }
    }
    test(top, target)
}

/// The commits that last changed lines `start` to `end` of `path`.
fn blame(top: &Path, path: &str, (start, end): (u64, u64)) -> Result<Found, String> {
    let range = format!("{start},{end}");
    let out = git(top, &["blame", "--porcelain", "-L", &range, "--", path])?;
    let mut found = Found::new("lines");
    let mut paths: HashMap<String, String> = HashMap::new();
    let mut current: Option<(String, u64, u64)> = None;
    for line in out.lines() {
        if line.starts_with('\t') {
            let Some((hash, orig, at)) = current.take() else {
                continue;
            };
            if hash == UNCOMMITTED {
                match found.uncommitted.last_mut() {
                    Some((_, last)) if *last + 1 == at => *last = at,
                    _ => found.uncommitted.push((at, at)),
                }
                continue;
            }
            let path = paths.get(&hash).cloned().unwrap_or_else(|| path.to_owned());
            let lines = match found.commits.iter().position(|(seen, _)| *seen == hash) {
                Some(index) => &mut found.commits[index].1,
                None => {
                    found.commits.push((hash, Vec::new()));
                    &mut found.commits.last_mut().expect("just pushed").1
                }
            };
            lines.push(Blamed { path, orig, at });
        } else if let Some(name) = line.strip_prefix("filename ") {
            if let Some((hash, _, _)) = &current {
                paths.insert(hash.clone(), name.to_owned());
            }
        } else {
            let mut words = line.split(' ');
            let (Some(hash), Some(orig), Some(at)) = (words.next(), words.next(), words.next())
            else {
                continue;
            };
            if hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                if let (Ok(orig), Ok(at)) = (orig.parse(), at.parse()) {
                    current = Some((hash.to_owned(), orig, at));
                }
            }
        }
    }
    Ok(found)
}

/// `lines`, in order, as runs.
fn runs(lines: impl Iterator<Item = u64>) -> Lines {
    let mut runs: Lines = Vec::new();
    for at in lines {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == at => *last = at,
            _ => runs.push((at, at)),
        }
    }
    runs
}

fn latest(top: &Path, path: &str, count: usize) -> Result<Vec<(String, Vec<Blamed>)>, String> {
    let count = format!("-n{count}");
    Ok(git(top, &["log", &count, "--format=%H", "--", path])?
        .lines()
        .map(|hash| (hash.to_owned(), Vec::new()))
        .collect())
}

/// The files a test's name or path points at and the latest commits to them.
fn test(top: &Path, target: &str) -> Result<Found, String> {
    let name = ["::", " > ", " › ", "#"]
        .iter()
        .fold(target, |rest, separator| {
            rest.rsplit(separator)
                .find(|part| !part.trim().is_empty())
                .unwrap_or(rest)
        })
        .trim()
        .trim_matches(['"', '\'', '`']);
    if name.is_empty() {
        return Err("name a commit, a file, file:line, or a test".into());
    }
    let out = git(top, &["grep", "-l", "-F", "-e", name]).unwrap_or_default();
    let mut files: Vec<&str> = out.lines().collect();
    files.sort_by_key(|path| {
        let lower = path.to_lowercase();
        !(lower.contains("test") || lower.contains("spec"))
    });
    files.truncate(MAX_TEST_FILES);
    if files.is_empty() {
        return Err(format!(
            "{target} is not a commit or a file here, and no file mentions {name}"
        ));
    }
    let mut found = Found::new("test");
    for path in &files {
        for commit in latest(top, path, LATEST_PER_FILE)? {
            if !found.commits.iter().any(|(seen, _)| *seen == commit.0) {
                found.commits.push(commit);
            }
        }
    }
    found.guess = Some(format!(
        "found by the name {name} in {}; check it is the test you meant",
        files.join(", ")
    ));
    Ok(found)
}

/// A commit as git describes it.
struct Described {
    hash: String,
    short: String,
    author: String,
    email: String,
    date: String,
    subject: String,
    footer: bool,
}

fn describe(top: &Path, hashes: &[&str]) -> Result<Vec<Described>, String> {
    let mut args = vec![
        "log",
        "--no-walk=unsorted",
        "--date=short",
        "--format=%H%x00%h%x00%an%x00%ae%x00%ad%x00%s%x00%B%x1e",
        "--end-of-options",
    ];
    args.extend(hashes);
    Ok(git(top, &args)?
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').splitn(7, '\0');
            let mut next = || fields.next().map(str::to_owned);
            Some(Described {
                hash: next()?,
                short: next()?,
                author: next()?,
                email: next()?,
                date: next()?,
                subject: next()?,
                footer: !trailers::agents(&next()?).is_empty(),
            })
        })
        .collect())
}

fn ranges(lines: &[(u64, u64)]) -> String {
    lines
        .iter()
        .map(|(start, end)| match start == end {
            true => start.to_string(),
            false => format!("{start}-{end}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn agent_value(owner: &AgentOwner) -> Value {
    let (action, how) = match (&owner.open, &owner.session_id) {
        (Some(agent_id), _) => ("message", format!("agent_send to {agent_id}")),
        (None, Some(session_id)) => (
            "resume",
            format!("agent_spawn with cli {} and resume {session_id}", owner.cli),
        ),
        (None, None) => (
            "ask the person",
            "its session is not known here any more".to_owned(),
        ),
    };
    json!({
        "kind": "agent",
        "name": owner.name,
        "cli": owner.cli,
        "model": owner.model,
        "title": owner.title,
        "openNow": owner.open,
        "sessionId": owner.session_id,
        "action": action,
        "how": how,
    })
}

/// The owners of a commit holding lines of several writers, by what its note
/// says each agent wrote: of the `blamed` lines when lines were asked about,
/// else of the commit, in `path` when a file was. The lines no agent wrote are
/// the `person`'s.
fn split(
    owners: &[AgentOwner],
    blamed: &[Blamed],
    path: Option<&str>,
    mut person: Value,
) -> Vec<Value> {
    let wrote = |owner: &AgentOwner, line: &Blamed| {
        owner
            .lines
            .as_ref()
            .is_some_and(|wrote| note::holds(wrote, &line.path, line.orig))
    };
    let mut answered = Vec::new();
    if blamed.is_empty() {
        for owner in owners {
            let lines: BTreeMap<&str, String> = owner
                .lines
                .iter()
                .flatten()
                .filter(|(file, _)| path.is_none_or(|path| path == *file))
                .map(|(file, runs)| {
                    let runs: Lines = runs.iter().map(|[first, last]| (*first, *last)).collect();
                    (file.as_str(), ranges(&runs))
                })
                .collect();
            if !lines.is_empty() {
                let mut agent = agent_value(owner);
                agent["linesInCommit"] = json!(lines);
                answered.push(agent);
            }
        }
        person["linesInCommit"] = "every other line of the commit".into();
        answered.push(person);
        return answered;
    }
    for owner in owners {
        let lines = runs(
            blamed
                .iter()
                .filter(|line| wrote(owner, line))
                .map(|line| line.at),
        );
        if !lines.is_empty() {
            let mut agent = agent_value(owner);
            agent["lines"] = ranges(&lines).into();
            answered.push(agent);
        }
    }
    let rest = runs(
        blamed
            .iter()
            .filter(|line| !owners.iter().any(|owner| wrote(owner, line)))
            .map(|line| line.at),
    );
    if !rest.is_empty() {
        person["lines"] = ranges(&rest).into();
        answered.push(person);
    }
    answered
}

/// What a `who_wrote` target names: commits found here, or a CI run to read
/// from the code host first.
enum Asked {
    Found(Found),
    Ci(ci::Asked),
}

/// What `who_wrote` answers.
pub(crate) async fn tool(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let target = request
        .params
        .get("target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .ok_or("target must be a CI run, a commit, a file, file:line, file:start-end, or a test")?
        .to_owned();
    let dir = PathBuf::from(&request.project);
    let (top, origin, asked) = blocking(move || {
        let top = PathBuf::from(git(&dir, &["rev-parse", "--show-toplevel"])?.trim());
        let origin = git(&top, &["remote", "get-url", "origin"])
            .ok()
            .map(|url| url.trim().to_owned());
        let asked = match ci::asked(&top, &target)? {
            Some(run) => Asked::Ci(run),
            None => Asked::Found(resolve(&dir, &top, &target)?),
        };
        Ok((top, origin, asked))
    })
    .await
    .map_err(|error| error.to_string())?;
    let (found, ci) = match asked {
        Asked::Found(found) => (found, None),
        Asked::Ci(run) => {
            let origin = origin
                .as_deref()
                .ok_or("this project has no origin remote to find its CI runs on")?;
            let picks = codehost::picks(core, Path::new(&request.project));
            let ci = ci::find(origin, &picks, run).await?;
            let top = top.clone();
            blocking(move || Ok((ci::commits(&top, &ci)?, Some(ci))))
                .await
                .map_err(|error| error.to_string())?
        }
    };
    let picks = codehost::picks(core, &top);
    let core = core.clone();
    let (found, mut commits) = blocking(move || {
        let hashes: Vec<&str> = found
            .commits
            .iter()
            .take(MAX_COMMITS)
            .map(|(hash, _)| hash.as_str())
            .collect();
        let described = if hashes.is_empty() {
            Vec::new()
        } else {
            describe(&top, &hashes)?
        };
        let listed = messages::listed(&core);
        let agents: Vec<(Described, Vec<Agent>)> = described
            .into_iter()
            .map(|commit| {
                let agents = match commit.footer {
                    true => verified(&core, &top, &commit.hash),
                    false => Vec::new(),
                };
                (commit, agents)
            })
            .collect();
        let mut notes = noted(
            &core,
            &top,
            agents
                .iter()
                .map(|(commit, agents)| (commit.hash.as_str(), agents.as_slice())),
        );
        let commits: Vec<(Described, Vec<AgentOwner>)> = agents
            .into_iter()
            .map(|(commit, agents)| {
                let owners = owners(&core, agents, &listed, notes.remove(&commit.hash));
                (commit, owners)
            })
            .collect();
        Ok((found, commits))
    })
    .await
    .map_err(|error| error.to_string())?;
    let mut pulls: HashMap<String, Value> = HashMap::new();
    if let Some(origin) = origin {
        let mut asked = tokio::task::JoinSet::new();
        for (commit, _) in commits.iter().filter(|(_, owners)| owners.is_empty()) {
            let (origin, picks, hash) = (origin.clone(), picks.clone(), commit.hash.clone());
            asked.spawn(async move {
                let found = codehost::pulls_of(&origin, &picks, &hash).await.ok()?;
                Some((hash, found.into_iter().next()?))
            });
        }
        let _ = tokio::time::timeout(PULL_BUDGET, async {
            while let Some(answer) = asked.join_next().await {
                if let Ok(Some((hash, pull))) = answer {
                    pulls.insert(hash, pull);
                }
            }
        })
        .await;
    }
    let blamed: HashMap<&str, &[Blamed]> = found
        .commits
        .iter()
        .map(|(hash, lines)| (hash.as_str(), lines.as_slice()))
        .collect();
    let answered: Vec<Value> = commits
        .drain(..)
        .map(|(commit, owners)| {
            let blamed = blamed
                .get(commit.hash.as_str())
                .copied()
                .unwrap_or_default();
            let person = |pull: Option<Value>| {
                let action = match pull {
                    Some(_) => "comment on the pull request",
                    None => "ask the person",
                };
                json!({
                    "kind": "person",
                    "name": commit.author,
                    "email": commit.email,
                    "pull": pull,
                    "action": action,
                })
            };
            let owners: Vec<Value> = if owners.is_empty() {
                let pull = pulls.get(&commit.hash).map(|pull| {
                    json!({
                        "number": pull["number"],
                        "title": pull["title"],
                        "state": pull["state"],
                        "url": pull["url"],
                    })
                });
                let mut person = person(pull);
                if commit.footer {
                    person["footer"] =
                        "its agent footer does not check out here, so it counts for nothing".into();
                }
                vec![person]
            } else if owners.iter().any(|owner| owner.lines.is_some()) {
                split(&owners, blamed, found.path.as_deref(), person(None))
            } else {
                owners.iter().map(agent_value).collect()
            };
            let lines = Some(runs(blamed.iter().map(|line| line.at)))
                .filter(|lines| !lines.is_empty())
                .map(|lines| ranges(&lines));
            json!({
                "commit": commit.short,
                "subject": commit.subject,
                "date": commit.date,
                "lines": lines,
                "why": found.why.get(&commit.hash),
                "owners": owners,
            })
        })
        .collect();
    let mut note = String::new();
    for line in ci.iter().flat_map(|ci| ci.notes()).chain(&found.notes) {
        note.push_str(line);
        note.push(' ');
    }
    let total = found.total.unwrap_or(found.commits.len());
    if total > MAX_COMMITS {
        note.push_str(&match ci {
            Some(_) => format!(
                "Showing {MAX_COMMITS} of the {total} commits since the run that passed, those the error points at first. "
            ),
            None => format!("Showing {MAX_COMMITS} of {total} commits; ask about fewer lines for the rest. "),
        });
    }
    if !found.uncommitted.is_empty() {
        note.push_str(&format!(
            "Lines {} are not committed yet. ",
            ranges(&found.uncommitted)
        ));
    }
    if answered.is_empty() && found.uncommitted.is_empty() {
        note.push_str("No commit touched it. ");
    }
    let mut answer = json!({
        "target": found.kind,
        "path": found.path,
        "guess": found.guess,
        "commits": answered,
        "note": note.trim_end(),
    });
    if let Some(ci) = &ci {
        ci.describe(&mut answer);
    }
    Ok(answer)
}
