//! The agent's `sim_*` tools. Each agent holds one simulator and acts on it
//! in device points; a person watching the same device sees every step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::claims::Claim;
use super::{checkpoint, pause, remaining, SimulatorManager};
use sikemux_core::cli::protocol::{HarnessRequest, SIM_CANCEL_METHOD, SIM_OFFERED_METHOD};

/// A cold boot can outlast one call, so it is reported as still going and finishes in the background.
const BOOT_MARGIN: Duration = Duration::from_secs(12);
/// Time kept back for reading the screen once an action is done.
const SETTLE_MARGIN: Duration = Duration::from_secs(4);
const SETTLE_STEP: Duration = Duration::from_millis(300);
/// A device reports itself booted a little before its screen can be read; this waits up to 30 s.
const SCREEN_READY_READS: usize = 30;
const SCREEN_READY_STEP: Duration = Duration::from_secs(1);
/// Enough reads for an app to finish launching, about three and a half seconds.
const SETTLE_READS: usize = 12;
/// Elements whose centre is above this are the status bar's: time, signal, battery.
const STATUS_BAR_HEIGHT: f64 = 60.0;
/// How many reads, a step apart, to wait for a launched app to come to the front: ten seconds.
const LAUNCH_READS: usize = 33;
/// A launch this recent that has not reached the front yet is still under way, and is not sent again.
const LAUNCH_IN_FLIGHT: Duration = Duration::from_secs(60);
/// How many reads, a step apart, to wait for an action to change the screen.
const CHANGE_READS: usize = 5;
/// A swipe that starts this close to an edge is sent as a system gesture, as the helper decides.
const EDGE: f64 = 10.0;
const MAX_ELEMENTS: usize = 200;
const MAX_REMOVED: usize = 50;
const MAX_TEXT: usize = 200;
const DEFAULT_LOG_LINES: usize = 200;
const MAX_LOG_LINES: usize = 500;
const MAX_LOG_LINE: usize = 2000;
/// Well under the 4 MB an answer may be.
const MAX_LOG_BYTES: usize = 256 * 1024;
/// Typed text is sent this many characters at a time, so a cancelled call stops typing soon.
const TEXT_CHUNK: usize = 40;
/// Two reads' elements this close, in points, can be the same element when nothing else tells them apart.
const SAME_PLACE: f64 = 40.0;
/// An agent cannot see the person's screen, so attaching says where the device went.
const SHOWN_ON_DESK: &str =
    "live on your desk in Sikemux, beside the person, who sees what you do and can use it too";
const TURNED_OFF: &str = "The iOS Simulator tools are turned off in Sikemux's Settings.";
/// Tells the window an agent attached a simulator, so its desk can show it.
pub const ATTACHED_EVENT: &str = "simulator-attached";
pub const DETACHED_EVENT: &str = "simulator-detached";
/// True while one of an agent's calls is driving its device, as the browser's acting highlight.
pub const ACTING_EVENT: &str = "simulator-acting";

pub fn execute(app: &AppHandle, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .as_deref()
        .ok_or("simulator tools need the agent's id")?;
    let manager = app.state::<SimulatorManager>();
    if request.method == SIM_OFFERED_METHOD {
        let offered = super::offered();
        manager.note_offered(agent_id, offered);
        return Ok(json!({ "offered": offered }));
    }
    if request.method == SIM_CANCEL_METHOD {
        manager.cancel(agent_id);
        return Ok(json!({}));
    }
    if let Some(reason) = refusal(super::enabled(), super::unsupported_reason()) {
        return Err(reason);
    }
    let acting = drives(&request.method);
    if acting {
        manager.emit(ACTING_EVENT, json!({ "agentId": agent_id, "acting": true }));
    }
    let result = tauri::async_runtime::block_on(manager.in_turn(
        agent_id,
        run(
            &manager,
            agent_id,
            &request.project,
            &request.method,
            &request.params,
        ),
    ));
    if acting {
        manager.emit(
            ACTING_EVENT,
            json!({ "agentId": agent_id, "acting": false }),
        );
    }
    result
}

/// Why an agent may not use the tools now, if it may not.
pub(super) fn refusal(enabled: bool, unsupported: Option<String>) -> Option<String> {
    unsupported.or_else(|| (!enabled).then(|| TURNED_OFF.into()))
}

/// Whether a method acts on the device, rather than only reading what is there.
fn drives(method: &str) -> bool {
    !matches!(
        method,
        "sim.devices" | "sim.state" | "sim.screenshot" | "sim.logs" | "sim.detach"
    )
}

