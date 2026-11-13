//! Brings what an older core left in the data folder up to the shape this
//! core reads, one format at a time, before anything in it is read.

use std::path::Path;

use serde_json::{json, Value};
use sikemux_process::files::write_atomic;

use super::ServerConfig;

const FILE: &str = "data-format.json";

type Step = fn(&Path, &ServerConfig) -> std::io::Result<()>;

/// Each step takes the folder from the format of its index to the next.
const STEPS: [Step; 4] = [
    prompts_say_whether_shown,
    host_key_named_by_socket_path,
    activity_database_in_the_data_folder,
    typed_answers_scrubbed,
];

pub(crate) fn upgrade(config: &ServerConfig) {
    let Some(data_dir) = config.data_dir.as_deref() else {
        return;
    };
    let file = data_dir.join(FILE);
    let from = match std::fs::read(&file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        read => match read
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|format| format["format"].as_u64())
        {
            Some(format) => format as usize,
            None => {
                eprintln!(
                    "sikemux core: {} names no format; the data folder is left as it is",
                    file.display()
                );
                return;
            }
        },
    };
    if from > STEPS.len() {
        eprintln!(
            "sikemux core: the data folder is in format {from}, newer than this core's {}; it is left as it is",
            STEPS.len()
        );
        return;
    }
    for (format, step) in STEPS.iter().enumerate().skip(from) {
        let done = step(data_dir, config).and_then(|()| {
            let next = json!({ "format": format + 1 });
            write_atomic(&file, next.to_string().as_bytes(), true)
        });
        if let Err(error) = done {
            eprintln!(
                "sikemux core: the data folder was not brought past format {format}: {error}"
            );
            return;
        }
        eprintln!(
            "sikemux core: the data folder is now in format {}",
            format + 1
        );
    }
}

/// The app kept the activity database before the core did: it moves over, for the core that
/// shares the app's data folder.
fn activity_database_in_the_data_folder(data_dir: &Path, _: &ServerConfig) -> std::io::Result<()> {
    if data_dir.file_name() != Some(sikemux_process::names::APP_IDENTIFIER.as_ref()) {
        return Ok(());
    }
    let path = data_dir.join(super::activity_log::DATABASE);
    let Some(old) = sikemux_process::paths::config_file("activity", "sqlite3") else {
        return Ok(());
    };
    if path.exists() || !old.exists() {
        return Ok(());
    }
    for suffix in ["", "-wal", "-shm"] {
        let from = std::path::PathBuf::from(format!("{}{suffix}", old.display()));
        if from.exists() {
            let _ = std::fs::rename(&from, format!("{}{suffix}", path.display()));
        }
    }
    Ok(())
}

/// Format 0 cores named the client a prompt to send again came from, or
/// none once every client had been shown it; now it is whether they have not.
fn prompts_say_whether_shown(data_dir: &Path, _: &ServerConfig) -> std::io::Result<()> {
    if let Ok(entries) = std::fs::read_dir(data_dir.join("chats")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                rewrite(&path, |chat| {
                    ["prompt", "offline"]
                        .into_iter()
                        .fold(false, |changed, key| {
                            chat.get_mut(key).is_some_and(announced_as_flag) | changed
                        })
                })?;
            }
        }
    }
    rewrite(&data_dir.join("limit-resumes.json"), |waiting| {
        let entries = waiting.as_array_mut().into_iter().flatten();
        entries.fold(false, |changed, entry| {
            entry.get_mut("prompt").is_some_and(announced_as_flag) | changed
        })
    })
}

/// Format 1 cores kept the host's key in the secret store under the socket's
/// file name, which a core started with another HOME overwrote.
fn host_key_named_by_socket_path(_: &Path, config: &ServerConfig) -> std::io::Result<()> {
    if !config.secret_store {
        return Ok(());
    }
    super::remote::rename_host_key(&config.socket)
        .map_err(|error| std::io::Error::other(error.to_string()))
}

