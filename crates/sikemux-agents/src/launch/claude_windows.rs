//! How big each Claude model's context window is on each account, as its turns
//! last reported it. Claude Code's list of models does not say, and plans differ.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use sikemux_process::files::write_atomic_unsynced;
use sikemux_process::sync::Locked;

use super::claude_model;

const FILE: &str = "claude-windows.json";

type Windows = BTreeMap<String, BTreeMap<String, u64>>;

static PLACE: OnceLock<PathBuf> = OnceLock::new();

/// Keeps what is learned in `data_dir`. Without it nothing is remembered past the chat.
pub fn open(data_dir: &Path) {
    let _ = PLACE.set(data_dir.join(FILE));
}

fn known() -> &'static Mutex<Windows> {
    static KNOWN: OnceLock<Mutex<Windows>> = OnceLock::new();
    KNOWN.get_or_init(|| {
        let read = PLACE
            .get()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Mutex::new(read.unwrap_or_default())
    })
}

fn key(account: &Path) -> String {
    account.to_string_lossy().into_owned()
}

/// The window `model`, by its full id, last ran with on the account kept in `account`.
pub fn of(account: &Path, model: &str) -> Option<u64> {
    known()
        .locked()
        .get(&key(account))?
        .get(claude_model::api_name(model))
        .copied()
}

pub fn learn(account: &Path, model: &str, size: u64) {
    let mut known = known().locked();
    let models = known.entry(key(account)).or_default();
    if models.insert(model.to_owned(), size) == Some(size) {
        return;
    }
    if let (Some(path), Ok(bytes)) = (PLACE.get(), serde_json::to_vec_pretty(&*known)) {
        let _ = write_atomic_unsynced(path, &bytes, true);
    }
}
