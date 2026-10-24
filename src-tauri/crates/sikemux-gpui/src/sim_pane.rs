//! The desk's iOS Simulator: the device an agent works on, live, which the
//! person can tap, scroll and type into too. The engine in `sikemux-simulator`
//! drives devices; this window watches the screen and sends the person's input.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{Bounds, Pixels, Point, ScrollDelta, ScrollWheelEvent, TouchPhase};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, img, px, relative, AnyElement, Context, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use serde_json::{json, Map, Value};
use sikemux_process::sync::Locked;
use sikemux_simulator::{tools, Host, SimulatorManager, WATCH_ENDED_EVENT};
use tokio::sync::mpsc;

use crate::icons::Icon;
use crate::sim_frame::{self, Chrome, Rect};
use crate::sim_screen::{Decoder, Picture, Turn};
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
    /// Each agent's device, as it attached it.
    attached: HashMap<String, Device>,
    acting: HashSet<String>,
    /// How far the helper's first download has come.
    pub progress: Option<f64>,
    screens: HashMap<String, Screen>,
    /// Where a decoded frame says the screen should draw again.
    heard: mpsc::UnboundedSender<Heard>,
    _hearing: gpui_kit::Task<()>,
}

#[derive(Clone)]
struct Device {
    udid: String,
    name: String,
}

