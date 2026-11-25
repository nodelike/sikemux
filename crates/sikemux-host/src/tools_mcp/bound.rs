//! Keeps every answer an agent reads under [`BUDGET`] bytes, cutting the longest
//! texts and lists first. Each cut is marked where it was made, and `omitted` names what
//! went, how to ask for less, and the file that holds the whole answer.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_core::cut::MARK;
use sikemux_process::files::write_atomic_unsynced;
use sikemux_process::sync::Locked;

/// About 2.5k tokens: keeps MCP tool answers near 10k characters, with the whole
/// JSON saved to a file the agent can read.
pub(super) const BUDGET: usize = 10 * 1024;

/// What is kept of a text, at least, however far over the budget an answer is.
const LEAST_TEXT: usize = 400;
const MOST_CUTS: usize = 24;
/// Room kept for the `omitted` notes themselves.
const NOTES: usize = 768;
/// Whole answers kept for the agent to read; the oldest goes when there is one more.
const MOST_KEPT: usize = 20;
const KEPT_FOLDER: &str = "sikemux-answers-";
/// How old another run's folder is before it is taken for one left behind.
const LEFT_BEHIND: Duration = Duration::from_secs(24 * 60 * 60);

/// Arguments that ask for less, in the order the hint names them.
const NARROWING: [&str; 21] = [
    "selector", "query", "search", "grep", "jql", "filter", "filters", "fields", "card", "since",
    "around", "chars", "lines", "tail", "limit", "perPage", "page", "offset", "start", "cursor",
    "next",
];

/// `value` cut to fit the budget, with `omitted` saying what was left out. `takes`
/// says whether the tool takes an argument, for the hint on how to ask for less.
pub(super) fn bounded(mut value: Value, takes: impl Fn(&str) -> bool) -> Value {
    if size(&value) <= BUDGET {
        return value;
    }
    let whole = keep(&value);
    let mut omitted = Vec::new();
    for _ in 0..MOST_CUTS {
        let over = (size(&value) + NOTES).saturating_sub(BUDGET);
        if over == 0 {
            break;
        }
        match cut(&mut value, over, String::new()) {
            Some(note) => omitted.push(note),
            None => break,
        }
    }
    if size(&value) > BUDGET {
        let text = value.to_string();
        omitted = vec![format!("all after {BUDGET} of {} bytes", text.len())];
        value = Value::String(format!("{}{MARK}", cut_text(&text, BUDGET)));
    }
    omitted.push(match whole {
        Ok(path) => format!(
            "the whole answer, as JSON, is in {}; read it there before sending any of it back changed",
            path.display()
        ),
        Err(error) => format!(
            "the whole answer could not be saved to a file ({error}), so send none of it back changed"
        ),
    });
    let narrowing: Vec<&str> = NARROWING.into_iter().filter(|name| takes(name)).collect();
    if !narrowing.is_empty() {
        omitted.push(format!(
            "to see what was cut, call again narrowed with {}",
            narrowing.join(", ")
        ));
    }
    match value {
        Value::Object(mut fields) => {
            let mut notes: Vec<Value> = match fields.remove("omitted") {
                Some(Value::Array(notes)) => notes,
                Some(note) => vec![note],
                None => Vec::new(),
            };
            notes.extend(omitted.into_iter().map(Value::String));
            fields.insert("omitted".into(), notes.into());
            Value::Object(fields)
        }
        other => json!({ "value": other, "omitted": omitted }),
    }
}

/// An error's words, cut to the budget.
pub(super) fn bounded_text(text: &str) -> String {
    if text.len() <= BUDGET {
        return text.to_owned();
    }
    format!(
        "{}\n({} more bytes left out)",
        cut_text(text, BUDGET),
        text.len() - BUDGET
    )
}

static KEPT: LazyLock<Kept> = LazyLock::new(|| Kept {
    folder: std::env::temp_dir().join(format!("{KEPT_FOLDER}{}", uuid::Uuid::new_v4().simple())),
    files: Mutex::default(),
});

struct Kept {
    folder: PathBuf,
    files: Mutex<VecDeque<PathBuf>>,
}

/// Saves a whole answer to a file only this user can read.
fn keep(value: &Value) -> io::Result<PathBuf> {
    let mut files = KEPT.files.locked();
    if files.is_empty() {
        clear_left_behind();
    }
    let name = format!("{}.json", uuid::Uuid::new_v4().simple());
    let path = KEPT.folder.join(name);
    write_atomic_unsynced(&path, &serde_json::to_vec_pretty(value)?, true)?;
    files.push_back(path.clone());
    if files.len() > MOST_KEPT {
        if let Some(oldest) = files.pop_front() {
            let _ = fs::remove_file(oldest);
        }
    }
    Ok(path)
}

/// Removes the answers kept for an agent that is gone.
pub(super) fn forget() {
    let _ = fs::remove_dir_all(&KEPT.folder);
}

fn clear_left_behind() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let old = || entry.metadata().ok()?.modified().ok()?.elapsed().ok();
        if entry.file_name().to_string_lossy().starts_with(KEPT_FOLDER)
            && old().is_some_and(|age| age > LEFT_BEHIND)
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}

/// Cuts at least `over` bytes from where most of `value`'s bytes are, and says what
/// went. A list holding one item bigger than all its others is cut inside that item.
fn cut(value: &mut Value, over: usize, path: String) -> Option<String> {
    match value {
        Value::String(text) => {
            let length = text.chars().count();
            let keep = text.len().saturating_sub(over + MARK.len()).max(LEAST_TEXT);
            if keep + MARK.len() >= text.len() {
                return None;
            }
            let start = cut_text(text, keep);
            let kept = start.chars().count();
            *text = format!("{start}{MARK}");
            Some(format!(
                "{} after {kept} of {length} characters",
                name(&path, "text")
            ))
        }
        Value::Object(fields) => {
            let (key, _) = biggest(fields.iter().map(|(key, value)| (key.clone(), value)))?;
            let inner = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            cut(fields.get_mut(&key)?, over, inner)
        }
        Value::Array(items) => {
            let marked = items.pop_if(|last| last == MARK);
            let total: usize = items.iter().map(size).sum();
            let (index, largest) = biggest(items.iter().enumerate())?;
            if items.len() == 1 || largest * 2 > total {
                let note = cut(items.get_mut(index)?, over, format!("{path}[{index}]"));
                items.extend(marked);
                return note;
            }
            let count = items.len();
            let mut dropped = 0;
            while items.len() > 1 && dropped < over + MARK.len() + 3 {
                dropped += items.pop().map_or(0, |item| size(&item) + 1);
            }
            let left = items.len();
            items.push(MARK.into());
            Some(format!(
                "{} of {count} {} after the first {left}",
                count - left,
                name(&path, "items"),
            ))
        }
        _ => None,
    }
}

fn biggest<'a, K>(entries: impl Iterator<Item = (K, &'a Value)>) -> Option<(K, usize)> {
    entries
        .map(|(key, value)| (key, size(value)))
        .max_by_key(|(_, size)| *size)
}

fn name<'a>(path: &'a str, fallback: &'a str) -> &'a str {
    if path.is_empty() {
        fallback
    } else {
        path
    }
}

/// The start of `text`, at most `bytes` long, ending on a whole character.
fn cut_text(text: &str, bytes: usize) -> &str {
    text.get(..text.floor_char_boundary(bytes)).unwrap_or(text)
}
