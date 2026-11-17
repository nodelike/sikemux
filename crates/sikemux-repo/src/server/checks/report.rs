//! What an agent reads of a failed check: a few lines naming the error, with
//! the whole output in a file, since context is what agents run out of.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::git::Git;
use crate::server::codehost::log::failure_lines;
use crate::text::clipped;

const MAX_ERROR_LINES: usize = 6;
const MAX_LINE_CHARS: usize = 200;
const LAST_LINES_WITHOUT_ERROR: usize = 3;
const LOGS_KEPT: usize = 40;

/// Keeps `text` whole in a file under the repository's git folder, which every checkout of
/// it can read and no `git clean` removes.
pub fn keep_log(dir: &Path, sha: &str, tier: &str, text: &str) -> Option<PathBuf> {
    let common = Git::quick(dir)
        .trimmed(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .ok()?;
    let folder = Path::new(&common).join("sikemux").join("check-logs");
    std::fs::create_dir_all(&folder).ok()?;
    let path = folder.join(format!("{}-{tier}.log", crate::git::short(sha)));
    std::fs::write(&path, text).ok()?;
    prune(&folder);
    Some(path)
}

fn prune(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut logs: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    logs.sort_by_key(|one| std::cmp::Reverse(one.0));
    for (_, old) in logs.into_iter().skip(LOGS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
}

/// The lines of `text` that name the error, then a count and where the whole of it is.
pub fn digest(text: &str, log: Option<&Path>) -> String {
    let (mut named, errors) = failure_lines(text, MAX_ERROR_LINES);
    if named.is_empty() {
        named = last_lines(text);
    }
    let total = text.lines().count();
    let shown: Vec<String> = named
        .iter()
        .map(|line| clipped(line, MAX_LINE_CHARS))
        .collect();
    let lines = if total == 1 { "line" } else { "lines" };
    let tally = match errors {
        0 => format!("No line names an error, of {total} {lines}."),
        1 => format!("1 line names an error, of {total} {lines}."),
        errors => format!("{errors} lines name an error, of {total} {lines}."),
    };
    let tail = match log {
        Some(log) => format!("{tally} Full log: {}", log.display()),
        None => tally,
    };
    format!("{}\n{tail}", shown.join("\n"))
}

fn last_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(LAST_LINES_WITHOUT_ERROR)
        .map(|line| line.trim().to_owned())
        .collect();
    lines.reverse();
    lines
}
