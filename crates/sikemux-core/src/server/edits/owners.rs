//! Which agent wrote each uncommitted line of a checkout: a file's recorded
//! edits replayed in order, each moving the lines the ones before it wrote,
//! then laid over what git reports as changed. Lines the person wrote, and
//! those of a file something the record does not know of changed, are left
//! without an owner.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sikemux_process::sync::Locked;

use super::{line_count, read_text, Edit, Lines};
use crate::git::Git;
use crate::server::Core;

const TIMEOUT: Duration = Duration::from_secs(60);
const OUTPUT_LIMIT: usize = 32 * 1024 * 1024;
/// What git diffs against before a repository's first commit.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Serialize, Default)]
pub(crate) struct Owners {
    pub authors: Vec<Author>,
    pub files: Vec<FileOwners>,
}

impl Owners {
    /// The authors that stand for `agent_id`: the agent, an earlier run of its
    /// CLI's session, or agents at work together when it was one of them.
    pub(crate) fn standing_for(&self, core: &Core, agent_id: &str) -> HashSet<usize> {
        let session = core.names.last_session(agent_id);
        let is_it = |id: &str| {
            id == agent_id
                || session
                    .as_deref()
                    .is_some_and(|session| core.names.last_session(id).as_deref() == Some(session))
        };
        self.authors
            .iter()
            .enumerate()
            .filter(|(_, author)| author.agents().any(is_it))
            .map(|(at, _)| at)
            .collect()
    }

    /// The authors with a line or a removal among the files, by number.
    pub(super) fn writers(&self) -> impl Iterator<Item = (usize, &Author)> {
        let wrote: HashSet<usize> = self
            .files
            .iter()
            .flat_map(|file| {
                let lines = file.lines.iter().map(|run| run.author);
                lines.chain(file.removed.iter().map(|removal| removal.author))
            })
            .collect();
        self.authors
            .iter()
            .enumerate()
            .filter(move |(at, _)| wrote.contains(at))
    }
}

/// An agent, or the agents at work together when no one of them could have
/// made a change, named "Otter or Briar", with no agent id of its own.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Author {
    pub agent_id: String,
    pub name: String,
    pub cli: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The chat run of its latest edit.
    pub feed: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shared: Vec<String>,
    /// Whether the agent is still open here; lines of one closed since block
    /// nobody who names their file.
    #[serde(skip)]
    pub open: bool,
}

impl Author {
    /// The agent, or each of the agents it stands for.
    pub(crate) fn agents(&self) -> impl Iterator<Item = &str> {
        let own = self.shared.is_empty().then_some(&self.agent_id);
        own.into_iter().chain(&self.shared).map(String::as_str)
    }
}

/// The changed lines of a file agents wrote, by its path in the checkout.
#[derive(Serialize)]
pub(crate) struct FileOwners {
    pub path: String,
    pub lines: Vec<Run>,
    pub removed: Vec<Removal>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

/// What to say beside a changed line: who wrote it, said where its run of changed
/// lines starts and wherever the writer changes within it, and whose lines it
/// rewrote.
#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct Note {
    pub line: u64,
    /// Whether the note names who wrote the line; `author` none when no agent did.
    pub named: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub over: Option<usize>,
}

/// `count` lines from `start` that `author` wrote last. `over` is the agent
/// whose lines it rewrote there.
#[derive(Serialize)]
pub(crate) struct Run {
    pub start: u64,
    pub count: u64,
    pub author: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub over: Option<usize>,
}

/// Lines `author` took out just before line `at`.
#[derive(Serialize)]
pub(crate) struct Removal {
    pub at: u64,
    pub author: usize,
}

/// `count` lines from `start` in the newer file stand where `removed` stood,
/// as in the edit record.
#[derive(Clone, Copy)]
pub(super) struct Hunk {
    pub start: u64,
    pub count: u64,
    pub removed: u64,
}