pub(super) async fn run(
    manager: &SimulatorManager,
    agent_id: &str,
    project: &str,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    let text = |key: &str| params.get(key).and_then(Value::as_str);
    let number = |key: &str| params.get(key).and_then(Value::as_f64);
    match method {
        "sim.devices" => {
            let devices = list_devices(manager).await?;
            let claims = manager.claims.all();
            Ok(json!({
                "devices": devices
                    .iter()
                    .map(|device| {
                        let holder = claims.get(&device.udid);
                        let mine = holder.is_some_and(|claim| manager.is_mine(claim, agent_id));
                        json!({
                            "udid": device.udid,
                            "name": device.name,
                            "os": device.os,
                            "booted": device.booted,
                            "attached": mine,
                            "inUseBy": holder.filter(|_| !mine).map(holder_text),
                        })
                    })
                    .collect::<Vec<_>>(),
            }))
        }
        "sim.attach" => attach(manager, agent_id, project, text("device")).await,
        "sim.state" => settled_state(manager, agent_id, None, Report::Full).await,
        "sim.tap" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let numbered = manager.number(agent_id, &before.elements);
            let point = match text("label") {
                Some(label) => labelled(&numbered, label)?,
                None => tap_point(
                    &numbered,
                    params.get("index").and_then(Value::as_u64),
                    number("x"),
                    number("y"),
                )?,
            };
            let mut fields = json!({ "udid": device.udid, "x": point.0, "y": point.1 });
            if let Some(duration) = number("duration") {
                fields["duration"] = duration.into();
            }
            manager.request("tap", fields).await?;
            settled_state(manager, agent_id, Some(before), report(params)?).await
        }
        "sim.swipe" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let from = (
                number("fromX").ok_or("fromX is required")?,
                number("fromY").ok_or("fromY is required")?,
            );
            let to = (
                number("toX").ok_or("toX is required")?,
                number("toY").ok_or("toY is required")?,
            );
            let mut fields =
                json!({ "udid": device.udid, "x": from.0, "y": from.1, "toX": to.0, "toY": to.1 });
            if let Some(duration) = number("duration") {
                fields["duration"] = duration.into();
            }
            let warning = edge_warning(before.size, from);
            manager.request("swipe", fields).await?;
            let mut state = settled_state(manager, agent_id, Some(before), report(params)?).await?;
            if let Some(warning) = warning {
                state["warning"] = warning.into();
            }
            Ok(state)
        }
        "sim.type" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let typed: Vec<char> = text("text").ok_or("text is required")?.chars().collect();
            for chunk in typed.chunks(TEXT_CHUNK) {
                let chunk: String = chunk.iter().collect();
                manager
                    .request("text", json!({ "udid": device.udid, "text": chunk }))
                    .await?;
            }
            settled_state(manager, agent_id, Some(before), report(params)?).await
        }
        "sim.button" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let button = text("button").ok_or("button is required")?;
            manager
                .request("button", json!({ "udid": device.udid, "button": button }))
                .await?;
            settled_state(manager, agent_id, Some(before), report(params)?).await
        }
        "sim.screenshot" => {
            let device = attached(manager, agent_id)?;
            let shot = manager
                .request(
                    "screenshot",
                    json!({ "udid": device.udid, "format": "jpeg", "pointSize": true }),
                )
                .await?;
            Ok(json!({
                "data": shot["jpeg"],
                "mimeType": "image/jpeg",
                "title": format!("{} ({})", device.name, device.os),
                "width": shot["width"],
                "height": shot["height"],
            }))
        }
        "sim.launch" => launch(manager, agent_id, params).await,
        "sim.terminate" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let bundle = text("bundleId").ok_or("bundleId is required")?;
            manager.forget_launch(agent_id, bundle);
            manager
                .request(
                    "terminate",
                    json!({ "udid": device.udid, "bundleId": bundle }),
                )
                .await?;
            settled_state(manager, agent_id, Some(before), report(params)?).await
        }
        "sim.install" => {
            let device = attached(manager, agent_id)?;
            let path = inside_project(project, text("path").ok_or("path is required")?)?;
            manager
                .request("install", json!({ "udid": device.udid, "path": path }))
                .await
        }
        "sim.openUrl" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let url = text("url").ok_or("url is required")?;
            manager
                .request("openUrl", json!({ "udid": device.udid, "url": url }))
                .await?;
            settled_state(manager, agent_id, Some(before), report(params)?).await
        }
        "sim.touchPath" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let points = path(params, &["x", "y"])?;
            let timed = timed(
                &points,
                number("duration"),
                |point| json!({ "x": point[0], "y": point[1] }),
            );
            manager
                .request("touchPath", json!({ "udid": device.udid, "points": timed }))
                .await?;
            settled_state(manager, agent_id, Some(before), report(params)?).await
        }
        "sim.touch2Path" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let points = path(params, &["x1", "y1", "x2", "y2"])?;
            let timed = timed(
                &points,
                number("duration"),
                |point| json!({ "x": point[0], "y": point[1], "x2": point[2], "y2": point[3] }),
            );
            manager
                .request(
                    "touch2Path",
                    json!({ "udid": device.udid, "points": timed }),
                )
                .await?;
            settled_state(manager, agent_id, Some(before), report(params)?).await
        }
        "sim.detach" => {
            let device = manager
                .release(agent_id)
                .ok_or("no simulator is attached")?;
            Ok(
                json!({ "detached": format!("{} ({})", device.name, device.os), "udid": device.udid }),
            )
        }
        "sim.logs" => logs(manager, agent_id, params).await,
        "sim.rotate" => {
            let device = attached(manager, agent_id)?;
            let orientation = text("orientation").ok_or("orientation is required")?;
            manager
                .request(
                    "orientation",
                    json!({ "udid": device.udid, "orientation": orientation }),
                )
                .await?;
            settled_state(manager, agent_id, None, Report::Full).await
        }
        other => Err(format!("unknown simulator method {other}")),
    }
}