/// Turns the client a prompt to send again came from into whether clients
/// have not been shown it, saying whether it changed anything.
pub(super) fn announced_as_flag(replay: &mut Value) -> bool {
    match replay.get_mut("announce") {
        Some(announce) if !announce.is_boolean() => {
            *announce = Value::Bool(!announce.is_null());
            true
        }
        _ => false,
    }
}

/// Format 3 cores recorded what the person typed while a secret or an access call waited,
/// marked as hidden, though the words may be a password or a token. Now such words are never
/// recorded: the prompts kept that way in the chats' records lose their words and the mark, and
/// a prompt kept to send again that holds such words is dropped, from the chats and from those
/// waiting for a usage limit to reset. A file that cannot be read or written is left as it is,
/// and said so; a line that is not a record, such as one that is not text, is left as it is.
fn typed_answers_scrubbed(data_dir: &Path, _: &ServerConfig) -> std::io::Result<()> {
    let left = |path: &Path, error: std::io::Error| {
        eprintln!(
            "sikemux core: {} was left as it is, and may still hold words typed for a private call: {error}",
            path.display()
        );
    };
    for path in files_in(&data_dir.join("chat-history"), "jsonl") {
        if let Err(error) = scrub_lines(&path) {
            left(&path, error);
        }
    }
    for path in files_in(&data_dir.join("chats"), "json") {
        let dropped = rewrite(&path, |chat| {
            ["prompt", "offline"]
                .into_iter()
                .fold(false, |changed, key| {
                    let marked = chat.get(key).is_some_and(hidden);
                    if marked {
                        chat[key] = Value::Null;
                    }
                    marked | changed
                })
        });
        if let Err(error) = dropped {
            left(&path, error);
        }
    }
    let resumes = data_dir.join("limit-resumes.json");
    let dropped = rewrite(&resumes, |waiting| {
        let Some(entries) = waiting.as_array_mut() else {
            return false;
        };
        let before = entries.len();
        entries.retain(|entry| !hidden(&entry["prompt"]));
        entries.len() != before
    });
    if let Err(error) = dropped {
        left(&resumes, error);
    }
    Ok(())
}

/// The files directly in `dir` with `extension`.
fn files_in(dir: &Path, extension: &str) -> Vec<std::path::PathBuf> {
    (std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|found| found == extension))
        .collect()
}

/// Whether `value` holds words typed while a call whose words are never kept waited.
fn hidden(value: &Value) -> bool {
    value.get("answered").and_then(Value::as_str) == Some("hiddenCall")
}

/// Scrubs each line of the record at `path` that holds hidden words, in place and at the same
/// length, so a core taking the chat over reads on from where the last one stopped.
fn scrub_lines(path: &Path) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let bytes = std::fs::read(path)?;
    let mut scrubbed = Vec::new();
    let mut at = 0;
    for line in bytes.split(|&byte| byte == b'\n') {
        if let Ok(mut value) = serde_json::from_slice::<Value>(line) {
            match scrub_line(&mut value).then(|| value.to_string().into_bytes()) {
                Some(mut kept) if kept.len() <= line.len() => {
                    kept.resize(line.len(), b' ');
                    scrubbed.push((at, kept));
                }
                Some(_) => eprintln!(
                    "sikemux core: a line at byte {at} of {} was left as it is, and may still hold words typed for a private call: it would not fit in its place",
                    path.display()
                ),
                None => {}
            }
        }
        at += line.len() as u64 + 1;
    }
    if scrubbed.is_empty() {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
    for (at, kept) in scrubbed {
        file.seek(SeekFrom::Start(at))?;
        file.write_all(&kept)?;
    }
    file.sync_all()
}

/// Empties the words of a recorded prompt marked as typed while a call whose words are never
/// kept waited, and drops the mark. Says whether it was one.
fn scrub_line(line: &mut Value) -> bool {
    let Some(payload) = (line.pointer_mut("/event/payload")).filter(|payload| hidden(payload))
    else {
        return false;
    };
    scrub_words(payload);
    true
}

/// Empties the words of a message and drops its mark.
fn scrub_words(message: &mut Value) {
    if let Some(fields) = message.as_object_mut() {
        fields.remove("answered");
        fields.insert("text".to_owned(), Value::String(String::new()));
    }
}