/// Who wrote each line `dir`'s checkout changed since its last commit, or,
/// when `staged`, each line it has staged, numbered as in the index.
pub(crate) fn owners(core: &Core, dir: &Path, staged: bool) -> Result<Owners, String> {
    traced(core, dir, staged).map(|(owners, _)| owners)
}

/// `owners`, reusing what git said of the checkout when last asked while its
/// watcher has seen nothing move, and bringing an earlier read up to date by
/// asking git again only of the paths that moved since.
pub(crate) fn owners_at(core: &Core, dir: &Path, staged: bool) -> Result<Owners, String> {
    let Some((key, generation)) = dir
        .to_str()
        .and_then(crate::git::status::watched_generation)
    else {
        return owners(core, dir, staged);
    };
    type Read = HashMap<(PathBuf, bool), (u64, Arc<Diff>)>;
    static READ: Mutex<Option<Read>> = Mutex::new(None);
    let at = (dir.to_path_buf(), staged);
    let kept = READ
        .locked()
        .get_or_insert_with(HashMap::new)
        .get(&at)
        .map(|(read_at, diff)| (*read_at, diff.clone()));
    let diff = match kept {
        Some((read_at, diff)) if read_at == generation => diff,
        kept => {
            let patched = kept
                .filter(|(_, diff)| diff.read_at.elapsed() < PATCHED_LIFETIME)
                .and_then(|(read_at, diff)| {
                    let repo = git2::Repository::open(&key).ok()?;
                    Some((diff, crate::git::status::moved_since(&repo, &key, read_at)?))
                })
                .map(|(diff, moved)| patched_diff(&diff, staged, &moved));
            let diff = Arc::new(match patched {
                Some(diff) => diff?,
                None => read_diff(dir, staged, true, None, false)?,
            });
            let mut read = READ.locked();
            let read = read.get_or_insert_with(HashMap::new);
            if read.len() >= REMEMBERED_CHECKOUTS && !read.contains_key(&at) {
                read.clear();
            }
            read.insert(at, (generation, diff.clone()));
            diff
        }
    };
    Ok(record_over(core, dir, staged, &diff))
}

/// How long a read may be carried forward by asking again only of moved
/// paths. The watcher cannot see ignore rules kept outside the checkout, so
/// it is read whole again after this.
const PATCHED_LIFETIME: Duration = Duration::from_secs(30);

const REMEMBERED_CHECKOUTS: usize = 32;

/// The diff `traced` laid the record over.
#[derive(Clone)]
pub(super) struct Diff {
    pub top: PathBuf,
    /// The commit it compared with, none before the first.
    pub base: Option<String>,
    pub changed: HashMap<String, Vec<Hunk>>,
    pub untracked: HashSet<String>,
    /// What the working tree changed beyond the index, read only for staged.
    unstaged: HashMap<String, Vec<Hunk>>,
    /// The files whose index entry differs from `base`, when asked for and
    /// not staged.
    staged_files: Option<HashSet<String>>,
    /// When git was last asked of the whole checkout.
    read_at: Instant,
}

pub(super) fn traced(core: &Core, dir: &Path, staged: bool) -> Result<(Owners, Diff), String> {
    let diff = read_diff(dir, staged, false, None, false)?;
    Ok((record_over(core, dir, staged, &diff), diff))
}

/// Who wrote each line staged in `index`, or in the checkout's own index, and
/// what git said changed: since `HEAD`, or since its first parent when the
/// commit made of it will `amend` `HEAD`.
pub(super) fn staged_in(
    core: &Core,
    dir: &Path,
    index: Option<&Path>,
    amend: bool,
) -> Result<(Owners, Diff), String> {
    let diff = read_diff(dir, true, false, index, amend)?;
    Ok((record_over(core, dir, true, &diff), diff))
}