/// One device's live screen and the person's input to it.
struct Screen {
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
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        let (heard, mut hearing) = mpsc::unbounded_channel();
        let engine = SimulatorManager::new(Arc::new(WindowHost {
            heard: heard.clone(),
            home: sikemux_process::paths::config_dir(),
        }));
        let task = cx.spawn(async move |this, cx| {
            while let Some(heard) = hearing.recv().await {
                let heard = this.update(cx, |bench, cx| bench.hear_simulator(heard, cx));
                if heard.is_err() {
                    break;
                }
            }
        });
        Self {
            engine,
            attached: HashMap::new(),
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

    /// Whether the agent has a device on its desk.
    pub fn holds(&self, agent: &str) -> bool {
        self.attached.contains_key(agent)
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
}

impl Workbench {
    fn hear_simulator(&mut self, heard: Heard, cx: &mut Context<Self>) {
        match heard {
            Heard::Attached(agent, device) => {
                self.sims.attached.insert(agent.clone(), device);
                self.sims.screens.remove(&agent);
                self.show_simulator(&agent);
            }
            Heard::Detached(agent) => {
                self.sims.attached.remove(&agent);
                self.sims.acting.remove(&agent);
                self.stop_screen(&agent);
            }
            Heard::Acting(agent, acting) => {
                if acting {
                    self.sims.acting.insert(agent);
                } else {
                    self.sims.acting.remove(&agent);
                }
            }
            Heard::Progress(fraction) => {
                self.sims.progress = (fraction < 1.).then_some(fraction);
                return cx.notify();
            }
            Heard::Frame => {
                for screen in self.sims.screens.values() {
                    screen.fresh.store(false, Ordering::SeqCst);
                }
            }
            Heard::Ended(id) => {
                let ended: Vec<String> = self
                    .sims
                    .screens
                    .iter()
                    .filter(|(_, screen)| screen.watch.load(Ordering::SeqCst) == id)
                    .map(|(agent, _)| agent.clone())
                    .collect();
                for agent in ended {
                    self.sims.screens.remove(&agent);
                }
            }
        }
        self.parts.redraw(&self.parts.stage, cx);
    }

    fn stop_screen(&mut self, agent: &str) {
        let Some(screen) = self.sims.screens.remove(agent) else {
            return;
        };
        let id = screen.watch.load(Ordering::SeqCst);
        if id == 0 {
            return;
        }
        let engine = Arc::clone(&self.sims.engine);
        crate::desk::spawn(async move {
            let _ = engine.sim().unwatch(&**engine.host(), id).await;
        });
    }

    /// Starts showing the agent's device: its screen streams, and its size is read for touches.
    pub(crate) fn start_screen(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(device) = self.sims.attached.get(agent).cloned() else {
            return;
        };
        if self.sims.screens.contains_key(agent) {
            return;
        }
        let engine = Arc::clone(&self.sims.engine);
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
            let (fresh, heard) = (Arc::clone(&fresh), self.sims.heard.clone());
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
            cx.spawn(async move |bench, cx| {
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
                let _ = bench.update(cx, |bench, cx| {
                    if let Some(screen) = bench.sims.screens.get_mut(&agent) {
                        screen.chrome = read.map(Rc::new);
                    }
                    bench.parts.redraw(&bench.parts.stage, cx);
                });
            })
            .detach();
        }
        self.sims.screens.insert(
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
        let Some(device) = self.sims.attached.get(agent) else {
            return;
        };
        let (engine, agent) = (Arc::clone(&self.sims.engine), agent.to_owned());
        let asked = request("screen", &device.udid, Map::new());
        cx.spawn(async move |bench, cx| {
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
            let _ = bench.update(cx, |bench, cx| {
                if let Some(screen) = bench.sims.screens.get_mut(&agent) {
                    screen.size =
                        Some(measured).filter(|(width, height)| *width > 0. && *height > 0.);
                    screen
                        .turn
                        .store(sim_frame::turn_degrees(&orientation), Ordering::Relaxed);
                    screen.orientation = orientation;
                }
                bench.parts.redraw(&bench.parts.stage, cx);
            });
        })
        .detach();
    }

    /// Turns the agent's device to the next way of holding it, then reads its screen again.
    fn turn_device(&self, agent: &str, cx: &mut Context<Self>) {
        let Some(udid) = self
            .sims
            .attached
            .get(agent)
            .map(|device| device.udid.clone())
        else {
            return;
        };
        let (engine, agent) = (Arc::clone(&self.sims.engine), agent.to_owned());
        cx.spawn(async move |bench, cx| {
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
                let _ = bench.update(cx, |bench, cx| bench.read_screen(&agent, cx));
            }
        })
        .detach();
    }

    /// Presses or lets go of one of the device's side buttons, as the frame shows them.
    fn side_button(&mut self, agent: &str, name: &str, down: bool) {
        let button = match name {
            "action" => "action",
            "volume-up" => "volumeUp",
            "volume-down" => "volumeDown",
            "power" => "lock",
            _ => return,
        };
        let Some(screen) = self.sims.screens.get_mut(agent) else {
            return;
        };
        match down {
            true if self.sims.acting.contains(agent) => return,
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
    }

    /// Sends one request about the agent's device in turn with the person's earlier input.
    fn send_input(&self, agent: &str, input: Input) {
        if let Some(screen) = self.sims.screens.get(agent) {
            let _ = screen.input.send(input);
        }
    }

    fn device_request(&self, agent: &str, kind: &str, fields: Map<String, Value>) {
        if let Some(device) = self.sims.attached.get(agent) {
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
        let screen = self.sims.screens.get(agent)?;
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
        if self.sims.acting(agent) {
            return;
        }
        if let Some(screen) = self.sims.screens.get(agent) {
            screen.focus.focus(window, cx);
        }
        let Some(udid) = self
            .sims
            .attached
            .get(agent)
            .map(|device| device.udid.clone())
        else {
            return;
        };
        let Some((x, y)) = self.device_point(agent, event.position, false) else {
            return;
        };
        if let Some(screen) = self.sims.screens.get_mut(agent) {
            screen.pressed = Some((x, y));
        }
        self.send_input(agent, Input::Request(touch(&udid, "down", x, y)));
    }

    fn drag(&mut self, agent: &str, event: &MouseMoveEvent) {
        if self
            .sims
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
        if let Some(screen) = self.sims.screens.get_mut(agent) {
            screen.pressed = Some((x, y));
        }
        self.send_input(agent, Input::Move(x, y));
    }

    fn lift(&mut self, agent: &str, position: Option<Point<Pixels>>) {
        let Some((last_x, last_y)) = self
            .sims
            .screens
            .get_mut(agent)
            .and_then(|screen| screen.pressed.take())
        else {
            return;
        };
        let Some(udid) = self
            .sims
            .attached
            .get(agent)
            .map(|device| device.udid.clone())
        else {
            return;
        };
        let (x, y) = position
            .and_then(|position| self.device_point(agent, position, true))
            .unwrap_or((last_x, last_y));
        self.send_input(agent, Input::Request(touch(&udid, "up", x, y)));
    }

    /// A trackpad scroll over the screen is a finger dragging it, lifted when the scroll stops.
    fn scroll(&mut self, agent: &str, event: &ScrollWheelEvent) {
        if self.sims.acting(agent) {
            return;
        }
        let Some(udid) = self
            .sims
            .attached
            .get(agent)
            .map(|device| device.udid.clone())
        else {
            return;
        };
        let delta = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            ScrollDelta::Lines(delta) => (delta.x * 16., delta.y * 16.),
        };
        let pressed = self
            .sims
            .screens
            .get(agent)
            .and_then(|screen| screen.pressed);
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
        let Some((width, height)) = self.sims.screens.get(agent).and_then(|screen| screen.size)
        else {
            return;
        };
        let drawn = self.sims.screens[agent].drawn.get();
        let scale = width / f32::from(drawn.size.width).max(1.);
        let next = (
            (at.0 + delta.0 * scale).clamp(0., width),
            (at.1 + delta.1 * scale).clamp(0., height),
        );
        let ended = event.touch_phase == TouchPhase::Ended;
        if let Some(screen) = self.sims.screens.get_mut(agent) {
            screen.pressed = (!ended).then_some(next);
        }
        if ended {
            self.send_input(agent, Input::Request(touch(&udid, "up", next.0, next.1)));
        } else {
            self.send_input(agent, Input::Move(next.0, next.1));
        }
    }

    fn type_into(&mut self, agent: &str, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.sims.acting(agent) {
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
    let acting = bench.sims.acting(agent);
    let button = |id: &'static str,
                  icon: Icon,
                  label: &'static str,
                  kind: &'static str,
                  more: Map<String, Value>,
                  cx: &mut Context<Workbench>| {
        let agent = agent.to_owned();
        IconButton::new(id, icon, label)
            .glyph(13.)
            .disabled(acting)
            .on_click(
                cx.listener(move |bench, _, _, _| bench.device_request(&agent, kind, more.clone())),
            )
    };
    div()
        .flex()
        .items_center()
        .gap(px(2.))
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
                .disabled(acting || !bench.sims.holds(agent))
                .on_click(cx.listener(move |bench, _, _, cx| bench.turn_device(&turning, cx)))
        })
        .into_any_element()
}

/// The agent's device, live: its screen fitted to the desk, taking the person's touches and keys.
pub fn body(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let t = crate::theme::Tokens::get(cx).clone();
    if !bench.sims.holds(agent) {
        let message = match bench.sims.progress {
            Some(fraction) => format!(
                "Getting the simulator ready… {}%",
                (fraction * 100.).round()
            ),
            None => "No device yet. When the agent attaches a simulator, it shows here.".into(),
        };
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.text_tertiary)
            .child(ui::text(message, 12.5, FontWeight::NORMAL))
            .into_any_element();
    }
    let Some(screen) = bench.sims.screens.get(agent) else {
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
            cx.listener(move |bench, event, window, cx| bench.press(&down, event, window, cx)),
        )
        .on_mouse_move(cx.listener(move |bench, event, _, _| bench.drag(&moved, event)))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |bench, event: &MouseUpEvent, _, _| {
                bench.lift(&up, Some(event.position))
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |bench, _, _, _| bench.lift(&out, None)),
        )
        .on_scroll_wheel(cx.listener(move |bench, event, _, _| bench.scroll(&wheel, event)))
        .on_key_down(cx.listener(move |bench, event, _, cx| bench.type_into(&keys, event, cx)))
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
        .map(|(chrome, size)| chrome.turned(sim_frame::turn_degrees(&screen.orientation), size));
    let device = match frame {
        Some(frame) => framed_device(
            agent,
            live,
            &frame,
            screen.held.as_deref(),
            bench.sims.acting(agent),
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
        .p(px(16.))
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
    frame: &sim_frame::Turned,
    held: Option<&str>,
    acting: bool,
    cx: &mut Context<Workbench>,
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
                        bench.side_button(&press.0, &press.1, true);
                        bench.parts.redraw(&bench.parts.stage, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |bench, _, _, cx| {
                        bench.side_button(&lift.0, &lift.1, false);
                        bench.parts.redraw(&bench.parts.stage, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |bench, _, _, cx| {
                        bench.side_button(&gone.0, &gone.1, false);
                        bench.parts.redraw(&bench.parts.stage, cx);
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
