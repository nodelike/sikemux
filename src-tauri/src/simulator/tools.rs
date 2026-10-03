//! The agent's `sim_*` tools. Each agent attaches one simulator and acts on it
//! in device points; a person watching the same device sees every step.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use super::SimulatorManager;
use sikemux_core::cli::protocol::HarnessRequest;

/// The sidecar stops waiting at 70 s, so a cold boot that runs longer is
/// reported as still going and finishes in the background.
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const SETTLE_STEP: Duration = Duration::from_millis(300);
/// Enough reads for an app to finish launching, about three and a half seconds.
const SETTLE_READS: usize = 12;
/// Elements whose centre is above this are the status bar's: time, signal, battery.
const STATUS_BAR_HEIGHT: f64 = 60.0;
/// How many reads, a step apart, to wait for a launched app to come to the front: ten seconds.
const LAUNCH_READS: usize = 33;
/// How many reads, a step apart, to wait for an action to change the screen.
const CHANGE_READS: usize = 5;
/// A swipe that starts this close to an edge is sent as a system gesture, as the helper decides.
const EDGE: f64 = 10.0;
const MAX_ELEMENTS: usize = 200;
const DEFAULT_LOG_LINES: usize = 200;
/// As many lines as the helper keeps, read whole when one process's lines are picked out.
const KEPT_LOG_LINES: usize = 2000;
/// An agent cannot see the person's screen, so attaching says where the device went.
const SHOWN_ON_DESK: &str =
    "live on your desk in Sikemux, beside the person, who sees what you do and can use it too";
/// Tells the window a device was turned, so its view turns with it.
pub const ROTATED_EVENT: &str = "simulator-rotated";
/// Tells the window an agent let go of a simulator, so its desk can close the tab.
pub const DETACHED_EVENT: &str = "simulator-detached";
/// Tells the window an agent attached a simulator, so its desk can show it.
pub const ATTACHED_EVENT: &str = "simulator-attached";