/// What git says changed in `dir`'s checkout, with the files it has staged
/// when `list_staged`, which `patched_diff` needs to stay quick. `index`
/// stands in for the checkout's own, and `amend` compares with `HEAD`'s first
/// parent.
pub(super) fn read_diff(
    dir: &Path,
    staged: bool,
    list_staged: bool,
    index: Option<&Path>,
    amend: bool,
) -> Result<Diff, String> {
    let (top, base) = top_and_base(dir, amend)?;
    let against = base.as_deref().unwrap_or(EMPTY_TREE);
    let mut runs = vec![diff_args(staged, Some(against))];
    if staged {
        runs.push(diff_args(false, None));
    } else {
        runs.push(UNTRACKED.to_vec());
        if list_staged {
            runs.push(vec![
                "diff",
                "--cached",
                "--name-only",
                "-z",
                "--no-renames",
                against,
            ]);
        }
    }
    let runs = runs.into_iter().map(|args| (args, &[][..])).collect();
    let mut outputs = in_parallel(&top, runs, index).into_iter();
    let mut next = || outputs.next().unwrap_or_else(|| Ok(String::new()));
    let changed = hunks(&next()?);
    let others = next()?;
    let (unstaged, untracked) = if staged {
        (hunks(&others), HashSet::new())
    } else {
        (HashMap::new(), split_paths(&others).collect())
    };
    let staged_files = if list_staged && !staged {
        Some(split_paths(&next()?).collect())
    } else {
        None
    };
    Ok(Diff {
        top,
        base,
        changed,
        untracked,
        unstaged,
        staged_files,
        read_at: Instant::now(),
    })
}

const UNTRACKED: [&str; 4] = ["ls-files", "-z", "--others", "--exclude-standard"];

fn split_paths(paths: &str) -> impl Iterator<Item = String> + '_ {
    paths
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}

/// Each of `runs` of git in `top` at once, each limited to its paths when
/// it names any.
fn in_parallel(
    top: &Path,
    runs: Vec<(Vec<&str>, &[&str])>,
    index: Option<&Path>,
) -> Vec<Result<String, String>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = runs
            .into_iter()
            .map(|(args, paths)| {
                let args = if paths.is_empty() {
                    args
                } else {
                    let mut scoped = vec!["--literal-pathspecs"];
                    scoped.extend(args);
                    scoped.push("--");
                    scoped.extend(paths);
                    scoped
                };
                scope.spawn(move || {
                    run(top, &args, index, None)
                        .map(|out| String::from_utf8_lossy(&out).into_owned())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("git did not run".to_owned()))
            })
            .collect()
    })
}

