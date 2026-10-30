//! The desk's iOS Simulator: the device an agent works on, live, which the
//! person can tap, scroll and type into too. The engine in `sikemux-simulator`
//! drives devices; this window watches the screen and sends the person's input.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{Bounds, Pixels, Point, ScrollDelta, ScrollEvent, TouchPhase};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, img, px, relative, AnyElement, Context, Entity, EventEmitter, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Window,
};
use serde_json::{json, Map, Value};
#[cfg(target_os = "macos")]
use sikemux_process::sync::Locked;
use sikemux_simulator::{tools, Host, SimulatorManager, WATCH_ENDED_EVENT};
use tokio::sync::mpsc;

use crate::icons::Icon;
use crate::sim::frame::{self, Chrome, Rect};
use crate::sim::screen::{Decoder, Picture, Turn};
use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2H, SPACE_4, TEXT_XS, TEXT_XS_H};
use crate::ui::{self, IconButton};
use crate::workbench::Workbench;

/// Frames are reported read in batches this size, so the helper keeps sending.
const READ_BATCH: u64 = 4;
const NAMED_KEYS: [&str; 8] = [
    "Enter",
    "Escape",
    "Backspace",
    "Delete",
    "ArrowRight",
    "ArrowLeft",
    "ArrowDown",
    "ArrowUp",
];

/// The simulator engine and what the desks show of it.
pub struct Simulators {
    engine: Arc<SimulatorManager>,
    /// The device each agent's desk shows: the one it attached, or the one the person picked.
    attached: HashMap<String, Device>,
    /// The agents whose desk shows a device the person picked rather than one they attached.
    picked: HashSet<String>,
    /// The iOS devices this Mac has, as last listed.
    devices: Option<Vec<Device>>,
    listing: bool,
    /// The devices starting up, by udid.
    booting: HashSet<String>,
    /// The devices shutting down, by udid.
    stopping: HashSet<String>,
    /// Why the last listing or start of a device failed.
    problem: Option<SharedString>,
    acting: HashSet<String>,
    /// How far the helper's first download has come.
    pub progress: Option<f64>,
    screens: HashMap<String, Screen>,
    /// Where a decoded frame says the screen should draw again.
    heard: mpsc::UnboundedSender<Heard>,
    _hearing: gpui_kit::Task<()>,
}

#[derive(Clone)]
pub struct Device {
    udid: String,
    name: String,
    booted: bool,
}

/// One device's live screen and the person's input to it.
struct Screen {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    picture: Picture,
    watch: Arc<AtomicU64>,
    /// The device's size in points as it is turned, which touches are measured in.
    size: Option<(f32, f32)>,
    /// Which way the device is held, as the helper names it.
    orientation: String,
    /// The same, as the degrees the decoder turns its upright pictures by.
    turn: Turn,
    /// The device's own frame, once the helper has drawn it.
    chrome: Option<Rc<Chrome>>,
    /// The side button the person is holding down.
    held: Option<String>,
    /// Where the screen was last drawn, to turn the pointer into device points.
    drawn: Rc<Cell<Bounds<Pixels>>>,
    input: mpsc::UnboundedSender<Input>,
    pressed: Option<(f32, f32)>,
    focus: FocusHandle,
    /// A frame decoded since the window last drew, so one event covers a burst of them.
    fresh: Arc<AtomicBool>,
}

enum Input {
    Request(Map<String, Value>),
    Move(f32, f32),
}

enum Heard {
    Attached(String, Device),
    Detached(String),
    Acting(String, bool),
    Progress(f64),
    Frame,
    Ended(u64),
}

struct WindowHost {
    heard: mpsc::UnboundedSender<Heard>,
    home: Option<PathBuf>,
}

impl Host for WindowHost {
    fn data_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn emit(&self, event: &str, payload: Value) {
        let agent = || payload["agentId"].as_str().map(str::to_owned);
        let heard = match event {
            tools::ATTACHED_EVENT => agent().map(|agent| {
                Heard::Attached(
                    agent,
                    Device {
                        udid: payload["udid"].as_str().unwrap_or_default().to_owned(),
                        name: payload["name"].as_str().unwrap_or_default().to_owned(),
                        booted: true,
                    },
                )
            }),
            tools::DETACHED_EVENT => agent().map(Heard::Detached),
            tools::ACTING_EVENT => {
                agent().map(|agent| Heard::Acting(agent, payload["acting"] == true))
            }
            sikemux_simulator::SIM_EVENT => payload["fraction"].as_f64().map(Heard::Progress),
            WATCH_ENDED_EVENT => payload["id"].as_u64().map(Heard::Ended),
            _ => None,
        };
        if let Some(heard) = heard {
            let _ = self.heard.send(heard);
        }
    }
}

