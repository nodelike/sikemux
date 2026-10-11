//! What agents and the desk do with a simulator: which device each agent
//! holds, how it was last read, and which devices Sikemux booted. Requests go
//! to the `sikemux-sim` helper through [`crate::sim::SimManager`].

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::sim::SimManager;
use claims::{Claim, Claims};
use tools::{Device, Element, Numbered, Numbering};

pub mod claims;
pub mod tools;

const CORE_SIMULATOR: &str = "/Library/Developer/PrivateFrameworks/CoreSimulator.framework";
/// Simulators take gigabytes of memory each, so agents may have Sikemux boot at most this many.
pub(crate) const MAX_BOOTED: usize = 3;
/// One tool call's whole budget, well inside the 66 s the core waits for an answer.
const CALL_DEADLINE: Duration = Duration::from_secs(50);
/// A device Sikemux booted is shut down this long after nobody holds it, so an agent that restarts finds it still running.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(180);
const CANCELLED: &str = "the simulator call was cancelled";
const OVERRAN: &str = "the simulator call ran out of time; call sim_state to see where it got to";

struct Attachment {
    device: Device,
    project: String,
    numbers: Numbering,
    /// The last read the agent was told about, to report changes against.
    read: Option<(String, Vec<Numbered>)>,
    /// Apps this agent launched that have not come to the front yet: when, and their pid once known.
    launches: HashMap<String, (Instant, Option<i64>)>,
    /// The log each cursor was numbered by, per process, as the helper names it.
    log_generations: HashMap<Option<String>, i64>,
}

/// One agent's device, as the desk lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub agent_id: String,
    pub udid: String,
    pub name: String,
    pub project: String,
}

/// Where the helper comes from: the app's own, downloaded if it must be, or a given one in tests.
enum Helper {
    App(AppHandle),
    #[cfg_attr(not(test), allow(dead_code))]
    At(PathBuf),
}

#[derive(Default)]
struct Shared {
    attachments: HashMap<String, Attachment>,
    /// The device the person picked on each agent's desk.
    desk: HashMap<String, String>,
    /// The device each project last used.
    projects: HashMap<String, String>,
    /// Devices Sikemux booted, which it shuts down once nobody holds them.
    booted: HashSet<String>,
    /// Whether each agent was offered the tools when it listed them.
    offered: HashMap<String, bool>,
}

pub struct SimulatorManager {
    sim: SimManager,
    helper: Helper,
    claims: Claims,
    shared: Mutex<Shared>,
    /// One call at a time per agent.
    turns: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Counts each agent's cancellations; a call started before the latest one stops.
    cancels: Mutex<HashMap<String, Arc<AtomicU64>>>,
}

/// The tool call the current task is running.
#[derive(Clone)]
struct Call {
    cancels: Arc<AtomicU64>,
    started_at: u64,
    deadline: Instant,
}

tokio::task_local! {
    static CALL: Call;
}

impl Call {
    fn check(&self) -> Result<(), String> {
        if self.cancels.load(Ordering::SeqCst) != self.started_at {
            return Err(CANCELLED.into());
        }
        if Instant::now() >= self.deadline {
            return Err(OVERRAN.into());
        }
        Ok(())
    }
}

/// Time left in the current tool call, or nothing outside one.
pub(crate) fn remaining() -> Option<Duration> {
    CALL.try_with(|call| call.deadline.saturating_duration_since(Instant::now()))
        .ok()
}

/// Fails once the current call is cancelled or out of time.
pub(crate) fn checkpoint() -> Result<(), String> {
    CALL.try_with(Call::check).unwrap_or(Ok(()))
}

