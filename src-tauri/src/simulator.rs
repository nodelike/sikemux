//! The window's side of the iOS Simulator: the engine in `sikemux-simulator`,
//! hosted on this app's data folder and told to the page as events, and the
//! commands the desk's device view and Settings call.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, AppResult};

pub use sikemux_simulator::{tools, SavedSimulators};

pub type SimulatorManager = Arc<sikemux_simulator::SimulatorManager>;

struct AppHost {
    app: AppHandle,
}

impl sikemux_simulator::Host for AppHost {
    fn data_dir(&self) -> Option<PathBuf> {
        self.app.path().app_data_dir().ok()
    }

    fn emit(&self, event: &str, payload: Value) {
        let _ = self.app.emit_to("main", event, payload);
    }
}

/// The engine for this app, with the tools on or off as the person last saved.
pub fn engine(app: &AppHandle) -> SimulatorManager {
    sikemux_simulator::set_enabled(saved_preference(&crate::state::state_load_sync()));
    sikemux_simulator::SimulatorManager::new(Arc::new(AppHost { app: app.clone() }))
}

/// The `iosSimulator` preference in the page's saved state, on unless it was turned off.
fn saved_preference(state: &str) -> bool {
    serde_json::from_str::<Value>(state)
        .ok()
        .and_then(|state| state["prefs"]["iosSimulator"].as_bool())
        .unwrap_or(true)
}

/// An agent stopped: its terminal's process exited, or its chat ended.
pub(crate) fn agent_stopped(app: &AppHandle, agent_id: &str) {
    if let Some(manager) = app.try_state::<SimulatorManager>() {
        manager.forget_agent(agent_id);
    }
}

#[tauri::command]
pub async fn sim_status(
    manager: State<'_, SimulatorManager>,
) -> AppResult<sikemux_simulator::SimStatus> {
    Ok(sikemux_simulator::status(&**manager.host()).await)
}

/// Downloads the helper if this build needs to, reporting progress as `sim` events.
#[tauri::command]
pub async fn sim_prepare(manager: State<'_, SimulatorManager>) -> AppResult<()> {
    sikemux_simulator::prepare(&**manager.host())
        .await
        .map_err(AppError::Other)
}

#[tauri::command]
pub async fn sim_call(
    manager: State<'_, SimulatorManager>,
    request: Map<String, Value>,
) -> AppResult<Value> {
    manager.window_call(request).await.map_err(AppError::Other)
}

/// Streams a device's screen to `on_frame`, and returns an id `sim_unwatch`
/// stops it by. The page tells how many frames it has read with
/// `sim_watch_read`; a watch that ends on its own sends `simulator-watch-ended`.
#[tauri::command]
pub async fn sim_watch(
    manager: State<'_, SimulatorManager>,
    udid: String,
    format: String,
    on_frame: Channel<Response>,
) -> AppResult<u64> {
    let host = Arc::clone(manager.host());
    manager
        .sim()
        .watch(host, udid, format, move |frame| {
            on_frame.send(Response::new(frame)).is_ok()
        })
        .await
        .map_err(AppError::Other)
}

/// The page has read this many frames of a watch.
#[tauri::command]
pub fn sim_watch_read(manager: State<'_, SimulatorManager>, id: u64, frames: u64) {
    manager.sim().watch_read(id, frames);
}

#[tauri::command]
pub async fn sim_unwatch(manager: State<'_, SimulatorManager>, id: u64) -> AppResult<()> {
    manager
        .sim()
        .unwatch(&**manager.host(), id)
        .await
        .map_err(AppError::Other)
}

#[tauri::command]
pub fn simulator_set_enabled(enabled: bool) {
    sikemux_simulator::set_enabled(enabled);
}

#[tauri::command]
pub async fn simulator_setup(manager: State<'_, SimulatorManager>) -> Result<Value, ()> {
    Ok(manager.setup().await)
}

/// Every agent's device, for the desk's device picker and an agent's tab.
#[tauri::command]
pub fn simulator_attachments(
    manager: State<'_, SimulatorManager>,
) -> Vec<sikemux_simulator::AttachmentInfo> {
    manager.attachments()
}

/// The person picked `udid` on this agent's desk. An attached agent moves to
/// it now; one that has not attached yet is given it when it does.
#[tauri::command]
pub async fn simulator_set_desk_device(
    manager: State<'_, SimulatorManager>,
    agent_id: String,
    udid: String,
) -> Result<(), String> {
    manager.set_desk_device(&agent_id, &udid).await
}

/// Turns a device, or with no `orientation` says which way it is turned.
#[tauri::command]
pub async fn simulator_orientation(
    manager: State<'_, SimulatorManager>,
    udid: String,
    orientation: Option<String>,
) -> Result<String, String> {
    manager.orientation(&udid, orientation).await
}
