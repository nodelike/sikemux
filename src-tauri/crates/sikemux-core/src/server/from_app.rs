//! Copies into the core, once each, what the app kept in its own files before
//! the core took it over. The steps that finished are recorded in `FILE`; one
//! that could not finish runs again when the core next starts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

use super::kept::Kept;
use super::{plugins, preferences, Core};

const FILE: &str = "carried-from-app.json";
const HOSTS_STEP: &str = "hosts.installedFrom";
const AWS_STEP: &str = "aws.paneProfile";
const AWS_PLUGIN: &str = "sikemux.aws";

/// The app's data folder in this build, which is the core's when the app
/// started it, and the only core that may read the app's files.
const APP_DATA_DIR: &str = if cfg!(debug_assertions) {
    "com.nodelike.sikemux.dev"
} else {
    "com.nodelike.sikemux"
};

pub(super) fn app_data_dir(data_dir: Option<&Path>) -> Option<&Path> {
    data_dir.filter(|dir| {
        !sikemux_process::paths::TRYOUT && dir.file_name() == Some(APP_DATA_DIR.as_ref())
    })
}

fn app_state_file() -> Option<PathBuf> {
    sikemux_process::paths::config_file("state", "sqlite3")
}

/// What the app last saved of its workspace, when this core may read it.
pub(super) fn app_state(data_dir: Option<&Path>) -> Option<Value> {
    app_data_dir(data_dir)?;
    let path = app_state_file()?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let json: String = connection
        .query_row(
            "SELECT core_json FROM workspace_snapshots WHERE slot = 0",
            [],
            |row| row.get(0),
        )
        .ok()?;
    serde_json::from_str(&json).ok()
}

pub(super) fn carry(core: &Arc<Core>, data_dir: Option<&Path>) {
    let Some(dir) = app_data_dir(data_dir) else {
        return;
    };
    let record = Kept::new(Some(dir), FILE);
    let mut done: BTreeSet<String> = record.load();
    let preference_steps: Vec<(String, String)> = preferences::keys()
        .into_iter()
        .map(|key| (format!("preferences.{key}"), key))
        .collect();
    let pending = preference_steps
        .iter()
        .map(|(step, _)| step.as_str())
        .chain([HOSTS_STEP, AWS_STEP])
        .any(|step| !done.contains(step));
    if !pending {
        return;
    }
    let app = match app_state(data_dir) {
        Some(app) => app,
        None if app_state_file().is_some_and(|file| file.exists()) => return,
        None => Value::Null,
    };
    let prefs = &app["prefs"];
    for (step, key) in preference_steps {
        if done.contains(&step) {
            continue;
        }
        if let Some(value) = prefs.get(&key) {
            let _ = core.preferences.set(&key, value.clone());
        }
        done.insert(step);
    }
    if !done.contains(HOSTS_STEP) {
        core.hosts.carry_installed_from(installed_from(dir));
        done.insert(HOSTS_STEP.to_owned());
    }
    let aws_profile = prefs["pluginSettings"][AWS_PLUGIN]["profile"]
        .as_str()
        .filter(|profile| !profile.trim().is_empty())
        .map(str::to_owned);
    if aws_profile.is_none() {
        done.insert(AWS_STEP.to_owned());
    }
    record.save(&done);
    let (Some(profile), false) = (aws_profile, done.contains(AWS_STEP)) else {
        return;
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        let picked = plugins::call(
            AWS_PLUGIN,
            "setDefaultAccount",
            json!({ "profile": profile }),
        )
        .await;
        if picked.is_ok() {
            done.insert(AWS_STEP.to_owned());
            record.save(&done);
        }
    });
}

/// The SSH host each host was installed on from here, from the app's
/// `hosts.json`.
fn installed_from(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read(dir.join("hosts.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|file| serde_json::from_value(file["ssh"].clone()).ok())
        .unwrap_or_default()
}