/// Waits, unless the call is cancelled or runs out of time first.
pub(crate) async fn pause(duration: Duration) -> Result<(), String> {
    checkpoint()?;
    tokio::time::sleep(remaining().map_or(duration, |left| duration.min(left))).await;
    checkpoint()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The helper says "Boot it first", but agents start a device with `sim_attach`.
fn for_agents(message: String) -> String {
    message.replace(
        "Boot it first.",
        "Call sim_attach to start it, or ask the person.",
    )
}

impl SimulatorManager {
    pub fn for_app(app: AppHandle, sim: SimManager) -> Self {
        Self::new(sim, Helper::App(app), Claims::shared())
    }

    #[cfg(test)]
    pub(crate) fn with_helper(path: PathBuf) -> Self {
        let claims = tempfile::tempdir().expect("a claims folder").keep();
        Self::new(
            SimManager::default(),
            Helper::At(path),
            Claims::at(claims, std::process::id()),
        )
    }

    fn new(sim: SimManager, helper: Helper, claims: Claims) -> Self {
        Self {
            sim,
            helper,
            claims,
            shared: Mutex::default(),
            turns: Mutex::default(),
            cancels: Mutex::default(),
        }
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        lock(&self.shared)
    }

    pub(crate) fn emit(&self, event: &str, payload: Value) {
        if let Helper::App(app) = &self.helper {
            let _ = app.emit_to("main", event, payload);
        }
    }

    /// Sends `kind` with `fields`, such as `tap` with `{"udid": …, "x": 10, "y": 20}`.
    pub async fn request(&self, kind: &str, fields: Value) -> Result<Value, String> {
        let limit = if kind == "boot" {
            Duration::from_secs(180)
        } else {
            Duration::from_secs(60)
        };
        self.request_within(kind, fields, limit).await
    }

    /// As [`Self::request`], giving up after `limit` or when the call's own time runs out.
    pub async fn request_within(
        &self,
        kind: &str,
        fields: Value,
        limit: Duration,
    ) -> Result<Value, String> {
        checkpoint()?;
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
        let limit = remaining().map_or(limit, |left| limit.min(left));
        let answer = self.sim.call_within(executable, request, limit).await;
        checkpoint()?;
        answer.map_err(|error| for_agents(error.to_string()))
    }

    /// Runs one of an agent's calls once its earlier ones are done, within the call's deadline.
    pub(crate) async fn in_turn<T>(
        &self,
        agent_id: &str,
        work: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        let cancels = Arc::clone(lock(&self.cancels).entry(agent_id.to_owned()).or_default());
        let call = Call {
            started_at: cancels.load(Ordering::SeqCst),
            cancels,
            deadline: Instant::now() + CALL_DEADLINE,
        };
        let turn = Arc::clone(lock(&self.turns).entry(agent_id.to_owned()).or_default());
        CALL.scope(call, async move {
            let _turn = tokio::time::timeout(CALL_DEADLINE, turn.lock_owned())
                .await
                .map_err(|_| {
                    "your previous simulator call is still running; try again once it answers"
                        .to_owned()
                })?;
            checkpoint()?;
            work.await
        })
        .await
    }

    /// Stops the agent's running call at its next step, and the calls waiting behind it.
    pub(crate) fn cancel(&self, agent_id: &str) {
        if let Some(cancels) = lock(&self.cancels).get(agent_id) {
            cancels.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(crate) fn is_mine(&self, claim: &Claim, agent_id: &str) -> bool {
        claim.pid == std::process::id() && claim.agent_id == agent_id
    }

    /// Devices someone other than this agent holds, in this copy of Sikemux or another.
    pub(crate) fn held_by_others(&self, agent_id: &str) -> HashMap<String, Claim> {
        self.claims
            .all()
            .into_iter()
            .filter(|(_, claim)| !self.is_mine(claim, agent_id))
            .collect()
    }

    /// Gives the agent the device, letting go of any other it held.
    pub(crate) fn hold(
        &self,
        agent_id: &str,
        project: &str,
        device: &Device,
    ) -> Result<(), String> {
        self.claims
            .claim(&device.udid, agent_id, project)
            .map_err(|holder| tools::in_use(device, &holder))?;
        let previous = {
            let mut shared = self.shared();
            shared
                .projects
                .insert(project.to_owned(), device.udid.clone());
            let previous = shared
                .attachments
                .get(agent_id)
                .map(|attachment| attachment.device.clone())
                .filter(|previous| previous.udid != device.udid);
            if previous.is_some() || !shared.attachments.contains_key(agent_id) {
                shared.attachments.insert(
                    agent_id.to_owned(),
                    Attachment {
                        device: device.clone(),
                        project: project.to_owned(),
                        numbers: Numbering::default(),
                        read: None,
                        launches: HashMap::new(),
                        log_generations: HashMap::new(),
                    },
                );
            }
            previous
        };
        if let Some(previous) = previous {
            self.claims.release(&previous.udid, agent_id);
            self.let_go(previous);
        }
        Ok(())
    }

    /// Keeps what attaching learned about the device, such as its screen.
    pub(crate) fn set_device(&self, agent_id: &str, device: &Device) {
        if let Some(attachment) = self.shared().attachments.get_mut(agent_id) {
            attachment.device = device.clone();
        }
    }

    /// Lets go of the agent's device, and tells the desk.
    pub fn release(&self, agent_id: &str) -> Option<Device> {
        let attachment = self.shared().attachments.remove(agent_id)?;
        self.claims.release(&attachment.device.udid, agent_id);
        self.emit(tools::DETACHED_EVENT, json!({ "agentId": agent_id }));
        self.let_go(attachment.device.clone());
        Some(attachment.device)
    }

    /// The agent is gone: it stopped, crashed or was closed.
    pub fn forget_agent(&self, agent_id: &str) {
        self.cancel(agent_id);
        self.release(agent_id);
        let mut shared = self.shared();
        shared.desk.remove(agent_id);
        shared.offered.remove(agent_id);
    }

    /// Stops following a device's log, and shuts it down a while later if
    /// Sikemux booted it and nobody has taken it up again.
    fn let_go(&self, device: Device) {
        let Helper::App(app) = &self.helper else {
            return;
        };
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let manager = app.state::<SimulatorManager>();
            let _ = manager
                .request("stopLogs", json!({ "udid": device.udid }))
                .await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
            if manager.unheld_boot(&device.udid) {
                let _ = manager
                    .request("shutdown", json!({ "udid": device.udid }))
                    .await;
                manager.shared().booted.remove(&device.udid);
            }
        });
    }

    /// Sikemux booted the device and no agent anywhere holds it now.
    fn unheld_boot(&self, udid: &str) -> bool {
        let shared = self.shared();
        shared.booted.contains(udid)
            && !shared
                .attachments
                .values()
                .any(|attachment| attachment.device.udid == udid)
            && self.claims.holder(udid).is_none()
    }

    /// Lets go of everything as Sikemux quits, and shuts down the devices it
    /// booted that no other copy of Sikemux holds.
    pub fn drain(&self) {
        let udids: Vec<String> = {
            let mut shared = self.shared();
            shared.attachments.clear();
            shared.booted.iter().cloned().collect()
        };
        self.claims.release_all();
        let unheld: Vec<String> = udids
            .into_iter()
            .filter(|udid| self.claims.holder(udid).is_none())
            .collect();
        if unheld.is_empty() {
            return;
        }
        if let Some(dir) = developer_dir() {
            let _ = sikemux_process::user_environment::command(dir.join("usr/bin/simctl"))
                .arg("shutdown")
                .args(&unheld)
                .env("DEVELOPER_DIR", &dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }

    pub fn attachments(&self) -> Vec<AttachmentInfo> {
        let mut listed: Vec<AttachmentInfo> = self
            .shared()
            .attachments
            .iter()
            .map(|(agent_id, attachment)| AttachmentInfo {
                agent_id: agent_id.clone(),
                udid: attachment.device.udid.clone(),
                name: attachment.device.name.clone(),
                project: attachment.project.clone(),
            })
            .collect();
        listed.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        listed
    }

    /// A device Sikemux booted, whether for an agent or from a desk tab.
    pub fn note_booted(&self, udid: &str) {
        self.shared().booted.insert(udid.to_owned());
    }

    /// How many of the devices running now Sikemux booted.
    pub(crate) fn booted_by_sikemux(&self, devices: &[Device]) -> usize {
        let shared = self.shared();
        devices
            .iter()
            .filter(|device| device.booted && shared.booted.contains(&device.udid))
            .count()
    }

    pub(crate) fn desk_device(&self, agent_id: &str) -> Option<String> {
        self.shared().desk.get(agent_id).cloned()
    }

    pub(crate) fn project_device(&self, project: &str) -> Option<String> {
        self.shared().projects.get(project).cloned()
    }

    pub(crate) fn note_offered(&self, agent_id: &str, offered: bool) {
        self.shared().offered.insert(agent_id.to_owned(), offered);
    }

    pub(crate) fn offered_to(&self, agent_id: &str) -> Option<bool> {
        self.shared().offered.get(agent_id).copied()
    }

    pub(crate) fn attached(&self, agent_id: &str) -> Option<Device> {
        self.shared()
            .attachments
            .get(agent_id)
            .map(|attachment| attachment.device.clone())
    }

    /// Numbers a fresh read the way the agent's earlier reads were numbered.
    pub(crate) fn number(&self, agent_id: &str, elements: &[Element]) -> Vec<Numbered> {
        match self.shared().attachments.get_mut(agent_id) {
            Some(attachment) => attachment.numbers.assign(elements),
            None => Numbering::default().assign(elements),
        }
    }

    pub(crate) fn remember_read(&self, agent_id: &str, app: String, elements: Vec<Numbered>) {
        if let Some(attachment) = self.shared().attachments.get_mut(agent_id) {
            attachment.read = Some((app, elements));
        }
    }

    pub(crate) fn last_read(&self, agent_id: &str) -> Option<(String, Vec<Numbered>)> {
        self.shared().attachments.get(agent_id)?.read.clone()
    }

    /// A launch of `bundle` this agent sent within `window` that has not reached the front: its pid, once known.
    pub(crate) fn launch_in_flight(
        &self,
        agent_id: &str,
        bundle: &str,
        window: Duration,
    ) -> Option<Option<i64>> {
        let shared = self.shared();
        let (at, pid) = shared.attachments.get(agent_id)?.launches.get(bundle)?;
        (at.elapsed() < window).then_some(*pid)
    }

    pub(crate) fn note_launch(&self, agent_id: &str, bundle: &str, pid: Option<i64>) {
        if let Some(attachment) = self.shared().attachments.get_mut(agent_id) {
            let at = attachment
                .launches
                .get(bundle)
                .filter(|_| pid.is_some())
                .map_or_else(Instant::now, |(at, _)| *at);
            attachment.launches.insert(bundle.to_owned(), (at, pid));
        }
    }

    pub(crate) fn forget_launch(&self, agent_id: &str, bundle: &str) {
        if let Some(attachment) = self.shared().attachments.get_mut(agent_id) {
            attachment.launches.remove(bundle);
        }
    }

    pub(crate) fn log_generation(&self, agent_id: &str, process: Option<&str>) -> Option<i64> {
        self.shared()
            .attachments
            .get(agent_id)?
            .log_generations
            .get(&process.map(str::to_owned))
            .copied()
    }

    pub(crate) fn set_log_generation(
        &self,
        agent_id: &str,
        process: Option<&str>,
        generation: i64,
    ) {
        if let Some(attachment) = self.shared().attachments.get_mut(agent_id) {
            attachment
                .log_generations
                .insert(process.map(str::to_owned), generation);
        }
    }
}

/// An agent stopped: its terminal's process exited, or its chat ended.
pub(crate) fn agent_stopped(app: &AppHandle, agent_id: &str) {
    if let Some(manager) = app.try_state::<SimulatorManager>() {
        manager.forget_agent(agent_id);
    }
}

const UNKNOWN: u8 = 2;
static ENABLED: AtomicU8 = AtomicU8::new(UNKNOWN);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(u8::from(enabled), Ordering::Relaxed);
}

/// Whether the person left the simulator tools on in Settings, as last saved
/// until the page says otherwise.
pub fn enabled() -> bool {
    match ENABLED.load(Ordering::Relaxed) {
        UNKNOWN => {
            let saved = saved_preference(&crate::state::state_load_sync());
            let _ = ENABLED.compare_exchange(
                UNKNOWN,
                u8::from(saved),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
            ENABLED.load(Ordering::Relaxed) == 1
        }
        known => known == 1,
    }
}

/// The `iosSimulator` preference in the page's saved state, on unless it was turned off.
fn saved_preference(state: &str) -> bool {
    serde_json::from_str::<Value>(state)
        .ok()
        .and_then(|state| state["prefs"]["iosSimulator"].as_bool())
        .unwrap_or(true)
}

/// Whether agents are offered the `sim_*` tools: the person has not turned them
/// off, and this Mac can run simulators.
pub fn offered() -> bool {
    enabled() && capable()
}

/// This Mac can run the helper and Xcode's simulators.
pub fn capable() -> bool {
    unsupported_reason().is_none()
}

/// The oldest macOS the helper runs on.
const MINIMUM_MACOS: u32 = 15;
/// Checking runs `xcode-select`, so an answer is kept this long.
const RECHECK: Duration = Duration::from_secs(30);

/// Why this Mac cannot run the simulator, or nothing when it can.
pub fn unsupported_reason() -> Option<String> {
    static CHECKED: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);
    let mut checked = CHECKED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, reason)) = checked.as_ref() {
        if at.elapsed() < RECHECK {
            return reason.clone();
        }
    }
    let reason = check_support();
    *checked = Some((Instant::now(), reason.clone()));
    reason
}