/// `diff` with what git now says of `moved` and the paths below them.
fn patched_diff(diff: &Diff, staged: bool, moved: &[String]) -> Result<Diff, String> {
    let at_or_below = |path: &str, moved: &str| {
        path.strip_prefix(moved)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    let outside = |path: &&String| !moved.iter().any(|moved| at_or_below(path, moved));
    let kept = |files: &HashMap<String, Vec<Hunk>>| -> HashMap<String, Vec<Hunk>> {
        files
            .iter()
            .filter(|(path, _)| outside(path))
            .map(|(path, hunks)| (path.clone(), hunks.clone()))
            .collect()
    };
    let mut patched = diff.clone();
    let all: Vec<&str> = moved.iter().map(String::as_str).collect();
    if all.is_empty() {
        return Ok(patched);
    }
    if staged {
        let mut unstaged = in_parallel(&diff.top, vec![(diff_args(false, None), &all)], None);
        patched.unstaged = kept(&diff.unstaged);
        patched.unstaged.extend(hunks(&unstaged.remove(0)?));
        return Ok(patched);
    }
    // Comparing with the index skips walking the whole commit, and says the
    // same as comparing with the commit wherever nothing is staged.
    let (from_commit, from_index): (Vec<&str>, Vec<&str>) = all.iter().partition(|moved| {
        diff.staged_files
            .as_ref()
            .is_none_or(|files| files.iter().any(|file| at_or_below(file, moved)))
    });
    let against = diff.base.as_deref().unwrap_or(EMPTY_TREE);
    let mut runs = vec![(UNTRACKED.to_vec(), &all[..])];
    for (args, paths) in [
        (diff_args(false, Some(against)), &from_commit),
        (diff_args(false, None), &from_index),
    ] {
        if !paths.is_empty() {
            runs.push((args, paths));
        }
    }
    let mut outputs = in_parallel(&diff.top, runs, None).into_iter();
    patched.untracked = diff.untracked.iter().filter(outside).cloned().collect();
    patched
        .untracked
        .extend(split_paths(&outputs.next().unwrap_or(Ok(String::new()))?));
    patched.changed = kept(&diff.changed);
    for changed in outputs {
        patched.changed.extend(hunks(&changed?));
    }
    Ok(patched)
}

/// The checkout's top folder and its `HEAD` commit, or that commit's first
/// parent when `amend`, none before the first, from one git run.
fn top_and_base(dir: &Path, amend: bool) -> Result<(PathBuf, Option<String>), String> {
    let output = paths_as_written(dir).output(&[
        "rev-parse",
        "--show-toplevel",
        "--verify",
        "--quiet",
        if amend {
            "HEAD^1^{commit}"
        } else {
            "HEAD^{commit}"
        },
    ])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let top = lines.next().filter(|top| !top.is_empty()).ok_or_else(|| {
        format!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })?;
    let base = lines
        .next()
        .filter(|_| output.status.success())
        .map(str::to_owned);
    Ok((PathBuf::from(top), base))
}

/// The record of agents' edits laid over what `diff` says changed.
fn record_over(core: &Core, dir: &Path, staged: bool, diff: &Diff) -> Owners {
    let Diff {
        top,
        changed,
        untracked,
        unstaged,
        ..
    } = diff;
    let paths: HashMap<String, String> = changed
        .keys()
        .chain(untracked)
        .map(|path| (top.join(path).to_string_lossy().into_owned(), path.clone()))
        .collect();
    let mut owners = Owners::default();
    if !paths.is_empty() {
        let project = core.workspaces.project_of(dir);
        let wanted = paths.keys().cloned().collect();
        let edits = core.edits.touching(&project, &wanted);
        let mut agents = Agents::default();
        let mut by_path: HashMap<&str, Vec<(usize, &Edit)>> = HashMap::new();
        for edit in &edits {
            let author = agents.of(edit);
            by_path.entry(&edit.path).or_default().push((author, edit));
        }
        let mut files = Vec::new();
        for (abs, edits) in by_path {
            let mut replay = Replay::default();
            for (author, edit) in edits {
                replay.apply(edit, author);
            }
            if replay.total.is_some()
                && read_text(Path::new(abs)).as_deref().map(line_count) != replay.total
            {
                continue;
            }
            let path = &paths[abs];
            let index = |line| {
                if staged {
                    in_index(unstaged.get(path).map_or(&[][..], Vec::as_slice), line)
                } else {
                    Some(line)
                }
            };
            let (lines, removed) = match changed.get(path) {
                Some(hunks) => replay.laid_over(hunks, index),
                None => (runs(replay.lines.iter().copied()), Vec::new()),
            };
            if !lines.is_empty() || !removed.is_empty() {
                files.push(FileOwners {
                    path: path.clone(),
                    lines,
                    removed,
                    notes: Vec::new(),
                });
            }
        }
        owners = agents.owners(core, files);
        for file in &mut owners.files {
            let changes: Vec<(u64, u64)> = match changed.get(&file.path) {
                Some(hunks) => hunks.iter().map(|hunk| (hunk.start, hunk.count)).collect(),
                None => read_text(&top.join(&file.path))
                    .as_deref()
                    .map(|text| vec![(1, line_count(text))])
                    .unwrap_or_default(),
            };
            file.notes = notes(&changes, &file.lines);
        }
    }
    owners
}

/// Who wrote which lines of a commit, as its note gives each of the agents its
/// footer names, by name.
#[derive(Serialize, Default)]
pub(crate) struct CommitLines {
    pub authors: Vec<Named>,
    pub files: Vec<FileOwners>,
}

/// Agents by name; several who wrote the same lines read "Otter or Briar".
#[derive(Serialize)]
pub(crate) struct Named {
    pub name: String,
}

/// Who wrote each line `commit` changed in `dir`, from `wrote`: each agent's name
/// with the runs of lines the commit's note gives it, by path.
pub(crate) fn commit_lines(
    dir: &Path,
    commit: &str,
    wrote: &[(&str, &super::note::Wrote)],
) -> Result<CommitLines, String> {
    let paths: std::collections::BTreeSet<&str> = wrote
        .iter()
        .flat_map(|(_, lines)| lines.keys().map(String::as_str))
        .collect();
    if paths.is_empty() {
        return Ok(CommitLines::default());
    }
    let parent = format!("{commit}^1");
    let against = match git(dir, &["rev-parse", "--verify", "--quiet", &parent]) {
        Ok(found) if !found.trim().is_empty() => parent,
        _ => EMPTY_TREE.to_owned(),
    };
    let mut args = diff_args(false, Some(&against));
    args.push(commit);
    let changed = hunks(&git(dir, &args)?);
    let mut lines = CommitLines::default();
    let mut named: HashMap<String, usize> = HashMap::new();
    for path in paths {
        let mut writers: std::collections::BTreeMap<u64, Vec<&str>> = Default::default();
        for (name, wrote) in wrote {
            for [first, last] in wrote.get(path).into_iter().flatten() {
                for line in *first..=*last {
                    writers.entry(line).or_default().push(name);
                }
            }
        }
        let owned = writers.into_iter().map(|(at, names)| {
            let name = names.join(" or ");
            let next = lines.authors.len();
            let author = *named.entry(name.clone()).or_insert(next);
            if author == next {
                lines.authors.push(Named { name });
            }
            Line {
                at,
                author,
                over: None,
            }
        });
        let runs = runs(owned.collect::<Vec<_>>());
        let changes: Vec<(u64, u64)> = changed
            .get(path)
            .into_iter()
            .flatten()
            .map(|hunk| (hunk.start, hunk.count))
            .collect();
        lines.files.push(FileOwners {
            path: path.to_owned(),
            notes: notes(&changes, &runs),
            lines: runs,
            removed: Vec::new(),
        });
    }
    Ok(lines)
}

/// The notes beside the lines of `changes`, each `count` changed lines from
/// `start`, given who wrote which in `lines`. A run of changed lines no agent
/// wrote any of gets none.
fn notes(changes: &[(u64, u64)], lines: &[Run]) -> Vec<Note> {
    let owner = |line: u64| {
        let after = lines.partition_point(|run| run.start + run.count <= line);
        lines.get(after).filter(|run| run.start <= line)
    };
    let mut notes = Vec::new();
    for &(start, count) in changes.iter().filter(|(_, count)| *count > 0) {
        let owners: Vec<Option<&Run>> = (start..start + count).map(owner).collect();
        let writers: HashSet<Option<usize>> =
            owners.iter().map(|run| run.map(|run| run.author)).collect();
        if writers.len() == 1 && owners[0].is_none() {
            continue;
        }
        let mixed = writers.len() > 1;
        let mut before: Option<&Run> = None;
        for (at, run) in owners.iter().enumerate() {
            let author = run.map(|run| run.author);
            let named = at == 0 || (mixed && author != before.map(|run| run.author));
            let over = run.and_then(|run| run.over).filter(|over| {
                Some(*over) != before.and_then(|run| run.over)
                    || author != before.map(|run| run.author)
            });
            if named || over.is_some() {
                notes.push(Note {
                    line: start + at as u64,
                    named,
                    author,
                    over,
                });
            }
            before = *run;
        }
    }
    notes
}

/// `agent_id` as its latest edit in `dir`'s project names it.
pub(super) fn latest(core: &Core, dir: &Path, agent_id: &str) -> Option<Author> {
    let project = core.workspaces.project_of(dir);
    core.edits
        .matching(&project, |edit| edit.agent_id == agent_id)
        .pop()
        .map(|edit| author(core, &edit))
}

/// What an agent no longer open, whose name is gone, is called.
const GONE: &str = "an agent no longer open";

fn author(core: &Core, edit: &Edit) -> Author {
    let name = |agent_id: &str| core.names.name(agent_id);
    let name = if edit.shared.is_empty() {
        name(&edit.agent_id)
            .or_else(|| edit.name.clone())
            .unwrap_or_else(|| match edit.cli.as_str() {
                "" => GONE.to_owned(),
                cli => cli.to_owned(),
            })
    } else {
        let mut names: Vec<String> = edit.shared.iter().filter_map(|id| name(id)).collect();
        if names.len() < edit.shared.len() {
            names.push(GONE.to_owned());
        }
        match names.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
            _ => names.concat(),
        }
    };
    Author {
        agent_id: edit.agent_id.clone(),
        name,
        cli: edit.cli.clone(),
        model: edit.model.clone(),
        feed: edit.feed.clone(),
        shared: edit.shared.clone(),
        open: core.names.get(&edit.agent_id).is_some(),
    }
}