/// What `workspace_inspect` says about simulators: whether this agent was
/// offered the tools, why not, and the device it holds.
pub fn inspect(manager: &SimulatorManager, agent_id: Option<&str>) -> Value {
    let attached = agent_id.and_then(|agent_id| manager.attached(agent_id));
    let available = agent_id
        .and_then(|agent_id| manager.offered_to(agent_id))
        .unwrap_or_else(super::offered);
    let reason = (!available).then(|| {
        super::unsupported_reason().unwrap_or_else(|| {
            if super::enabled() {
                "This agent started before the iOS Simulator tools were turned on; start a new one to use them.".into()
            } else {
                TURNED_OFF.into()
            }
        })
    });
    json!({
        "available": available,
        "reason": reason,
        "attached": attached.map(|device| json!({ "udid": device.udid, "name": device.name, "os": device.os })),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Device {
    pub udid: String,
    pub name: String,
    pub os: String,
    pub booted: bool,
    /// Width and height in points, upright, known once the device is booted.
    pub screen: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Element {
    pub role: String,
    pub label: String,
    pub value: String,
    pub identifier: String,
    pub enabled: bool,
    pub center: (f64, f64),
    /// Scrolled out of view, or under the screen's edge, so it cannot be tapped.
    pub offscreen: bool,
}

/// An element with the number an agent refers to it by.
pub(crate) type Numbered = (usize, Element);

/// One accessibility read: the frontmost app, its elements, and the size of the
/// app's window, which follows the app when it turns.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Read {
    pub app: String,
    pub elements: Vec<Element>,
    pub size: Option<(f64, f64)>,
}

/// Gives each element a number that stays with it from read to read. An
/// element that is still on screen keeps its number; one that appears takes
/// a number never used before, so a number from an earlier read either
/// reaches the same element or is not on the screen at all.
#[derive(Clone, Debug, Default)]
pub(super) struct Numbering {
    next: usize,
    known: Vec<Numbered>,
}

impl Numbering {
    pub fn assign(&mut self, elements: &[Element]) -> Vec<Numbered> {
        let key = |element: &Element| {
            (
                element.role.clone(),
                element.label.clone(),
                element.identifier.clone(),
            )
        };
        let mut taken = vec![false; self.known.len()];
        let mut numbered = Vec::with_capacity(elements.len());
        for element in elements {
            let wanted = key(element);
            let candidates: Vec<usize> = (0..self.known.len())
                .filter(|&index| !taken[index] && key(&self.known[index].1) == wanted)
                .collect();
            let alone = candidates.len() == 1
                && elements.iter().filter(|other| key(other) == wanted).count() == 1
                && self
                    .known
                    .iter()
                    .filter(|(_, other)| key(other) == wanted)
                    .count()
                    == 1;
            let chosen = if alone {
                candidates.first().copied()
            } else {
                candidates
                    .into_iter()
                    .map(|index| (index, distance(self.known[index].1.center, element.center)))
                    .filter(|(_, apart)| *apart <= SAME_PLACE)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(index, _)| index)
            };
            let number = match chosen {
                Some(index) => {
                    taken[index] = true;
                    self.known[index].0
                }
                None => {
                    self.next += 1;
                    self.next - 1
                }
            };
            numbered.push((number, element.clone()));
        }
        self.known = numbered.clone();
        numbered
    }
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

pub(super) async fn list_devices(manager: &SimulatorManager) -> Result<Vec<Device>, String> {
    let reply = manager.request("devices", json!({})).await?;
    Ok(reply["devices"]
        .as_array()
        .map(|devices| devices.iter().filter_map(device_from).collect())
        .unwrap_or_default())
}

fn device_from(value: &Value) -> Option<Device> {
    Some(Device {
        udid: value.get("udid")?.as_str()?.to_owned(),
        name: value.get("name")?.as_str()?.to_owned(),
        os: value
            .get("runtime")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        booted: value.get("state").and_then(Value::as_str) == Some("booted"),
        screen: None,
    })
}

fn holder_text(claim: &Claim) -> String {
    format!("another agent in {}", claim.project_name())
}

pub(super) fn in_use(device: &Device, claim: &Claim) -> String {
    format!(
        "{} is in use by {}; pick another from sim_devices or ask the person",
        device.name,
        holder_text(claim)
    )
}

/// The name Sikemux gives a device it makes for a project.
pub(super) fn project_device_name(project: &str) -> String {
    let folder = Path::new(project)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    format!("Sikemux · {folder}")
}

#[derive(Debug, PartialEq)]
pub(super) enum Pick<'a> {
    Use(&'a Device),
    Create,
}

/// The device for an agent that named none: the one the person picked on its
/// desk, else the one its project used before, else the device Sikemux made
/// for the project, else a booted iPhone nobody holds, else a new one.
pub(super) fn pick_device<'a>(
    devices: &'a [Device],
    held: &HashMap<String, Claim>,
    desk: Option<&str>,
    remembered: Option<&str>,
    project_device: &str,
) -> Pick<'a> {
    let free = |device: &&Device| !held.contains_key(&device.udid);
    let by_udid = |udid: Option<&str>| {
        udid.and_then(|udid| {
            devices
                .iter()
                .filter(free)
                .find(|device| device.udid == udid)
        })
    };
    if let Some(device) = by_udid(desk).or_else(|| by_udid(remembered)) {
        return Pick::Use(device);
    }
    let newest = |candidates: Vec<&'a Device>| {
        candidates.into_iter().max_by(|a, b| {
            (a.booted, os_version(&a.os))
                .partial_cmp(&(b.booted, os_version(&b.os)))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    };
    let made = newest(
        devices
            .iter()
            .filter(free)
            .filter(|device| device.name == project_device)
            .collect(),
    );
    let booted = || {
        newest(
            devices
                .iter()
                .filter(free)
                .filter(|device| device.booted && device.name.starts_with("iPhone"))
                .collect(),
        )
    };
    made.or_else(booted).map_or(Pick::Create, Pick::Use)
}

/// A named device by udid or name, newest iOS first when a name repeats, a booted one before one that is not.
pub(super) fn choose_device<'a>(devices: &'a [Device], wanted: &str) -> Result<&'a Device, String> {
    devices
        .iter()
        .find(|device| device.udid == wanted)
        .or_else(|| {
            devices
                .iter()
                .filter(|device| device.name == wanted)
                .max_by(|a, b| {
                    (a.booted, os_version(&a.os))
                        .partial_cmp(&(b.booted, os_version(&b.os)))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .ok_or_else(|| {
            format!(
                "no simulator is named {wanted}; sim_devices lists them, and Xcode's Devices and Simulators window adds more"
            )
        })
}

fn os_version(os: &str) -> Vec<u32> {
    os.rsplit(' ')
        .next()
        .unwrap_or_default()
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

async fn attach(
    manager: &SimulatorManager,
    agent_id: &str,
    project: &str,
    wanted: Option<&str>,
) -> Result<Value, String> {
    let devices = list_devices(manager).await?;
    let held = manager.held_by_others(agent_id);
    let current = manager
        .attached(agent_id)
        .and_then(|current| devices.iter().find(|device| device.udid == current.udid));
    let mut device = match (wanted, current) {
        (Some(wanted), _) => {
            let device = choose_device(&devices, wanted)?;
            if let Some(claim) = held.get(&device.udid) {
                return Err(in_use(device, claim));
            }
            device.clone()
        }
        (None, Some(current)) => current.clone(),
        (None, None) => match pick_device(
            &devices,
            &held,
            manager.desk_device(agent_id).as_deref(),
            manager.project_device(project).as_deref(),
            &project_device_name(project),
        ) {
            Pick::Use(device) => device.clone(),
            Pick::Create => create_device(manager, project, &devices).await?,
        },
    };
    if !device.booted && manager.booted_by_sikemux(&devices) >= super::MAX_BOOTED {
        return Err(format!(
            "Sikemux already runs {} simulators it started, which is as many as it will; sim_detach one you no longer need, or ask the person to shut one down",
            super::MAX_BOOTED
        ));
    }
    manager.hold(agent_id, project, &device)?;
    if !device.booted {
        manager.note_booted(&device.udid);
        let budget = remaining()
            .unwrap_or(Duration::from_secs(180))
            .saturating_sub(BOOT_MARGIN);
        match manager
            .request_within("boot", json!({ "udid": device.udid }), budget)
            .await
        {
            Ok(_) => {}
            Err(error) if error.contains("in time") => {
                return Err(format!(
                    "{} is still booting; call sim_attach again in a moment",
                    device.name
                ))
            }
            Err(error) => {
                manager.release(agent_id);
                return Err(error);
            }
        }
    }
    let screen = manager
        .request("screen", json!({ "udid": device.udid }))
        .await?;
    device.screen = screen["width"].as_f64().zip(screen["height"].as_f64());
    manager.set_device(agent_id, &device);
    let _ = manager
        .request("logs", json!({ "udid": device.udid, "limit": 1 }))
        .await;
    for _ in 0..SCREEN_READY_READS {
        if near_deadline() || read_screen(manager, &device).await.is_ok() {
            break;
        }
        pause(SCREEN_READY_STEP).await?;
    }
    manager.emit(
        ATTACHED_EVENT,
        json!({ "agentId": agent_id, "udid": device.udid, "name": device.name }),
    );
    let mut state = settled_state(manager, agent_id, None, Report::Full).await?;
    state["shown"] = SHOWN_ON_DESK.into();
    Ok(state)
}

/// Makes the project its own iPhone, of the newest kind on the newest iOS installed.
async fn create_device(
    manager: &SimulatorManager,
    project: &str,
    devices: &[Device],
) -> Result<Device, String> {
    let base = project_device_name(project);
    let name = (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base} {n}")
            }
        })
        .find(|name| devices.iter().all(|device| &device.name != name))
        .unwrap_or(base);
    let udid = tauri::async_runtime::spawn_blocking(move || {
        let dir = super::developer_dir().ok_or("the iOS Simulator needs Xcode")?;
        let runtimes = super::simctl(&dir, &["list", "runtimes", "--json"])
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .ok_or("could not list Xcode's iOS runtimes")?;
        let (kind, runtime) = newest_iphone(&runtimes)
            .ok_or("no iOS runtime is installed; add one in Xcode's Settings, under Components")?;
        super::simctl(&dir, &["create", &name, &kind, &runtime])
            .filter(|udid| !udid.is_empty())
            .ok_or_else(|| format!("Xcode could not make a simulator named {name}"))
    })
    .await
    .map_err(|error| error.to_string())??;
    list_devices(manager)
        .await?
        .into_iter()
        .find(|device| device.udid == udid)
        .ok_or_else(|| format!("the new simulator {udid} did not appear"))
}

/// The newest iOS runtime installed, and the newest iPhone it runs, which
/// `simctl` lists first, as `simctl create` takes them.
pub(super) fn newest_iphone(runtimes: &Value) -> Option<(String, String)> {
    let runtime = runtimes["runtimes"]
        .as_array()?
        .iter()
        .filter(|runtime| runtime["isAvailable"].as_bool() == Some(true))
        .filter(|runtime| runtime["platform"].as_str() == Some("iOS"))
        .filter(|runtime| {
            runtime["supportedDeviceTypes"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind["productFamily"] == "iPhone"))
        })
        .max_by_key(|runtime| os_version(runtime["version"].as_str().unwrap_or_default()))?;
    let kind = runtime["supportedDeviceTypes"]
        .as_array()?
        .iter()
        .find(|kind| kind["productFamily"] == "iPhone")?;
    Some((
        kind["identifier"].as_str()?.to_owned(),
        runtime["identifier"].as_str()?.to_owned(),
    ))
}

fn attached(manager: &SimulatorManager, agent_id: &str) -> Result<Device, String> {
    manager
        .attached(agent_id)
        .ok_or_else(|| "no simulator is attached; call sim_attach first".into())
}

fn near_deadline() -> bool {
    remaining().is_some_and(|left| left < SETTLE_MARGIN)
}

/// Launches an app, or, when a launch this agent sent is still under way,
/// waits for that one rather than starting the app over.
async fn launch(
    manager: &SimulatorManager,
    agent_id: &str,
    params: &Value,
) -> Result<Value, String> {
    let device = attached(manager, agent_id)?;
    let before = read_screen(manager, &device).await?;
    let bundle = params
        .get("bundleId")
        .and_then(Value::as_str)
        .ok_or("bundleId is required")?;
    let pid = match manager.launch_in_flight(agent_id, bundle, LAUNCH_IN_FLIGHT) {
        Some(pid) => pid,
        None => {
            let mut fields = json!({ "udid": device.udid, "bundleId": bundle });
            for key in ["arguments", "environment"] {
                if let Some(value) = params.get(key) {
                    fields[key] = value.clone();
                }
            }
            manager.note_launch(agent_id, bundle, None);
            let budget = remaining()
                .unwrap_or(Duration::from_secs(60))
                .saturating_sub(SETTLE_MARGIN);
            match manager.request_within("launch", fields, budget).await {
                Ok(launched) => {
                    let pid = launched["pid"].as_i64();
                    manager.note_launch(agent_id, bundle, pid);
                    pid
                }
                Err(error) if error.contains("in time") => None,
                Err(error) => {
                    manager.forget_launch(agent_id, bundle);
                    return Err(error);
                }
            }
        }
    };
    let in_front = match pid {
        Some(pid) => wait_for_front(manager, &device, pid).await?,
        None => false,
    };
    if in_front {
        manager.forget_launch(agent_id, bundle);
    }
    let mut state = settled_state(manager, agent_id, Some(before), report(params)?).await?;
    state["pid"] = pid.into();
    if !in_front {
        state["launching"] = format!(
            "{bundle} is still launching; call sim_state in a moment, or sim_launch again, which waits for this launch rather than starting another"
        )
        .into();
    }
    Ok(state)
}

/// A newly installed app takes a few seconds to open the first time, longer than
/// any screen change is waited for, so a launch waits until its process is in front.
async fn wait_for_front(
    manager: &SimulatorManager,
    device: &Device,
    pid: i64,
) -> Result<bool, String> {
    for _ in 0..LAUNCH_READS {
        if near_deadline() {
            return Ok(false);
        }
        let front = manager
            .request("tree", json!({ "udid": device.udid }))
            .await
            .ok()
            .and_then(|reply| frontmost_pid(&reply));
        if front == Some(pid) {
            return Ok(true);
        }
        pause(SETTLE_STEP).await?;
    }
    Ok(false)
}

pub(super) fn frontmost_pid(reply: &Value) -> Option<i64> {
    reply["elements"]
        .as_array()?
        .iter()
        .find(|element| element["type"] == "Application")?["pid"]
        .as_i64()
}

/// A path the agent gave, which must name something inside its project.
pub(super) fn inside_project(project: &str, path: &str) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(project).map_err(|error| error.to_string())?;
    let target = std::fs::canonicalize(root.join(path))
        .map_err(|_| format!("no app at {}", root.join(path).display()))?;
    if !target.starts_with(&root) {
        return Err(
            "the app must be inside the project; build it into the project's folder, such as with xcodebuild -derivedDataPath build".into(),
        );
    }
    Ok(target)
}