fn check_support() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return Some("The iOS Simulator is only available on macOS.".into());
    }
    if macos_major().is_some_and(|major| major < MINIMUM_MACOS) {
        return Some(format!(
            "The iOS Simulator needs macOS {MINIMUM_MACOS} or later."
        ));
    }
    if crate::sim::local_helper().is_none() && crate::sim::published_helper().is_none() {
        return Some("This build does not include the simulator helper.".into());
    }
    if developer_dir().is_none() {
        return Some(if Path::new(COMMAND_LINE_TOOLS).exists() {
            "The iOS Simulator needs Xcode, and only Apple's Command Line Tools are installed. Install Xcode from the App Store and open it once.".into()
        } else {
            "The iOS Simulator needs Xcode. Install it from the App Store and open it once.".into()
        });
    }
    if !Path::new(CORE_SIMULATOR).exists() {
        return Some(
            "Xcode's simulators are not set up yet. Open Xcode once to finish installing them."
                .into(),
        );
    }
    None
}

const COMMAND_LINE_TOOLS: &str = "/Library/Developer/CommandLineTools";

fn macos_major() -> Option<u32> {
    let plist = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
    let after = plist.split("<key>ProductVersion</key>").nth(1)?;
    let version = after.split("<string>").nth(1)?.split("</string>").next()?;
    version.split('.').next()?.trim().parse().ok()
}

