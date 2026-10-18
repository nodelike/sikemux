//! Which agent wrote what in a checkout, as the core works it out from its
//! record of agents' edits.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde_json::{json, Value};
use tauri::State;

use crate::agent_names::call;
use crate::error::AppResult;
use crate::pty::PtyManager;

/// Who wrote each uncommitted line in `repo`, or each staged one when `staged`.
#[tauri::command]
pub async fn git_change_owners(
    manager: State<'_, PtyManager>,
    repo: String,
    staged: bool,
) -> AppResult<Value> {
    let (generation, moved) = match super::status::watched_generation(&repo) {
        Some((key, generation)) => (Some(named(generation)), moved(&key, staged, generation)),
        None => (None, None),
    };
    let client = manager.client().await?;
    call(
        &client,
        "changeOwners".to_owned(),
        json!({ "dir": repo, "staged": staged, "generation": generation, "moved": moved }),
    )
    .await
}

/// The agents each of `commits` has a footer for that checks out, as the core
/// knows them now.
#[tauri::command]
pub async fn git_commit_owners(
    manager: State<'_, PtyManager>,
    repo: String,
    commits: Vec<String>,
) -> AppResult<Value> {
    let client = manager.client().await?;
    call(
        &client,
        "commitOwners".to_owned(),
        json!({ "dir": repo, "commits": commits }),
    )
    .await
}

/// The core's name for the watched state `generation`. Change numbers start
/// again with the app, and the core outlives it.
fn named(generation: u64) -> String {
    static RUN: OnceLock<uuid::Uuid> = OnceLock::new();
    format!("{}:{generation}", RUN.get_or_init(uuid::Uuid::new_v4))
}

/// The paths of the watched checkout `key` that moved since its owners were
/// last asked for, when the core can ask git again of only them.
fn moved(key: &str, staged: bool, generation: u64) -> Option<Value> {
    static ASKED: Mutex<Option<HashMap<(String, bool), u64>>> = Mutex::new(None);
    let since = {
        let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
        let asked = asked.get_or_insert_with(HashMap::new);
        let at = (key.to_owned(), staged);
        if asked.len() >= REMEMBERED_CHECKOUTS && !asked.contains_key(&at) {
            asked.clear();
        }
        asked.insert(at, generation)?
    };
    let repo = git2::Repository::open(key).ok()?;
    let paths = super::status::moved_since(&repo, key, since)?;
    Some(json!({ "since": named(since), "paths": paths }))
}

const REMEMBERED_CHECKOUTS: usize = 32;