async fn logs(manager: &SimulatorManager, agent_id: &str, params: &Value) -> Result<Value, String> {
    let device = attached(manager, agent_id)?;
    let cursor = params.get("cursor").and_then(Value::as_u64).unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_LOG_LINES, |limit| limit as usize)
        .clamp(1, MAX_LOG_LINES);
    let process = params.get("process").and_then(Value::as_str);
    let mut fields = json!({ "udid": device.udid, "after": cursor, "limit": limit });
    if let Some(process) = process {
        fields["process"] = process.into();
    }
    let generation = params
        .get("generation")
        .and_then(Value::as_i64)
        .or_else(|| manager.log_generation(agent_id, process));
    if let Some(generation) = generation {
        fields["generation"] = generation.into();
    }
    let mut reply = manager.request("logs", fields).await?;
    if let Some(generation) = reply.get("generation").and_then(Value::as_i64) {
        manager.set_log_generation(agent_id, process, generation);
    }
    if let Some(fields) = reply.as_object_mut() {
        if fields.remove("reset") == Some(Value::Bool(true)) && cursor > 0 {
            fields.insert(
                "restarted".into(),
                "the log started over, as when the device or Sikemux's simulator helper restarts, so these lines are read from its start; use the new cursor from here".into(),
            );
        }
    }
    cap_logs(&mut reply, MAX_LOG_BYTES);
    Ok(reply)
}

