//! Agents keep running in the core while an update restarts the app. What they
//! had open in the app itself, their browser tabs and simulators, is written
//! down as the old app quits and read back once as the new one starts.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_core::protocol::ChatState;
use tauri::{AppHandle, Manager};

use crate::browser::{BrowserManager, SavedStrip};
use crate::pty::PtyManager;
use crate::simulator::{SavedSimulators, SimulatorManager};

/// Older than this, the save is from some earlier restart that never came back.
const FRESH_FOR: Duration = Duration::from_secs(5 * 60);
const CORE_ATTEMPTS: u32 = 30;
const CORE_RETRY: Duration = Duration::from_secs(2);

static UPDATE_INSTALLED: AtomicBool = AtomicBool::new(false);
static RESTARTING: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Handover {
    saved_at: u64,
    browser: Vec<SavedStrip>,
    simulators: SavedSimulators,
}

pub fn note_update_installed() {
    UPDATE_INSTALLED.store(true, Ordering::Release);
}

pub fn note_exit_requested(code: Option<i32>) {
    if code == Some(tauri::RESTART_EXIT_CODE) {
        RESTARTING.store(true, Ordering::Release);
    }
}

/// The app is quitting to start the update it just installed.
pub fn for_update() -> bool {
    UPDATE_INSTALLED.load(Ordering::Acquire) && RESTARTING.load(Ordering::Acquire)
}

fn path(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("update-handover.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Writes the handover for the next start. Without it the simulators kept
/// running would have nobody to shut them down, so on failure quit as usual.
pub fn save(app: &AppHandle) -> std::io::Result<()> {
    write(app).inspect_err(|error| {
        eprintln!("Sikemux could not save agents' tabs and simulators for the update: {error}");
    })
}

fn write(app: &AppHandle) -> std::io::Result<()> {
    let path = path(app).ok_or_else(|| std::io::Error::other("no app data folder"))?;
    let handover = Handover {
        saved_at: now(),
        browser: app
            .try_state::<BrowserManager>()
            .map(|browser| browser.saved_strips())
            .unwrap_or_default(),
        simulators: app
            .try_state::<SimulatorManager>()
            .map(|simulators| simulators.saved())
            .unwrap_or_default(),
    };
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no app data folder"))?;
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(&serde_json::to_vec(&handover)?)?;
    file.persist(&path).map_err(|error| error.error)?;
    Ok(())
}

pub fn restore(app: &AppHandle) {
    let Some(path) = path(app) else { return };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let fresh = serde_json::from_slice::<Handover>(&bytes)
        .ok()
        .filter(|handover| now().saturating_sub(handover.saved_at) <= FRESH_FOR.as_secs());
    let Some(handover) = fresh else {
        adopt_booted(app, &bytes);
        return;
    };
    let tabs = app
        .try_state::<BrowserManager>()
        .map(|browser| browser.reopen(app, handover.browser))
        .unwrap_or_default();
    let devices = app
        .try_state::<SimulatorManager>()
        .map(|simulators| simulators.restore(handover.simulators))
        .unwrap_or_default();
    if tabs.is_empty() && devices.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        forget_departed(&app, tabs, devices).await;
    });
}

/// A save too old or too damaged to restore still names the simulators the
/// last run booted, which nobody else will ever shut down.
fn adopt_booted(app: &AppHandle, bytes: &[u8]) {
    let Ok(saved) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    let booted: Vec<String> = saved["simulators"]["booted"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|udid| udid.as_str().map(str::to_owned))
        .collect();
    if let Some(simulators) = app.try_state::<SimulatorManager>() {
        simulators.adopt_booted(booted);
    }
}

/// Agents that ended while the app was down never told it, so their tabs and
/// devices are let go once the core says who is still there.
async fn forget_departed(app: &AppHandle, tabs: Vec<String>, devices: Vec<String>) {
    let Some((known, running)) = live_agents(app).await else {
        return;
    };
    if let Some(browser) = app.try_state::<BrowserManager>() {
        for agent_id in tabs.iter().filter(|id| !known.contains(*id)) {
            let _ = browser.close_agent(app, agent_id);
        }
    }
    if let Some(simulators) = app.try_state::<SimulatorManager>() {
        for agent_id in devices.iter().filter(|id| !running.contains(*id)) {
            simulators.forget_agent(agent_id);
        }
    }
}

/// Every agent the core still has, and those of them still running.
async fn live_agents(app: &AppHandle) -> Option<(HashSet<String>, HashSet<String>)> {
    let pty = app.try_state::<PtyManager>()?;
    for _ in 0..CORE_ATTEMPTS {
        if let Ok(client) = pty.client().await {
            if let (Ok(sessions), Ok(chats)) = (pty.sessions().await, client.acp_list().await) {
                let mut known = HashSet::new();
                let mut running = HashSet::new();
                for session in sessions {
                    if let Some(agent_id) = session.agent_id {
                        if session.running {
                            running.insert(agent_id.clone());
                        }
                        known.insert(agent_id);
                    }
                }
                for chat in chats {
                    if chat.state != ChatState::Stopped {
                        running.insert(chat.agent_id.clone());
                    }
                    known.insert(chat.agent_id);
                }
                return Some((known, running));
            }
        }
        tokio::time::sleep(CORE_RETRY).await;
    }
    None
}
