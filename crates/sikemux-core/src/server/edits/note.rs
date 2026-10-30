//! Which lines of a commit each agent wrote, for a commit no one writer has
//! all the lines of: a git note on it under [`NOTES_REF`], put there by the
//! hooks that sign its footer. Lines are numbered as the commit has its files,
//! as `git blame` names them. The host signs a note as it does a footer, and
//! a note counts only for the agents the commit's verified footer names.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use iroh::PublicKey;

use super::owners::{run, Diff, Owners};
use crate::trailers::{verify_note, Commit, NOTES_REF};

/// Runs of lines, first and last, by path.
pub(crate) type Wrote = BTreeMap<String, Vec<[u64; 2]>>;

/// What each agent wrote, by its session, or its chat when its session was
/// not known.
pub(crate) type Note = BTreeMap<String, Wrote>;

/// The note for a commit of what `diff` says was staged, when its lines are
/// not all one agent's: the person's commit holding some lines no agent wrote,
/// or any commit holding lines of several. It names only the agents the
/// commit's footer names, by their `keys`, given by chat.
pub(super) fn of(
    owners: &Owners,
    diff: &Diff,
    keys: &HashMap<&str, &str>,
    by_person: bool,
) -> Option<String> {
    let mut note = Note::new();
    let mut owned = 0;
    for file in owners
        .files
        .iter()
        .filter(|file| diff.changed.contains_key(&file.path))
    {
        for run in &file.lines {
            let last = run.start + run.count - 1;
            let mut counted = false;
            let authors: HashSet<&str> = owners.authors[run.author]
                .agents()
                .filter_map(|chat| keys.get(chat).copied())
                .collect();
            for key in authors {
                counted = true;
                let runs = note
                    .entry(key.to_owned())
                    .or_default()
                    .entry(file.path.clone())
                    .or_default();
                match runs.last_mut() {
                    Some([_, end]) if *end + 1 == run.start => *end = last,
                    _ => runs.push([run.start, last]),
                }
            }
            if counted {
                owned += run.count;
            }
        }
    }
    let added: u64 = diff.changed.values().flatten().map(|hunk| hunk.count).sum();
    let one_writer = note.len() == 1 && (!by_person || owned >= added);
    if note.is_empty() || one_writer {
        return None;
    }
    serde_json::to_string(&note).ok()
}

/// The notes on `commits`, full hashes, by commit, from one run of git, that
/// one of `hosts` signed for the commit they are on.
pub(crate) fn read(dir: &Path, commits: &[&str], hosts: &[PublicKey]) -> HashMap<String, Note> {
    let commits: Vec<&str> = commits
        .iter()
        .copied()
        .filter(|commit| matches!(commit.len(), 40 | 64))
        .collect();
    if commits.is_empty() || hosts.is_empty() {
        return HashMap::new();
    }
    // Git spreads a big notes tree over folders named for each hash's first
    // characters, so each commit is asked for at every depth, after itself.
    let mut input = String::new();
    for commit in &commits {
        input.push_str(&format!("{commit}\n"));
        for depth in 0..=2 {
            let (folders, rest) = commit.split_at(depth * 2);
            let folders: Vec<&str> = (0..depth).map(|at| &folders[at * 2..at * 2 + 2]).collect();
            let path = folders
                .into_iter()
                .chain([rest])
                .collect::<Vec<_>>()
                .join("/");
            input.push_str(&format!("{NOTES_REF}:{path}\n"));
        }
    }
    let Ok(out) = run(dir, &["cat-file", "--batch"], None, Some(input.as_bytes())) else {
        return HashMap::new();
    };
    let mut answers = Answers(out.as_slice());
    let mut notes = HashMap::new();
    for commit in commits {
        let object = answers.next();
        let blobs: Vec<Option<(String, &[u8])>> = (0..=2).map(|_| answers.next()).collect();
        let Some(("commit", object)) = object
            .as_ref()
            .map(|(kind, object)| (kind.as_str(), *object))
        else {
            continue;
        };
        let header = String::from_utf8_lossy(object);
        let header = header.split("\n\n").next().unwrap_or_default();
        let tree = header.lines().find_map(|line| line.strip_prefix("tree "));
        let parents: Vec<String> = header
            .lines()
            .filter_map(|line| line.strip_prefix("parent "))
            .map(str::to_owned)
            .collect();
        let Some(tree) = tree else { continue };
        let made_of = Commit {
            tree,
            parents: &parents,
        };
        let note = blobs
            .into_iter()
            .flatten()
            .filter(|(kind, _)| kind == "blob")
            .find_map(|(_, blob)| {
                let blob = std::str::from_utf8(blob).ok()?;
                let note = verify_note(blob, &made_of, hosts)?;
                serde_json::from_str::<Note>(note).ok()
            });
        if let Some(mut note) = note {
            note.values_mut()
                .flat_map(BTreeMap::values_mut)
                .for_each(|runs| runs.sort_unstable());
            notes.insert(commit.to_owned(), note);
        }
    }
    notes
}

/// What `git cat-file --batch` answered, one object at a time: its kind and
/// content, or none when it found nothing.
struct Answers<'a>(&'a [u8]);

impl<'a> Answers<'a> {
    fn next(&mut self) -> Option<(String, &'a [u8])> {
        let end = self.0.iter().position(|byte| *byte == b'\n')?;
        let header = String::from_utf8_lossy(&self.0[..end]).into_owned();
        self.0 = &self.0[end + 1..];
        let mut words = header.split(' ');
        let (Some(_), Some(kind), Some(size)) = (words.next(), words.next(), words.next()) else {
            return None;
        };
        let size = size.parse::<usize>().ok()?;
        let content = self.0.get(..size)?;
        self.0 = self.0.get(size + 1..).unwrap_or_default();
        Some((kind.to_owned(), content))
    }
}

/// Whether `line` of `path` is among `wrote`'s.
pub(crate) fn holds(wrote: &Wrote, path: &str, line: u64) -> bool {
    wrote.get(path).is_some_and(|runs| {
        let after = runs.partition_point(|[first, _]| *first <= line);
        after > 0 && line <= runs[after - 1][1]
    })
}
