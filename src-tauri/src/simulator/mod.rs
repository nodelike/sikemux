//! What agents and the desk do with a simulator: which device each agent has
//! attached, how it was last read and which way it is turned. Requests go to
//! the `sikemux-sim` helper through [`crate::sim::SimManager`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use serde_json::{Map, Value};
use tauri::AppHandle;

use crate::sim::SimManager;
use tools::{Device, Element};

pub mod tools;
pub mod view;

const CORE_SIMULATOR: &str = "/Library/Developer/PrivateFrameworks/CoreSimulator.framework";

struct Attachment {
    device: Device,
    read: Option<(String, Vec<Element>)>,
}

/// Where the helper comes from: the app's own, downloaded if it must be, or a given one in tests.
enum Helper {
    App(AppHandle),
    #[cfg_attr(not(test), allow(dead_code))]
    At(PathBuf),
}

pub struct SimulatorManager {
    sim: SimManager,
    helper: Helper,
    attachments: Mutex<HashMap<String, Attachment>>,
    orientations: Mutex<HashMap<String, String>>,
}

impl SimulatorManager {
    pub fn for_app(app: AppHandle, sim: SimManager) -> Self {
        Self::new(sim, Helper::App(app))
    }

    #[cfg(test)]
    pub(crate) fn with_helper(path: PathBuf) -> Self {
        Self::new(SimManager::default(), Helper::At(path))
    }

    fn new(sim: SimManager, helper: Helper) -> Self {
        Self {
            sim,
            helper,
            attachments: Mutex::default(),
            orientations: Mutex::default(),
        }
    }

    /// Sends `kind` with `fields`, such as `tap` with `{"udid": …, "x": 10, "y": 20}`.
    pub async fn request(&self, kind: &str, fields: Value) -> Result<Value, String> {
        let mut request = match fields {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            _ => return Err("simulator request fields must be an object".into()),
        };
        request.insert("type".into(), kind.into());
        let executable = match &self.helper {
            Helper::App(app) => crate::sim::executable(app)
                .await
                .map_err(|error| error.to_string())?,
            Helper::At(path) => path.clone(),
        };
        self.sim
            .call(executable, request)
            .await
            .map_err(|error| error.to_string())
    }

    fn detach(&self, agent_id: &str) -> Option<Device> {
        self.lock_attachments()
            .remove(agent_id)
            .map(|attachment| attachment.device)
    }

    fn attach(&self, agent_id: &str, device: Device) {
        self.lock_attachments()
            .insert(agent_id.to_owned(), Attachment { device, read: None });
    }

    fn attached(&self, agent_id: &str) -> Option<Device> {
        self.lock_attachments()
            .get(agent_id)
            .map(|attachment| attachment.device.clone())
    }

    fn remember_read(&self, agent_id: &str, app: String, elements: Vec<Element>) {
        if let Some(attachment) = self.lock_attachments().get_mut(agent_id) {
            attachment.read = Some((app, elements));
        }
    }

    fn set_orientation(&self, udid: &str, orientation: &str) {
        self.orientations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(udid.to_owned(), orientation.to_owned());
    }

    fn screen_for(&self, device: &Device) -> Option<(f64, f64)> {
        let (width, height) = device.screen?;
        let sideways = self
            .orientations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&device.udid)
            .is_some_and(|orientation| orientation.starts_with("landscape"));
        Some(if sideways {
            (height, width)
        } else {
            (width, height)
        })
    }

    fn last_read(&self, agent_id: &str) -> Option<(String, Vec<Element>)> {
        self.lock_attachments().get(agent_id)?.read.clone()
    }

    fn elements(&self, agent_id: &str) -> Vec<Element> {
        self.last_read(agent_id)
            .map(|(_, elements)| elements)
            .unwrap_or_default()
    }

    fn lock_attachments(&self) -> MutexGuard<'_, HashMap<String, Attachment>> {
        self.attachments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether agents are offered the `sim_*` tools: the person has not turned them
/// off, and this Mac can run simulators.
pub fn offered() -> bool {
    ENABLED.load(Ordering::Relaxed) && capable()
}

/// This build has a helper, beside it or to download, and Xcode's simulators are installed.
pub fn capable() -> bool {
    crate::sim::unsupported_reason().is_none() && Path::new(CORE_SIMULATOR).exists()
}

#[cfg(test)]
mod tests;
