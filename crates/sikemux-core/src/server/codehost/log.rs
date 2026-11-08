//! What a failing CI job's log says: the error it shows, the files the error
//! names, and the tests that failed.

use std::collections::BTreeSet;

/// Lines read from the end of a failing job's log.
pub(crate) const LOG_LINES: usize = 800;
const MAX_LINE_CHARS: usize = 240;
const EXCERPT_BYTES: usize = 1500;
const MAX_NAMED: usize = 24;
/// Failed tests named per job.
const MAX_TESTS: usize = 4;
/// What GitHub writes when a step fails; what follows is the job cleaning up.
const STEP_FAILED: &str = "##[error]Process completed with exit code";
/// Paths in a log that point into installed dependencies, not the project.
const DEPENDENCIES: [&str; 6] = [
    "node_modules/",
    ".pnpm/",
    ".cargo/",
    "/rustc/",
    "site-packages/",
    "/usr/",
];

/// `line` without terminal colours, cut to a readable length.
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    if out.chars().count() > MAX_LINE_CHARS {
        out = out.chars().take(MAX_LINE_CHARS).collect::<String>() + "…";
    }
    out
}

fn says_failed(line: &str) -> bool {
    let lower = line.to_lowercase();
    let failed = ["error", "failed", "panicked", "fail ", "✗", "assertion"]
        .iter()
        .any(|word| lower.contains(word))
        || lower.trim_start().starts_with("fail");
    failed && (!lower.contains("warning") || lower.contains("error:"))
}
/// Lines that only repeat that something failed, or where: stack frames and
/// the exit status, also behind a tool's `name: ` prefix.
fn is_noise(line: &str) -> bool {
    let trimmed = line.trim();
    let said = trimmed
        .split_once(": ")
        .filter(|(prefix, _)| prefix.len() <= 40 && !prefix.contains(['/', '.']))
        .map_or(trimmed, |(_, said)| said.trim());
    said.is_empty() || said.starts_with("at ") || trimmed.starts_with(STEP_FAILED)
}
/// Lines a passing tool writes that only look like an error.
fn is_routine(line: &str) -> bool {
    let trimmed = line.trim();
    let lower = trimmed.to_lowercase();
    [
        "compiling ",
        "checking ",
        "downloaded ",
        "downloading ",
        "finished ",
    ]
    .iter()
    .any(|word| lower.starts_with(word))
        || lower.ends_with(" ok")
        || lower.contains(" 0 failed")
        || lower.starts_with("0 failed")
        || lower.contains(" 0 errors")
        || lower.starts_with("0 errors")
}

/// The first `most` different lines of `text` that name what failed, each with the place
/// a compiler gives on the next line, and how many lines name a failure in all.
pub(crate) fn failure_lines(text: &str, most: usize) -> (Vec<String>, usize) {
    let lines: Vec<String> = text.lines().map(plain).collect();
    let mut named: Vec<String> = Vec::new();
    let mut count = 0;
    for (at, line) in lines.iter().enumerate() {
        if !says_failed(line) || is_noise(line) || is_routine(line) {
            continue;
        }
        count += 1;
        if named.len() >= most {
            continue;
        }
        let place = lines
            .get(at + 1)
            .map(|next| next.trim())
            .filter(|next| next.starts_with("-->"));
        let line = match place {
            Some(place) => format!("{}  {place}", line.trim()),
            None => line.trim().to_owned(),
        };
        if !named.contains(&line) {
            named.push(line);
        }
    }
    (named, count)
}

/// A short excerpt of the error in a failing job's log `lines`, and the
/// files it names.
pub(crate) fn read_error(lines: &[String]) -> (Option<String>, Vec<String>) {
    let ended = lines
        .iter()
        .position(|line| line.starts_with(STEP_FAILED))
        .map_or(lines.len(), |at| at + 1);
    let lines: Vec<String> = lines[..ended].iter().map(|line| plain(line)).collect();
    let mut around = BTreeSet::new();
    for (at, line) in lines.iter().enumerate() {
        if says_failed(line) && !is_noise(line) {
            around.extend(at.saturating_sub(1)..(at + 4).min(lines.len()));
        }
    }
    let mut excerpt = String::new();
    let mut last = None;
    let mut named = Vec::new();
    for at in around {
        let line = &lines[at];
        for path in paths_in(line) {
            if named.len() < MAX_NAMED && !named.contains(&path) {
                named.push(path);
            }
        }
        if is_noise(line) || excerpt.len() + line.len() > EXCERPT_BYTES {
            continue;
        }
        if last.is_some_and(|last| last + 1 != at) {
            excerpt.push_str("…\n");
        }
        excerpt.push_str(line.trim_end());
        excerpt.push('\n');
        last = Some(at);
    }
    let excerpt = excerpt.trim_end();
    ((!excerpt.is_empty()).then(|| excerpt.to_owned()), named)
}

/// The file paths `line` names, without their line and column, leading `./`
/// and `../`, and paths into installed dependencies.
fn paths_in(line: &str) -> Vec<String> {
    line.split(|c: char| c.is_whitespace() || "\"'`()[]{}<>,;|│╭╰─".contains(c))
        .filter_map(|token| {
            let token = token.strip_prefix("file://").unwrap_or(token);
            if token.contains("://") || DEPENDENCIES.iter().any(|at| token.contains(at)) {
                return None;
            }
            let mut path = token.trim_end_matches(['.', ':']);
            while let Some((rest, tail)) = path.rsplit_once(':') {
                if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
                    break;
                }
                path = rest;
            }
            while let Some(rest) = path.strip_prefix("./").or_else(|| path.strip_prefix("../")) {
                path = rest;
            }
            let (stem, extension) = path.rsplit_once('.')?;
            let named = !stem.is_empty()
                && !stem.ends_with('/')
                && (1..=6).contains(&extension.len())
                && extension.starts_with(|c: char| c.is_ascii_alphabetic())
                && extension.chars().all(|c| c.is_ascii_alphanumeric());
            named.then(|| path.to_owned())
        })
        .collect()
}

pub(crate) fn failed_tests(lines: &[String]) -> Vec<String> {
    let mut tests: Vec<String> = Vec::new();
    for test in lines
        .iter()
        .filter_map(|line| failed_test(plain(line).trim()))
    {
        if tests.len() >= MAX_TESTS {
            break;
        }
        if !tests.contains(&test) {
            tests.push(test);
        }
    }
    tests
}

/// The test a log line says failed, from lines such as `test a::b ... FAILED`
/// (cargo), `FAIL [0.1s] crate a::b` (nextest), `--- FAIL: TestA (0.1s)` (Go),
/// `FAILED t.py::test_a - Error` (pytest) and `FAIL a.test.ts > suite > a`
/// (Vitest).
fn failed_test(line: &str) -> Option<String> {
    let test = if let Some(rest) = line.strip_prefix("test ") {
        rest.strip_suffix(" ... FAILED")?
    } else if let Some(rest) = line.strip_prefix("--- FAIL: ") {
        rest.split_whitespace().next()?
    } else if let Some(rest) = line.strip_prefix("FAILED ") {
        rest.split(" - ")
            .next()
            .filter(|test| !test.starts_with('('))?
    } else {
        let rest = line.strip_prefix("FAIL ")?.trim_start();
        match rest.strip_prefix('[') {
            Some(timed) => timed.split_once(']')?.1.split_whitespace().last()?,
            None if rest.contains(" > ") => rest,
            None => return None,
        }
    };
    let test = test.trim();
    (!test.is_empty()).then(|| test.to_owned())
}