impl Simulators {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (heard, mut hearing) = mpsc::unbounded_channel();
        let engine = SimulatorManager::new(Arc::new(WindowHost {
            heard: heard.clone(),
            home: sikemux_process::paths::config_dir(),
        }));
        let task = cx.spawn(async move |this, cx| {
            while let Some(heard) = hearing.recv().await {
                let heard = this.update(cx, |sims, cx| sims.hear(heard, cx));
                if heard.is_err() {
                    break;
                }
            }
        });
        Self {
            engine,
            attached: HashMap::new(),
            picked: HashSet::new(),
            devices: None,
            listing: false,
            booting: HashSet::new(),
            stopping: HashSet::new(),
            problem: None,
            acting: HashSet::new(),
            progress: None,
            screens: HashMap::new(),
            heard,
            _hearing: task,
        }
    }

    pub fn engine(&self) -> &Arc<SimulatorManager> {
        &self.engine
    }

    /// Takes up the devices the engine gave back to agents after an update's restart.
    pub fn restored(&mut self) {
        for held in self.engine.attachments() {
            let device = Device {
                udid: held.udid,
                name: held.name,
                booted: true,
            };
            self.attached.insert(held.agent_id, device);
        }
    }

    /// Whether the agent has a device on its desk.
    pub fn holds(&self, agent: &str) -> bool {
        self.attached.contains_key(agent)
    }

    /// Whether the agent attached the device on its desk itself.
    pub fn attached_by(&self, agent: &str) -> bool {
        self.holds(agent) && !self.picked.contains(agent)
    }

    /// Whether the device on the agent's desk is running, so its screen can show.
    pub fn live(&self, agent: &str) -> bool {
        self.attached.get(agent).is_some_and(|device| device.booted)
    }

    /// The device's name, as an agent's desk shows it.
    pub fn device_name(&self, agent: &str) -> Option<&str> {
        self.attached.get(agent).map(|device| device.name.as_str())
    }

    /// Whether the agent's calls are driving its device, when the person's input waits.
    pub fn acting(&self, agent: &str) -> bool {
        self.acting.contains(agent)
    }

    /// Whether this Mac can run the simulator, and whether its helper is in place.
    pub fn status(
        &self,
    ) -> impl std::future::Future<Output = Option<sikemux_simulator::SimStatus>> {
        let engine = Arc::clone(&self.engine);
        crate::desk::run(async move { sikemux_simulator::status(&**engine.host()).await })
    }

    /// Downloads the helper if this build needs it; `progress` follows as it comes.
    pub fn prepare(&self) -> impl std::future::Future<Output = Option<Result<(), String>>> {
        let engine = Arc::clone(&self.engine);
        crate::desk::run(async move { sikemux_simulator::prepare(&**engine.host()).await })
    }

    /// The Xcode in use, its iOS runtimes and where the helper stands, as Settings lists them.
    pub fn setup(&self) -> impl std::future::Future<Output = Option<Value>> {
        let engine = Arc::clone(&self.engine);
        crate::desk::run(async move { engine.setup().await })
    }

    /// Whether agents started from now on get the `sim_*` tools.
    pub fn offer_to_agents(&self, offered: bool) {
        sikemux_simulator::set_enabled(offered);
    }

    fn hear(&mut self, heard: Heard, cx: &mut Context<Self>) {
        match heard {
            Heard::Attached(agent, device) => {
                self.picked.remove(&agent);
                self.stop_screen(&agent);
                self.attached.insert(agent.clone(), device);
                cx.emit(SimNews::Attached(agent));
            }
            Heard::Detached(agent) => {
                self.picked.remove(&agent);
                self.attached.remove(&agent);
                self.acting.remove(&agent);
                self.stop_screen(&agent);
            }
            Heard::Acting(agent, acting) => {
                if acting {
                    self.acting.insert(agent);
                } else {
                    self.acting.remove(&agent);
                }
            }
            Heard::Progress(fraction) => {
                self.progress = (fraction < 1.).then_some(fraction);
            }
            Heard::Frame => {
                for screen in self.screens.values() {
                    screen.fresh.store(false, Ordering::SeqCst);
                }
            }
            Heard::Ended(id) => {
                let ended: Vec<String> = self
                    .screens
                    .iter()
                    .filter(|(_, screen)| screen.watch.load(Ordering::SeqCst) == id)
                    .map(|(agent, _)| agent.clone())
                    .collect();
                for agent in ended {
                    self.screens.remove(&agent);
                }
            }
        }
        cx.notify();
    }

    /// Lets go of the agent's device: the one it attached, or the one the person picked for it.
    pub(crate) fn let_go_device(&mut self, agent: &str) {
        self.engine.forget_agent(agent);
        if self.picked.remove(agent) {
            self.attached.remove(agent);
            self.stop_screen(agent);
        }
    }

    /// Puts a device on a desk that has none, once the devices are listed: one already running,
    /// else an iPhone.
    pub(crate) fn pick_device_for(&mut self, agent: &str, cx: &mut Context<Self>) {
        if self.holds(agent) {
            return;
        }
        let Some(devices) = &self.devices else {
            if self.problem.is_none() {
                self.list_devices(cx);
            }
            return;
        };
        let pick = devices
            .iter()
            .find(|device| device.booted)
            .or_else(|| {
                devices
                    .iter()
                    .find(|device| device.name.starts_with("iPhone"))
            })
            .cloned();
        if let Some(device) = pick {
            self.choose_device(agent, device, cx);
        }
    }

    /// Lists the iOS devices this Mac has, for the desks' pickers.
    pub(crate) fn list_devices(&mut self, cx: &mut Context<Self>) {
        if self.listing {
            return;
        }
        self.listing = true;
        let engine = Arc::clone(&self.engine);
        cx.spawn(async move |sims, cx| {
            let asked = Map::from_iter([("type".to_owned(), Value::from("devices"))]);
            let answer = crate::desk::run(async move { engine.window_call(asked).await }).await;
            let _ = sims.update(cx, |sims, cx| {
                sims.listing = false;
                match answer {
                    Some(Ok(answer)) => {
                        sims.problem = None;
                        sims.devices = Some(ios_devices(&answer));
                    }
                    Some(Err(error)) => sims.problem = Some(error.into()),
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows `device` on the agent's desk. An agent that attached one moves to it; one that has
    /// not takes it when it attaches.
    pub(crate) fn choose_device(&mut self, agent: &str, device: Device, cx: &mut Context<Self>) {
        let attached = self.attached_by(agent);
        if !attached {
            self.stop_screen(agent);
            self.picked.insert(agent.to_owned());
            self.attached.insert(agent.to_owned(), device.clone());
        }
        let (engine, agent) = (Arc::clone(&self.engine), agent.to_owned());
        cx.spawn(async move |sims, cx| {
            let udid = device.udid.clone();
            let chose = {
                let agent = agent.clone();
                crate::desk::run(async move { engine.set_desk_device(&agent, &udid).await }).await
            };
            if let Some(Err(error)) = chose {
                let _ = sims.update(cx, |_, cx| cx.emit(SimNews::Failed(error.into())));
            }
        })
        .detach();
        cx.notify();
    }

    /// Starts the device on the agent's desk.
    fn boot_device(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(udid) = self.attached.get(agent).map(|device| device.udid.clone()) else {
            return;
        };
        if !self.booting.insert(udid.clone()) {
            return;
        }
        let engine = Arc::clone(&self.engine);
        cx.spawn(async move |sims, cx| {
            let asked = request("boot", &udid, Map::new());
            let booted = crate::desk::run(async move { engine.window_call(asked).await }).await;
            let _ = sims.update(cx, |sims, cx| {
                sims.booting.remove(&udid);
                match booted {
                    Some(Ok(_)) => {
                        for device in sims.attached.values_mut() {
                            if device.udid == udid {
                                device.booted = true;
                            }
                        }
                        sims.list_devices(cx);
                        sims.problem = None;
                    }
                    Some(Err(error)) => sims.problem = Some(error.into()),
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn shut_down(&mut self, udid: String, cx: &mut Context<Self>) {
        if !self.stopping.insert(udid.clone()) {
            return;
        }
        let showing: Vec<String> = self
            .attached
            .iter()
            .filter(|(_, device)| device.udid == udid)
            .map(|(agent, _)| agent.clone())
            .collect();
        for agent in &showing {
            self.stop_screen(agent);
        }
        let engine = Arc::clone(&self.engine);
        cx.spawn(async move |sims, cx| {
            let asked = request("shutdown", &udid, Map::new());
            let stopped = crate::desk::run(async move { engine.window_call(asked).await }).await;
            let _ = sims.update(cx, |sims, cx| {
                sims.stopping.remove(&udid);
                match stopped {
                    Some(Ok(_)) => {
                        for device in sims.attached.values_mut() {
                            if device.udid == udid {
                                device.booted = false;
                            }
                        }
                        sims.list_devices(cx);
                        sims.problem = None;
                    }
                    Some(Err(error)) => cx.emit(SimNews::Failed(error.into())),
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Saves the device's screen as a picture on the Desktop.
    fn screenshot(&self, agent: &str, cx: &mut Context<Self>) {
        let Some(device) = self.attached.get(agent) else {
            return;
        };
        let path = screenshot_path(&device.name);
        let asked = request("screenshot", &device.udid, fields([("path", path.into())]));
        let engine = Arc::clone(&self.engine);
        cx.spawn(async move |sims, cx| {
            let saved = crate::desk::run(async move { engine.window_call(asked).await }).await;
            let _ = sims.update(cx, |_, cx| match saved {
                Some(Ok(_)) => cx.emit(SimNews::Shot),
                Some(Err(error)) => cx.emit(SimNews::Failed(error.into())),
                None => {}
            });
        })
        .detach();
    }

    fn stop_screen(&mut self, agent: &str) {
        let Some(screen) = self.screens.remove(agent) else {
            return;
        };
        let id = screen.watch.load(Ordering::SeqCst);
        if id == 0 {
            return;
        }
        let engine = Arc::clone(&self.engine);
        crate::desk::spawn(async move {
            let _ = engine.sim().unwatch(&**engine.host(), id).await;
        });
    }

    /// Stops the screens of the desks the tab in front no longer shows, which would otherwise
    /// stream and decode every frame out of sight.
    pub(crate) fn stop_screens_hidden(&mut self, shown: &[String]) {
        let hidden: Vec<String> = self
            .screens
            .keys()
            .filter(|agent| !shown.contains(agent))
            .cloned()
            .collect();
        for agent in hidden {
            self.stop_screen(&agent);
        }
    }

    /// Starts showing the agent's device: its screen streams, and its size is read for touches.
    pub(crate) fn start_screen(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(device) = self.attached.get(agent).cloned() else {
            return;
        };
        if self.screens.contains_key(agent) {
            return;
        }
        let engine = Arc::clone(&self.engine);
        let picture: Picture = Arc::new(Mutex::new(None));
        let watch = Arc::new(AtomicU64::new(0));
        let (input, mut inputs) = mpsc::unbounded_channel::<Input>();
        {
            let (engine, udid) = (Arc::clone(&engine), device.udid.clone());
            crate::desk::spawn(async move {
                while let Some(first) = inputs.recv().await {
                    let mut next = Some(first);
                    while let Some(input) = next.take() {
                        let request = match input {
                            Input::Request(request) => request,
                            Input::Move(mut x, mut y) => {
                                while let Ok(Input::Move(later_x, later_y)) = inputs.try_recv() {
                                    (x, y) = (later_x, later_y);
                                }
                                touch(&udid, "move", x, y)
                            }
                        };
                        if let Err(error) = engine.window_call(request).await {
                            eprintln!("sikemux: the simulator refused the person's input: {error}");
                        }
                    }
                }
            });
        }
        let fresh = Arc::new(AtomicBool::new(false));
        let turn = Turn::default();
        {
            let (engine, watch, picture, turning, udid) = (
                Arc::clone(&engine),
                Arc::clone(&watch),
                Arc::clone(&picture),
                Arc::clone(&turn),
                device.udid.clone(),
            );
            let (fresh, heard) = (Arc::clone(&fresh), self.heard.clone());
            crate::desk::spawn(async move {
                let host = Arc::clone(engine.host());
                let sim = engine.sim().clone();
                let mut decoder = Decoder::new(picture, turning);
                let mut frames = 0u64;
                let (reading, reader) = (Arc::clone(&watch), sim.clone());
                let started = sim
                    .watch(host, udid, "h264".into(), move |frame| {
                        if let Err(error) = decoder.decode(&frame) {
                            eprintln!("sikemux: {error}");
                        }
                        frames += 1;
                        if frames.is_multiple_of(READ_BATCH) {
                            reader.watch_read(reading.load(Ordering::SeqCst), frames);
                        }
                        if !fresh.swap(true, Ordering::SeqCst) {
                            let _ = heard.send(Heard::Frame);
                        }
                        true
                    })
                    .await;
                match started {
                    Ok(id) => watch.store(id, Ordering::SeqCst),
                    Err(error) => {
                        eprintln!("sikemux: the simulator's screen did not stream: {error}")
                    }
                }
            });
        }
        {
            let (engine, udid, agent) =
                (Arc::clone(&engine), device.udid.clone(), agent.to_owned());
            cx.spawn(async move |sims, cx| {
                let ask = |kind: &str| {
                    let (engine, asked) = (Arc::clone(&engine), request(kind, &udid, Map::new()));
                    crate::desk::run(async move { engine.window_call(asked).await })
                };
                let (chrome, mask) = (ask("chrome").await, ask("mask").await);
                let mask = mask.and_then(Result::ok);
                let read = chrome.and_then(Result::ok).and_then(|chrome| {
                    Chrome::read(
                        &chrome["chrome"],
                        mask.as_ref().and_then(|mask| mask["mask"].as_str()),
                    )
                });
                let _ = sims.update(cx, |sims, cx| {
                    if let Some(screen) = sims.screens.get_mut(&agent) {
                        screen.chrome = read.map(Rc::new);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        self.screens.insert(
            agent.to_owned(),
            Screen {
                picture,
                watch,
                size: None,
                orientation: String::new(),
                turn,
                chrome: None,
                held: None,
                drawn: Rc::new(Cell::new(Bounds::default())),
                input,
                pressed: None,
                focus: cx.focus_handle(),
                fresh,
            },
        );
        self.read_screen(agent, cx);
    }

    /// Reads how large the agent's device is and which way it is held, for touches and its frame.
    fn read_screen(&self, agent: &str, cx: &mut Context<Self>) {
        let Some(device) = self.attached.get(agent) else {
            return;
        };
        let (engine, agent) = (Arc::clone(&self.engine), agent.to_owned());
        let asked = request("screen", &device.udid, Map::new());
        cx.spawn(async move |sims, cx| {
            let answer = crate::desk::run(async move { engine.window_call(asked).await }).await;
            let Some(Ok(screen)) = answer else {
                return;
            };
            let measured = (
                screen["width"].as_f64().unwrap_or(0.) as f32,
                screen["height"].as_f64().unwrap_or(0.) as f32,
            );
            let orientation = screen["orientation"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let _ = sims.update(cx, |sims, cx| {
                if let Some(screen) = sims.screens.get_mut(&agent) {
                    screen.size =
                        Some(measured).filter(|(width, height)| *width > 0. && *height > 0.);
                    screen
                        .turn
                        .store(frame::turn_degrees(&orientation), Ordering::Relaxed);
                    screen.orientation = orientation;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Turns the agent's device to the next way of holding it, then reads its screen again.
    fn turn_device(&self, agent: &str, cx: &mut Context<Self>) {
        let Some(udid) = self.attached.get(agent).map(|device| device.udid.clone()) else {
            return;
        };
        let (engine, agent) = (Arc::clone(&self.engine), agent.to_owned());
        cx.spawn(async move |sims, cx| {
            let turned = crate::desk::run(async move {
                let now = engine.orientation(&udid, None).await.unwrap_or_default();
                let next = match now.as_str() {
                    "portrait" => "landscapeLeft",
                    "landscapeLeft" => "portraitUpsideDown",
                    "portraitUpsideDown" => "landscapeRight",
                    _ => "portrait",
                };
                engine.orientation(&udid, Some(next.into())).await
            })
            .await;
            if turned.is_some() {
                let _ = sims.update(cx, |sims, cx| sims.read_screen(&agent, cx));
            }
        })
        .detach();
    }

    /// Presses or lets go of one of the device's side buttons, as the frame shows them.
    fn side_button(&mut self, agent: &str, name: &str, down: bool, cx: &mut Context<Self>) {
        let button = match name {
            "action" => "action",
            "volume-up" => "volumeUp",
            "volume-down" => "volumeDown",
            "power" => "lock",
            _ => return,
        };
        let Some(screen) = self.screens.get_mut(agent) else {
            return;
        };
        match down {
            true if self.acting.contains(agent) => return,
            true => screen.held = Some(name.to_owned()),
            false if screen.held.as_deref() != Some(name) => return,
            false => screen.held = None,
        }
        let phase = if down { "down" } else { "up" };
        self.device_request(
            agent,
            "button",
            fields([("button", button.into()), ("phase", phase.into())]),
        );
        cx.notify();
    }

    /// Sends one request about the agent's device in turn with the person's earlier input.
    fn send_input(&self, agent: &str, input: Input) {
        if let Some(screen) = self.screens.get(agent) {
            let _ = screen.input.send(input);
        }
    }

    fn device_request(&self, agent: &str, kind: &str, fields: Map<String, Value>) {
        if let Some(device) = self.attached.get(agent) {
            self.send_input(agent, Input::Request(request(kind, &device.udid, fields)));
        }
    }

    /// The device point under `position`, when it is on the screen.
    fn device_point(
        &self,
        agent: &str,
        position: Point<Pixels>,
        clamp: bool,
    ) -> Option<(f32, f32)> {
        let screen = self.screens.get(agent)?;
        let (width, height) = screen.size?;
        let drawn = screen.drawn.get();
        if drawn.size.width <= px(0.) || drawn.size.height <= px(0.) {
            return None;
        }
        let x = f32::from(position.x - drawn.left()) / f32::from(drawn.size.width) * width;
        let y = f32::from(position.y - drawn.top()) / f32::from(drawn.size.height) * height;
        if clamp {
            return Some((x.clamp(0., width), y.clamp(0., height)));
        }
        (x >= 0. && y >= 0. && x <= width && y <= height).then_some((x, y))
    }

    fn press(
        &mut self,
        agent: &str,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.acting(agent) {
            return;
        }
        if let Some(screen) = self.screens.get(agent) {
            screen.focus.focus(window, cx);
        }
        let Some(udid) = self.attached.get(agent).map(|device| device.udid.clone()) else {
            return;
        };
        let Some((x, y)) = self.device_point(agent, event.position, false) else {
            return;
        };
        if let Some(screen) = self.screens.get_mut(agent) {
            screen.pressed = Some((x, y));
        }
        self.send_input(agent, Input::Request(touch(&udid, "down", x, y)));
    }

    fn drag(&mut self, agent: &str, event: &MouseMoveEvent) {
        if self
            .screens
            .get(agent)
            .and_then(|screen| screen.pressed)
            .is_none()
        {
            return;
        }
        let Some((x, y)) = self.device_point(agent, event.position, true) else {
            return;
        };
        if let Some(screen) = self.screens.get_mut(agent) {
            screen.pressed = Some((x, y));
        }
        self.send_input(agent, Input::Move(x, y));
    }

    fn lift(&mut self, agent: &str, position: Option<Point<Pixels>>) {
        let Some((last_x, last_y)) = self
            .screens
            .get_mut(agent)
            .and_then(|screen| screen.pressed.take())
        else {
            return;
        };
        let Some(udid) = self.attached.get(agent).map(|device| device.udid.clone()) else {
            return;
        };
        let (x, y) = position
            .and_then(|position| self.device_point(agent, position, true))
            .unwrap_or((last_x, last_y));
        self.send_input(agent, Input::Request(touch(&udid, "up", x, y)));
    }

    /// A trackpad scroll over the screen is a finger dragging it, lifted when the scroll stops.
    fn scroll(&mut self, agent: &str, event: &ScrollEvent) {
        if self.acting(agent) {
            return;
        }
        let Some(udid) = self.attached.get(agent).map(|device| device.udid.clone()) else {
            return;
        };
        let delta = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            ScrollDelta::Lines(delta) => (delta.x * 16., delta.y * 16.),
        };
        let pressed = self.screens.get(agent).and_then(|screen| screen.pressed);
        let at = match pressed {
            Some(at) => at,
            None => {
                let Some(start) = self.device_point(agent, event.position, false) else {
                    return;
                };
                self.send_input(
                    agent,
                    Input::Request(touch(&udid, "down", start.0, start.1)),
                );
                start
            }
        };
        let Some((width, height)) = self.screens.get(agent).and_then(|screen| screen.size) else {
            return;
        };
        let drawn = self.screens[agent].drawn.get();
        let scale = width / f32::from(drawn.size.width).max(1.);
        let next = (
            (at.0 + delta.0 * scale).clamp(0., width),
            (at.1 + delta.1 * scale).clamp(0., height),
        );
        let ended = event.touch_phase == TouchPhase::Ended;
        if let Some(screen) = self.screens.get_mut(agent) {
            screen.pressed = (!ended).then_some(next);
        }
        if ended {
            self.send_input(agent, Input::Request(touch(&udid, "up", next.0, next.1)));
        } else {
            self.send_input(agent, Input::Move(next.0, next.1));
        }
    }

    fn type_into(&mut self, agent: &str, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.acting(agent) {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform && keystroke.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.device_request(agent, "text", fields([("text", text.into())]));
            }
            return;
        }
        if keystroke.modifiers.platform || keystroke.modifiers.control {
            return;
        }
        let named = match keystroke.key.as_str() {
            "enter" => Some("Enter"),
            "escape" => Some("Escape"),
            "backspace" => Some("Backspace"),
            "delete" => Some("Delete"),
            "right" => Some("ArrowRight"),
            "left" => Some("ArrowLeft"),
            "down" => Some("ArrowDown"),
            "up" => Some("ArrowUp"),
            _ => None,
        }
        .filter(|key| NAMED_KEYS.contains(key));
        if let Some(key) = named {
            self.device_request(agent, "key", fields([("key", key.into())]));
        } else if let Some(text) = keystroke
            .key_char
            .as_ref()
            .filter(|text| text.chars().count() == 1)
        {
            self.device_request(agent, "text", fields([("text", text.as_str().into())]));
        }
    }
}

/// What the simulators tell the window, which owns the desks and the toasts.
pub enum SimNews {
    /// An agent attached a device, so its desk turns to it.
    Attached(String),
    /// A screenshot was saved to the Desktop.
    Shot,
    Failed(SharedString),
}

impl EventEmitter<SimNews> for Simulators {}

impl Workbench {
    pub(crate) fn hear_sims(
        &mut self,
        _: Entity<Simulators>,
        news: &SimNews,
        cx: &mut Context<Self>,
    ) {
        match news {
            SimNews::Attached(agent) => self.show_simulator(agent, cx),
            SimNews::Shot => {
                self.notify(crate::toast::success("Screenshot saved to the Desktop"), cx)
            }
            SimNews::Failed(error) => self.notify(crate::toast::error(error.clone()), cx),
        }
    }

    /// Shuts down the device on the agent's desk, asking first when an agent is using it.
    fn shut_down_device(&mut self, agent: &str, cx: &mut Context<Self>) {
        let sims = self.sims.read(cx);
        let Some(device) = sims.attached.get(agent).cloned() else {
            return;
        };
        let holder = sims
            .engine
            .attachments()
            .into_iter()
            .find(|held| held.udid == device.udid);
        let Some(holder) = holder else {
            return self
                .sims
                .update(cx, |sims, cx| sims.shut_down(device.udid, cx));
        };
        let who = self
            .agent_by_id(&holder.agent_id)
            .and_then(|row| row.name)
            .unwrap_or_else(|| "An agent".to_owned());
        self.confirm(
            format!("Shut down {}?", device.name),
            format!("{who} is using this device."),
            "Shut down",
            true,
            move |bench, yes, _, cx| {
                if yes {
                    let udid = device.udid.clone();
                    bench.sims.update(cx, |sims, cx| sims.shut_down(udid, cx));
                }
            },
            cx,
        );
    }
}

/// The iPhones and iPads in the helper's list, those running first.
fn ios_devices(answer: &Value) -> Vec<Device> {
    let mut devices: Vec<Device> = answer["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|device| {
            device["runtime"]
                .as_str()
                .is_some_and(|runtime| runtime.starts_with("iOS") || runtime.starts_with("iPadOS"))
        })
        .filter_map(|device| {
            Some(Device {
                udid: device["udid"].as_str()?.to_owned(),
                name: device["name"].as_str()?.to_owned(),
                booted: device["state"] == "booted",
            })
        })
        .collect();
    devices.sort_by_key(|device| device_group(device) as u8);
    devices
}

#[derive(Clone, Copy, PartialEq)]
enum Group {
    Running,
    IPhone,
    IPad,
}

fn device_group(device: &Device) -> Group {
    if device.booted {
        Group::Running
    } else if device.name.starts_with("iPad") {
        Group::IPad
    } else {
        Group::IPhone
    }
}

/// The device's name, which opens the list of devices to show on the desk instead.
pub fn picker(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let t = crate::theme::Tokens::get(cx).clone();
    let sims = bench.sims.read(cx);
    let name = sims.device_name(agent)?.to_owned();
    let trigger = Button::new("sim-device")
        .ghost()
        .compact()
        .h(px(24.))
        .px(SPACE_1H)
        .gap(SPACE_1)
        .child(ui::text(name, TEXT_XS, FontWeight::MEDIUM).text_color(t.text_secondary))
        .child(ui::icon(Icon::Chevron, 10.).text_color(t.text_tertiary));
    let Some(devices) = sims.devices.clone() else {
        return Some(trigger.into_any_element());
    };
    let (this, agent) = (bench.sims.downgrade(), agent.to_owned());
    let current = sims.attached.get(&agent).map(|device| device.udid.clone());
    Some(
        trigger
            .dropdown_menu_with_anchor(
                gpui::Anchor::TopLeft,
                ui::menu(move |mut popup, _, _| {
                    let mut group = None;
                    for device in &devices {
                        let now = device_group(device);
                        if group != Some(now) {
                            group = Some(now);
                            popup = popup.label(match now {
                                Group::Running => "Running",
                                Group::IPhone => "iPhone",
                                Group::IPad => "iPad",
                            });
                        }
                        let (this, agent, chosen) = (this.clone(), agent.clone(), device.clone());
                        popup = popup.item(
                            PopupMenuItem::new(device.name.clone())
                                .icon(Icon::Phone)
                                .checked(current.as_deref() == Some(device.udid.as_str()))
                                .on_click(move |_, _, cx| {
                                    let _ = this.update(cx, |sims, cx| {
                                        sims.choose_device(&agent, chosen.clone(), cx)
                                    });
                                }),
                        );
                    }
                    popup
                }),
            )
            .into_any_element(),
    )
}

/// Where a screenshot of `device` goes: the Desktop, named by the device and the time.
fn screenshot_path(device: &str) -> String {
    let name: String = device
        .chars()
        .map(|c| match c.is_alphanumeric() || " ._()-".contains(c) {
            true => c,
            false => '-',
        })
        .collect();
    let name = name.trim_start_matches([' ', '.', '-']).trim();
    let name = if name.is_empty() { "Device" } else { name };
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S");
    format!("~/Desktop/Simulator {name} {stamp}.png")
}

fn fields<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn request(kind: &str, udid: &str, mut fields: Map<String, Value>) -> Map<String, Value> {
    fields.insert("type".into(), kind.into());
    fields.insert("udid".into(), udid.into());
    fields
}

fn touch(udid: &str, phase: &str, x: f32, y: f32) -> Map<String, Value> {
    request(
        "touch",
        udid,
        fields([("phase", phase.into()), ("x", json!(x)), ("y", json!(y))]),
    )
}

/// The buttons over the agent's device: Home, Lock and turning it.
pub fn tools(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let sims = bench.sims.read(cx);
    let acting = sims.acting(agent) || !sims.live(agent);
    let button = |id: &'static str,
                  icon: Icon,
                  label: &'static str,
                  kind: &'static str,
                  more: Map<String, Value>,
                  cx: &Context<Workbench>| {
        let agent = agent.to_owned();
        IconButton::new(id, icon, label)
            .glyph(13.)
            .disabled(acting)
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench
                    .sims
                    .read(cx)
                    .device_request(&agent, kind, more.clone())
            }))
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_0)
        .child(button(
            "sim-home",
            Icon::HomeButton,
            "Home",
            "button",
            fields([("button", "home".into())]),
            cx,
        ))
        .child(button(
            "sim-lock",
            Icon::Phone,
            "Lock",
            "button",
            fields([("button", "lock".into())]),
            cx,
        ))
        .child({
            let turning = agent.to_owned();
            IconButton::new("sim-rotate", Icon::Rotate, "Turn the device")
                .glyph(13.)
                .disabled(acting)
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench
                        .sims
                        .update(cx, |sims, cx| sims.turn_device(&turning, cx))
                }))
        })
        .child({
            let shooting = agent.to_owned();
            IconButton::new("sim-screenshot", Icon::Camera, "Screenshot to the Desktop")
                .glyph(13.)
                .disabled(!sims.live(agent))
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench
                        .sims
                        .update(cx, |sims, cx| sims.screenshot(&shooting, cx))
                }))
        })
        .child(div().w(px(6.)))
        .child({
            let udid = sims.attached.get(agent).map(|device| device.udid.as_str());
            let starting = udid.is_some_and(|udid| sims.booting.contains(udid));
            let stopping = udid.is_some_and(|udid| sims.stopping.contains(udid));
            let live = sims.live(agent);
            let label = match (starting, stopping, live) {
                (true, _, _) => "Starting…",
                (_, true, _) => "Shutting down…",
                (_, _, true) => "Shut down",
                _ => "Start",
            };
            let switching = agent.to_owned();
            IconButton::new("sim-power", Icon::Power, label)
                .glyph(13.)
                .disabled(udid.is_none() || starting || stopping)
                .on_click(cx.listener(move |bench, _, _, cx| {
                    match bench.sims.read(cx).live(&switching) {
                        true => bench.shut_down_device(&switching, cx),
                        false => bench
                            .sims
                            .update(cx, |sims, cx| sims.boot_device(&switching, cx)),
                    }
                }))
        })
        .into_any_element()
}

/// The agent's device, live: its screen fitted to the desk, taking the person's touches and keys.
pub fn body(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let t = crate::theme::Tokens::get(cx).clone();
    let sims = bench.sims.read(cx);
    let says = |message: String| {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(SPACE_2H)
            .items_center()
            .justify_center()
            .text_color(t.text_tertiary)
            .child(ui::text(message, TEXT_XS_H, FontWeight::NORMAL))
    };
    if let Some(fraction) = sims.progress {
        let percent = (fraction * 100.).round();
        return says(format!("Getting the simulator ready… {percent}%")).into_any_element();
    }
    let Some(device) = sims.attached.get(agent) else {
        let message = match (&sims.problem, &sims.devices) {
            (Some(problem), _) => problem.to_string(),
            (None, Some(devices)) if devices.is_empty() => {
                "No iPhone or iPad simulators are installed. Add one in Xcode.".into()
            }
            _ => "Finding devices…".into(),
        };
        let failed = sims.problem.is_some();
        return says(message)
            .when(failed, |column| {
                column.child(Button::new("sim-list-again").label("Try again").on_click(
                    cx.listener(|bench, _, _, cx| {
                        bench.sims.update(cx, |sims, cx| {
                            sims.problem = None;
                            sims.list_devices(cx);
                        })
                    }),
                ))
            })
            .into_any_element();
    };
    if !device.booted {
        let booting = sims.booting.contains(&device.udid);
        let message = match (&sims.problem, booting) {
            (_, true) => format!("Starting {}…", device.name),
            (Some(problem), false) => problem.to_string(),
            (None, false) => format!("{} is off.", device.name),
        };
        let agent = agent.to_owned();
        return says(message)
            .when(!booting, |column| {
                column.child(Button::new("sim-boot").label("Start").on_click(cx.listener(
                    move |bench, _, _, cx| {
                        bench
                            .sims
                            .update(cx, |sims, cx| sims.boot_device(&agent, cx))
                    },
                )))
            })
            .into_any_element();
    }
    let Some(screen) = sims.screens.get(agent) else {
        return div().into_any_element();
    };
    #[cfg(target_os = "macos")]
    let picture = screen
        .picture
        .locked()
        .as_ref()
        .map(|frame| gpui::surface(frame.buffer()).size_full());
    #[cfg(not(target_os = "macos"))]
    let picture: Option<gpui::Empty> = None;
    let ratio = screen
        .size
        .map_or(9. / 19.5, |(width, height)| width / height);
    let drawn = Rc::clone(&screen.drawn);
    let focus = screen.focus.clone();
    let (down, moved, up, out, wheel, keys) = (
        agent.to_owned(),
        agent.to_owned(),
        agent.to_owned(),
        agent.to_owned(),
        agent.to_owned(),
        agent.to_owned(),
    );
    let live = div()
        .id(SharedString::from(format!("sim-screen-{agent}")))
        .track_focus(&focus)
        .key_context("SimulatorScreen")
        .overflow_hidden()
        .bg(gpui::black())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |bench, event, window, cx| {
                bench
                    .sims
                    .update(cx, |sims, cx| sims.press(&down, event, window, cx))
            }),
        )
        .on_mouse_move(cx.listener(move |bench, event, _, cx| {
            bench.sims.update(cx, |sims, _| sims.drag(&moved, event))
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |bench, event: &MouseUpEvent, _, cx| {
                bench
                    .sims
                    .update(cx, |sims, _| sims.lift(&up, Some(event.position)))
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |bench, _, _, cx| {
                bench.sims.update(cx, |sims, _| sims.lift(&out, None))
            }),
        )
        .on_scroll(cx.listener(move |bench, event, _, cx| {
            bench.sims.update(cx, |sims, _| sims.scroll(&wheel, event))
        }))
        .on_key_down(cx.listener(move |bench, event, _, cx| {
            bench
                .sims
                .update(cx, |sims, cx| sims.type_into(&keys, event, cx))
        }))
        .child(
            canvas(move |bounds, _, _| drawn.set(bounds), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .children(picture);
    let framed = bench.preferences().simulator_frame;
    let frame = framed
        .then(|| screen.chrome.as_ref().zip(screen.size))
        .flatten()
        .map(|(chrome, size)| chrome.turned(frame::turn_degrees(&screen.orientation), size));
    let device = match frame {
        Some(frame) => framed_device(
            agent,
            live,
            &frame,
            screen.held.as_deref(),
            sims.acting(agent),
            cx,
        ),
        None => live
            .h_full()
            .max_w_full()
            .aspect_ratio(ratio)
            .rounded(px(28.))
            .into_any_element(),
    };
    div()
        .size_full()
        .p(SPACE_4)
        .flex()
        .items_center()
        .justify_center()
        .child(device)
        .into_any_element()
}

/// `rect` placed in a frame `size` points across, in shares of it, so it scales with the frame.
fn place<E: Styled>(element: E, rect: Rect, size: (f32, f32)) -> E {
    element
        .absolute()
        .left(relative(rect.x / size.0))
        .top(relative(rect.y / size.1))
        .w(relative(rect.w / size.0))
        .h(relative(rect.h / size.1))
}

/// The live screen inside the device's own frame: the side buttons under the bezel, sliding
/// out under the pointer, then the bezel, the screen, and the bezel again where its outline
/// covers the screen's corners.
fn framed_device(
    agent: &str,
    live: gpui::Stateful<gpui::Div>,
    frame: &frame::Turned,
    held: Option<&str>,
    acting: bool,
    cx: &Context<Workbench>,
) -> AnyElement {
    let size = frame.size;
    let buttons = frame.buttons.iter().map(|button| {
        let pressed = held == Some(button.name.as_str());
        let within = |rect: Rect| (rect, (button.clip.w, button.clip.h));
        let (rest, clip) = within(button.rest);
        let (out, _) = within(button.out);
        let image = match (pressed, &button.image_down) {
            (true, Some(down)) => down.clone(),
            _ => button.image.clone(),
        };
        let group = SharedString::from(format!("sim-side-{agent}-{}", button.name));
        let (press, lift, gone) = (
            (agent.to_owned(), button.name.clone()),
            (agent.to_owned(), button.name.clone()),
            (agent.to_owned(), button.name.clone()),
        );
        place(
            div()
                .id(group.clone())
                .group(group.clone())
                .overflow_hidden()
                .when(!acting, |side| side.cursor_pointer())
                .tooltip(ui::tooltip(button.title.clone()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |bench, _, _, cx| {
                        bench.sims.update(cx, |sims, cx| {
                            sims.side_button(&press.0, &press.1, true, cx)
                        })
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |bench, _, _, cx| {
                        bench
                            .sims
                            .update(cx, |sims, cx| sims.side_button(&lift.0, &lift.1, false, cx))
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |bench, _, _, cx| {
                        bench
                            .sims
                            .update(cx, |sims, cx| sims.side_button(&gone.0, &gone.1, false, cx))
                    }),
                )
                .child({
                    let shown = if pressed { out } else { rest };
                    place(img(image), shown, clip).when(!acting && !pressed, |slide| {
                        slide.group_hover(group, move |style| {
                            style
                                .left(relative(out.x / clip.0))
                                .top(relative(out.y / clip.1))
                        })
                    })
                }),
            button.clip,
            size,
        )
    });
    let (bezel_at, bezel) = &frame.bezel;
    div()
        .relative()
        .h_full()
        .max_w_full()
        .aspect_ratio(size.0 / size.1)
        .children(buttons)
        .child(place(img(bezel.clone()), *bezel_at, size))
        .child(place(live, frame.screen, size))
        .children(
            frame
                .cap
                .clone()
                .map(|cap| place(img(cap), frame.screen, size)),
        )
        .into_any_element()
}