/// The agents met while replaying, each by its latest edit. The person is
/// met too, so their edits take lines back, but owns none in the end.
#[derive(Default)]
struct Agents<'a> {
    index: HashMap<(&'a str, &'a [String]), usize>,
    latest: Vec<&'a Edit>,
}

impl<'a> Agents<'a> {
    fn person(&self, at: usize) -> bool {
        let edit = self.latest[at];
        edit.agent_id.is_empty() && edit.shared.is_empty()
    }

    fn of(&mut self, edit: &'a Edit) -> usize {
        let at = *self
            .index
            .entry((&edit.agent_id, &edit.shared))
            .or_insert(self.latest.len());
        if at == self.latest.len() {
            self.latest.push(edit);
        } else {
            self.latest[at] = edit;
        }
        at
    }

    /// `files` with their agents numbered again among only those they name.
    fn owners(self, core: &Core, mut files: Vec<FileOwners>) -> Owners {
        for file in &mut files {
            file.lines.retain(|run| !self.person(run.author));
            for run in &mut file.lines {
                run.over = run.over.filter(|over| !self.person(*over));
            }
            file.removed.retain(|removal| !self.person(removal.author));
        }
        files.retain(|file| !file.lines.is_empty() || !file.removed.is_empty());
        let mut numbers: Vec<Option<usize>> = vec![None; self.latest.len()];
        let mut authors = Vec::new();
        files.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        let mut renumber = |agent: &mut usize| {
            *agent = *numbers[*agent].get_or_insert_with(|| {
                authors.push(author(core, self.latest[*agent]));
                authors.len() - 1
            });
        };
        for file in &mut files {
            for run in &mut file.lines {
                renumber(&mut run.author);
                if let Some(over) = &mut run.over {
                    renumber(over);
                }
            }
            for removal in &mut file.removed {
                renumber(&mut removal.author);
            }
        }
        Owners { authors, files }
    }
}