pub fn execute(app: &AppHandle, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .as_deref()
        .ok_or("simulator tools need the agent's id")?;
    let manager = app.state::<SimulatorManager>();
    let result = tauri::async_runtime::block_on(run(
        &manager,
        agent_id,
        &request.project,
        &request.method,
        &request.params,
    ));
    if request.method == "sim.attach" && result.is_ok() {
        if let Some(device) = manager.attached(agent_id) {
            let _ = app.emit_to(
                "main",
                ATTACHED_EVENT,
                json!({
                    "agentId": agent_id,
                    "udid": device.udid,
                    "name": device.name,
                    "os": device.os,
                    "screen": device.screen.map(|(width, height)| json!({ "width": width, "height": height })),
                }),
            );
        }
    }
    if request.method == "sim.rotate" && result.is_ok() {
        if let (Some(device), Some(orientation)) = (
            manager.attached(agent_id),
            request.params.get("orientation").and_then(Value::as_str),
        ) {
            let _ = app.emit_to(
                "main",
                ROTATED_EVENT,
                json!({ "udid": device.udid, "orientation": orientation }),
            );
        }
    }
    if request.method == "sim.detach" {
        if let Ok(detached) = &result {
            let _ = app.emit_to(
                "main",
                DETACHED_EVENT,
                json!({ "agentId": agent_id, "udid": detached["udid"] }),
            );
        }
    }
    result
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
            let attached = manager.attached(agent_id).map(|device| device.udid);
            let devices = list_devices(manager).await?;
            Ok(json!({
                "devices": devices
                    .iter()
                    .map(|device| json!({
                        "udid": device.udid,
                        "name": device.name,
                        "os": device.os,
                        "booted": device.booted,
                        "attached": attached.as_deref() == Some(device.udid.as_str()),
                    }))
                    .collect::<Vec<_>>(),
            }))
        }
        "sim.attach" => {
            let devices = list_devices(manager).await?;
            let device = choose_device(&devices, text("device"))?.clone();
            let boot = manager.request("boot", json!({ "udid": device.udid }));
            match tokio::time::timeout(BOOT_TIMEOUT, boot).await {
                Ok(booted) => booted?,
                Err(_) => {
                    return Err(format!(
                        "{} is still booting; call sim_attach again in a moment",
                        device.name
                    ))
                }
            };
            manager.attach(agent_id, device.clone());
            if let Ok(turned) = manager
                .request("orientation", json!({ "udid": device.udid }))
                .await
            {
                if let Some(orientation) = turned["orientation"].as_str() {
                    manager.set_orientation(&device.udid, orientation);
                }
            }
            let _ = manager
                .request("logs", json!({ "udid": device.udid, "limit": 1 }))
                .await;
            let mut state = settled_state(manager, agent_id, None, Report::Full).await?;
            state["shown"] = SHOWN_ON_DESK.into();
            Ok(state)
        }
        "sim.state" => settled_state(manager, agent_id, None, Report::Full).await,
        "sim.tap" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let point = match text("label") {
                Some(label) => labelled(&before.1, label)?,
                None => tap_point(
                    &manager.elements(agent_id),
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
            manager.request("swipe", fields).await?;
            let mut state = settled_state(manager, agent_id, Some(before), report(params)?).await?;
            if let Some(warning) = edge_warning(manager.screen_for(&device), from) {
                state["warning"] = warning.into();
            }
            Ok(state)
        }
        "sim.type" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let typed = text("text").ok_or("text is required")?;
            manager
                .request("text", json!({ "udid": device.udid, "text": typed }))
                .await?;
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
        "sim.launch" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let mut fields = json!({ "udid": device.udid, "bundleId": text("bundleId").ok_or("bundleId is required")? });
            for key in ["arguments", "environment"] {
                if let Some(value) = params.get(key) {
                    fields[key] = value.clone();
                }
            }
            let launched = manager.request("launch", fields).await?;
            if let Some(pid) = launched["pid"].as_i64() {
                wait_for_front(manager, &device, pid).await;
            }
            let mut state = settled_state(manager, agent_id, Some(before), report(params)?).await?;
            state["pid"] = launched["pid"].clone();
            Ok(state)
        }
        "sim.terminate" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let bundle = text("bundleId").ok_or("bundleId is required")?;
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
            let path = Path::new(project).join(text("path").ok_or("path is required")?);
            if !path.exists() {
                return Err(format!("no app at {}", path.display()));
            }
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
            let device = manager.detach(agent_id).ok_or("no simulator is attached")?;
            Ok(
                json!({ "detached": format!("{} ({})", device.name, device.os), "udid": device.udid }),
            )
        }
        "sim.logs" => {
            let device = attached(manager, agent_id)?;
            let cursor = params.get("cursor").and_then(Value::as_u64).unwrap_or(0);
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_LOG_LINES, |limit| limit as usize);
            let Some(process) = text("process") else {
                return manager
                    .request(
                        "logs",
                        json!({ "udid": device.udid, "after": cursor, "limit": limit }),
                    )
                    .await;
            };
            // The helper follows the whole device from sim.attach, so reading one
            // process's lines picks them out of that same feed rather than
            // starting a fresh, native, process-only tail too late to catch
            // what it logged before this was first asked for.
            let read = manager
                .request(
                    "logs",
                    json!({ "udid": device.udid, "after": cursor, "limit": KEPT_LOG_LINES }),
                )
                .await?;
            Ok(lines_of(&read, cursor, process, limit))
        }
        "sim.rotate" => {
            let device = attached(manager, agent_id)?;
            let orientation = text("orientation").ok_or("orientation is required")?;
            rotate(manager, &device.udid, orientation).await?;
            settled_state(manager, agent_id, None, Report::Full).await
        }
        other => Err(format!("unknown simulator method {other}")),
    }
}

