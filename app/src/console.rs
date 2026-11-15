//! What the app prints to stderr, kept as it goes so an agent can read it with `app_console`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sikemux_process::sync::Locked;

const MAX_ENTRIES: usize = 500;
const MAX_TEXT: usize = 2000;
const READ_BUDGET: usize = 24000;

struct Entry {
    level: &'static str,
    at: DateTime<Utc>,
    text: String,
}

static ENTRIES: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

/// Sends stderr through a pipe whose reader keeps each line and still prints it.
pub fn install() {
    let Ok((lines, pipe)) = std::io::pipe() else {
        return;
    };
    let Some(mut printed) = platform::redirect(pipe) else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("console".into())
        .spawn(move || {
            for line in BufReader::new(lines).lines() {
                let Ok(line) = line else { break };
                if let Some(printed) = printed.as_mut() {
                    let _ = writeln!(printed, "{line}");
                }
                keep(line);
            }
        });
    if let Err(error) = spawned {
        eprintln!("sikemux: the app's console could not be kept: {error}");
    }
}

fn keep(line: String) {
    let level = if line.starts_with("thread '") && line.contains("panicked") {
        "uncaught"
    } else if line.starts_with("sikemux") {
        sikemux_hostkit::crash_reports::printed_error(&line);
        "error"
    } else {
        "log"
    };
    let text = match line.char_indices().nth(MAX_TEXT) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    };
    let mut entries = ENTRIES.locked();
    if entries.len() == MAX_ENTRIES {
        entries.pop_front();
    }
    entries.push_back(Entry {
        level,
        at: Utc::now(),
        text,
    });
}

/// The latest lines, as `app_console` asks for them: `limit` of them, or only problems with `errors`.
pub fn read(params: &Value) -> Result<Value, String> {
    let limit = match params.get("limit") {
        None | Some(Value::Null) => 20,
        Some(limit) => limit
            .as_u64()
            .filter(|limit| (1..=200).contains(limit))
            .ok_or("limit must be an integer between 1 and 200")? as usize,
    };
    let errors = match params.get("errors") {
        None | Some(Value::Null) => false,
        Some(errors) => errors.as_bool().ok_or("errors must be a boolean")?,
    };
    let entries = ENTRIES.locked();
    let matched: Vec<&Entry> = entries
        .iter()
        .filter(|entry| !errors || entry.level != "log")
        .collect();
    let mut messages: VecDeque<Value> = matched[matched.len().saturating_sub(limit)..]
        .iter()
        .map(|entry| {
            json!({
                "level": entry.level,
                "at": entry.at.to_rfc3339_opts(SecondsFormat::Millis, true),
                "text": entry.text,
            })
        })
        .collect();
    let mut size: usize = messages
        .iter()
        .map(|message| message.to_string().len())
        .sum();
    let mut omitted = 0;
    while messages.len() > 1 && size > READ_BUDGET {
        if let Some(message) = messages.pop_front() {
            size -= message.to_string().len();
        }
        omitted += 1;
    }
    let mut answer = Map::new();
    answer.insert("recorded".into(), entries.len().into());
    answer.insert("matched".into(), matched.len().into());
    answer.insert("messages".into(), Value::Array(messages.into()));
    if omitted > 0 {
        answer.insert(
            "omitted".into(),
            format!("{omitted} older messages left out to keep this short; pass errors to see only problems").into(),
        );
    }
    Ok(Value::Object(answer))
}
