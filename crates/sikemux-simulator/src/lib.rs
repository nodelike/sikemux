//! The iOS Simulator for agents and the desk, with no UI toolkit: the
//! `sikemux-sim` helper that drives devices ([`SimManager`]), which device each
//! agent holds, how it was last read and which devices Sikemux booted
//! ([`SimulatorManager`]), and the agents' `sim` tool ([`tools`]). The app
//! hosting it gives it a [`Host`] for its data folder and its events.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sikemux_base::cli::protocol::PAGE_REPLY_TIMEOUT;
use sikemux_process::sync::Locked;

use claims::{Claim, Claims};
use tools::{Device, Element, Numbered, Numbering};

pub mod android;
mod bundle;
pub mod claims;
mod helper;
mod load;
pub mod tools;

pub use helper::{
    executable, installed, local_helper, prepare, published_helper, status, SimManager, SimStatus,
    SIM_EVENT, WATCH_ENDED_EVENT,
};

pub type SimResult<T> = Result<T, String>;

/// How far setting up the Android Emulator has come, or that it finished, and how.
pub const ANDROID_SETUP_EVENT: &str = "android-setup";
/// Which devices agents hold, boot or wait for changed.
pub const LOAD_EVENT: &str = "simulator-load";

/// What the app hosting the simulator gives it.
pub trait Host: Send + Sync + 'static {
    /// Where the helper is kept once downloaded.
    fn data_dir(&self) -> Option<PathBuf>;
    /// Tells the window that something changed, such as the helper's download
    /// or an agent attaching a device, as `event` with `payload`.
    fn emit(&self, event: &str, payload: Value);
}

/// A host with nowhere to keep the helper and nobody to tell, for tests.
#[cfg(test)]
struct NoHost;

#[cfg(test)]
impl Host for NoHost {
    fn data_dir(&self) -> Option<PathBuf> {
        None
    }

    fn emit(&self, _: &str, _: Value) {}
}

const CORE_SIMULATOR: &str = "/Library/Developer/PrivateFrameworks/CoreSimulator.framework";
/// One tool call's whole budget, well inside the time the page has to answer.
const CALL_DEADLINE: Duration = Duration::from_secs(PAGE_REPLY_TIMEOUT.as_secs() - 15);
/// A device Sikemux booted is shut down this long after nobody holds it, so an agent that restarts finds it still running.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(180);
const BOOT_LIMIT: Duration = Duration::from_secs(180);
/// An agent keeps its device while it uses it; one that has not for this long gives it up to an agent waiting for one.
const LEASE: Duration = Duration::from_secs(600);
/// An agent waiting for a simulator keeps its place in line while it calls again within this.
const PLACE_KEPT: Duration = Duration::from_secs(60);
/// The longest an agent waits in line before it is told to ask the person.
const MOST_WAIT: Duration = Duration::from_secs(300);
const CANCELLED: &str = "the simulator call was cancelled";
const OVERRAN: &str = "the simulator call ran out of time; call sim state to see where it got to";

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
    /// When the agent last called a simulator tool.
    used: Instant,
    /// When the agent took the device, in Unix milliseconds.
    since: u64,
}

impl Attachment {
    fn new(device: Device, project: String) -> Self {
        Self {
            device,
            project,
            numbers: Numbering::default(),
            read: None,
            launches: HashMap::new(),
            log_generations: HashMap::new(),
            used: Instant::now(),
            since: sikemux_process::clock::epoch_ms(),
        }
    }
}

/// An agent waiting for room to start a simulator.
struct Waiter {
    agent_id: String,
    joined: Instant,
    seen: Instant,
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

/// A device an agent holds, as the person sees what loads the Mac.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub agent_id: String,
    pub name: String,
    pub booting: bool,
    /// When the agent took it, in Unix milliseconds.
    pub since: u64,
}

/// An agent waiting for room to start a simulator, and how many wait ahead of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub agent_id: String,
    pub ahead: usize,
}

