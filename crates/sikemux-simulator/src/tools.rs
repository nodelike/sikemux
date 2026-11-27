//! The agent's `sim` tool. Each agent holds one simulator and acts on it
//! in device points; a person watching the same device sees every step.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::android::is_android;
use super::claims::Claim;
use super::load::Wait;
use super::{checkpoint, pause, remaining, Queued, SimulatorManager};
use sikemux_base::cli::protocol::{HarnessRequest, SIM_CANCEL_METHOD, SIM_OFFERED_METHOD};

/// A cold boot can outlast one call, so it is reported as still going and finishes in the background.
const BOOT_MARGIN: Duration = Duration::from_secs(12);
/// Time kept back for reading the screen once an action is done.
const SETTLE_MARGIN: Duration = Duration::from_secs(4);
/// How often an agent waiting for a simulator looks again.
const WAIT_STEP: Duration = Duration::from_secs(2);
/// A device reports itself booted a little before its screen can be read; this waits up to 30 s.
pub(crate) const SCREEN_READY_READS: usize = 30;
pub(crate) const SCREEN_READY_STEP: Duration = Duration::from_secs(1);
/// Enough reads for an app to finish launching, about three and a half seconds.
const SETTLE_READS: usize = 12;
/// Bounded retries for a transient empty app tree (such as a modal fade-out): ~500 ms.
const EMPTY_READS: usize = 5;
/// Elements whose centre is above this are the status bar's: time, signal, battery.
const STATUS_BAR_HEIGHT: f64 = 60.0;
/// How many reads, a step apart, to wait for a launched app to come to the front: ten seconds.
const LAUNCH_READS: usize = 33;
/// A launch this recent that has not reached the front yet is still under way, and is not sent again.
const LAUNCH_IN_FLIGHT: Duration = Duration::from_secs(60);
/// How many reads, a step apart, to wait for an action to change the screen.
const CHANGE_READS: usize = 5;
/// What the helper says when a read finds no app in front.
const NO_APP_IN_FRONT: &str = "No translation object";
const NO_APP_WAIT: Duration = Duration::from_secs(30);
/// How many reads, a step apart, to wait for Home to bring the Home Screen up from an app:
/// it always does, but a busy device starts leaving the app late. Six seconds.
const HOME_READS: usize = 20;
/// How many reads, a step apart, to wait for iOS's own keyboard to leave once a
/// hardware key is pressed: six seconds.
const KEYBOARD_READS: usize = 20;
/// How many pictures, a step apart, to wait for a turn to come round: three seconds.
const TURN_READS: usize = 20;

/// How long to wait between two looks at a device.
#[derive(Clone, Copy)]
pub(crate) struct Steps {
    pub settle: Duration,
    pub turn: Duration,
    pub empty: Duration,
}

impl Steps {
    pub const DEVICE: Self = Self {
        settle: Duration::from_millis(300),
        turn: Duration::from_millis(150),
        empty: Duration::from_millis(100),
    };
    /// For a stand-in helper in tests, whose screen never moves.
    #[cfg(all(test, unix))]
    pub const STAND_IN: Self = Self {
        settle: Duration::from_millis(10),
        turn: Duration::from_millis(10),
        empty: Duration::from_millis(10),
    };
}

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

