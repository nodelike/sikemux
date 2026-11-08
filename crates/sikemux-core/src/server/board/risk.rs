//! How much review a change needs, from what the core can see of it: its size, what it touches
//! and how work in those folders went before. Anything that reads as credentials, a migration or
//! the way changes merge and ship always keeps the full path.

use super::record::RunRecord;
use super::{Board, Card};

const MAX_LOW_RISK_FILES: usize = 3;
const MAX_LOW_RISK_LINES: usize = 40;
const MAX_LOW_RISK_CARD_CHARS: usize = 600;
const MAX_AREAS: usize = 50;
const NEVER_LIGHTER: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "credential",
    "api_key",
    "apikey",
    "auth",
    "token",
    "keychain",
    "migrat",
    "schema",
    ".sql",
    "merge",
    "rebase",
    "release",
    "deploy",
    ".github/",
    ".gitlab-ci",
    "jenkinsfile",
];

/// What a finished change is rated on.
pub(super) struct Change<'a> {
    pub paths: &'a [String],
    pub diff: &'a str,
    pub cut: bool,
    /// How many times a reviewer already sent it back.
    pub rounds: u32,
}

pub(super) struct Rating {
    pub low: bool,
    pub why: String,
}

fn never_lighter(text: &str) -> Option<&'static str> {
    let text = text.to_lowercase();
    NEVER_LIGHTER
        .iter()
        .find(|word| text.contains(**word))
        .copied()
}

/// The folders `paths` sit in.
pub(super) fn areas(paths: &[String]) -> Vec<String> {
    let mut areas: Vec<String> = paths
        .iter()
        .map(|path| path.rsplit_once('/').map_or(".", |(folder, _)| folder))
        .map(str::to_owned)
        .collect();
    areas.sort_unstable();
    areas.dedup();
    areas.truncate(MAX_AREAS);
    areas
}

/// What went wrong before in any of `areas`, the worst of it.
fn trouble<'a>(
    areas: &[String],
    past: impl Iterator<Item = &'a RunRecord>,
) -> Option<&'static str> {
    let mut asked = false;
    for record in past.filter(|record| record.areas.iter().any(|area| areas.contains(area))) {
        if record.broke_after_landing {
            return Some("work in the same folders broke after landing before");
        }
        asked |= record.findings > 0;
    }
    asked.then_some("reviewers asked for changes in the same folders before")
}

fn size(files: usize, lines: usize) -> String {
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    format!(
        "{files} file{} and {lines} changed line{}",
        plural(files),
        plural(lines)
    )
}

fn changed_lines(diff: &str) -> impl Iterator<Item = &str> {
    diff.lines().filter(|line| {
        (line.starts_with('+') && !line.starts_with("+++"))
            || (line.starts_with('-') && !line.starts_with("---"))
    })
}

/// Whether `change` can do with a read review, and why, in the words the run record keeps.
pub(super) fn of_change<'a>(change: &Change, past: impl Iterator<Item = &'a RunRecord>) -> Rating {
    let full = |why: String| Rating {
        low: false,
        why: format!("Full review: {why}"),
    };
    if change.rounds > 0 {
        return full("a reviewer already sent it back".to_owned());
    }
    let named = (change.paths.iter()).find_map(|path| Some((path, never_lighter(path)?)));
    if let Some((path, word)) = named {
        return full(format!("{path} reads as sensitive ({word})"));
    }
    if let Some(word) = changed_lines(change.diff).find_map(never_lighter) {
        return full(format!("its changed lines mention {word}"));
    }
    let (files, lines) = (change.paths.len(), changed_lines(change.diff).count());
    if change.cut || files > MAX_LOW_RISK_FILES || lines > MAX_LOW_RISK_LINES {
        return full(format!("{} is not small", size(files, lines)));
    }
    if let Some(why) = trouble(&areas(change.paths), past) {
        return full(why.to_owned());
    }
    Rating {
        low: true,
        why: format!(
            "Read review: {}, nothing sensitive and no earlier trouble in those folders",
            size(files, lines)
        ),
    }
}

fn paths_named(text: &str) -> Vec<String> {
    let mut named: Vec<String> = text
        .split_whitespace()
        .map(|word| word.trim_matches(|letter: char| "`'\"()[],;:.".contains(letter)))
        .filter(|word| !word.contains("://"))
        .map(|word| word.split(':').next().unwrap_or(word))
        .filter(|word| {
            word.rsplit_once('/')
                .is_some_and(|(_, name)| name.contains('.'))
        })
        .map(str::to_owned)
        .collect();
    named.sort_unstable();
    named.dedup();
    named
}

/// Why `card` can go without a contract round, in the record's words. Nothing is built yet, so
/// only a short card that names the few files it changes can say it is small; the change itself
/// is rated again before it is reviewed.
pub(super) fn no_contract(board: &Board, card: &Card) -> Option<String> {
    let text = format!("{}\n{}\n{}", card.title, card.details, card.acceptance);
    if text.chars().count() > MAX_LOW_RISK_CARD_CHARS || never_lighter(&text).is_some() {
        return None;
    }
    let named = paths_named(&text);
    if named.is_empty() || named.len() > MAX_LOW_RISK_FILES {
        return None;
    }
    let others = (board.cards.iter())
        .filter(|other| other.id != card.id)
        .filter_map(|other| other.record.as_ref());
    if trouble(&areas(&named), others).is_some() {
        return None;
    }
    Some(format!(
        "skipped: low risk, a short card that names only {}",
        named.join(", ")
    ))
}