/// Which agent held which device, carried across an update's restart.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSimulators {
    attachments: Vec<SavedAttachment>,
    desk: HashMap<String, String>,
    projects: HashMap<String, String>,
    booted: Vec<String>,
    offered: HashMap<String, bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedAttachment {
    agent_id: String,
    project: String,
    udid: String,
    name: String,
    os: String,
    screen: Option<(f64, f64)>,
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
    /// Agents waiting for room to start a simulator, first come first served.
    line: VecDeque<Waiter>,
    /// Devices Sikemux is booting now, and room reserved for ones about to boot.
    starting: HashSet<String>,
}

pub struct SimulatorManager {
    sim: SimManager,
    android: android::Android,
    host: Arc<dyn Host>,
    /// The helper tests run in place of the app's own, which is downloaded if it must be.
    helper: Option<PathBuf>,
    claims: Claims,
    enabled: AtomicBool,
    shared: Mutex<Shared>,
    /// One call at a time per agent.
    turns: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Counts each agent's cancellations; a call started before the latest one stops.
    cancels: Mutex<HashMap<String, Arc<AtomicU64>>>,
    me: Weak<SimulatorManager>,
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

impl SimulatorManager {
    pub fn new(host: Arc<dyn Host>) -> Arc<Self> {
        Self::made(host, None, Claims::shared())
    }

    #[cfg(test)]
    pub(crate) fn with_helper(path: PathBuf) -> Arc<Self> {
        let claims = tempfile::tempdir().expect("a claims folder").keep();
        Self::made(
            Arc::new(NoHost),
            Some(path),
            Claims::at(claims, std::process::id()),
        )
    }