/// One part of an edit, starting at line `old` in the file before it.
/// `shift` is how far it and the parts above it move the lines below.
struct Block {
    old: u64,
    new: Hunk,
    shift: i64,
}

#[derive(Clone, Copy)]
struct Line {
    at: u64,
    author: usize,
    over: Option<usize>,
}

/// The lines agents wrote in one file as it stands after the edits so far.
#[derive(Default)]
struct Replay {
    lines: Vec<Line>,
    removed: Vec<(u64, usize)>,
    total: Option<u64>,
}

impl Replay {
    fn apply(&mut self, edit: &Edit, author: usize) {
        let grown: i64 = edit
            .lines
            .iter()
            .map(|[_, count, removed]| *count as i64 - *removed as i64)
            .sum();
        let follows = match (self.total, edit.total) {
            (Some(before), Some(after)) => before as i64 + grown == after as i64,
            _ => true,
        };
        self.total = edit.total;
        if edit.lines.is_empty() || !follows {
            self.lines.clear();
            self.removed.clear();
            if edit.lines.is_empty() {
                return;
            }
        }
        let blocks = blocks(&edit.lines);
        let mut over = vec![None; blocks.len()];
        let mut lines = Vec::with_capacity(self.lines.len());
        for line in self.lines.drain(..) {
            match place(&blocks, line.at) {
                Ok(at) => lines.push(Line { at, ..line }),
                Err(block) if line.author != author => over[block] = Some(line.author),
                Err(block) => over[block] = over[block].or(line.over),
            }
        }
        self.removed = self
            .removed
            .iter()
            .filter_map(|&(at, by)| place_gap(&blocks, at).map(|at| (at, by)))
            .collect();
        for (Block { new: hunk, .. }, over) in blocks.iter().zip(over) {
            if hunk.count == 0 && hunk.removed > 0 {
                self.removed.push((hunk.start, author));
            }
            lines.extend((hunk.start..hunk.start + hunk.count).map(|at| Line { at, author, over }));
        }
        lines.sort_unstable_by_key(|line| line.at);
        self.lines = lines;
    }

