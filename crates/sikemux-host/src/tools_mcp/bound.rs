//! Keeps every answer an agent reads under [`BUDGET`] bytes, cutting the longest
//! texts and lists first. Each cut is marked where it was made, and `omitted` names what
//! went, how to ask for less, and the file that holds the whole answer.

use std::collections::VecDeque;
use std::fmt;
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
    let mut tree = SizeTree::measure(&value);
    if tree.bytes() <= BUDGET {
        return value;
    }
    let whole = keep(&value);
    let mut omitted = Vec::new();
    for _ in 0..MOST_CUTS {
        let over = (tree.bytes() + NOTES).saturating_sub(BUDGET);
        if over == 0 {
            break;
        }
        match cut(&mut value, &mut tree, over, String::new()) {
            Some(note) => omitted.push(note),
            None => break,
        }
    }
    let mut tail_notes = Vec::new();
    tail_notes.push(match whole {
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
        tail_notes.push(format!(
            "to see what was cut, call again narrowed with {}",
            narrowing.join(", ")
        ));
    }

    let final_notes_size = |notes: &[String]| -> usize {
        let items: usize = notes.iter().map(|n| string_size(n)).sum();
        let commas = notes.len().saturating_sub(1);
        let arr = 2 + items + commas;
        if value.is_object() {
            // ,"omitted":[...]
            1 + string_size("omitted") + 1 + arr
        } else {
            // {"value":,"omitted":[...]}
            2 + string_size("value") + 1 + 1 + string_size("omitted") + 1 + arr
        }
    };

    omitted.extend(tail_notes);
    let overhead = final_notes_size(&omitted);
    if tree.bytes() + overhead > BUDGET {
        let room = BUDGET.saturating_sub(overhead + 64);
        let overflow_note = fit_overflow(&mut value, &mut tree, room);
        if let Some(note) = overflow_note {
            omitted.insert(0, note);
        }
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

struct ByteCount(usize);

impl fmt::Write for ByteCount {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 += s.len();
        Ok(())
    }
}

fn string_size(s: &str) -> usize {
    let mut extra = 0;
    for byte in s.bytes() {
        match byte {
            b'"' | b'\\' | b'\x08' | b'\x0c' | b'\n' | b'\r' | b'\t' => extra += 1,
            0x00..=0x1f => extra += 5,
            _ => {}
        }
    }
    2 + s.len() + extra
}

fn number_size(n: &serde_json::Number) -> usize {
    let mut count = ByteCount(0);
    let _ = fmt::Write::write_fmt(&mut count, format_args!("{n}"));
    count.0
}

enum SizeTree {
    Leaf(usize),
    Array(usize, Vec<SizeTree>),
    Object(usize, Vec<(String, usize, SizeTree)>),
}

impl SizeTree {
    fn measure(value: &Value) -> Self {
        match value {
            Value::Null => Self::Leaf(4),
            Value::Bool(true) => Self::Leaf(4),
            Value::Bool(false) => Self::Leaf(5),
            Value::Number(n) => Self::Leaf(number_size(n)),
            Value::String(s) => Self::Leaf(string_size(s)),
            Value::Array(items) => {
                if items.is_empty() {
                    return Self::Array(2, Vec::new());
                }
                let children: Vec<SizeTree> = items.iter().map(Self::measure).collect();
                let total =
                    2 + children.len() - 1 + children.iter().map(Self::bytes).sum::<usize>();
                Self::Array(total, children)
            }
            Value::Object(fields) => {
                if fields.is_empty() {
                    return Self::Object(2, Vec::new());
                }
                let children: Vec<(String, usize, SizeTree)> = fields
                    .iter()
                    .map(|(k, v)| (k.clone(), string_size(k) + 1, Self::measure(v)))
                    .collect();
                let total = 2 + children.len() - 1
                    + children
                        .iter()
                        .map(|(_, key_bytes, child)| key_bytes + child.bytes())
                        .sum::<usize>();
                Self::Object(total, children)
            }
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Leaf(n) | Self::Array(n, _) | Self::Object(n, _) => *n,
        }
    }
}

/// Cuts at least `over` bytes from where most of `value`'s bytes are, and says what
/// went. A list holding one item bigger than all its others is cut inside that item.
fn cut(value: &mut Value, tree: &mut SizeTree, over: usize, path: String) -> Option<String> {
    match (value, tree) {
        (Value::String(text), SizeTree::Leaf(bytes)) => {
            let length = text.chars().count();
            let keep = text.len().saturating_sub(over + MARK.len()).max(LEAST_TEXT);
            if keep + MARK.len() >= text.len() {
                return None;
            }
            let start = cut_text(text, keep);
            let kept = start.chars().count();
            *text = format!("{start}{MARK}");
            *bytes = string_size(text);
            Some(format!(
                "{} after {kept} of {length} characters",
                name(&path, "text")
            ))
        }
        (Value::Object(fields), SizeTree::Object(total, entries)) => {
            if entries.is_empty() {
                return None;
            }
            let mut order: Vec<usize> = (0..entries.len()).collect();
            let entry_bytes = |i: usize| entries.get(i).map_or(0, |(_, _, child)| child.bytes());
            order.sort_unstable_by_key(|&a| std::cmp::Reverse(entry_bytes(a)));
            for idx in order {
                let Some((key, _, child)) = entries.get_mut(idx) else {
                    continue;
                };
                if matches!(child, SizeTree::Leaf(n) if *n <= LEAST_TEXT + MARK.len() + 2) {
                    continue;
                }
                let before = child.bytes();
                let inner = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if let Some(field) = fields.get_mut(key.as_str()) {
                    if let Some(note) = cut(field, child, over, inner) {
                        *total = (*total + child.bytes()).saturating_sub(before);
                        return Some(note);
                    }
                }
            }
            if entries.len() <= 1 {
                return None;
            }
            let count = entries.len();
            let mut dropped = 0;
            while entries.len() > 1 && dropped < over {
                if let Some((key, key_bytes, child)) = entries.pop() {
                    fields.remove(&key);
                    let removed = key_bytes + child.bytes() + 1;
                    dropped += removed;
                    *total = total.saturating_sub(removed);
                }
            }
            let left = entries.len();
            Some(format!(
                "{} of {count} {} fields after the first {left}",
                count - left,
                name(&path, "object"),
            ))
        }
        (Value::Array(items), SizeTree::Array(total, children)) => {
            let mark_bytes = string_size(MARK);
            let marked = items.pop_if(|last| last == MARK);
            if marked.is_some() {
                children.pop();
                *total = if children.is_empty() {
                    2
                } else {
                    total.saturating_sub(mark_bytes + 1)
                };
            }
            let sum: usize = children.iter().map(SizeTree::bytes).sum();
            let Some((index, largest)) = children
                .iter()
                .enumerate()
                .map(|(i, child)| (i, child.bytes()))
                .max_by_key(|(_, bytes)| *bytes)
            else {
                if let Some(mark) = marked {
                    items.push(mark);
                    children.push(SizeTree::Leaf(mark_bytes));
                    *total = 2 + mark_bytes;
                }
                return None;
            };
            if items.len() == 1 || largest * 2 > sum {
                let note = if let Some((item, child)) =
                    items.get_mut(index).zip(children.get_mut(index))
                {
                    let before = child.bytes();
                    let note = cut(item, child, over, format!("{path}[{index}]"));
                    *total = (*total + child.bytes()).saturating_sub(before);
                    note
                } else {
                    None
                };
                if let Some(mark) = marked {
                    let add = if children.is_empty() {
                        mark_bytes
                    } else {
                        mark_bytes + 1
                    };
                    items.push(mark);
                    children.push(SizeTree::Leaf(mark_bytes));
                    *total += add;
                }
                return note;
            }
            let count = items.len();
            let mut dropped = 0;
            while items.len() > 1 && dropped < over + mark_bytes + 1 {
                items.pop();
                if let Some(child) = children.pop() {
                    let removed = child.bytes() + 1;
                    dropped += removed;
                    *total = total.saturating_sub(removed);
                }
            }
            let left = items.len();
            items.push(MARK.into());
            children.push(SizeTree::Leaf(mark_bytes));
            *total += mark_bytes + 1;
            Some(format!(
                "{} of {count} {} after the first {left}",
                count - left,
                name(&path, "items"),
            ))
        }
        _ => None,
    }
}

fn fit_overflow(value: &mut Value, tree: &mut SizeTree, room: usize) -> Option<String> {
    match (value, tree) {
        (Value::Object(fields), SizeTree::Object(total, entries)) => {
            let count = entries.len();
            while *total > room && !entries.is_empty() {
                if let Some((key, key_bytes, child)) = entries.pop() {
                    fields.remove(&key);
                    *total = if entries.is_empty() {
                        2
                    } else {
                        total.saturating_sub(key_bytes + child.bytes() + 1)
                    };
                }
            }
            let left = entries.len();
            (left < count)
                .then(|| format!("{} of {count} fields omitted to fit budget", count - left))
        }
        (Value::Array(items), SizeTree::Array(total, children)) => {
            let mark_bytes = string_size(MARK);
            let had_mark = items.pop_if(|last| last == MARK).is_some();
            if had_mark {
                children.pop();
                *total = if children.is_empty() {
                    2
                } else {
                    total.saturating_sub(mark_bytes + 1)
                };
            }
            let count = items.len();
            let mark_limit = room.saturating_sub(mark_bytes + 1);
            while *total > mark_limit && !items.is_empty() {
                items.pop();
                if let Some(child) = children.pop() {
                    *total = if children.is_empty() {
                        2
                    } else {
                        total.saturating_sub(child.bytes() + 1)
                    };
                }
            }
            let left = items.len();
            if room >= 2 + mark_bytes {
                let add = if children.is_empty() {
                    mark_bytes
                } else {
                    mark_bytes + 1
                };
                items.push(MARK.into());
                children.push(SizeTree::Leaf(mark_bytes));
                *total += add;
            }
            (left < count)
                .then(|| format!("{} of {count} items omitted to fit budget", count - left))
        }
        (Value::String(text), SizeTree::Leaf(bytes)) => {
            let length = text.chars().count();
            let mut keep = room.saturating_sub(string_size(MARK));
            loop {
                let candidate = format!("{}{MARK}", cut_text(text, keep));
                let sz = string_size(&candidate);
                if sz <= room || keep == 0 {
                    let kept = cut_text(text, keep).chars().count();
                    *text = candidate;
                    *bytes = sz;
                    return Some(format!("text after {kept} of {length} characters"));
                }
                keep = keep.saturating_sub((sz - room).max(1));
            }
        }
        _ => None,
    }
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