pub fn execute(manager: &SimulatorManager, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .as_deref()
        .ok_or("simulator tools need the agent's id")?;
    if request.method == SIM_OFFERED_METHOD {
        let offered = manager.offered();
        manager.note_offered(agent_id, offered);
        return Ok(json!({ "offered": offered }));
    }
    if request.method == SIM_CANCEL_METHOD {
        manager.cancel(agent_id);
        return Ok(json!({}));
    }
    if let Some(reason) = refusal(manager.enabled(), manager.unavailable()) {
        return Err(reason);
    }
    let acting = drives(&request.method);
    if acting {
        manager.emit(ACTING_EVENT, json!({ "agentId": agent_id, "acting": true }));
    }
    let result = sikemux_base::runtime::block_on(manager.in_turn(
        agent_id,
        run(
            manager,
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
                            "platform": if is_android(&device.udid) { "android" } else { "ios" },
                            "attached": mine,
                            "inUseBy": holder.filter(|_| !mine).map(holder_text),
                        })
                    })
                    .collect::<Vec<_>>(),
            }))
        }
        "sim.attach" => {
            let fresh = params.get("fresh").and_then(Value::as_bool) == Some(true);
            attach(manager, agent_id, project, text("device"), fresh).await
        }
        "sim.state" => settled_state(manager, agent_id, None, Report::Full).await,
        "sim.tap" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let numbered = manager.number(agent_id, &before.elements);
            let (point, intent) = match text("label") {
                Some(label) => {
                    let element = labelled_element(&numbered, label)?.clone();
                    (element.center, TapIntent::Label(label.to_owned(), element))
                }
                None => match params.get("index").and_then(Value::as_u64) {
                    Some(index) => {
                        let (_, target) = tap_target(&numbered, Some(index), None, None)?;
                        let element = target.ok_or("element is not on the screen now")?.clone();
                        (element.center, TapIntent::Index(element))
                    }
                    None => {
                        let (point, _) = tap_target(&numbered, None, number("x"), number("y"))?;
                        let hit = interactive_element_at(&before, point).cloned();
                        (point, TapIntent::Point(point, hit))
                    }
                },
            };
            let targeted_dismiss_region = intent.targets_dismiss_region();
            let duration = number("duration");
            let report = report(params)?;
            send_tap(manager, &device.udid, point, duration).await?;
            let after_first = settle_read(manager, &device, Some(&before)).await?;
            let retap = if popover_was_dismissed(&before, &after_first)
                && !targeted_dismiss_region
                && !near_deadline()
            {
                popover_retap_point(&intent, &before, &after_first)
            } else {
                None
            };
            let (mut state, note) = if let Some(retap_point) = retap {
                send_tap(manager, &device.udid, retap_point, duration).await?;
                let after_second = settle_read(manager, &device, Some(&after_first)).await?;
                let (state, _) =
                    finish_state(manager, agent_id, &device, after_second, report).await?;
                (state, Some(POPOVER_RETAPPED_NOTE))
            } else {
                let (state, after) =
                    finish_state(manager, agent_id, &device, after_first, report).await?;
                let note = popover_dismissed_note_for_tap(
                    &intent,
                    &before,
                    &after,
                    targeted_dismiss_region,
                );
                (state, note)
            };
            if let (Some(note), Some(state)) = (note, state.as_object_mut()) {
                state.insert("note".to_owned(), note.into());
            }
            Ok(state)
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
            if let (Some(duration), Some(fields)) = (number("duration"), fields.as_object_mut()) {
                fields.insert("duration".to_owned(), duration.into());
            }
            let warning = edge_warning(before.size, from);
            manager.request("swipe", fields).await?;
            let mut state = settled_state(manager, agent_id, Some(before), report(params)?).await?;
            if let (Some(warning), Some(state)) = (warning, state.as_object_mut()) {
                state.insert("warning".to_owned(), warning.into());
            }
            Ok(state)
        }
        "sim.type" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let raw = text("text").ok_or("text is required")?;
            let submit = params.get("submit").and_then(Value::as_bool) == Some(true);
            let steps = type_steps(raw, submit);
            // The first key from a hardware keyboard sends iOS's own keyboard away, and keys
            // that arrive while it goes are lost, so a lone Shift sends it away first.
            let shift = json!({ "udid": device.udid, "key": "Shift" });
            let fields = if manager.request("key", shift).await.is_ok() {
                keyboard_gone(manager, &device, before.clone()).await?
            } else {
                before.clone()
            };
            for step in &steps {
                match step {
                    TypeStep::Text(chunk) => {
                        manager
                            .request("text", json!({ "udid": device.udid, "text": chunk }))
                            .await?;
                    }
                    TypeStep::Key(key) => {
                        manager
                            .request("key", json!({ "udid": device.udid, "key": key }))
                            .await?;
                    }
                }
            }
            let (mut state, after) =
                settled_state_with_read(manager, agent_id, Some(before.clone()), report(params)?)
                    .await?;
            let printable = raw.chars().any(|typed| !typed.is_control());
            let submitted = submit || steps.contains(&TypeStep::Key("Enter"));
            if printable && !landed(before.keyboard, &fields, &after, raw, submitted) {
                return Err(NOT_TYPED.into());
            }
            let warning = if !submit && trailing_escaped_newline(raw) {
                Some(
                    "text ended with literal \\n instead of a newline; pass submit: true (or a real newline) to press Return"
                        .to_owned(),
                )
            } else {
                dropped_text_warning(&fields, &after, raw, submitted)
            };
            if let (Some(warning), Some(state)) = (warning, state.as_object_mut()) {
                state.insert("warning".to_owned(), warning.into());
            }
            Ok(state)
        }
        "sim.button" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let button = text("button").ok_or("button is required")?;
            manager
                .request("button", json!({ "udid": device.udid, "button": button }))
                .await?;
            if button == "home" && before.app != HOME_SCREEN {
                for _ in 0..HOME_READS {
                    if near_deadline() || read_screen(manager, &device).await?.app == HOME_SCREEN {
                        break;
                    }
                    pause(manager.steps().settle).await?;
                }
            }
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
                "data": shot.get("jpeg"),
                "mimeType": "image/jpeg",
                "title": format!("{} ({})", device.name, device.os),
                "width": shot.get("width"),
                "height": shot.get("height"),
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
            let (mut reply, bundle) = if !is_android(&device.udid) {
                let (udid, app) = (device.udid.clone(), path.clone());
                let installed = tokio::task::spawn_blocking(move || {
                    super::bundle::already_installed(&udid, &app)
                        .then(|| super::bundle::bundle_id(&app))
                        .flatten()
                })
                .await
                .ok()
                .flatten();
                if let Some(bundle) = installed {
                    (
                        json!({ "bundleId": bundle, "unchanged": "this build is already on the device" }),
                        Some(bundle),
                    )
                } else {
                    let reply = manager
                        .request("install", json!({ "udid": device.udid, "path": path }))
                        .await?;
                    let bundle = reply
                        .get("bundleId")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    (reply, bundle)
                }
            } else {
                let reply = manager
                    .request("install", json!({ "udid": device.udid, "path": path }))
                    .await?;
                (reply, None)
            };
            if let Some(bundle) = reply.get("bundleId").and_then(Value::as_str) {
                manager.forget_launch(agent_id, bundle);
            }
            if !is_android(&device.udid) {
                let (udid, app, bundle_id) = (
                    device.udid.clone(),
                    path.clone(),
                    bundle.unwrap_or_default(),
                );
                let conflicts = tokio::task::spawn_blocking(move || {
                    let custom: Vec<String> = super::bundle::bundle_schemes(&app)
                        .into_iter()
                        .filter(|scheme| is_custom_scheme(scheme))
                        .collect();
                    super::bundle::duplicate_scheme_conflicts(&udid, &bundle_id, &custom)
                })
                .await
                .unwrap_or_default();
                if let (Some(warning), Some(fields)) = (
                    duplicate_scheme_install_warning(&conflicts),
                    reply.as_object_mut(),
                ) {
                    fields.insert("warning".to_owned(), warning.into());
                }
            }
            Ok(reply)
        }
        "sim.uninstall" => {
            let device = attached(manager, agent_id)?;
            let bundle = text("bundleId").ok_or("bundleId is required")?;
            manager.forget_launch(agent_id, bundle);
            manager
                .request(
                    "uninstall",
                    json!({ "udid": device.udid, "bundleId": bundle }),
                )
                .await?;
            Ok(json!({ "uninstalled": bundle }))
        }
        "sim.openUrl" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let url = text("url").ok_or("url is required")?;
            let open_reply = manager
                .request("openUrl", json!({ "udid": device.udid, "url": url }))
                .await?;
            let (mut state, after) =
                settled_state_with_read(manager, agent_id, Some(before.clone()), report(params)?)
                    .await?;
            if !is_android(&device.udid) {
                if let Some(scheme) = custom_url_scheme(url) {
                    let helper_claimants: Vec<String> = open_reply
                        .get("matchingBundleIds")
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default();
                    let claimants = if !helper_claimants.is_empty() {
                        Some(helper_claimants)
                    } else {
                        let (udid, scheme_owned) = (device.udid.clone(), scheme.clone());
                        tokio::task::spawn_blocking(move || {
                            super::bundle::installed_scheme_claimants(&udid, &scheme_owned)
                        })
                        .await
                        .ok()
                        .flatten()
                    };
                    if after.app == HOME_SCREEN || after.app == SYSTEM {
                        let installed = claimants.as_ref().map(|ids| !ids.is_empty());
                        if let Some(error) =
                            unhandled_custom_url_scheme_error(url, &before, &after, installed)
                        {
                            return Err(error);
                        }
                    }
                    if let Some(claimants) = claimants.filter(|ids| ids.len() > 1) {
                        let foreground_bundle = match after.bundle_id.clone() {
                            Some(id) => Some(id),
                            None => match after.pid {
                                Some(pid) => tokio::task::spawn_blocking(move || {
                                    super::bundle::bundle_id_for_pid(pid)
                                })
                                .await
                                .ok()
                                .flatten(),
                                None => None,
                            },
                        };
                        if let (Some(warning), Some(fields)) = (
                            duplicate_scheme_open_warning(
                                &scheme,
                                &claimants,
                                foreground_bundle.as_deref(),
                                &after.app,
                            ),
                            state.as_object_mut(),
                        ) {
                            fields.insert("warning".to_owned(), warning.into());
                        }
                    }
                }
            }
            Ok(state)
        }
        "sim.touchPath" => {
            let device = attached(manager, agent_id)?;
            let before = read_screen(manager, &device).await?;
            let points = path(params, &["x", "y"])?;
            let timed = timed(
                &points,
                number("duration"),
                |point| json!({ "x": point.first(), "y": point.get(1) }),
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
            let timed = timed(&points, number("duration"), |point| {
                json!({
                    "x": point.first(),
                    "y": point.get(1),
                    "x2": point.get(2),
                    "y2": point.get(3),
                })
            });
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
            let before = read_screen(manager, &device)
                .await
                .ok()
                .and_then(|read| read.size);
            manager
                .request(
                    "orientation",
                    json!({ "udid": device.udid, "orientation": orientation }),
                )
                .await?;
            turned(
                manager,
                &device,
                before,
                orientation.starts_with("landscape"),
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
        .unwrap_or_else(|| manager.offered());
    let reason = (!available).then(|| {
        manager.unavailable().unwrap_or_else(|| {
            if manager.enabled() {
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
    /// Booting or shutting down.
    pub busy: bool,
    /// Booted now or before, so it starts without a first boot.
    pub warm: bool,
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

/// A frame in device points: `(x, y, width, height)`.
pub(super) type Frame = (f64, f64, f64, f64);

/// One accessibility read: the frontmost app, its elements, the size of the
/// app's window, which follows the app when it turns, whether iOS's own
/// keyboard is on screen, and its text fields with focus metadata.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Read {
    pub app: String,
    pub pid: Option<i64>,
    pub bundle_id: Option<String>,
    pub elements: Vec<Element>,
    pub frames: Vec<Frame>,
    pub size: Option<(f64, f64)>,
    pub keyboard: bool,
    pub text_fields: Vec<TextFieldState>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct TextFieldState {
    pub identifier: String,
    pub label: String,
    pub value: String,
    pub focused: bool,
    pub secure: bool,
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
            let candidates: Vec<usize> = (self.known.iter().zip(&taken).enumerate())
                .filter(|(_, ((_, known), taken))| !**taken && key(known) == wanted)
                .map(|(index, _)| index)
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
                    .filter_map(|index| {
                        let (_, known) = self.known.get(index)?;
                        Some((index, distance(known.center, element.center)))
                    })
                    .filter(|(_, apart)| *apart <= SAME_PLACE)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(index, _)| index)
            };
            let kept = chosen.and_then(|index| {
                *taken.get_mut(index)? = true;
                self.known.get(index).map(|(number, _)| *number)
            });
            let number = match kept {
                Some(number) => number,
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
    Ok(reply
        .get("devices")
        .and_then(Value::as_array)
        .map(|devices| devices.iter().filter_map(device_from).collect())
        .unwrap_or_default())
}

fn device_from(value: &Value) -> Option<Device> {
    let udid = value.get("udid")?.as_str()?.to_owned();
    let state = value.get("state").and_then(Value::as_str);
    let booted = state == Some("booted");
    Some(Device {
        warm: booted || is_android(&udid) || super::load::booted_before(&udid),
        name: value.get("name")?.as_str()?.to_owned(),
        os: value
            .get("runtime")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        booted,
        busy: state == Some("busy"),
        screen: None,
        udid,
    })
}

fn holder_text(claim: &Claim) -> String {
    format!("another agent in {}", claim.project_name())
}

pub(super) fn in_use(device: &Device, claim: &Claim) -> String {
    format!(
        "{} is in use by {} until its turn ends; leave out device for a free one, or pick another from sim devices",
        device.name,
        holder_text(claim)
    )
}

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
/// desk, else the one its project used before, else the warmest free one of
/// the device Sikemux made for the project and the iPhones booted now or
/// before, else a new one. A running iPhone Sikemux did not boot, such as one
/// the person runs from Xcode, comes after the project's own once that was booted before.
pub(super) fn pick_device<'a>(
    devices: &'a [Device],
    held: &HashMap<String, Claim>,
    desk: Option<&str>,
    remembered: Option<&str>,
    project_device: &str,
    booted_by_sikemux: &HashSet<String>,
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
    let ours = |device: &Device| device.name == project_device;
    let warmth = |device: &Device| {
        (
            device.booted && booted_by_sikemux.contains(&device.udid),
            ours(device) && device.warm,
            device.booted,
            device.warm,
            ours(device),
            os_version(&device.os),
        )
    };
    devices
        .iter()
        .filter(free)
        .filter(|device| ours(device) || (device.warm && device.name.starts_with("iPhone")))
        .max_by(|a, b| {
            warmth(a)
                .partial_cmp(&warmth(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map_or(Pick::Create, Pick::Use)
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
                "no device is named {wanted}; sim devices lists them. Xcode's Devices and Simulators window adds iOS simulators, Android Studio's Device Manager adds Android ones"
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
    fresh: bool,
) -> Result<Value, String> {
    let line = InLine { manager, agent_id };
    let mut reclaimed = HashSet::new();
    let (mut device, mut starting) = loop {
        let devices = list_devices(manager).await?;
        let chosen = choose_for(manager, agent_id, project, wanted, fresh, &devices)?;
        if let Some(device) = chosen
            .as_ref()
            .filter(|device| device.booted || device.busy)
        {
            break (device.clone(), None);
        }
        let cold = chosen.as_ref().is_none_or(|device| !device.warm);
        let target_udid = chosen.as_ref().map(|device| device.udid.as_str());
        match manager.room(agent_id, &devices, cold, target_udid) {
            Ok(starting) => match chosen {
                Some(device) => break (device, Some(starting)),
                None => {
                    break (
                        create_device(manager, project, &devices).await?,
                        Some(starting),
                    )
                }
            },
            Err(queued @ (Queued::TooLong { .. } | Queued::Blocked { .. })) => {
                return Err(queued_text(&queued));
            }
            Err(
                ref queued @ Queued::Waiting {
                    place: 0,
                    wait: Some(Wait::Full { .. }),
                    ..
                },
            ) => {
                if let Some((idle, was_mine)) =
                    manager.reclaim_candidate(agent_id, target_udid, &devices, &reclaimed)
                {
                    reclaimed.insert(idle.udid.clone());
                    if was_mine {
                        manager.release(agent_id);
                    }
                    if manager
                        .request("shutdown", json!({ "udid": idle.udid }))
                        .await
                        .is_ok()
                    {
                        manager.forget_booted(&idle.udid);
                    }
                    continue;
                }
                if near_deadline() {
                    line.keep_place();
                    return Err(queued_text(queued));
                }
                pause(WAIT_STEP).await?;
            }
            Err(queued) if near_deadline() => {
                line.keep_place();
                return Err(queued_text(&queued));
            }
            Err(_) => pause(WAIT_STEP).await?,
        }
    };
    drop(line);
    manager.hold(agent_id, project, &device)?;
    if !device.booted {
        manager.note_booted(&device.udid);
        if let Some(starting) = starting.as_mut() {
            starting.boots(&device.udid);
        }
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
                    "{} is still booting; call sim attach again in a moment",
                    device.name
                ))
            }
            Err(error) => {
                manager.release(agent_id);
                return Err(error);
            }
        }
    }
    drop(starting);
    let screen = manager
        .request("screen", json!({ "udid": device.udid }))
        .await?;
    let side = |name: &str| screen.get(name).and_then(Value::as_f64);
    device.screen = side("width").zip(side("height"));
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
    if let Some(state) = state.as_object_mut() {
        state.insert("shown".to_owned(), SHOWN_ON_DESK.into());
    }
    Ok(state)
}

/// Takes the agent out of the line for a simulator however its attach ends.
struct InLine<'a> {
    manager: &'a SimulatorManager,
    agent_id: &'a str,
}

impl InLine<'_> {
    /// The agent stays in line, to come back for its turn.
    fn keep_place(self) {
        std::mem::forget(self);
    }
}

impl Drop for InLine<'_> {
    fn drop(&mut self) {
        self.manager.leave_line(self.agent_id);
    }
}

/// The device `sim attach` takes, or none when the project needs one made.
fn choose_for(
    manager: &SimulatorManager,
    agent_id: &str,
    project: &str,
    wanted: Option<&str>,
    fresh: bool,
    devices: &[Device],
) -> Result<Option<Device>, String> {
    let held = manager.held_by_others(agent_id);
    let current = manager
        .attached(agent_id)
        .and_then(|current| devices.iter().find(|device| device.udid == current.udid));
    match (wanted, current) {
        (Some(wanted), _) => {
            let device = choose_device(devices, wanted)?;
            if let Some(claim) = held.get(&device.udid) {
                return Err(in_use(device, claim));
            }
            if !device.warm && !fresh {
                let warm = devices.iter().find(|other| {
                    other.warm
                        && other.name.starts_with("iPhone")
                        && !held.contains_key(&other.udid)
                });
                if let Some(warm) = warm {
                    return Err(format!(
                        "{} was never booted, and a first boot is the heaviest work a simulator does. {} is ready; attach it, or pass fresh: true if you need this model or iOS version",
                        device.name, warm.name
                    ));
                }
            }
            Ok(Some(device.clone()))
        }
        (None, Some(current)) => Ok(Some(current.clone())),
        (None, None) => {
            let (desk, remembered) = (
                manager.desk_device(agent_id),
                manager.project_device(project),
            );
            let chosen = [&desk, &remembered].into_iter().flatten().any(|udid| {
                devices
                    .iter()
                    .any(|device| &device.udid == udid && !held.contains_key(udid))
            });
            let android = !super::capable();
            if android && !chosen && manager.android().sdk().is_none() {
                return Err(super::android::NO_SDK.into());
            }
            let candidates: Vec<Device> = devices
                .iter()
                .filter(|device| chosen || is_android(&device.udid) == android)
                .cloned()
                .collect();
            match pick_device(
                &candidates,
                &held,
                desk.as_deref(),
                remembered.as_deref(),
                &project_device_name(project),
                &manager.booted_by_sikemux(),
            ) {
                Pick::Use(device) => Ok(Some(device.clone())),
                Pick::Create if android => any_android(&candidates, &held).map(Some),
                Pick::Create => Ok(None),
            }
        }
    }
}

/// What an agent waiting for a simulator is told.
pub(super) fn queued_text(queued: &Queued) -> String {
    let reason = |wait: &Option<Wait>, machine: &super::load::Machine, slots: &[String]| {
        let holders = if slots.is_empty() {
            String::new()
        } else {
            format!(": {}", slots.join(", "))
        };
        match wait {
            Some(Wait::Full { running, limit }) if machine.busy() => format!(
                "{} is busy (load {:.0} on {} cores) and already runs {running} simulator{} (limit {limit}{holders})",
                this_computer(),
                machine.load.unwrap_or_default(),
                machine.cores,
                if *running == 1 { "" } else { "s" },
            ),
            Some(Wait::Full { running, limit }) => format!(
                "{} runs {running} simulator{}, as many as its cores and memory take now (limit {limit}{holders})",
                this_computer(),
                if *running == 1 { "" } else { "s" },
            ),
            Some(Wait::Starting) => {
                format!("another simulator is starting, and a first boot goes alone{holders}")
            }
            None => format!("agents ahead of you start theirs first{holders}"),
        }
    };
    match queued {
        Queued::TooLong {
            wait,
            machine,
            slots,
        } => format!(
            "you waited {} minutes and no simulator freed up: {}. Ask the person which to shut down; never boot one with simctl or Xcode yourself",
            super::MOST_WAIT.as_secs() / 60,
            reason(wait, machine, slots),
        ),
        Queued::Blocked {
            wait,
            machine,
            slots,
        } => format!(
            "no simulator slot is available: {}. Ask the person to shut down an unused simulator; never boot one with simctl or Xcode yourself",
            reason(wait, machine, slots),
        ),
        Queued::Waiting {
            place,
            wait,
            machine,
            slots,
        } => {
            let line = match *place {
                0 => "you are next in line".to_owned(),
                ahead => format!(
                    "{ahead} agent{} ahead of you",
                    if ahead == 1 { " is" } else { "s are" }
                ),
            };
            format!(
                "waiting for a simulator: {line}, as {}. Call sim attach again within a minute to keep your place; never boot one with simctl or Xcode yourself",
                reason(wait, machine, slots),
            )
        }
    }
}

fn this_computer() -> &'static str {
    if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this computer"
    }
}

/// A free Android device for an agent that named none: one already running, else the virtual
/// device on the newest Android.
fn any_android(devices: &[Device], held: &HashMap<String, Claim>) -> Result<Device, String> {
    devices
        .iter()
        .filter(|device| !held.contains_key(&device.udid))
        .max_by(|a, b| {
            (a.booted, os_version(&a.os))
                .partial_cmp(&(b.booted, os_version(&b.os)))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
        .ok_or_else(|| {
            if devices.is_empty() {
                super::android::NO_DEVICE.to_owned()
            } else {
                "every Android device is in use by another agent; ask the person".to_owned()
            }
        })
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
    let udid = tokio::task::spawn_blocking(move || {
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
        .ok_or_else(|| "no simulator is attached; call sim attach first".into())
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
            let mut fields = Map::new();
            fields.insert("udid".to_owned(), device.udid.clone().into());
            fields.insert("bundleId".to_owned(), bundle.into());
            for key in ["arguments", "environment"] {
                if let Some(value) = params.get(key) {
                    fields.insert(key.to_owned(), value.clone());
                }
            }
            let fields = Value::Object(fields);
            manager.note_launch(agent_id, bundle, None);
            let budget = remaining()
                .unwrap_or(Duration::from_secs(60))
                .saturating_sub(SETTLE_MARGIN);
            match manager.request_within("launch", fields, budget).await {
                Ok(launched) => {
                    let pid = launched.get("pid").and_then(Value::as_i64);
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
    if let Some(state) = state.as_object_mut() {
        state.insert("pid".to_owned(), pid.into());
        if !in_front {
            state.insert(
                "launching".to_owned(),
                format!(
                    "{bundle} is still launching; call sim state in a moment, or sim launch again, which waits for this launch rather than starting another"
                )
                .into(),
            );
        }
    }
    Ok(state)
}

/// A newly installed app takes a few seconds to open the first time, longer than
/// any screen change is waited for, so a launch waits until its process is in front
/// and has drawn more than a blank launch screen.
async fn wait_for_front(
    manager: &SimulatorManager,
    device: &Device,
    pid: i64,
) -> Result<bool, String> {
    for _ in 0..LAUNCH_READS {
        if near_deadline() {
            return Ok(false);
        }
        let reply = manager
            .request("tree", json!({ "udid": device.udid }))
            .await
            .ok();
        let drawn = reply.as_ref().is_some_and(|reply| {
            frontmost_pid(reply) == Some(pid) && !launching(&elements_from(reply, None))
        });
        if drawn {
            return Ok(true);
        }
        pause(manager.steps().settle).await?;
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

pub(super) fn frontmost_bundle_id(reply: &Value) -> Option<String> {
    reply["elements"]
        .as_array()?
        .iter()
        .find(|element| element["type"] == "Application")?["bundleId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// A path the agent gave, which must name something inside its project.
pub(super) fn inside_project(project: &str, path: &str) -> Result<PathBuf, String> {
    let confined = sikemux_process::files::confine_to_project(project, path).map_err(|error| {
        if std::fs::metadata(project).is_ok() {
            format!("no app at {}", Path::new(project).join(path).display())
        } else {
            error.to_string()
        }
    })?;
    confined.ok_or_else(|| {
        "the app must be inside the project; build it into the project's folder, such as with xcodebuild -derivedDataPath build, or ./gradlew assembleDebug for Android".into()
    })
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
    let mut fields = Map::new();
    fields.insert("udid".to_owned(), device.udid.clone().into());
    fields.insert("after".to_owned(), cursor.into());
    fields.insert("limit".to_owned(), limit.into());
    if let Some(process) = process {
        fields.insert("process".to_owned(), process.into());
    }
    let generation = params
        .get("generation")
        .and_then(Value::as_i64)
        .or_else(|| manager.log_generation(agent_id, process));
    if let Some(generation) = generation {
        fields.insert("generation".to_owned(), generation.into());
    }
    let mut reply = manager.request("logs", Value::Object(fields)).await?;
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

/// Reads the screen, looking again while no app is in front, as happens while one
/// opens or leaves: on a busy device that has been seen to last well over ten seconds.
async fn read_screen(manager: &SimulatorManager, device: &Device) -> Result<Read, String> {
    let started = Instant::now();
    let reply = loop {
        match manager
            .request("tree", json!({ "udid": device.udid }))
            .await
        {
            Err(error)
                if error.contains(NO_APP_IN_FRONT)
                    && started.elapsed() < NO_APP_WAIT
                    && !near_deadline() =>
            {
                pause(manager.steps().settle).await?;
            }
            reply => break reply?,
        }
    };
    let size = app_size(&reply).or(device.screen);
    let (app, elements, frames) = screen_from(&reply, size);
    let pid = frontmost_pid(&reply);
    let bundle_id = frontmost_bundle_id(&reply);
    let elements_read = reply.get("elements").map(flattened).unwrap_or_default();
    let keyboard = elements_read
        .iter()
        .any(|element| has_trait(element, "KeyboardKey"));
    let text_fields = text_fields_from(&elements_read);
    Ok(Read {
        app,
        pid,
        bundle_id,
        elements,
        frames,
        size,
        keyboard,
        text_fields,
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

/// Waits for a turn to finish: for the app in front to report its turned shape, when the turn
/// changes it, then for the picture on screen to stop moving. A busy device can start the turn
/// late, and a touch sent before the screen has come round is dropped.
async fn turned(
    manager: &SimulatorManager,
    device: &Device,
    before: Option<(f64, f64)>,
    wide: bool,
) -> Result<(), String> {
    if before.is_some_and(|(width, height)| (width > height) != wide) {
        for _ in 0..TURN_READS {
            let size = read_screen(manager, device).await?.size;
            if near_deadline() || size.is_some_and(|(width, height)| (width > height) == wide) {
                break;
            }
            pause(manager.steps().turn).await?;
        }
    }
    let mut last = None;
    for _ in 0..TURN_READS {
        if near_deadline() {
            break;
        }
        let shot = manager
            .request(
                "screenshot",
                json!({ "udid": device.udid, "format": "jpeg", "pointSize": true }),
            )
            .await?;
        let picture = shot.get("jpeg").cloned().unwrap_or_default();
        if last.as_ref() == Some(&picture) {
            break;
        }
        last = Some(picture);
        pause(manager.steps().turn).await?;
    }
    Ok(())
}

const NOT_TYPED: &str = "The text did not reach a field: no text field's value changed. Tap the field to focus it, then type again.";

const TEXT_ROLES: [&str; 5] = [
    "TextField",
    "SecureTextField",
    "SearchField",
    "TextView",
    "TextArea",
];

#[derive(Debug, PartialEq, Eq)]
pub(super) enum TypeStep {
    Text(String),
    Key(&'static str),
}

/// Splits `text` into bounded printable chunks and explicit hardware key presses so
/// line endings, Tab, Backspace, and `submit` trigger real key events on both iOS and Android.
pub(super) fn type_steps(text: &str, submit: bool) -> Vec<TypeStep> {
    let mut steps = Vec::new();
    let mut chunk = String::new();
    let mut chunk_len = 0usize;
    let flush = |steps: &mut Vec<TypeStep>, chunk: &mut String, chunk_len: &mut usize| {
        if !chunk.is_empty() {
            steps.push(TypeStep::Text(std::mem::take(chunk)));
            *chunk_len = 0;
        }
    };
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                flush(&mut steps, &mut chunk, &mut chunk_len);
                steps.push(TypeStep::Key("Enter"));
            }
            '\n' => {
                flush(&mut steps, &mut chunk, &mut chunk_len);
                steps.push(TypeStep::Key("Enter"));
            }
            '\t' => {
                flush(&mut steps, &mut chunk, &mut chunk_len);
                steps.push(TypeStep::Key("Tab"));
            }
            '\u{8}' => {
                flush(&mut steps, &mut chunk, &mut chunk_len);
                steps.push(TypeStep::Key("Backspace"));
            }
            other => {
                chunk.push(other);
                chunk_len += 1;
                if chunk_len >= TEXT_CHUNK {
                    flush(&mut steps, &mut chunk, &mut chunk_len);
                }
            }
        }
    }
    flush(&mut steps, &mut chunk, &mut chunk_len);
    if submit {
        steps.push(TypeStep::Key("Enter"));
    }
    steps
}

pub(super) fn trailing_escaped_newline(text: &str) -> bool {
    let Some(prefix) = text.strip_suffix('n') else {
        return false;
    };
    let backslashes = prefix.chars().rev().take_while(|&c| c == '\\').count();
    backslashes % 2 == 1
}

fn has_trait(element: &Value, name: &str) -> bool {
    element["traits"]
        .as_array()
        .is_some_and(|traits| traits.iter().any(|trait_| trait_ == name))
}

fn value_text(element: &Value) -> String {
    match element.get("value") {
        Some(Value::String(text)) => readable(text),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

pub(super) fn text_fields_from(elements: &[&Value]) -> Vec<TextFieldState> {
    elements
        .iter()
        .filter_map(|element| {
            let role = element
                .get("type")
                .and_then(Value::as_str)
                .map(readable)
                .unwrap_or_default();
            if !TEXT_ROLES.contains(&role.as_str()) && !has_trait(element, "TextEntry") {
                return None;
            }
            let frame = &element["frame"];
            let (Some(width), Some(height)) = (frame["width"].as_f64(), frame["height"].as_f64())
            else {
                return None;
            };
            if width <= 0.0 || height <= 0.0 {
                return None;
            }
            let text_field = |key: &str| {
                element
                    .get(key)
                    .and_then(Value::as_str)
                    .map(readable)
                    .unwrap_or_default()
            };
            let focused = element.get("focused").and_then(Value::as_bool) == Some(true)
                || has_trait(element, "IsEditing");
            Some(TextFieldState {
                identifier: text_field("identifier"),
                label: text_field("label"),
                value: value_text(element),
                focused,
                secure: role == "SecureTextField",
            })
        })
        .collect()
}

fn expected_typed(raw: &str) -> String {
    let mut out = String::new();
    for character in raw.chars() {
        match character {
            '\u{8}' => {
                out.pop();
            }
            '\r' | '\n' | '\t' => {}
            other if !other.is_control() => out.push(other),
            _ => {}
        }
    }
    readable(&out)
}

fn non_status_bar_changed(before: &[Element], after: &[Element]) -> bool {
    fn body(elements: &[Element]) -> Vec<&Element> {
        elements
            .iter()
            .filter(|element| element.center.1 >= STATUS_BAR_HEIGHT)
            .collect()
    }
    body(before) != body(after)
}

/// Whether typing reached a text field.
/// A screen without text fields, such as a game drawing its own, cannot tell and counts as typed.
/// On iOS, an empty UITextField reports its placeholder as AXValue, so typing text equal to that
/// placeholder leaves AXValue unchanged; we accept that only when focus, the active keyboard,
/// or a non-status-bar UI change confirms delivery.
pub(super) fn landed(
    before_keyboard: bool,
    before: &Read,
    after: &Read,
    raw: &str,
    submitted: bool,
) -> bool {
    fn values(read: &Read) -> Vec<(&str, &str, &str)> {
        read.text_fields
            .iter()
            .map(|field| {
                (
                    field.identifier.as_str(),
                    field.label.as_str(),
                    field.value.as_str(),
                )
            })
            .collect()
    }
    let (was, now) = (values(before), values(after));
    if (was.is_empty() && now.is_empty()) || was != now {
        return true;
    }
    let any_focused = before
        .text_fields
        .iter()
        .chain(&after.text_fields)
        .any(|field| field.focused);
    let ui_changed = non_status_bar_changed(&before.elements, &after.elements);
    if submitted && (any_focused || before_keyboard) && ui_changed {
        return true;
    }
    let expected = expected_typed(raw);
    if expected.is_empty() {
        return false;
    }
    before
        .text_fields
        .iter()
        .zip(&after.text_fields)
        .any(|(was, now)| {
            if now.value != expected {
                return false;
            }
            let focused = was.focused || now.focused;
            if any_focused {
                focused
            } else {
                before_keyboard || ui_changed
            }
        })
}

/// Non-fatal readback warning when a single non-secure text field changed on a non-submitted
/// `sim type` and read back a shorter subsequence missing at least one alphanumeric character.
pub(super) fn dropped_text_warning(
    before: &Read,
    after: &Read,
    raw: &str,
    submitted: bool,
) -> Option<String> {
    if submitted || raw.contains('\u{8}') || raw.contains('\t') {
        return None;
    }
    let expected = expected_typed(raw);
    if expected.chars().count() < 2 {
        return None;
    }
    let mut changed = before
        .text_fields
        .iter()
        .zip(&after.text_fields)
        .filter(|(was, now)| {
            was.identifier == now.identifier && was.label == now.label && was.value != now.value
        });
    let (was, now) = changed.next()?;
    if changed.next().is_some() || now.secure || now.value.contains('•') {
        return None;
    }
    let entered = if !was.value.is_empty() {
        now.value
            .strip_prefix(&was.value)
            .filter(|suffix| !suffix.is_empty())
            .unwrap_or(now.value.as_str())
    } else {
        now.value.as_str()
    };
    if entered.is_empty() || entered.chars().count() >= expected.chars().count() {
        return None;
    }
    if !is_subsequence_missing_alnum(entered, &expected) {
        return None;
    }
    Some(format!(
        "typed \"{expected}\" but the field reads \"{entered}\" (a character may have been dropped)"
    ))
}

fn is_subsequence_missing_alnum(actual: &str, expected: &str) -> bool {
    let mut actual_chars = actual.chars().peekable();
    let mut missing_alnum = false;
    for expected_char in expected.chars() {
        if actual_chars
            .peek()
            .is_some_and(|actual_char| actual_char.eq_ignore_ascii_case(&expected_char))
        {
            actual_chars.next();
        } else if expected_char.is_ascii_alphanumeric() {
            missing_alnum = true;
        }
    }
    actual_chars.peek().is_none() && missing_alnum
}

/// The screen once iOS's own keyboard has left it. It starts leaving a second
/// or more after the key that sends it away, longer than two reads take.
async fn keyboard_gone(
    manager: &SimulatorManager,
    device: &Device,
    mut latest: Read,
) -> Result<Read, String> {
    for _ in 0..KEYBOARD_READS {
        if !latest.keyboard || near_deadline() {
            break;
        }
        pause(manager.steps().settle).await?;
        latest = read_screen(manager, device).await?;
    }
    Ok(latest)
}

/// Reads the screen until two reads agree, so an animation has finished before
/// the agent is told what is on screen. After an action it first waits for the
/// screen to change from `before`, because an app can take a moment to start
/// leaving and would otherwise read as settled on its way out. An app's blank
/// launch screen is waited out for as long as the call allows, since a busy
/// device can take most of that to open an app. Near the call's deadline it
/// answers with what it has.
async fn settled_state(
    manager: &SimulatorManager,
    agent_id: &str,
    before: Option<Read>,
    report: Report,
) -> Result<Value, String> {
    settled_state_with_read(manager, agent_id, before, report)
        .await
        .map(|(state, _)| state)
}

async fn settled_state_with_read(
    manager: &SimulatorManager,
    agent_id: &str,
    before: Option<Read>,
    report: Report,
) -> Result<(Value, Read), String> {
    let device = attached(manager, agent_id)?;
    let latest = settle_read(manager, &device, before.as_ref()).await?;
    finish_state(manager, agent_id, &device, latest, report).await
}

async fn settle_read(
    manager: &SimulatorManager,
    device: &Device,
    before: Option<&Read>,
) -> Result<Read, String> {
    let mut latest = read_screen(manager, device).await?;
    if let Some(before) = before {
        for _ in 0..CHANGE_READS {
            if &latest != before || near_deadline() {
                break;
            }
            pause(manager.steps().settle).await?;
            latest = read_screen(manager, device).await?;
        }
    }
    let retry_empty = |read: &Read| {
        read.elements.is_empty()
            && !is_launching(read)
            && before.is_none_or(|before| !before.elements.is_empty() || before.app != read.app)
    };
    let mut reads = 1;
    let mut empty_reads = 0;
    while !near_deadline() {
        let step = if retry_empty(&latest) && empty_reads < EMPTY_READS {
            manager.steps().empty
        } else {
            manager.steps().settle
        };
        pause(step).await?;
        let next = read_screen(manager, device).await?;
        let opening = is_launching(&next);
        let waiting_empty = retry_empty(&next) && {
            empty_reads += 1;
            empty_reads < EMPTY_READS
        };
        let settled = next == latest && !opening && !waiting_empty;
        latest = next;
        reads += 1;
        if settled || (reads >= SETTLE_READS && !opening) {
            break;
        }
    }
    checkpoint()?;
    Ok(latest)
}

async fn finish_state(
    manager: &SimulatorManager,
    agent_id: &str,
    device: &Device,
    latest: Read,
    report: Report,
) -> Result<(Value, Read), String> {
    let numbered = manager.number(agent_id, &latest.elements);
    let previous = manager.last_read(agent_id);
    let mut state = Map::new();
    state.insert(
        "device".to_owned(),
        format!("{} ({})", device.name, device.os).into(),
    );
    state.insert("app".to_owned(), latest.app.clone().into());
    match (report, previous) {
        (Report::Outcome, _) => {}
        (Report::Changes, Some((previous_app, previous))) if previous_app == latest.app => {
            let (changed, removed) = changes(&previous, &numbered);
            let changes = if changed.is_empty() && removed.is_empty() {
                "none".into()
            } else {
                json!({ "elements": changed, "removed": removed })
            };
            state.insert("changes".to_owned(), changes);
        }
        _ => {
            state.insert("elements".to_owned(), element_lines(&numbered).into());
        }
    }
    if let Some((width, height)) = latest.size {
        state.insert(
            "screen".to_owned(),
            json!({ "width": width, "height": height }),
        );
    }
    if !matches!(report, Report::Outcome) {
        manager.remember_read(agent_id, latest.app.clone(), numbered);
    }
    Ok((Value::Object(state), latest))
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
            "… {} more changed; call sim state to read them all",
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

fn is_launching(read: &Read) -> bool {
    read.app == SYSTEM
        && read
            .elements
            .iter()
            .all(|element| element.center.1 < STATUS_BAR_HEIGHT)
}

/// The frontmost app's name and its elements, from one accessibility read.
pub(super) fn elements_from(reply: &Value, screen: Option<(f64, f64)>) -> (String, Vec<Element>) {
    let (app, elements, _) = screen_from(reply, screen);
    (app, elements)
}

pub(super) fn screen_from(
    reply: &Value,
    screen: Option<(f64, f64)>,
) -> (String, Vec<Element>, Vec<Frame>) {
    let mut app = String::new();
    let mut elements = Vec::new();
    let mut frames = Vec::new();
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
        frames.push((x, y, width, height));
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
    (app, elements, frames)
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
            if let Some(timed) = timed.as_object_mut() {
                timed.insert("t".to_owned(), (step * index as f64).into());
            }
            timed
        })
        .collect()
}

const POPOVER_DISMISS_REGION: &str = "PopoverDismissRegion";
const POPOVER_DISMISSED_NOTE: &str = "This tap dismissed an iPadOS popover overlay (PopoverDismissRegion) and likely did not activate the target underneath; tap again to trigger it.";
const POPOVER_RETAPPED_NOTE: &str = "First tap dismissed an iPadOS popover overlay (PopoverDismissRegion), so the target was tapped again to activate it.";

const INTERACTIVE_ROLES: [&str; 17] = [
    "Button",
    "Link",
    "Cell",
    "CheckBox",
    "RadioButton",
    "Switch",
    "TextField",
    "SecureTextField",
    "SearchField",
    "TextView",
    "TextArea",
    "Tab",
    "MenuItem",
    "Slider",
    "Stepper",
    "PopUpButton",
    "DisclosureTriangle",
];

enum TapIntent {
    Label(String, Element),
    Index(Element),
    Point((f64, f64), Option<Element>),
}

impl TapIntent {
    fn targets_dismiss_region(&self) -> bool {
        match self {
            Self::Label(_, element) | Self::Index(element) => is_popover_dismiss_region(element),
            Self::Point(_, Some(element)) => is_popover_dismiss_region(element),
            Self::Point(_, None) => false,
        }
    }
}

async fn send_tap(
    manager: &SimulatorManager,
    udid: &str,
    point: (f64, f64),
    duration: Option<f64>,
) -> Result<(), String> {
    let mut fields = json!({ "udid": udid, "x": point.0, "y": point.1 });
    if let (Some(duration), Some(fields)) = (duration, fields.as_object_mut()) {
        fields.insert("duration".to_owned(), duration.into());
    }
    manager.request("tap", fields).await?;
    Ok(())
}

fn is_popover_dismiss_region(element: &Element) -> bool {
    element.identifier == POPOVER_DISMISS_REGION
}

fn popover_was_dismissed(before: &Read, after: &Read) -> bool {
    before.app == after.app
        && before.elements.iter().any(is_popover_dismiss_region)
        && !after.elements.iter().any(is_popover_dismiss_region)
}

fn interactive_element_at(read: &Read, point: (f64, f64)) -> Option<&Element> {
    read.elements
        .iter()
        .zip(&read.frames)
        .filter(|(element, (x, y, width, height))| {
            !is_popover_dismiss_region(element)
                && !element.offscreen
                && element.enabled
                && INTERACTIVE_ROLES.contains(&element.role.as_str())
                && point.0 >= *x
                && point.0 <= *x + *width
                && point.1 >= *y
                && point.1 <= *y + *height
        })
        .min_by(|(_, a), (_, b)| (a.2 * a.3).total_cmp(&(b.2 * b.3)))
        .map(|(element, _)| element)
}

fn same_identity(a: &Element, b: &Element) -> bool {
    a.role == b.role && a.label == b.label && a.identifier == b.identifier
}

fn still_untriggered_target(before: &Element, after: &Element) -> bool {
    !is_popover_dismiss_region(after)
        && !after.offscreen
        && after.enabled
        && same_identity(before, after)
        && after.value == before.value
}

fn find_same_element<'a>(
    before_elements: &[Element],
    after_elements: &'a [Element],
    target: &Element,
) -> Option<&'a Element> {
    let matches: Vec<&Element> = after_elements
        .iter()
        .filter(|candidate| same_identity(candidate, target))
        .collect();
    if matches.len() == 1
        && before_elements
            .iter()
            .filter(|candidate| same_identity(candidate, target))
            .count()
            == 1
    {
        return matches.first().copied();
    }
    matches
        .into_iter()
        .map(|candidate| (candidate, distance(target.center, candidate.center)))
        .filter(|(_, apart)| *apart <= SAME_PLACE)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(candidate, _)| candidate)
}

fn popover_retap_point(intent: &TapIntent, before: &Read, after: &Read) -> Option<(f64, f64)> {
    match intent {
        TapIntent::Label(label, before_el) => {
            let numbered: Vec<Numbered> = after.elements.iter().cloned().enumerate().collect();
            let after_el = labelled_element(&numbered, label).ok()?;
            still_untriggered_target(before_el, after_el).then_some(after_el.center)
        }
        TapIntent::Index(before_el) => {
            let after_el = find_same_element(&before.elements, &after.elements, before_el)?;
            still_untriggered_target(before_el, after_el).then_some(after_el.center)
        }
        TapIntent::Point(point, Some(before_el)) => {
            let after_el = interactive_element_at(after, *point)?;
            still_untriggered_target(before_el, after_el).then_some(*point)
        }
        TapIntent::Point(_, None) => None,
    }
}

fn popover_dismissed_note_for_tap(
    intent: &TapIntent,
    before: &Read,
    after: &Read,
    targeted_dismiss_region: bool,
) -> Option<&'static str> {
    if targeted_dismiss_region || !popover_was_dismissed(before, after) {
        return None;
    }
    let target_still_present = match intent {
        TapIntent::Label(_, before_el) | TapIntent::Index(before_el) => {
            find_same_element(&before.elements, &after.elements, before_el).is_some()
        }
        TapIntent::Point(_, Some(before_el)) => {
            find_same_element(&before.elements, &after.elements, before_el).is_some()
        }
        TapIntent::Point(_, None) => false,
    };
    target_still_present.then_some(POPOVER_DISMISSED_NOTE)
}

const SYSTEM_URL_SCHEMES: [&str; 43] = [
    "http",
    "https",
    "about",
    "data",
    "file",
    "blob",
    "tel",
    "telprompt",
    "mailto",
    "sms",
    "mms",
    "facetime",
    "facetime-audio",
    "maps",
    "geo",
    "prefs",
    "app-prefs",
    "app-settings",
    "settings",
    "itms-apps",
    "itms-appss",
    "itms-services",
    "shortcuts",
    "calshow",
    "mobilenotes",
    "photos-redirect",
    "shoebox",
    "wallet",
    "clock-alarm",
    "clock-timer",
    "weather",
    "music",
    "podcasts",
    "books",
    "ibooks",
    "news",
    "applenews",
    "stocks",
    "home",
    "findmy",
    "x-web-search",
    "x-apple-reminderkit",
    "imessage",
];

pub(super) fn is_custom_scheme(scheme: &str) -> bool {
    !scheme.is_empty() && !SYSTEM_URL_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
}

pub(super) fn custom_url_scheme(url: &str) -> Option<String> {
    let (scheme, _) = url.trim().split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let lower = scheme.to_ascii_lowercase();
    is_custom_scheme(&lower).then_some(lower)
}

pub(super) fn duplicate_scheme_open_warning(
    scheme: &str,
    matching_bundle_ids: &[String],
    foreground_bundle_id: Option<&str>,
    foreground_app: &str,
) -> Option<String> {
    if matching_bundle_ids.len() < 2 {
        return None;
    }
    let opened = match foreground_bundle_id {
        Some(bundle_id) => format!("opened {bundle_id} in the foreground"),
        None if !foreground_app.is_empty() => format!("foreground app: \"{foreground_app}\""),
        None => "foreground app unknown".to_owned(),
    };
    Some(format!(
        "Multiple installed apps claim scheme '{scheme}://': {} ({opened}). Remove stale bundles with sim uninstall.",
        matching_bundle_ids.join(", ")
    ))
}

pub(super) fn duplicate_scheme_install_warning(
    conflicts: &[(String, Vec<String>)],
) -> Option<String> {
    if conflicts.is_empty() {
        return None;
    }
    let parts: Vec<String> = conflicts
        .iter()
        .map(|(scheme, ids)| format!("'{scheme}://' ({})", ids.join(", ")))
        .collect();
    Some(format!(
        "Multiple installed apps claim custom URL scheme(s): {}. Deep links may open the wrong app; remove stale bundles with sim uninstall.",
        parts.join("; ")
    ))
}

pub(super) fn unhandled_custom_url_scheme_error(
    url: &str,
    before: &Read,
    after: &Read,
    installed_handles: Option<bool>,
) -> Option<String> {
    let scheme = custom_url_scheme(url)?;
    if installed_handles == Some(true) {
        return None;
    }
    let on_springboard = |app: &str| app == HOME_SCREEN || app == SYSTEM;
    if !on_springboard(&after.app) {
        return None;
    }
    let alert_appeared =
        after.app == SYSTEM && non_status_bar_changed(&before.elements, &after.elements);
    if alert_appeared {
        return None;
    }
    if installed_handles == Some(false) || on_springboard(&before.app) {
        return Some(format!(
            "no installed app handles scheme '{scheme}://'; install the app first with sim install"
        ));
    }
    None
}

/// The element `name` labels or identifies. An exact label or identifier wins
/// over a partial label, and more than one equally good match is an error, so a
/// tap never lands on an element the agent did not mean.
#[cfg(all(test, unix))]
pub(super) fn labelled(elements: &[Numbered], name: &str) -> Result<(f64, f64), String> {
    labelled_element(elements, name).map(|element| element.center)
}

fn labelled_element<'a>(elements: &'a [Numbered], name: &str) -> Result<&'a Element, String> {
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
        [(_, element)] => Ok(element),
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
#[cfg(all(test, unix))]
pub(super) fn tap_point(
    elements: &[Numbered],
    index: Option<u64>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(f64, f64), String> {
    tap_target(elements, index, x, y).map(|(point, _)| point)
}

fn tap_target(
    elements: &[Numbered],
    index: Option<u64>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<((f64, f64), Option<&Element>), String> {
    if let Some(index) = index {
        let (_, element) = elements
            .iter()
            .find(|(number, _)| *number as u64 == index)
            .ok_or_else(|| {
                format!("element {index} is not on the screen now; the screen changed since you read it, so read it again with sim state")
            })?;
        if element.offscreen {
            return Err(format!(
                "element {index} is off the screen; scroll it into view, then read again"
            ));
        }
        return Ok((element.center, Some(element)));
    }
    match (x, y) {
        (Some(x), Some(y)) => Ok(((x, y), None)),
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
        "the swipe started at the {edge} edge, which the system treats as a gesture (home, back, notifications or Control Center) rather than scrolling"
    ))
}