/// Keeps a log answer under `budget` bytes: long lines are cut, and lines past
/// the budget are left for the next read, with the cursor moved back to match.
pub(super) fn cap_logs(reply: &mut Value, budget: usize) {
    let Some(lines) = reply["lines"].as_array().cloned() else {
        return;
    };
    let mut kept = Vec::new();
    let mut used = 0;
    for line in &lines {
        let line = clip(line.as_str().unwrap_or_default(), MAX_LOG_LINE);
        used += line.len() + 4;
        if used > budget && !kept.is_empty() {
            break;
        }
        kept.push(Value::String(line));
    }
    let left = lines.len() - kept.len();
    reply["lines"] = kept.into();
    if left > 0 {
        if let Some(cursor) = reply["cursor"].as_u64() {
            reply["cursor"] = cursor.saturating_sub(left as u64).into();
        }
        reply["more"] = true.into();
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(limit).collect();
    clipped.push('…');
    clipped
}

async fn read_screen(manager: &SimulatorManager, device: &Device) -> Result<Read, String> {
    let reply = manager
        .request("tree", json!({ "udid": device.udid }))
        .await?;
    let size = app_size(&reply).or(device.screen);
    let (app, elements) = elements_from(&reply, size);
    Ok(Read {
        app,
        elements,
        size,
    })
}

/// The size of the frontmost app's window, which turns with the app, and stays
/// upright for an app that only runs upright.
pub(super) fn app_size(reply: &Value) -> Option<(f64, f64)> {
    let frame = &reply["elements"]
        .as_array()?
        .iter()
        .find(|element| element["type"] == "Application")?["frame"];
    let size = frame["width"].as_f64().zip(frame["height"].as_f64())?;
    (size.0 > 0.0 && size.1 > 0.0).then_some(size)
}

/// Reads the screen until two reads agree, so an animation has finished before
/// the agent is told what is on screen. After an action it first waits for the
/// screen to change from `before`, because an app can take a moment to start
/// leaving and would otherwise read as settled on its way out. Near the call's
/// deadline it answers with what it has.
async fn settled_state(
    manager: &SimulatorManager,
    agent_id: &str,
    before: Option<Read>,
    report: Report,
) -> Result<Value, String> {
    let device = attached(manager, agent_id)?;
    let mut latest = read_screen(manager, &device).await?;
    if let Some(before) = before {
        for _ in 0..CHANGE_READS {
            if latest != before || near_deadline() {
                break;
            }
            pause(SETTLE_STEP).await?;
            latest = read_screen(manager, &device).await?;
        }
    }
    for _ in 1..SETTLE_READS {
        if near_deadline() {
            break;
        }
        pause(SETTLE_STEP).await?;
        let next = read_screen(manager, &device).await?;
        let settled = next == latest && !launching(&(next.app.clone(), next.elements.clone()));
        latest = next;
        if settled {
            break;
        }
    }
    checkpoint()?;
    let numbered = manager.number(agent_id, &latest.elements);
    let previous = manager.last_read(agent_id);
    let mut state = json!({
        "device": format!("{} ({})", device.name, device.os),
        "app": latest.app,
    });
    match (report, previous) {
        (Report::Outcome, _) => {}
        (Report::Changes, Some((previous_app, previous))) if previous_app == latest.app => {
            let (changed, removed) = changes(&previous, &numbered);
            state["changes"] = if changed.is_empty() && removed.is_empty() {
                "none".into()
            } else {
                json!({ "elements": changed, "removed": removed })
            };
        }
        _ => state["elements"] = element_lines(&numbered).into(),
    }
    if let Some((width, height)) = latest.size {
        state["screen"] = json!({ "width": width, "height": height });
    }
    if !matches!(report, Report::Outcome) {
        manager.remember_read(agent_id, latest.app, numbered);
    }
    Ok(state)
}

/// How much an acting tool says about the screen afterwards, as the browser's tools do.
#[derive(Clone, Copy)]
pub(super) enum Report {
    /// What appeared, changed or went away since the agent's last read of this app.
    Changes,
    /// Only the device and the frontmost app, for a run of steps checked afterwards.
    Outcome,
    Full,
}

fn report(params: &Value) -> Result<Report, String> {
    match params.get("report").and_then(Value::as_str) {
        None | Some("changes") => Ok(Report::Changes),
        Some("outcome") => Ok(Report::Outcome),
        Some("full") => Ok(Report::Full),
        Some(other) => Err(format!(
            "report must be changes, outcome or full, not {other}"
        )),
    }
}

/// The lines of elements that are new or changed since `previous`, and of those that went away.
pub(super) fn changes(previous: &[Numbered], next: &[Numbered]) -> (Vec<String>, Vec<String>) {
    let before: HashMap<usize, &Element> = previous
        .iter()
        .map(|(number, element)| (*number, element))
        .collect();
    let changed: Vec<Numbered> = next
        .iter()
        .filter(|(number, element)| before.get(number) != Some(&element))
        .cloned()
        .collect();
    let mut changed_lines: Vec<String> = changed.iter().take(MAX_ELEMENTS).map(line).collect();
    if changed.len() > MAX_ELEMENTS {
        changed_lines.push(format!(
            "… {} more changed; call sim_state to read them all",
            changed.len() - MAX_ELEMENTS
        ));
    }
    let after: std::collections::HashSet<usize> = next.iter().map(|(number, _)| *number).collect();
    let gone: Vec<String> = previous
        .iter()
        .filter(|(number, _)| !after.contains(number))
        .map(|numbered| {
            let line = line(numbered);
            line.split_once(' ')
                .map_or(line.clone(), |(_, rest)| rest.to_owned())
        })
        .collect();
    let mut removed: Vec<String> = gone.iter().take(MAX_REMOVED).cloned().collect();
    if gone.len() > MAX_REMOVED {
        removed.push(format!("… {} more went away", gone.len() - MAX_REMOVED));
    }
    (changed_lines, removed)
}

/// What iOS itself draws, rather than an app, is the home screen when its app icons are
/// showing, and otherwise an alert, Control Center, the lock screen or a launch screen.
const HOME_SCREEN: &str = "Home Screen";
const SYSTEM: &str = "System";

/// An app on its way in shows a blank launch screen that no app owns yet, so
/// nothing but the status bar can be read.
pub(super) fn launching(screen: &(String, Vec<Element>)) -> bool {
    let (app, elements) = screen;
    app == SYSTEM
        && elements
            .iter()
            .all(|element| element.center.1 < STATUS_BAR_HEIGHT)
}

/// The frontmost app's name and its elements, from one accessibility read.
pub(super) fn elements_from(reply: &Value, screen: Option<(f64, f64)>) -> (String, Vec<Element>) {
    let mut app = String::new();
    let mut elements = Vec::new();
    let mut app_icons = false;
    for value in flattened(&reply["elements"]) {
        app_icons |= value["traits"]
            .as_array()
            .is_some_and(|traits| traits.iter().any(|trait_| trait_ == "LaunchIcon"));
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(readable)
                .unwrap_or_default()
        };
        let role = text("type");
        if role == "Application" {
            app = text("label");
            continue;
        }
        let frame = &value["frame"];
        let (Some(x), Some(y), Some(width), Some(height)) = (
            frame["x"].as_f64(),
            frame["y"].as_f64(),
            frame["width"].as_f64(),
            frame["height"].as_f64(),
        ) else {
            continue;
        };
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        let label = text("label");
        let value_text = match value.get("value") {
            Some(Value::String(text)) => readable(text),
            Some(Value::Number(number)) => number.to_string(),
            _ => String::new(),
        };
        if label.is_empty() && value_text.is_empty() && role == "GenericElement" {
            continue;
        }
        elements.push(Element {
            role,
            label,
            value: value_text,
            identifier: text("identifier"),
            enabled: value
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            center: ((x + width / 2.0).round(), (y + height / 2.0).round()),
            offscreen: screen.is_some_and(|(screen_width, screen_height)| {
                let (center_x, center_y) = (x + width / 2.0, y + height / 2.0);
                center_x < 0.0
                    || center_y < 0.0
                    || center_x > screen_width
                    || center_y > screen_height
            }),
        });
    }
    if app.is_empty() {
        app = if app_icons { HOME_SCREEN } else { SYSTEM }.to_owned();
    }
    (app, elements)
}