/// Scrubs what a core handing over carried in `value` of words typed while a call whose words
/// are never kept waited: a prompt to send again that holds them is dropped, and any other
/// message loses its words and the mark.
pub(super) fn scrub_carried(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for field in fields.values_mut() {
                match hidden(field) && field.get("announce").is_some() {
                    true => *field = Value::Null,
                    false => scrub_carried(field),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(scrub_carried),
        _ => {}
    }
    if hidden(value) {
        scrub_words(value);
    }
}

/// Changes the JSON file at `path` in place, when it is there, holds JSON and
/// `change` says it changed it.
fn rewrite(path: &Path, change: impl FnOnce(&mut Value) -> bool) -> std::io::Result<()> {
    let bytes = match std::fs::read(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        read => read?,
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(());
    };
    if !change(&mut value) {
        return Ok(());
    }
    write_atomic(path, value.to_string().as_bytes(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QA 2026-11-13: a passphrase typed while an access call waited stayed in the chat's record,
    /// marked hidden, for anything that stopped reading the mark to quote, and in prompts kept to
    /// send again. Read in #228: the record is scrubbed in place, so a core taking the chat over
    /// reads on at the same place, and one file that is not text stops nothing.
    #[test]
    fn words_kept_as_hidden_are_scrubbed_in_place_and_nothing_else_moves() {
        let data = tempfile::tempdir().expect("temp dir");
        let history = data.path().join("chat-history");
        let chats = data.path().join("chats");
        std::fs::create_dir_all(&history).expect("history");
        std::fs::create_dir_all(&chats).expect("chats");
        std::fs::write(history.join("broken.jsonl"), [0xff, 0xfe, b'\n', 0x80]).expect("broken");
        let hidden = json!({ "index": 4, "seq": 0, "startsTurn": true, "point": false, "event": {
            "kind": "prompt",
            "payload": { "text": "the passphrase is correct horse battery staple", "person": true, "answered": "hiddenCall" },
        }});
        let said = json!({ "index": 5, "seq": 0, "startsTurn": true, "point": false, "event": {
            "kind": "prompt",
            "payload": { "text": "run the vault tests", "person": true },
        }});
        let record = history.join("feed.jsonl");
        let written = format!("{hidden}\n{said}\n");
        std::fs::write(&record, &written).expect("record");
        let chat = chats.join("agent.json");
        let resend = json!({ "prompt": { "announce": false, "text": "the key is swordfish", "person": true, "answered": "hiddenCall" }, "offline": null });
        std::fs::write(&chat, resend.to_string()).expect("chat");
        let resumes = data.path().join("limit-resumes.json");
        let waiting = json!([
            { "agentId": "a", "at": 1, "prompt": { "announce": false, "text": "the key is swordfish", "answered": "hiddenCall" } },
            { "agentId": "b", "at": 2, "prompt": { "announce": false, "text": "carry on" } },
        ]);
        std::fs::write(&resumes, waiting.to_string()).expect("resumes");

        typed_answers_scrubbed(data.path(), &ServerConfig::new(std::path::PathBuf::new()))
            .expect("scrubbed");

        let scrubbed = std::fs::read_to_string(&record).expect("record");
        assert_eq!(scrubbed.len(), written.len(), "{scrubbed}");
        let second = written.find('\n').expect("a line") + 1;
        assert_eq!(scrubbed[second..], written[second..]);
        assert!(
            !scrubbed.contains("correct horse") && !scrubbed.contains("hiddenCall"),
            "{scrubbed}"
        );
        let first: Value =
            serde_json::from_str(scrubbed.lines().next().expect("a line")).expect("a record");
        assert_eq!(first["event"]["payload"]["text"], json!(""));
        let chat = std::fs::read_to_string(&chat).expect("chat");
        assert!(!chat.contains("swordfish"), "{chat}");
        let resumes = std::fs::read_to_string(&resumes).expect("resumes");
        assert!(
            !resumes.contains("swordfish") && resumes.contains("carry on"),
            "{resumes}"
        );
    }
}