/// A full Xcode's developer directory: the one `xcode-select` chose, or else
/// one in /Applications. The Command Line Tools alone have no simulators.
pub(crate) fn developer_dir() -> Option<PathBuf> {
    let full = |dir: &Path| dir.join("usr/bin/simctl").is_file();
    let selected = std::env::var_os("DEVELOPER_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            sikemux_process::user_environment::command("/usr/bin/xcode-select")
                .arg("-p")
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        });
    if let Some(selected) = selected.filter(|dir| full(dir)) {
        return Some(selected);
    }
    let mut installed: Vec<PathBuf> = std::fs::read_dir("/Applications")
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("Xcode") && name.ends_with(".app"))
        })
        .map(|app| app.join("Contents/Developer"))
        .filter(|dir| full(dir))
        .collect();
    installed.sort();
    installed.into_iter().next()
}

/// Every agent's device, for the desk's device picker and an agent's tab.
#[tauri::command]
pub fn simulator_attachments(manager: State<'_, SimulatorManager>) -> Vec<AttachmentInfo> {
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
    manager.shared().desk.insert(agent_id.clone(), udid.clone());
    let Some(project) = manager
        .shared()
        .attachments
        .get(&agent_id)
        .map(|attachment| attachment.project.clone())
    else {
        return Ok(());
    };
    let device = tools::list_devices(&manager)
        .await?
        .into_iter()
        .find(|device| device.udid == udid)
        .ok_or_else(|| format!("no simulator has the id {udid}"))?;
    manager.hold(&agent_id, &project, &device)?;
    manager.emit(
        tools::ATTACHED_EVENT,
        json!({ "agentId": agent_id, "udid": device.udid, "name": device.name }),
    );
    Ok(())
}

