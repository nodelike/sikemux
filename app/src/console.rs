//! What the app prints to stderr, kept as it goes so an agent can read it with `app_console`.
//! Its errors also go to `app.log` beside the core's log, so they outlive the process.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sikemux_process::files::create_private;
use sikemux_process::sync::Locked;

const MAX_ENTRIES: usize = 500;
const MAX_TEXT: usize = 2000;
const READ_BUDGET: usize = 24000;
const LOG: &str = "app.log";
const LOG_BEFORE: &str = "app.1.log";
const PREVIOUS_LOG: &str = "app-previous.log";
const LOG_CAP: u64 = 1 << 20;

#[derive(Clone)]
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
            let mut log = crate::core::logs_dir().and_then(|dir| Log::start(&dir));
            for line in BufReader::new(lines).lines() {
                let Ok(line) = line else { break };
                if let Some(printed) = printed.as_mut() {
                    let _ = writeln!(printed, "{line}");
                }
                let entry = entry(line);
                if let Some(log) = log.as_mut().filter(|_| entry.level != "log") {
                    log.write(&entry);
                }
                keep(entry);
            }
        });
    if let Err(error) = spawned {
        eprintln!("sikemux: the app's console could not be kept: {error}");
    }
}

/// This process's errors on disk. The last process's move to `app-previous.log` when it starts,
/// and past `LOG_CAP` the older half of this one's moves to `app.1.log`.
struct Log {
    dir: PathBuf,
    file: File,
    size: u64,
}

impl Log {
    fn start(dir: &Path) -> Option<Self> {
        sikemux_process::files::private_folder(dir).ok()?;
        let _ = std::fs::rename(dir.join(LOG), dir.join(PREVIOUS_LOG));
        let _ = std::fs::remove_file(dir.join(LOG_BEFORE));
        Some(Self {
            dir: dir.to_path_buf(),
            file: create_private(&dir.join(LOG)).ok()?,
            size: 0,
        })
    }

    fn write(&mut self, entry: &Entry) {
        if self.size > LOG_CAP {
            let _ = std::fs::rename(self.dir.join(LOG), self.dir.join(LOG_BEFORE));
            match create_private(&self.dir.join(LOG)) {
                Ok(file) => self.file = file,
                Err(_) => return,
            }
            self.size = 0;
        }
        let mut line = entry.to_json().to_string();
        line.push('\n');
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.size += line.len() as u64;
        }
    }
}

fn entry(line: String) -> Entry {
    let level = if line.starts_with("thread '") && line.contains("panicked") {
        "uncaught"
    } else if line.starts_with("sikemux") {
        sikemux_hostkit::crash_reports::printed_error(&line);
        "error"
    } else {
        "log"
    };
    let text = match line.char_indices().nth(MAX_TEXT) {
        Some((end, _)) => format!("{}…", line.get(..end).unwrap_or_default()),
        None => line,
    };
    Entry {
        level,
        at: Utc::now(),
        text,
    }
}

fn keep(entry: Entry) {
    let mut entries = ENTRIES.locked();
    if entries.len() == MAX_ENTRIES {
        entries.pop_front();
    }
    entries.push_back(entry);
}

impl Entry {
    fn to_json(&self) -> Value {
        json!({
            "level": self.level,
            "at": self.at.to_rfc3339_opts(SecondsFormat::Millis, true),
            "text": self.text,
        })
    }

    fn of_json(line: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(line).ok()?;
        let level = match value.get("level")?.as_str()? {
            "uncaught" => "uncaught",
            "error" => "error",
            _ => "log",
        };
        Some(Self {
            level,
            at: DateTime::parse_from_rfc3339(value.get("at")?.as_str()?)
                .ok()?
                .with_timezone(&Utc),
            text: value.get("text")?.as_str()?.to_owned(),
        })
    }
}

/// The lines the app's last process wrote to its log before it ended.
fn previous() -> Result<Vec<Entry>, String> {
    let path = crate::core::logs_dir()
        .ok_or("this system has no logs folder")?
        .join(PREVIOUS_LOG);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    Ok(text.lines().filter_map(Entry::of_json).collect())
}

/// The latest lines, as `app_console` asks for them: `limit` of them, or only problems with
/// `errors`, from the app's last process with `previous`.
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
    let previous_run = match params.get("previous") {
        None | Some(Value::Null) => false,
        Some(previous) => previous.as_bool().ok_or("previous must be a boolean")?,
    };
    let entries: Vec<Entry> = if previous_run {
        previous()?
    } else {
        ENTRIES.locked().iter().cloned().collect()
    };
    let matched: Vec<&Entry> = entries
        .iter()
        .filter(|entry| !errors || entry.level != "log")
        .collect();
    let mut messages: VecDeque<Value> = (matched.iter())
        .skip(matched.len().saturating_sub(limit))
        .map(|entry| entry.to_json())
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
