//! `sikemux triage`: gathers Sikemux's logs with secrets taken out and starts the person's
//! own coding agent on them, with a playbook for finding what went wrong.

use std::io::Read;
use std::time::SystemTime;

use serde_json::{json, Value};

use super::*;

const PLAYBOOK: &str = include_str!("../../../../.github/triage/PLAYBOOK.md");
const AGENTS: [&str; 2] = ["claude", "codex"];
const LOG_TAIL_BYTES: u64 = 256 * 1024;
const REPORTS_KEPT: usize = 6;
const REPORT_BYTES: u64 = 512 * 1024;
const HIDDEN: &str = "[redacted]";
/// Words that, ending a key, mark the value after it as a secret.
const SECRET_KEYS: [&str; 8] = [
    "token",
    "secret",
    "password",
    "passwd",
    "key",
    "authorization",
    "cookie",
    "ticket",
];

pub(super) fn triage(args: &[String]) -> Result<i32, String> {
    let mut agent = None;
    let mut print = false;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--agent" => {
                let name = words.next().ok_or("--agent needs claude or codex")?;
                if !AGENTS.contains(&name.as_str()) {
                    return Err(format!("--agent takes claude or codex, not {name}"));
                }
                agent = Some(name.as_str());
            }
            "--print" => print = true,
            other => return Err(format!("triage takes --agent and --print, not {other}")),
        }
    }
    let folder = gather()?;
    let prompt = format!(
        "Read PLAYBOOK.md in {} and follow it to find what is wrong with my Sikemux.",
        folder.display()
    );
    if print {
        println!("{}\n\n{prompt}", folder.display());
        return Ok(0);
    }
    let agent = match agent {
        Some(agent) => agent,
        None => AGENTS
            .into_iter()
            .find(|agent| on_path(agent))
            .ok_or("neither claude nor codex is on PATH; install one, or run `sikemux triage --print` and hand the folder to another agent")?,
    };
    eprintln!("Logs gathered in {}", folder.display());
    eprintln!("Starting {agent}. Describe what went wrong when it asks.");
    let status = sikemux_process::user_environment::command(agent)
        .arg(&prompt)
        .current_dir(&folder)
        .status()
        .map_err(|error| format!("could not start {agent}: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

fn on_path(program: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|directory| directory.join(program).is_file())
    })
}

fn gather() -> Result<PathBuf, String> {
    let home = sikemux_process::paths::home().ok_or("home directory is unavailable")?;
    let config = sikemux_process::paths::config_dir().ok_or("home directory is unavailable")?;
    let data = crate::login_item::data_dir().ok_or("home directory is unavailable")?;
    let logs = crate::login_item::headless_launch()
        .and_then(|launch| launch.log.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| data.join("logs"));
    let autopsies = crate::hang_reports::autopsy_dir().ok_or("home directory is unavailable")?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let folder = env::temp_dir().join(format!("sikemux-triage-{stamp}"));
    let copies = folder.join("logs");
    private_dir(&copies)?;
    let scrub = |text: &str| redact(text, &home);
    let mut gathered = Vec::new();
    for log in ["core.log", "notch.log"] {
        let source = logs.join(log);
        if let Some(text) = read_tail(&source, LOG_TAIL_BYTES) {
            write_private(&copies.join(log), &scrub(&text))?;
            gathered.push(log.to_owned());
        }
    }
    let reports = copies.join("reports");
    for source in newest(&autopsies, REPORTS_KEPT) {
        let Some(name) = source.file_name() else {
            continue;
        };
        let files = if source.is_dir() {
            fs::read_dir(&source)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![source.clone()]
        };
        for file in files.into_iter().filter(|file| file.is_file()) {
            let Some(text) = read_tail(&file, REPORT_BYTES) else {
                continue;
            };
            let target = if source.is_dir() {
                reports
                    .join(name)
                    .join(file.file_name().unwrap_or_default())
            } else {
                reports.join(name)
            };
            if let Some(parent) = target.parent() {
                private_dir(parent)?;
            }
            write_private(&target, &scrub(&text))?;
        }
        gathered.push(format!("reports/{}", name.to_string_lossy()));
    }
    let tilde = |path: &Path| scrub(&path.to_string_lossy());
    let never_read: Vec<String> = [
        "agent-secrets",
        "vault.json",
        "account",
        "browser",
        "plugins",
        "chats",
        "chat-history",
        "sessions.sqlite3",
        "memory",
    ]
    .iter()
    .map(|name| tilde(&data.join(name)))
    .chain(
        ["secrets", "mobile.token", "browser"]
            .iter()
            .map(|name| tilde(&config.join(name))),
    )
    .collect();
    let context = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commit": crate::BUILD_COMMIT,
        "os": env::consts::OS,
        "arch": env::consts::ARCH,
        "status": status(),
        "gathered": gathered,
        "paths": {
            "logs": tilde(&logs),
            "reports": tilde(&autopsies),
            "data": tilde(&data),
            "config": tilde(&config),
        },
        "sourceCache": tilde(&config.join("cache").join("triage-source")),
        "neverRead": never_read,
    });
    write_private(
        &folder.join("context.json"),
        &serde_json::to_string_pretty(&context).unwrap_or_default(),
    )?;
    write_private(&folder.join("PLAYBOOK.md"), PLAYBOOK)?;
    Ok(folder)
}