    fn made(host: Arc<dyn Host>, helper: Option<PathBuf>, claims: Claims) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            sim: SimManager::default(),
            android: android::Android::new(host.data_dir()),
            host,
            helper,
            claims,
            enabled: AtomicBool::new(true),
            shared: Mutex::default(),
            turns: Mutex::default(),
            cancels: Mutex::default(),
            me: me.clone(),
        })
    }

    /// The helper's own requests, as a window's device view sends them.
    pub fn sim(&self) -> &SimManager {
        &self.sim
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
    }

    /// The person turned the simulator tools on or off in Settings; the app says so as it starts too.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn android(&self) -> &android::Android {
        &self.android
    }

    /// Whether agents are offered the `sim` tool: the person has not turned them
    /// off, and this computer can run iOS simulators or has the Android SDK.
    pub(crate) fn offered(&self) -> bool {
        self.enabled() && (capable() || self.android.sdk().is_some())
    }

    /// Why no device can be driven here, when neither platform can run.
    pub(crate) fn unavailable(&self) -> Option<String> {
        let ios = unsupported_reason()?;
        if self.android.sdk().is_some() {
            return None;
        }
        Some(format!("{ios} {}", android::NO_SDK))
    }

    pub(crate) fn emit(&self, event: &str, payload: Value) {
        self.host.emit(event, payload);
    }

    /// Sends `kind` with `fields`, such as `tap` with `{"udid": …, "x": 10, "y": 20}`.
    pub async fn request(&self, kind: &str, fields: Value) -> Result<Value, String> {
        let limit = if kind == "boot" {
            BOOT_LIMIT
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
        let limit = remaining().map_or(limit, |left| limit.min(left));
        if kind == "devices" {
            return self.all_devices(limit).await;
        }
        if request
            .get("udid")
            .and_then(Value::as_str)
            .is_some_and(android::is_android)
        {
            let answer = self.android.request(kind, request, limit).await;
            checkpoint()?;
            return answer;
        }
        request.insert("type".into(), kind.into());
        let executable = match &self.helper {
            Some(path) => path.clone(),
            None => helper::executable(&*self.host).await?,
        };
        let answer = self.sim.call_within(executable, request, limit).await;
        checkpoint()?;
        answer.map_err(|error| error.to_string())
    }

    /// The iOS simulators the helper lists, when this Mac can run them, then the Android devices.
    async fn all_devices(&self, limit: Duration) -> SimResult<Value> {
        let ios = async {
            if self.helper.is_none() && helper::supported().await.is_err() {
                return Ok(Vec::new());
            }
            let executable = match &self.helper {
                Some(path) => path.clone(),
                None => helper::executable(&*self.host).await?,
            };
            let request = json!({ "type": "devices" });
            let answer = self
                .sim
                .call_within(
                    executable,
                    request.as_object().cloned().unwrap_or_default(),
                    limit,
                )
                .await?;
            Ok::<_, String>(answer["devices"].as_array().cloned().unwrap_or_default())
        };
        let (ios, android) = tokio::join!(ios, self.android.devices());
        let mut devices = match ios {
            Ok(devices) => devices,
            Err(error) if android.is_empty() => return Err(error),
            Err(_) => Vec::new(),
        };
        devices.extend(android);
        Ok(json!({ "devices": devices }))
    }

    /// Runs one of an agent's calls once its earlier ones are done, within the call's deadline.
    pub(crate) async fn in_turn<T>(
        &self,
        agent_id: &str,
        work: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        let cancels = Arc::clone(
            self.cancels
                .locked()
                .entry(agent_id.to_owned())
                .or_default(),
        );
        let call = Call {
            started_at: cancels.load(Ordering::SeqCst),
            cancels,
            deadline: Instant::now() + CALL_DEADLINE,
        };
        let turn = Arc::clone(self.turns.locked().entry(agent_id.to_owned()).or_default());
        if let Some(attachment) = self.shared.locked().attachments.get_mut(agent_id) {
            attachment.used = Instant::now();
        }
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
        if let Some(cancels) = self.cancels.locked().get(agent_id) {
            cancels.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(crate) fn is_mine(&self, claim: &Claim, agent_id: &str) -> bool {
        claim.pid == std::process::id() && claim.agent_id == agent_id
    }

    /// Devices someone other than this agent holds, in this copy of Sikemux or another.
    /// The devices Sikemux booted.
    pub(crate) fn booted_by_sikemux(&self) -> HashSet<String> {
        self.shared.locked().booted.clone()
    }

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
            let mut shared = self.shared.locked();
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
                    Attachment::new(device.clone(), project.to_owned()),
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
        if let Some(attachment) = self.shared.locked().attachments.get_mut(agent_id) {
            attachment.device = device.clone();
        }
    }

    /// Lets go of the agent's device, and tells the desk.
    pub(crate) fn release(&self, agent_id: &str) -> Option<Device> {
        let attachment = self.shared.locked().attachments.remove(agent_id)?;
        self.claims.release(&attachment.device.udid, agent_id);
        self.emit(tools::DETACHED_EVENT, json!({ "agentId": agent_id }));
        self.let_go(attachment.device.clone());
        Some(attachment.device)
    }

    /// The agent is gone: it stopped, crashed or was closed.
    pub fn forget_agent(&self, agent_id: &str) {
        self.cancel(agent_id);
        self.release(agent_id);
        let mut shared = self.shared.locked();
        shared.desk.remove(agent_id);
        shared.offered.remove(agent_id);
        shared.line.retain(|waiter| waiter.agent_id != agent_id);
    }

    /// Stops following a device's log, and shuts it down a while later if
    /// Sikemux booted it and nobody has taken it up again.
    fn let_go(&self, device: Device) {
        self.shut_down_when_idle(device.udid, true);
    }

    fn shut_down_when_idle(&self, udid: String, stop_logs: bool) {
        let Some(manager) = self.me.upgrade().filter(|_| self.helper.is_none()) else {
            return;
        };
        sikemux_base::runtime::spawn(async move {
            if stop_logs {
                let _ = manager.request("stopLogs", json!({ "udid": udid })).await;
            }
            tokio::time::sleep(SHUTDOWN_GRACE).await;
            if manager.unheld_boot(&udid) {
                let _ = manager.request("shutdown", json!({ "udid": udid })).await;
                manager.shared.locked().booted.remove(&udid);
            }
        });
    }

    /// Sikemux booted the device and no agent anywhere holds it now.
    fn unheld_boot(&self, udid: &str) -> bool {
        let shared = self.shared.locked();
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
        let udids: Vec<String> = self.shared.locked().booted.iter().cloned().collect();
        self.step_aside();
        let unheld: Vec<String> = udids
            .into_iter()
            .filter(|udid| self.claims.holder(udid).is_none())
            .collect();
        let (emulators, unheld): (Vec<String>, Vec<String>) = unheld
            .into_iter()
            .partition(|udid| android::is_android(udid));
        self.android.shut_down(&emulators);
        self.android.drain();
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

    /// Lets go of every device but leaves them running, for an update's
    /// restart to take them up again.
    pub fn step_aside(&self) {
        self.shared.locked().attachments.clear();
        self.claims.release_all();
    }

    pub fn saved(&self) -> SavedSimulators {
        let shared = self.shared.locked();
        SavedSimulators {
            attachments: shared
                .attachments
                .iter()
                .map(|(agent_id, attachment)| SavedAttachment {
                    agent_id: agent_id.clone(),
                    project: attachment.project.clone(),
                    udid: attachment.device.udid.clone(),
                    name: attachment.device.name.clone(),
                    os: attachment.device.os.clone(),
                    screen: attachment.device.screen,
                })
                .collect(),
            desk: shared.desk.clone(),
            projects: shared.projects.clone(),
            booted: shared.booted.iter().cloned().collect(),
            offered: shared.offered.clone(),
        }
    }

    /// Gives each agent back the device it held before an update's restart,
    /// and names those agents. Devices Sikemux booted that nobody holds now
    /// shut down after the usual wait.
    pub fn restore(&self, saved: SavedSimulators) -> Vec<String> {
        let mut taken = Vec::new();
        let mut agents = Vec::new();
        for held in saved.attachments {
            if self
                .claims
                .claim(&held.udid, &held.agent_id, &held.project)
                .is_err()
            {
                continue;
            }
            let device = Device {
                udid: held.udid,
                name: held.name,
                os: held.os,
                booted: true,
                busy: false,
                warm: true,
                screen: held.screen,
            };
            taken.push(device.udid.clone());
            agents.push(held.agent_id.clone());
            self.shared
                .locked()
                .attachments
                .insert(held.agent_id, Attachment::new(device, held.project));
        }
        {
            let mut shared = self.shared.locked();
            shared.desk = saved.desk;
            shared.projects = saved.projects;
            shared.offered = saved.offered;
        }
        let idle = saved
            .booted
            .into_iter()
            .filter(|udid| !taken.contains(udid))
            .collect();
        self.adopt_booted(idle);
        self.shared.locked().booted.extend(taken.iter().cloned());
        self.follow_logs(taken);
        agents
    }

    /// Takes over devices an earlier run booted and nobody holds, so they shut down after the usual wait.
    pub fn adopt_booted(&self, udids: Vec<String>) {
        for udid in udids {
            self.shared.locked().booted.insert(udid.clone());
            self.shut_down_when_idle(udid, false);
        }
    }

    /// Starts reading the devices' logs again, as attaching does.
    fn follow_logs(&self, udids: Vec<String>) {
        let Some(manager) = self.me.upgrade().filter(|_| self.helper.is_none()) else {
            return;
        };
        if udids.is_empty() {
            return;
        }
        sikemux_base::runtime::spawn(async move {
            for udid in udids {
                let _ = manager
                    .request("logs", json!({ "udid": udid, "limit": 1 }))
                    .await;
            }
        });
    }

    pub fn attachments(&self) -> Vec<AttachmentInfo> {
        let mut listed: Vec<AttachmentInfo> = self
            .shared
            .locked()
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
        self.shared.locked().booted.insert(udid.to_owned());
    }

    /// Whether the agent may start a simulator now, `cold` for one never booted: it
    /// must be first in line, and the Mac must have room beside every simulator
    /// running or reserved, whoever started it. A yes reserves the room until the
    /// guard is dropped. Agents that stopped using their devices give them up when
    /// there is no room.
    pub(crate) fn room(
        &self,
        agent_id: &str,
        devices: &[Device],
        cold: bool,
    ) -> Result<Starting<'_>, Queued> {
        let mut running = load::running();
        running.extend(
            devices
                .iter()
                .filter(|device| device.booted || device.busy)
                .map(|device| device.udid.clone()),
        );
        let (place, joined) = self.take_place(agent_id);
        if joined.elapsed() >= MOST_WAIT {
            self.leave_line(agent_id);
            return Err(Queued::TooLong);
        }
        let machine = load::Machine::now();
        let mut shared = self.shared.locked();
        running.extend(shared.starting.iter().cloned());
        let starting = devices.iter().filter(|device| device.busy).count() + shared.starting.len();
        let limit = machine.limit(running.len());
        let wait = load::room(running.len(), starting, cold, limit).err();
        if place == 0 && wait.is_none() {
            let key = format!("reserved-by-{agent_id}");
            shared.starting.insert(key.clone());
            shared.line.retain(|waiter| waiter.agent_id != agent_id);
            drop(shared);
            self.emit(LOAD_EVENT, json!({}));
            return Ok(Starting { manager: self, key });
        }
        drop(shared);
        if matches!(wait, Some(load::Wait::Full { .. })) {
            self.expire_leases(agent_id);
        }
        Err(Queued::Waiting {
            place,
            wait,
            machine,
        })
    }

    /// The agent's place in line, joining it or keeping it, and when it joined.
    fn take_place(&self, agent_id: &str) -> (usize, Instant) {
        let mut shared = self.shared.locked();
        let now = Instant::now();
        let before = shared.line.len();
        shared
            .line
            .retain(|waiter| waiter.agent_id == agent_id || waiter.seen.elapsed() < PLACE_KEPT);
        let place = match shared
            .line
            .iter()
            .position(|waiter| waiter.agent_id == agent_id)
        {
            Some(place) => place,
            None => {
                shared.line.push_back(Waiter {
                    agent_id: agent_id.to_owned(),
                    joined: now,
                    seen: now,
                });
                shared.line.len() - 1
            }
        };
        let moved = shared.line.len() != before;
        let waiter = &mut shared.line[place];
        waiter.seen = now;
        let taken = (place, waiter.joined);
        drop(shared);
        if moved {
            self.emit(LOAD_EVENT, json!({}));
        }
        taken
    }

    pub(crate) fn leave_line(&self, agent_id: &str) {
        let left = {
            let mut shared = self.shared.locked();
            let before = shared.line.len();
            shared.line.retain(|waiter| waiter.agent_id != agent_id);
            shared.line.len() != before
        };
        if left {
            self.emit(LOAD_EVENT, json!({}));
        }
    }

    /// The devices agents hold, oldest first, and the agents waiting for one, first in line first.
    pub fn heavy(&self) -> (Vec<Held>, Vec<Waiting>) {
        let shared = self.shared.locked();
        let mut held: Vec<Held> = (shared.attachments.iter())
            .map(|(agent_id, attachment)| Held {
                agent_id: agent_id.clone(),
                name: attachment.device.name.clone(),
                booting: shared.starting.contains(&attachment.device.udid),
                since: attachment.since,
            })
            .collect();
        held.sort_by_key(|held| held.since);
        let waiting = (shared.line.iter().enumerate())
            .filter(|(_, waiter)| waiter.seen.elapsed() < PLACE_KEPT)
            .map(|(ahead, waiter)| Waiting {
                agent_id: waiter.agent_id.clone(),
                ahead,
            })
            .collect();
        (held, waiting)
    }

    /// The person stopped what the agent does with simulators: its call stops, it leaves the
    /// line, and the device it holds shuts down now unless another agent takes it up.
    pub fn stop(&self, agent_id: &str) {
        self.cancel(agent_id);
        self.leave_line(agent_id);
        let Some(device) = self.release(agent_id) else {
            return;
        };
        let Some(manager) = self.me.upgrade().filter(|_| self.helper.is_none()) else {
            return;
        };
        sikemux_base::runtime::spawn(async move {
            let ours = manager.shared.locked().booted.contains(&device.udid);
            if ours && manager.claims.holder(&device.udid).is_none() {
                let _ = manager
                    .request("shutdown", json!({ "udid": device.udid }))
                    .await;
                manager.shared.locked().booted.remove(&device.udid);
            }
        });
    }

    /// Lets go of the devices of agents other than `waiting` that have not used them for a while.
    fn expire_leases(&self, waiting: &str) {
        let expired: Vec<String> = self
            .shared
            .locked()
            .attachments
            .iter()
            .filter(|(agent_id, attachment)| {
                agent_id.as_str() != waiting && attachment.used.elapsed() >= LEASE
            })
            .map(|(agent_id, _)| agent_id.clone())
            .collect();
        for agent_id in expired {
            self.release(&agent_id);
        }
    }

    pub(crate) fn desk_device(&self, agent_id: &str) -> Option<String> {
        self.shared.locked().desk.get(agent_id).cloned()
    }

    pub(crate) fn project_device(&self, project: &str) -> Option<String> {
        self.shared.locked().projects.get(project).cloned()
    }

    pub(crate) fn note_offered(&self, agent_id: &str, offered: bool) {
        self.shared
            .locked()
            .offered
            .insert(agent_id.to_owned(), offered);
    }

    pub(crate) fn offered_to(&self, agent_id: &str) -> Option<bool> {
        self.shared.locked().offered.get(agent_id).copied()
    }

    pub(crate) fn attached(&self, agent_id: &str) -> Option<Device> {
        self.shared
            .locked()
            .attachments
            .get(agent_id)
            .map(|attachment| attachment.device.clone())
    }

    /// Numbers a fresh read the way the agent's earlier reads were numbered.
    pub(crate) fn number(&self, agent_id: &str, elements: &[Element]) -> Vec<Numbered> {
        match self.shared.locked().attachments.get_mut(agent_id) {
            Some(attachment) => attachment.numbers.assign(elements),
            None => Numbering::default().assign(elements),
        }
    }

    pub(crate) fn remember_read(&self, agent_id: &str, app: String, elements: Vec<Numbered>) {
        if let Some(attachment) = self.shared.locked().attachments.get_mut(agent_id) {
            attachment.read = Some((app, elements));
        }
    }

    pub(crate) fn last_read(&self, agent_id: &str) -> Option<(String, Vec<Numbered>)> {
        self.shared.locked().attachments.get(agent_id)?.read.clone()
    }

    /// A launch of `bundle` this agent sent within `window` that has not reached the front: its pid, once known.
    pub(crate) fn launch_in_flight(
        &self,
        agent_id: &str,
        bundle: &str,
        window: Duration,
    ) -> Option<Option<i64>> {
        let shared = self.shared.locked();
        let (at, pid) = shared.attachments.get(agent_id)?.launches.get(bundle)?;
        (at.elapsed() < window).then_some(*pid)
    }

    pub(crate) fn note_launch(&self, agent_id: &str, bundle: &str, pid: Option<i64>) {
        if let Some(attachment) = self.shared.locked().attachments.get_mut(agent_id) {
            let at = attachment
                .launches
                .get(bundle)
                .filter(|_| pid.is_some())
                .map_or_else(Instant::now, |(at, _)| *at);
            attachment.launches.insert(bundle.to_owned(), (at, pid));
        }
    }

    pub(crate) fn forget_launch(&self, agent_id: &str, bundle: &str) {
        if let Some(attachment) = self.shared.locked().attachments.get_mut(agent_id) {
            attachment.launches.remove(bundle);
        }
    }

    pub(crate) fn log_generation(&self, agent_id: &str, process: Option<&str>) -> Option<i64> {
        self.shared
            .locked()
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
        if let Some(attachment) = self.shared.locked().attachments.get_mut(agent_id) {
            attachment
                .log_generations
                .insert(process.map(str::to_owned), generation);
        }
    }
}

/// Why an agent waits for a simulator.
pub(crate) enum Queued {
    Waiting {
        place: usize,
        wait: Option<load::Wait>,
        machine: load::Machine,
    },
    TooLong,
}

/// Room reserved for one simulator to start, held until it has booted.
pub(crate) struct Starting<'a> {
    manager: &'a SimulatorManager,
    key: String,
}