/// Turns a device, or with no `orientation` says which way it is turned.
#[tauri::command]
pub async fn simulator_orientation(
    manager: State<'_, SimulatorManager>,
    udid: String,
    orientation: Option<String>,
) -> Result<String, String> {
    let mut fields = json!({ "udid": udid });
    if let Some(orientation) = &orientation {
        fields["orientation"] = orientation.as_str().into();
    }
    let answer = manager.request("orientation", fields).await?;
    Ok(answer["orientation"]
        .as_str()
        .map(str::to_owned)
        .or(orientation)
        .unwrap_or_else(|| "portrait".into()))
}

#[tauri::command]
pub fn simulator_set_enabled(enabled: bool) {
    set_enabled(enabled);
}

/// What Settings shows about the simulator: the Xcode in use, its iOS runtimes,
/// and where the helper stands.
#[tauri::command]
pub async fn simulator_setup(app: AppHandle) -> Value {
    let (xcode, runtimes) = tauri::async_runtime::spawn_blocking(|| {
        let Some(dir) = developer_dir() else {
            let tools_only = Path::new(COMMAND_LINE_TOOLS).exists().then(|| {
                format!("{COMMAND_LINE_TOOLS} (Command Line Tools only; the simulator needs Xcode)")
            });
            return (tools_only, Vec::new());
        };
        let runtimes = simctl(&dir, &["list", "runtimes", "--json"])
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|list| {
                list["runtimes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|runtime| runtime["isAvailable"].as_bool() == Some(true))
                    .filter_map(|runtime| runtime["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        (Some(dir.to_string_lossy().into_owned()), runtimes)
    })
    .await
    .unwrap_or_default();
    let helper = if crate::sim::local_helper().is_some() {
        "built with this copy of Sikemux"
    } else if crate::sim::installed(&app) {
        "ready"
    } else if crate::sim::published_helper().is_some() {
        "downloaded when first needed"
    } else {
        "not included in this build"
    };
    json!({
        "xcode": xcode,
        "runtimes": runtimes,
        "helper": helper,
    })
}

/// Runs Xcode's `simctl` from a full Xcode, never through `xcrun`, which offers
/// to install the developer tools on a Mac without them.
pub(crate) fn simctl(developer_dir: &Path, args: &[&str]) -> Option<String> {
    sikemux_process::user_environment::command(developer_dir.join("usr/bin/simctl"))
        .args(args)
        .env("DEVELOPER_DIR", developer_dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests;