/// What `workspace_inspect` says about simulators: whether this Mac can run
/// them, and the device the agent has attached, so an agent does not reach for
/// `sim_*` on a Mac without Xcode.
pub fn inspect(manager: &SimulatorManager, agent_id: Option<&str>) -> Value {
    let attached = agent_id.and_then(|agent_id| manager.attached(agent_id));
    json!({
        "available": super::offered(),
        "attached": attached.map(|device| json!({ "udid": device.udid, "name": device.name, "os": device.os })),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Device {
    pub udid: String,
    pub name: String,
    pub os: String,
    pub booted: bool,
    /// Width and height in points.
    pub screen: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Element {
    pub role: String,
    pub label: String,
    pub value: String,
    pub identifier: String,
    pub enabled: bool,
    pub center: (f64, f64),
    /// Scrolled out of view, or under the screen's edge, so it cannot be tapped.
    pub offscreen: bool,
}

pub(super) async fn list_devices(manager: &SimulatorManager) -> Result<Vec<Device>, String> {
    let reply = manager.request("devices", json!({})).await?;
    Ok(reply["devices"]
        .as_array()
        .map(|devices| devices.iter().filter_map(device_from).collect())
        .unwrap_or_default())
}

fn device_from(value: &Value) -> Option<Device> {
    let screen = value.get("screen").and_then(|screen| {
        Some((
            screen.get("width")?.as_f64()?,
            screen.get("height")?.as_f64()?,
        ))
    });
    Some(Device {
        udid: value.get("udid")?.as_str()?.to_owned(),
        name: value.get("name")?.as_str()?.to_owned(),
        os: value
            .get("runtime")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        booted: value.get("state").and_then(Value::as_str) == Some("booted"),
        screen,
    })
}

/// A named device by udid or name, newest iOS first when a name repeats; with no
/// name, the iPhone already booted, else an iPhone on the newest iOS.
pub(super) fn choose_device<'a>(
    devices: &'a [Device],
    wanted: Option<&str>,
) -> Result<&'a Device, String> {
    let newest = |candidates: Vec<&'a Device>| {
        candidates.into_iter().max_by(|a, b| {
            (a.booted, os_version(&a.os))
                .partial_cmp(&(b.booted, os_version(&b.os)))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    };
    let chosen = match wanted {
        Some(wanted) => devices
            .iter()
            .find(|device| device.udid == wanted)
            .or_else(|| {
                newest(
                    devices
                        .iter()
                        .filter(|device| device.name == wanted)
                        .collect(),
                )
            }),
        None => newest(
            devices
                .iter()
                .filter(|device| device.name.starts_with("iPhone"))
                .collect(),
        ),
    };
    chosen.ok_or_else(|| match wanted {
        Some(wanted) => format!(
            "no simulator is named {wanted}; sim_devices lists them, and Xcode's Devices and Simulators window adds more"
        ),
        None => "no iPhone simulator is installed; add one in Xcode's Devices and Simulators window".into(),
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

fn attached(manager: &SimulatorManager, agent_id: &str) -> Result<Device, String> {
    manager
        .attached(agent_id)
        .ok_or_else(|| "no simulator is attached; call sim_attach first".into())
}

pub(super) type Screen = (String, Vec<Element>);

/// A newly installed app takes a few seconds to open the first time, longer than
/// any screen change is waited for, so a launch waits until its process is in front.
async fn wait_for_front(manager: &SimulatorManager, device: &Device, pid: i64) {
    for _ in 0..LAUNCH_READS {
        let front = manager
            .request("tree", json!({ "udid": device.udid }))
            .await
            .ok()
            .and_then(|reply| frontmost_pid(&reply));
        if front == Some(pid) {
            return;
        }
        tokio::time::sleep(SETTLE_STEP).await;
    }
}

pub(super) fn frontmost_pid(reply: &Value) -> Option<i64> {
    reply["elements"]
        .as_array()?
        .iter()
        .find(|element| element["type"] == "Application")?["pid"]
        .as_i64()
}

async fn read_screen(manager: &SimulatorManager, device: &Device) -> Result<Screen, String> {
    let reply = manager
        .request("tree", json!({ "udid": device.udid }))
        .await?;
    Ok(elements_from(&reply, manager.screen_for(device)))
}

/// Reads the screen until two reads agree, so an animation has finished before
/// the agent is told what is on screen. After an action it first waits for the
/// screen to change from `before`, because an app can take a moment to start
/// leaving and would otherwise read as settled on its way out.
async fn settled_state(
    manager: &SimulatorManager,
    agent_id: &str,
    before: Option<Screen>,
    report: Report,
) -> Result<Value, String> {
    let device = attached(manager, agent_id)?;
    let mut latest = read_screen(manager, &device).await?;
    if let Some(before) = before {
        for _ in 0..CHANGE_READS {
            if latest != before {
                break;
            }
            tokio::time::sleep(SETTLE_STEP).await;
            latest = read_screen(manager, &device).await?;
        }
    }
    for _ in 1..SETTLE_READS {
        tokio::time::sleep(SETTLE_STEP).await;
        let next = read_screen(manager, &device).await?;
        let settled = next == latest && !launching(&next);
        latest = next;
        if settled {
            break;
        }
    }
    let (app, elements) = latest;
    let previous = manager.last_read(agent_id);
    let mut state = json!({
        "device": format!("{} ({})", device.name, device.os),
        "app": app,
    });
    match (report, previous) {
        (Report::Outcome, _) => {}
        (Report::Changes, Some((previous_app, previous))) if previous_app == app => {
            let (changed, removed) = changes(&previous, &elements);
            state["changes"] = if changed.is_empty() && removed.is_empty() {
                "none".into()
            } else {
                json!({ "elements": changed, "removed": removed })
            };
        }
        _ => state["elements"] = element_lines(&elements).into(),
    }
    if let Some((width, height)) = manager.screen_for(&device) {
        state["screen"] = json!({ "width": width, "height": height });
    }
    manager.remember_read(agent_id, app, elements);
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

/// The lines of elements that are new or changed, numbered as in `next`, and the
/// lines of those that went away. An element keeps its identity by role, label
/// and identifier, so one that moved or took a new value reads as changed.
pub(super) fn changes(previous: &[Element], next: &[Element]) -> (Vec<String>, Vec<String>) {
    let identity = |element: &Element| {
        (
            element.role.clone(),
            element.label.clone(),
            element.identifier.clone(),
        )
    };
    let mut unmatched: Vec<Option<&Element>> = previous.iter().map(Some).collect();
    let lines = element_lines(next);
    let mut changed = Vec::new();
    for (index, element) in next.iter().enumerate() {
        let matched = unmatched
            .iter_mut()
            .find(|candidate| {
                candidate.is_some_and(|candidate| identity(candidate) == identity(element))
            })
            .and_then(Option::take);
        if matched != Some(element) {
            if let Some(line) = lines.get(index) {
                changed.push(line.clone());
            }
        }
    }
    let removed = unmatched
        .into_iter()
        .flatten()
        .map(|element| {
            let line = &element_lines(std::slice::from_ref(element))[0];
            line.split_once(' ')
                .map_or(line.clone(), |(_, rest)| rest.to_owned())
        })
        .collect();
    (changed, removed)
}

/// What iOS itself draws, rather than an app, is the home screen when its app icons are
/// showing, and otherwise an alert, Control Center, the lock screen or a launch screen.
const HOME_SCREEN: &str = "Home Screen";
const SYSTEM: &str = "System";

/// An app on its way in shows a blank launch screen that no app owns yet, so
/// nothing but the status bar can be read.
pub(super) fn launching(screen: &Screen) -> bool {
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
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One line per element: `3 Button "General" at (201, 418)`.
pub(super) fn element_lines(elements: &[Element]) -> Vec<String> {
    let mut lines: Vec<String> = elements
        .iter()
        .take(MAX_ELEMENTS)
        .enumerate()
        .map(|(index, element)| {
            let mut line = format!("{index} {}", element.role);
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
        })
        .collect();
    if elements.len() > MAX_ELEMENTS {
        lines.push(format!(
            "… {} more; tap by label to reach them",
            elements.len() - MAX_ELEMENTS
        ));
    }
    lines
}

/// Turns the device and remembers which way, so touches and the screen's shape follow.
pub(super) async fn rotate(
    manager: &SimulatorManager,
    udid: &str,
    orientation: &str,
) -> Result<(), String> {
    manager
        .request(
            "rotate",
            json!({ "udid": udid, "orientation": orientation }),
        )
        .await?;
    manager.set_orientation(udid, orientation);
    Ok(())
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
pub(super) fn labelled(elements: &[Element], name: &str) -> Result<(f64, f64), String> {
    let exact = |element: &&Element| element.label == name || element.identifier == name;
    let partial = |element: &&Element| element.label.to_lowercase().contains(&name.to_lowercase());
    let mut matches: Vec<&Element> = elements.iter().filter(exact).collect();
    if matches.is_empty() {
        matches = elements.iter().filter(partial).collect();
    }
    let reachable: Vec<&Element> = matches
        .iter()
        .copied()
        .filter(|element| !element.offscreen)
        .collect();
    match reachable.as_slice() {
        [element] => Ok(element.center),
        [] if !matches.is_empty() => Err(format!(
            "\"{name}\" is off the screen; scroll it into view, then tap it"
        )),
        [] => Err(format!(
            "no element on screen is labelled \"{name}\"; it may be scrolled out of view, so scroll and read the screen again"
        )),
        several => {
            let listed: Vec<String> = several
                .iter()
                .take(5)
                .map(|element| {
                    format!(
                        "{} at ({}, {})",
                        quoted(if element.label.is_empty() {
                            &element.identifier
                        } else {
                            &element.label
                        }),
                        element.center.0,
                        element.center.1
                    )
                })
                .collect();
            Err(format!(
                "{} elements match \"{name}\": {}; tap one by its coordinates",
                several.len(),
                listed.join(", ")
            ))
        }
    }
}

/// A process's own lines from a read of the device's log, at most `limit`, with
/// the cursor to read on from. The helper numbers lines from 1 and hands back
/// the number of the last one it read.
pub(super) fn lines_of(read: &Value, after: u64, process: &str, limit: usize) -> Value {
    let lines = read["lines"].as_array().cloned().unwrap_or_default();
    let last = read["cursor"].as_u64().unwrap_or(after);
    let first = last + 1 - lines.len() as u64;
    let mut kept = Vec::new();
    let mut cursor = last;
    for (number, line) in (first..).zip(&lines) {
        let Some(text) = line.as_str() else { continue };
        if process_of(text) != Some(process) {
            continue;
        }
        if kept.len() == limit {
            cursor = number - 1;
            break;
        }
        kept.push(text.to_owned());
    }
    json!({
        "lines": kept,
        "cursor": cursor,
        "more": cursor < last || read["more"].as_bool().unwrap_or(false),
    })
}

/// A compact log line names its process before the bracket holding its id:
/// `2026-10-03 14:00:00.123 Df Maps[1234:5678] message`.
fn process_of(line: &str) -> Option<&str> {
    line.split_whitespace()
        .find_map(|word| word.split_once('['))
        .map(|(name, _)| name)
        .filter(|name| !name.is_empty())
}

/// Where to tap: an element number from the latest read, or a point.
pub(super) fn tap_point(
    elements: &[Element],
    index: Option<u64>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(f64, f64), String> {
    if let Some(index) = index {
        let element = elements.get(index as usize).ok_or_else(|| {
            format!("no element {index} in the latest read; call sim_state for current numbers")
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
        _ => Err("give an element index, a label, or both x and y".into()),
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