/// Every element of a read and those inside it, in the order a person reads them.
fn flattened(elements: &Value) -> Vec<&Value> {
    let mut all = Vec::new();
    for element in elements.as_array().into_iter().flatten() {
        all.push(element);
        all.extend(flattened(&element["children"]));
    }
    all
}

/// Text as a person reads it: without the invisible marks that set reading
/// direction, which Safari puts around an address.
fn readable(text: &str) -> String {
    text.chars()
        .filter(|character| !matches!(character, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
        .collect::<String>()
        .trim()
        .to_owned()
}

fn quoted(text: &str) -> String {
    format!(
        "\"{}\"",
        clip(text, MAX_TEXT)
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

/// One element's line: `3 Button "General" at (201, 418)`.
fn line((number, element): &Numbered) -> String {
    let mut line = format!("{number} {}", element.role);
    if !element.label.is_empty() {
        line.push_str(&format!(" {}", quoted(&element.label)));
    }
    if !element.value.is_empty() && element.value != element.label {
        line.push_str(&format!(" value={}", quoted(&element.value)));
    }
    if !element.identifier.is_empty() && element.identifier != element.label {
        line.push_str(&format!(" id={}", quoted(&element.identifier)));
    }
    if !element.enabled {
        line.push_str(" [disabled]");
    }
    if element.offscreen {
        line.push_str(" [offscreen]");
    }
    line.push_str(&format!(" at ({}, {})", element.center.0, element.center.1));
    line
}

/// One line per element, at most [`MAX_ELEMENTS`] of them.
pub(super) fn element_lines(elements: &[Numbered]) -> Vec<String> {
    let mut lines: Vec<String> = elements.iter().take(MAX_ELEMENTS).map(line).collect();
    if elements.len() > MAX_ELEMENTS {
        lines.push(format!(
            "… {} more; tap by label to reach them",
            elements.len() - MAX_ELEMENTS
        ));
    }
    lines
}

/// The points of a touch path, each with the given coordinates, from `params.points`.
fn path(params: &Value, keys: &[&str]) -> Result<Vec<Vec<f64>>, String> {
    let points: Vec<Vec<f64>> = params["points"]
        .as_array()
        .ok_or("points is required")?
        .iter()
        .map(|point| {
            keys.iter()
                .map(|key| point[*key].as_f64())
                .collect::<Option<Vec<f64>>>()
        })
        .collect::<Option<_>>()
        .ok_or_else(|| format!("each point needs {}", keys.join(", ")))?;
    if points.len() < 2 {
        return Err("a path needs at least two points".into());
    }
    Ok(points)
}

/// The points of a path spread evenly over `duration` seconds, each with its time
/// `t`, so the finger or fingers go down on the first and lift on the last.
pub(super) fn timed(
    points: &[Vec<f64>],
    duration: Option<f64>,
    point: impl Fn(&[f64]) -> Value,
) -> Vec<Value> {
    let step = duration.unwrap_or(0.5) / (points.len() - 1) as f64;
    points
        .iter()
        .enumerate()
        .map(|(index, coordinates)| {
            let mut timed = point(coordinates);
            timed["t"] = (step * index as f64).into();
            timed
        })
        .collect()
}

/// The element `name` labels or identifies. An exact label or identifier wins
/// over a partial label, and more than one equally good match is an error, so a
/// tap never lands on an element the agent did not mean.
pub(super) fn labelled(elements: &[Numbered], name: &str) -> Result<(f64, f64), String> {
    let exact = |(_, element): &&Numbered| element.label == name || element.identifier == name;
    let partial =
        |(_, element): &&Numbered| element.label.to_lowercase().contains(&name.to_lowercase());
    let mut matches: Vec<&Numbered> = elements.iter().filter(exact).collect();
    if matches.is_empty() {
        matches = elements.iter().filter(partial).collect();
    }
    let reachable: Vec<&Numbered> = matches
        .iter()
        .copied()
        .filter(|(_, element)| !element.offscreen)
        .collect();
    match reachable.as_slice() {
        [(_, element)] => Ok(element.center),
        [] if !matches.is_empty() => Err(format!(
            "\"{name}\" is off the screen; scroll it into view, then tap it"
        )),
        [] => Err(format!(
            "no element on screen is labelled \"{name}\"; it may be scrolled out of view, so scroll and read the screen again"
        )),
        several => {
            let listed: Vec<String> = several.iter().take(5).map(|numbered| line(numbered)).collect();
            Err(format!(
                "{} elements match \"{name}\": {}; tap one by its number",
                several.len(),
                listed.join("; ")
            ))
        }
    }
}

/// Where to tap: an element by its number, or a point.
pub(super) fn tap_point(
    elements: &[Numbered],
    index: Option<u64>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(f64, f64), String> {
    if let Some(index) = index {
        let (_, element) = elements
            .iter()
            .find(|(number, _)| *number as u64 == index)
            .ok_or_else(|| {
                format!("element {index} is not on the screen now; the screen changed since you read it, so read it again with sim_state")
            })?;
        if element.offscreen {
            return Err(format!(
                "element {index} is off the screen; scroll it into view, then read again"
            ));
        }
        return Ok(element.center);
    }
    match (x, y) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err("give an element number, a label, or both x and y".into()),
    }
}

pub(super) fn edge_warning(screen: Option<(f64, f64)>, from: (f64, f64)) -> Option<String> {
    let (width, height) = screen?;
    let edge = if from.1 >= height - EDGE {
        "bottom"
    } else if from.1 <= EDGE {
        "top"
    } else if from.0 <= EDGE {
        "left"
    } else if from.0 >= width - EDGE {
        "right"
    } else {
        return None;
    };
    Some(format!(
        "the swipe started at the {edge} edge, which iOS treats as a system gesture (home, Notification Center, Control Center or back) rather than scrolling"
    ))
}