#[cfg(unix)]
fn status() -> Value {
    scrub_value(crate::headless::status_without_identity())
}

#[cfg(not(unix))]
fn status() -> Value {
    Value::Null
}

fn scrub_value(value: Value) -> Value {
    let home = sikemux_process::paths::home().unwrap_or_default();
    serde_json::from_str(&redact(&value.to_string(), &home)).unwrap_or(value)
}

fn newest(directory: &Path, count: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    entries
        .into_iter()
        .take(count)
        .map(|(_, path)| path)
        .collect()
}

fn read_tail(path: &Path, limit: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(limit)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(match length > limit {
        true => text
            .split_once('\n')
            .map_or(text.to_string(), |(_, rest)| rest.to_owned()),
        false => text.into_owned(),
    })
}

fn private_dir(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(path)
        .map_err(|error| format!("could not make {}: {error}", path.display()))
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    std::io::Write::write_all(&mut file, text.as_bytes())
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// `text` with the home folder written `~`, and anything that looks like a token, a key, an
/// email address, or the value of a secret-named field, replaced.
pub(super) fn redact(text: &str, home: &Path) -> String {
    let home = home.to_string_lossy();
    let text = match home.len() > 1 {
        true => text.replace(home.as_ref(), "~"),
        false => text.to_owned(),
    };
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let mut after_key = Key::None;
    for ch in text.chars().chain(['\n']) {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.' | '@' | '+' | '/' | '~') {
            word.push(ch);
            continue;
        }
        if !word.is_empty() {
            let hidden = after_key == Key::Value || looks_secret(&word);
            out.push_str(if hidden { HIDDEN } else { &word });
            let lower = word.to_ascii_lowercase();
            after_key = if lower == "bearer" || lower == "basic" {
                Key::Value
            } else if SECRET_KEYS.iter().any(|key| lower.ends_with(key)) {
                Key::Named
            } else {
                Key::None
            };
            word.clear();
        }
        after_key = match (after_key, ch) {
            (Key::Named, ':' | '=') => Key::Value,
            (Key::Named, '"' | '\'') => Key::Named,
            (Key::Value, ' ' | '"' | '\'' | ':' | '=') => Key::Value,
            _ => Key::None,
        };
        out.push(ch);
    }
    out.pop();
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    None,
    /// A word naming a secret just ended.
    Named,
    /// The next word is a secret's value.
    Value,
}

fn looks_secret(word: &str) -> bool {
    let word = word.trim_end_matches('.');
    if word.contains('@') && word.contains('.') && !word.starts_with('@') {
        return true;
    }
    if word.starts_with("eyJ") && word.matches('.').count() == 2 {
        return true;
    }
    if word.contains('/') || word.len() < 24 {
        return false;
    }
    let digits = word.chars().filter(char::is_ascii_digit).count();
    let letters = word.chars().filter(char::is_ascii_alphabetic).count();
    digits >= 4 && letters >= 4 && digits + letters >= word.len() * 3 / 4
}