    /// The lines that are among `hunks`' new ones, numbered by `index`.
    fn laid_over(
        &self,
        hunks: &[Hunk],
        index: impl Fn(u64) -> Option<u64>,
    ) -> (Vec<Run>, Vec<Removal>) {
        let added = |line: u64| {
            let after = hunks.partition_point(|hunk| hunk.start <= line);
            after > 0 && line < hunks[after - 1].start + hunks[after - 1].count
        };
        let lines = self
            .lines
            .iter()
            .filter_map(|line| index(line.at).map(|at| Line { at, ..*line }))
            .filter(|line| added(line.at));
        let removed = self
            .removed
            .iter()
            .filter_map(|&(at, author)| index(at).map(|at| Removal { at, author }))
            .filter(|removal| {
                hunks
                    .iter()
                    .any(|hunk| hunk.count == 0 && hunk.start == removal.at)
            })
            .collect();
        (runs(lines), removed)
    }
}

/// An edit's parts in order, each with where it stood in the file before it.
fn blocks(lines: &Lines) -> Vec<Block> {
    let mut sorted = lines.clone();
    sorted.sort_unstable();
    let mut shift = 0i64;
    sorted
        .into_iter()
        .map(|[start, count, removed]| {
            let old = (start as i64 - shift).max(1) as u64;
            shift += count as i64 - removed as i64;
            Block {
                old,
                new: Hunk {
                    start,
                    count,
                    removed,
                },
                shift,
            }
        })
        .collect()
}

/// A line the record says an agent wrote.
pub(crate) struct Authored {
    pub line: u64,
    pub agent_id: String,
}

/// Who wrote each line the record knows of in a file after `edits`, its
/// edits oldest first, and how many lines the file had then, when known.
pub(crate) fn authors(edits: &[Edit]) -> (Vec<Authored>, Option<u64>) {
    let mut agents = Agents::default();
    let mut replay = Replay::default();
    for edit in edits {
        let author = agents.of(edit);
        replay.apply(edit, author);
    }
    let lines = replay
        .lines
        .iter()
        .map(|line| Authored {
            line: line.at,
            agent_id: agents.latest[line.author].agent_id.clone(),
        })
        .collect();
    (lines, replay.total)
}

/// Each part of an edit as `[start, count, removed]` in the file after it,
/// with the first line it replaced in the file before it, counted from 1.
pub(crate) fn before(lines: &Lines) -> Vec<(u64, [u64; 3])> {
    blocks(lines)
        .into_iter()
        .map(|block| {
            (
                block.old,
                [block.new.start, block.new.count, block.new.removed],
            )
        })
        .collect()
}

/// Where `line` moves to through an edit's `blocks`, or the block that
/// replaced it.
fn place(blocks: &[Block], line: u64) -> Result<u64, usize> {
    let mut shift = 0;
    for (at, block) in blocks.iter().enumerate() {
        if line < block.old {
            break;
        }
        if line < block.old + block.new.removed {
            return Err(at);
        }
        shift = block.shift;
    }
    Ok((line as i64 + shift) as u64)
}

