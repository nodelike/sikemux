//! Brings what an older core left in the data folder up to the shape this
//! core reads, one format at a time, before anything in it is read.

use std::path::Path;

use serde_json::{json, Value};
use sikemux_process::files::write_atomic;

use super::ServerConfig;

const FILE: &str = "data-format.json";

type Step = fn(&Path, &ServerConfig) -> std::io::Result<()>;

/// Each step takes the folder from the format of its index to the next.
const STEPS: [Step; 2] = [prompts_say_whether_shown, host_key_named_by_socket_path];

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