impl Starting<'_> {
    /// The reserved room is the device `udid`, which is booting now.
    pub(crate) fn boots(&mut self, udid: &str) {
        let mut shared = self.manager.shared.locked();
        shared.starting.remove(&self.key);
        shared.starting.insert(udid.to_owned());
        drop(shared);
        self.key = udid.to_owned();
        self.manager.emit(LOAD_EVENT, json!({}));
    }
}

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        self.manager.shared.locked().starting.remove(&self.key);
        self.manager.emit(LOAD_EVENT, json!({}));
    }
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
    let mut checked = CHECKED.locked();
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
    if local_helper().is_none() && published_helper().is_none() {
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

impl SimulatorManager {
    /// The person picked `udid` on this agent's desk. An attached agent moves to
    /// it now; one that has not attached yet is given it when it does.
    pub async fn set_desk_device(&self, agent_id: &str, udid: &str) -> SimResult<()> {
        self.shared
            .locked()
            .desk
            .insert(agent_id.to_owned(), udid.to_owned());
        let Some(project) = self
            .shared
            .locked()
            .attachments
            .get(agent_id)
            .map(|attachment| attachment.project.clone())
        else {
            return Ok(());
        };
        let device = tools::list_devices(self)
            .await?
            .into_iter()
            .find(|device| device.udid == udid)
            .ok_or_else(|| format!("no simulator has the id {udid}"))?;
        self.hold(agent_id, &project, &device)?;
        self.emit(
            tools::ATTACHED_EVENT,
            json!({ "agentId": agent_id, "udid": device.udid, "name": device.name }),
        );
        Ok(())
    }

    /// Streams an Android emulator's screen to `on_frame` as [`android::Android::stream`] sends
    /// it, answering the id [`SimManager::unwatch`] stops it by.
    pub fn watch_android(
        &self,
        udid: String,
        on_frame: impl FnMut(Vec<u8>) -> bool + Send + 'static,
    ) -> SimResult<u64> {
        let manager = self.me.upgrade().ok_or("Sikemux is quitting")?;
        let streamed = udid.clone();
        Ok(self
            .sim
            .follow(Arc::clone(&self.host), udid, "rgb".into(), async move {
                manager.android.stream(&streamed, on_frame).await
            }))
    }

    /// What setting up the Android Emulator would download here, or nothing when it is set up.
    pub async fn android_plan(&self) -> SimResult<Option<android::install::Plan>> {
        let data_dir = self.host.data_dir();
        let root = android::sdk::root(data_dir.as_deref())
            .or_else(|| {
                data_dir
                    .as_deref()
                    .map(sikemux_process::android_sdk::own_root)
            })
            .ok_or("Sikemux has no folder to set up the Android SDK in")?;
        let avd_home = android::sdk::avd_home().ok_or("no folder for virtual devices")?;
        let plan = android::install::plan(root, avd_home).await?;
        Ok((!plan.packages.is_empty() || plan.device).then_some(plan))
    }

    /// What Settings shows about Android: the SDK in use, its emulator's version and the
    /// virtual devices, or what setting it up would download.
    pub async fn android_info(&self) -> Value {
        let root = self.android.sdk();
        let (emulator, devices) = tokio::task::spawn_blocking({
            let root = root.clone();
            move || {
                let emulator = root
                    .as_ref()
                    .and_then(|root| {
                        std::fs::read_to_string(root.join("emulator/source.properties")).ok()
                    })
                    .and_then(|text| android::sdk::ini(&text).get("Pkg.Revision").cloned());
                let devices: Vec<String> = android::sdk::avds()
                    .into_iter()
                    .map(|avd| avd.name)
                    .collect();
                (emulator, devices)
            }
        })
        .await
        .unwrap_or_default();
        let offer = match root.is_none() || emulator.is_none() || devices.is_empty() {
            true => self
                .android_plan()
                .await
                .ok()
                .flatten()
                .map(|plan| plan.size()),
            false => None,
        };
        json!({ "sdk": root, "emulator": emulator, "devices": devices, "offer": offer })
    }

    /// Sets up the Android Emulator, telling the window how far it has come as [`ANDROID_SETUP_EVENT`]s.
    pub async fn set_up_android(&self) -> SimResult<()> {
        let Some(plan) = self.android_plan().await? else {
            return Ok(());
        };
        let mut reported = -1.0;
        let result = android::install::install(&plan, |progress| {
            if progress.fraction - reported >= 0.005 || progress.fraction >= 1.0 {
                reported = progress.fraction;
                self.emit(
                    ANDROID_SETUP_EVENT,
                    json!({ "step": progress.step, "fraction": progress.fraction }),
                );
            }
        })
        .await;
        self.emit(
            ANDROID_SETUP_EVENT,
            json!({ "done": true, "error": result.as_ref().err() }),
        );
        result
    }

    /// Turns a device, or with no `orientation` says which way it is turned.
    pub async fn orientation(&self, udid: &str, orientation: Option<String>) -> SimResult<String> {
        let mut fields = json!({ "udid": udid });
        if let Some(orientation) = &orientation {
            fields["orientation"] = orientation.as_str().into();
        }
        let answer = self.request("orientation", fields).await?;
        Ok(answer["orientation"]
            .as_str()
            .map(str::to_owned)
            .or(orientation)
            .unwrap_or_else(|| "portrait".into()))
    }

    /// A request from the window's own view of a device, such as a tap the person
    /// made on its screen. A boot is remembered as Sikemux's.
    pub async fn window_call(&self, mut request: Map<String, Value>) -> SimResult<Value> {
        let kind = request
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let android = request
            .get("udid")
            .and_then(Value::as_str)
            .is_some_and(android::is_android);
        if kind == "devices" || android {
            request.remove("type");
            let limit = if kind == "boot" {
                BOOT_LIMIT
            } else {
                Duration::from_secs(60)
            };
            let answer = match kind.as_str() {
                "devices" => self.all_devices(limit).await,
                _ => self.android.request(&kind, request.clone(), limit).await,
            };
            if let (Ok(_), "boot", Some(udid)) = (
                &answer,
                kind.as_str(),
                request.get("udid").and_then(Value::as_str),
            ) {
                self.note_booted(udid);
            }
            return answer;
        }
        helper::supported().await?;
        let boot = request.get("type").and_then(Value::as_str) == Some("boot");
        let udid = request
            .get("udid")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let answer = self
            .sim
            .call(helper::executable(&*self.host).await?, request)
            .await?;
        if let (true, Some(udid)) = (boot, udid) {
            self.note_booted(&udid);
        }
        Ok(answer)
    }

    /// What Settings shows about the simulator: the Xcode in use, its iOS runtimes,
    /// and where the helper stands.
    pub async fn setup(&self) -> Value {
        let (xcode, runtimes) = tokio::task::spawn_blocking(|| {
            let Some(dir) = developer_dir() else {
                let tools_only = Path::new(COMMAND_LINE_TOOLS).exists().then(|| {
                    format!(
                        "{COMMAND_LINE_TOOLS} (Command Line Tools only; the simulator needs Xcode)"
                    )
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
        let helper = if local_helper().is_some() {
            "built with this copy of Sikemux"
        } else if installed(&*self.host) {
            "ready"
        } else if published_helper().is_some() {
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
