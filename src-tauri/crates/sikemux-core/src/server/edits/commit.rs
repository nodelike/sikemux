//! Commits the lines one agent wrote and nothing else. The commit is built in
//! an index of its own from the last commit plus those lines, and the branch
//! moves only if no one moved it meanwhile. The checkout's files stay as they
//! are, and the shared index takes in only what was committed.

use std::collections::{BTreeSet, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::footer::{agent, known};
use super::owners::{git, run, traced, Diff, Hunk, Owners};
use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::Core;
use crate::trailers::Commit;

const ATTEMPTS: usize = 4;
const INDEX_LOCKED_TRIES: usize = 5;

#[derive(Serialize)]
pub(crate) struct Committed {
    commit: String,
    branch: String,
    files: Vec<Taken>,
    left: Vec<Left>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
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

/// `git_commit`: commits the calling agent's lines in its checkout.
pub(crate) async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .clone()
        .ok_or("git_commit needs the calling agent's id")?;
    let message = match request.params.get("message") {
        Some(Value::String(message)) => message.clone(),
        _ => return Err("message must be the commit message".into()),
    };
    let named: Vec<String> = match request.params.get("paths") {
        None | Some(Value::Null) => Vec::new(),
        Some(paths) => {
            serde_json::from_value(paths.clone()).map_err(|_| "paths must be a list of paths")?
        }
    };
    let (core, dir) = (core.clone(), PathBuf::from(&request.project));
    blocking(move || Ok(commit(&core, &dir, &agent_id, &message, &named)?))
        .await
        .map_err(|error| error.to_string())
        .and_then(|committed| serde_json::to_value(committed).map_err(|error| error.to_string()))
}

/// Commits what `agent_id` wrote in `dir`'s checkout, and every change in the
/// `named` paths that holds no other agent's lines.
pub(crate) fn commit(
    core: &Core,
    dir: &Path,
    agent_id: &str,
    message: &str,
    named: &[String],
) -> Result<Committed, String> {
    if message.trim().is_empty() {
        return Err("message must say what the commit does".into());
    }
    for _ in 0..ATTEMPTS {
        let branch = git(dir, &["symbolic-ref", "-q", "HEAD"])
            .map_or_else(|_| "HEAD".to_owned(), |branch| branch.trim().to_owned());
        let (owners, diff) = traced(core, dir, false)?;
        let named = named
            .iter()
            .map(|path| in_checkout(&diff.top, dir, path))
            .collect::<Result<Vec<_>, _>>()?;
        let (changes, left) = plan(&owners, &diff, agent_id, &named)?;
        if changes.is_empty() {
            return Err(nothing_yours(&left));
        }
        let me = owners
            .authors
            .iter()
            .find(|author| author.agent_id == agent_id);
        let signer = me
            .map(|me| agent(core, me))
            .or_else(|| known(core, dir, agent_id));
        let key = core.remote.host_key();
        let signed = |commit: &Commit| {
            let footer =
                crate::trailers::footer(Vec::from_iter(signer).as_slice(), commit, key.as_ref());
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
                let note = share(&diff.top, &changes, &blobs).err();
                let paths: Vec<String> = changes.iter().map(|change| change.path.clone()).collect();
                core.collisions
                    .committed(core, agent_id, &diff.top, &paths, &commit, &message);
                return Ok(Committed {
                    commit,
                    branch,
                    files: changes
                        .into_iter()
                        .map(|change| Taken {
                            path: change.path,
                            added: change.added,
                            removed: change.removed,
                        })
                        .collect(),
                    left,
                    note,
                });
            }
            Err(error) if head(&diff.top) == diff.base => return Err(error),
            Err(_) => {}
        }
    }
    Err("the branch kept moving while committing; call this again".into())
}