/// Where the gap just before `line` moves to, unless it was inside lines the
/// edit replaced.
fn place_gap(blocks: &[Block], line: u64) -> Option<u64> {
    let mut shift = 0;
    for block in blocks {
        if line <= block.old {
            break;
        }
        if line < block.old + block.new.removed {
            return None;
        }
        shift = block.shift;
    }
    Some((line as i64 + shift) as u64)
}

/// The index's number for a working tree line, given the working tree's
/// unstaged `hunks`, or none when the line is not staged.
fn in_index(hunks: &[Hunk], line: u64) -> Option<u64> {
    let mut shift = 0i64;
    for hunk in hunks {
        if line < hunk.start {
            break;
        }
        if line < hunk.start + hunk.count {
            return None;
        }
        shift += hunk.removed as i64 - hunk.count as i64;
    }
    Some((line as i64 + shift) as u64)
}

fn runs(lines: impl IntoIterator<Item = Line>) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for line in lines {
        match runs.last_mut() {
            Some(run)
                if run.start + run.count == line.at
                    && run.author == line.author
                    && run.over == line.over =>
            {
                run.count += 1;
            }
            _ => runs.push(Run {
                start: line.at,
                count: 1,
                author: line.author,
                over: line.over,
            }),
        }
    }
    runs
}

fn diff_args(staged: bool, base: Option<&str>) -> Vec<&str> {
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "-U0",
    ];
    if staged {
        args.push("--cached");
    }
    args.extend(base);
    args
}

/// Each file's hunks in a `-U0` diff, by path. A binary file has none.
fn hunks(diff: &str) -> HashMap<String, Vec<Hunk>> {
    let mut files: HashMap<String, Vec<Hunk>> = HashMap::new();
    let mut path: Option<String> = None;
    let mut left = 0u64;
    for line in diff.lines() {
        if left > 0 {
            if line.starts_with(['+', '-']) {
                left -= 1;
            }
            continue;
        }
        if let Some(target) = line.strip_prefix("+++ ") {
            path = target.strip_prefix("b/").map(str::to_owned);
        } else if let Some(binary) = line
            .strip_prefix("Binary files ")
            .and_then(|names| names.strip_suffix(" differ"))
        {
            if let Some((_, new)) = binary.split_once(" and b/") {
                files.entry(new.to_owned()).or_default();
            }
        } else if let Some(hunk) = line.strip_prefix("@@ -").and_then(header) {
            left = hunk.count + hunk.removed;
            if let Some(path) = &path {
                files.entry(path.clone()).or_default().push(hunk);
            }
        }
    }
    files
}

/// A hunk from its header after `@@ -`: `a,b +c,d @@`.
fn header(text: &str) -> Option<Hunk> {
    let (old, rest) = text.split_once(" +")?;
    let new = rest.split_once(" @@")?.0;
    let range = |range: &str| -> Option<(u64, u64)> {
        match range.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((range.parse().ok()?, 1)),
        }
    };
    let (_, removed) = range(old)?;
    let (start, count) = range(new)?;
    Some(Hunk {
        start: if count == 0 { start + 1 } else { start },
        count,
        removed,
    })
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    run(dir, args, None, None).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// Runs git in `dir`, with `index` in place of the checkout's own and `input`
/// on its standard input.
pub(super) fn run(
    dir: &Path,
    args: &[&str],
    index: Option<&Path>,
    input: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let mut git = paths_as_written(dir);
    if let Some(index) = index {
        git = git.index(index);
    }
    if let Some(input) = input {
        git = git.input(input);
    }
    git.stdout(args)
}

/// Git in `dir` writing paths as they are, never quoted.
fn paths_as_written(dir: &Path) -> Git<'static> {
    Git::at(dir)
        .config("core.quotePath", "false")
        .timeout(TIMEOUT)
        .limit(OUTPUT_LIMIT)
}