fn head(top: &Path) -> Option<String> {
    git(top, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .ok()
        .map(|head| head.trim().to_owned())
}

fn nothing_yours(left: &[Left]) -> String {
    let mut said = "None of the uncommitted lines here are yours".to_owned();
    if !left.is_empty() {
        let files: Vec<&str> = left.iter().map(|left| left.path.as_str()).collect();
        said.push_str(&format!(
            "; changed by others or by no agent Sikemux knows of: {}. Name paths to commit lines no agent wrote",
            files.join(", ")
        ));
    }
    said
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

/// What the commit holds of each changed file, and what it leaves out.
fn plan(
    owners: &Owners,
    diff: &Diff,
    agent_id: &str,
    named: &[String],
) -> Result<(Vec<Change>, Vec<Left>), String> {
    let me = owners
        .authors
        .iter()
        .position(|author| author.agent_id == agent_id);
    let by_path: HashMap<&str, _> = owners
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
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
    let before = tree_entries(diff, paths.iter().copied())?;
    let name = |author: Option<usize>| {
        author.map_or_else(
            || "no agent".to_owned(),
            |at| owners.authors[at].name.clone(),
        )
    };
    let mut changes = Vec::new();
    let mut left = Vec::new();
    for path in paths {
        let file = by_path.get(path);
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
        let others = authors.iter().any(|author| {
            Some(*author) != me && !owners.authors[*author].agents().any(|id| id == agent_id)
        });
        let whole = !others && names(named, path);
        if !whole && !me.is_some_and(|me| authors.contains(&me)) {
            let mut by: BTreeSet<String> = authors.iter().map(|at| name(Some(*at))).collect();
            if by.is_empty() {
                by.insert(name(None));
            }
            left.push(Left {
                path: path.to_owned(),
                by: by.into_iter().collect(),
            });
            continue;
        }
        let entry = before.get(path);
        let old = match entry {
            Some((_, blob)) => run(&diff.top, &["cat-file", "blob", blob], None, None)?,
            None => Vec::new(),
        };
        let Ok(meta) = std::fs::symlink_metadata(diff.top.join(path)) else {
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
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let new = std::fs::read(diff.top.join(path)).map_err(|error| format!("{path}: {error}"))?;
        let hunks = match diff.changed.get(path) {
            Some(hunks) => hunks.clone(),
            None => vec![Hunk {
                start: 1,
                count: lines(&new).len() as u64,
                removed: 0,
            }],
        };
        let owner = |line| owner_of.get(&line).copied();
        let removal = |line| removed_by.get(&line).copied();
        let mine = |author: Option<usize>| whole || (me.is_some() && author == me);
        let mut tally = Tally::default();
        let content = splice(&old, &new, &hunks, &mut tally, owner, removal, mine)
            .ok_or_else(|| format!("{path} changed while it was read; call this again"))?;
        if !tally.left.is_empty() {
            let by: BTreeSet<String> = tally.left.iter().map(|author| name(*author)).collect();
            left.push(Left {
                path: path.to_owned(),
                by: by.into_iter().collect(),
            });
        }
        if tally.added + tally.removed == 0 {
            continue;
        }
        let mode = match entry {
            Some((mode, _)) => mode.clone(),
            None if meta.permissions().mode() & 0o111 != 0 => "100755".into(),
            None => "100644".into(),
        };
        changes.push(Change {
            path: path.to_owned(),
            mode,
            content: Some(content),
            before: entry.map(|(_, blob)| blob.clone()),
            added: tally.added,
            removed: tally.removed,
        });
    }
    Ok((changes, left))
}

#[derive(Default)]
struct Tally {
    added: u64,
    removed: u64,
    left: BTreeSet<Option<usize>>,
}

/// `old` with the parts of `new`'s hunks whose author is `mine`. A hunk that
/// both removes and adds lines goes in whole or not at all, since its lines
/// cannot be told apart from the ones they replaced.
fn splice(
    old: &[u8],
    new: &[u8],
    hunks: &[Hunk],
    tally: &mut Tally,
    owner: impl Fn(u64) -> Option<usize>,
    removal: impl Fn(u64) -> Option<usize>,
    mine: impl Fn(Option<usize>) -> bool,
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
        let picked: Vec<u64> = match (hunk.count, hunk.removed) {
            (_, 0) => added.clone().filter(|line| mine(owner(*line))).collect(),
            (0, _) => Vec::new(),
            _ if added.clone().all(|line| mine(owner(line))) => added.clone().collect(),
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
/// own, and the blob each change wrote.
/// The commit of `changes`, with the message `signed` gives for its tree and
/// parents.
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
    let commit = run(&diff.top, &args, None, Some(message.as_bytes()))?;
    Ok((
        String::from_utf8_lossy(&commit).trim().to_owned(),
        message,
        blobs,
    ))
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
