//! The app's whole window under load, against a throwaway core and the core's fake
//! agents, drawn offscreen and timed. Every number it prints is headless and in-app:
//! a frame's CPU from the window's render to its paint, never compared with what
//! LatencyProbe measures on screen.
//!
//!   HOME=/tmp/sx-stress/home SIKEMUX_CORE_SOCKET=/tmp/sx-stress/core.sock \
//!   SIKEMUX_SIDECAR_PATH=<release sikemux-host> STRESS_AGENT=<sikemux-fake-acp-agent> \
//!     <core's folder>/sikemux <scenario> [minutes]
//!
//! Copied beside the core as `sikemux`, as a core trusts its app by where it lives.
//! Scenarios: `agents` (ten chats streaming, one in front, then the front one
//! changing), `long` (a chat of thousands of rows scrolled, searched and streamed
//! into), `flood` (a terminal flooding beside a streaming chat), `repo <folder>` (a
//! large repository's Git tab, files, search and finder), `newchat` (⌘N beside three
//! open chats), `soak` (`agents` for `minutes`, ten by default, watching memory), and
//! `windowed` (a chat filled to `STRESS_WINDOWED_ROWS` rows, its memory once settled, its
//! first message found by the core and then by the find bar, asleep with
//! `STRESS_WINDOWED_SLEEP`).
//! Each ends by typing in the front chat's composer while its load runs. `make-repo <folder> [files]` writes a
//! generated repository to stress `repo` with.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    point, px, size, AnyElement, AnyWindowHandle, App, AppContext, Bounds, Context, Element,
    ElementId, Entity, Focusable, GlobalElementId, HeadlessAppContext, InspectorElementId,
    IntoElement, Keystroke, LayoutId, ParentElement, Pixels, PlatformInput, Render, ScrollDelta,
    ScrollEvent, Styled, Window, WindowBounds, WindowOptions,
};
use gpui_transcript::Row as _;
use serde_json::{json, Value};
use sikemux_client::client::CoreClient;
use sikemux_gpui::workbench::Workbench;
use sikemux_gpui::{core, icons, memory, theme, WINDOW_SIZE};
use sikemux_wire::protocol::{ChatAttachment, LaunchIdentity, Pane, SpawnTarget, TerminalSpawn};

const FRAME: Duration = Duration::from_micros(8_333);
const PROJECT: &str = "stress";

thread_local! {
    /// Each drawn frame's CPU from render to paint, in ms.
    static DRAWS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
    /// The transcript's work counters as each frame began, beside `DRAWS`, with `counts` on.
    static COUNTED: RefCell<Vec<Vec<(&'static str, u64)>>> = const { RefCell::new(Vec::new()) };
}

/// The window's content: the workbench, timed wherever the test platform draws it.
struct Probe {
    bench: Entity<Workbench>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        DRAWS.with_borrow_mut(|draws| draws.push(0.));
        #[cfg(feature = "counts")]
        COUNTED.with_borrow_mut(|counted| counted.push(gpui_transcript::counts::now()));
        gpui::div()
            .size_full()
            .child(Timed(self.bench.clone().into_any_element()))
    }
}

struct Timed(AnyElement);

impl IntoElement for Timed {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Timed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let began = Instant::now();
        let layout = self.0.request_layout(window, cx);
        add_to_draw(began);
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let began = Instant::now();
        self.0.prepaint(window, cx);
        add_to_draw(began);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let began = Instant::now();
        self.0.paint(window, cx);
        add_to_draw(began);
    }
}

/// What the transcript counted while frame `frame` drew, as ` · ROWS 4, TEXTS 2`; nothing
/// without `counts`.
fn counted_in(frame: usize) -> String {
    COUNTED.with_borrow(|counted| {
        let Some(start) = counted.get(frame) else {
            return String::new();
        };
        let end = counted.get(frame + 1).cloned();
        #[cfg(feature = "counts")]
        let end = end.or_else(|| Some(gpui_transcript::counts::now()));
        let Some(end) = end else {
            return String::new();
        };
        let done: Vec<String> = start
            .iter()
            .zip(&end)
            .filter(|((_, from), (_, to))| to > from)
            .map(|((name, from), (_, to))| format!("{name} {}", to - from))
            .collect();
        match done.is_empty() {
            true => String::new(),
            false => format!(" · {}", done.join(", ")),
        }
    })
}

fn add_to_draw(began: Instant) {
    DRAWS.with_borrow_mut(|draws| {
        if let Some(draw) = draws.last_mut() {
            *draw += began.elapsed().as_secs_f64() * 1e3;
        }
    });
}

fn thread_cpu_ms() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time.tv_sec as f64 * 1e3 + time.tv_nsec as f64 / 1e6
}

/// Resident memory of a process, in MB, as `ps` reads it.
fn rss_mb(pid: u32) -> Option<f64> {
    let out = sikemux_process::user_environment::command("/bin/ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kb: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kb / 1024.)
}

fn footprint_mb(pid: u32) -> Option<f64> {
    memory::footprint(pid).map(|bytes| bytes as f64 / 1_048_576.)
}

fn core_pid(socket: &str) -> Option<u32> {
    let out = sikemux_process::user_environment::command("/usr/bin/pgrep")
        .args(["-f", &format!("socket {socket}")])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn spread(mut values: Vec<f64>) -> String {
    values.sort_by(f64::total_cmp);
    let at = |q: f64| {
        values
            .get(((values.len().max(1) - 1) as f64 * q).round() as usize)
            .copied()
            .unwrap_or(0.)
    };
    format!(
        "p50 {:.2} ms, p95 {:.2}, max {:.2} ({} frames)",
        at(0.5),
        at(0.95),
        at(1.),
        values.len()
    )
}

/// What the core and its fake agents are asked to do, from a thread of their own.
struct Driver {
    runtime: tokio::runtime::Runtime,
    client: CoreClient,
    agent: String,
    folder: PathBuf,
}

impl Driver {
    fn call(&self, method: &str, params: Value) -> Value {
        self.runtime
            .block_on(self.client.call(method, params))
            .unwrap_or_else(|error| panic!("{method}: {error}"))
    }

    fn setup(&self, project_folder: &Path) {
        let profile = json!({ "id": "fake", "name": "Fake", "provider": "opencode", "accent": "#a277ff", "executablePath": self.agent });
        self.call("setAgentAccounts", json!({ "profiles": [profile] }));
        self.call(
            "setPreference",
            json!({ "key": "onboardingComplete", "value": true }),
        );
        self.call(
            "addProject",
            json!({ "id": PROJECT, "name": PROJECT, "path": project_folder }),
        );
    }

    fn chat(&self) -> String {
        let (chat, _) = self
            .runtime
            .block_on(
                self.client
                    .start_chat("opencode:fake".into(), PROJECT.into(), None),
            )
            .expect("a chat starts");
        self.front(&chat, "agent");
        chat
    }

    fn front(&self, target: &str, kind: &str) {
        self.call(
            "openTab",
            json!({ "project": PROJECT, "kind": kind, "target": target }),
        );
    }

    /// Sends `text` to `chat`, waking it first when the core put it to sleep, as the app
    /// does before it sends to a sleeping chat.
    fn prompt(&self, chat: &str, text: String) {
        let send = |text: String| {
            self.runtime.block_on(self.client.acp_prompt(
                chat.to_owned(),
                None,
                text,
                Vec::new(),
                Vec::new(),
            ))
        };
        let sent = match send(text.clone()) {
            Err(error) if error.to_string().contains("not running") => {
                if let Err(error) = self.runtime.block_on(self.client.acp_wake(chat.to_owned())) {
                    eprintln!("wake {chat}: {error}");
                }
                send(text)
            }
            sent => sent,
        };
        if let Err(error) = sent {
            eprintln!("prompt to {chat}: {error}");
        }
    }

    /// Keeps `chat` busy until `stop`: a long stream, then a turn of tool calls.
    fn keep_busy(self: &Arc<Self>, chat: String, words: u64, stop: Arc<AtomicBool>) {
        let driver = self.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                driver.prompt(&chat, format!("stream {words}"));
                std::thread::sleep(Duration::from_millis(words * 2 + 800));
                driver.prompt(&chat, "work 150".into());
                std::thread::sleep(Duration::from_millis(1_500));
            }
        });
    }

    fn terminal(&self, script: &str) -> String {
        let target = SpawnTarget::Terminal(TerminalSpawn {
            cols: 120,
            rows: 40,
            cwd: Some(self.folder.to_string_lossy().into_owned()),
            startup: Some(script.into()),
            ..TerminalSpawn::default()
        });
        let launch = LaunchIdentity {
            version: "0.0.0-stress".into(),
            ..LaunchIdentity::default()
        };
        self.runtime
            .block_on(self.client.spawn(launch, target))
            .expect("a terminal starts")
            .to_string()
    }
}

struct Run {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    bench: Entity<Workbench>,
    last: Instant,
    clock: Instant,
    pid: u32,
    core: Option<u32>,
    shots: PathBuf,
}

/// A phase's frames and how busy the window's thread was.
struct Measured {
    frames: Vec<f64>,
    busy: f64,
    wall: f64,
}

impl Measured {
    fn line(&self, name: &str) -> String {
        format!(
            "{name}: {} · window thread {:.0}% busy · {:.0} frames/s [headless, in-app]",
            spread(self.frames.clone()),
            self.busy,
            self.frames.len() as f64 / self.wall
        )
    }
}

impl Run {
    /// Runs what is ready, as a window's thread would between frames. Never waits
    /// to park: while chats stream, more is always arriving.
    fn pump(&mut self) {
        self.cx.advance_clock(self.clock.elapsed());
        self.clock = Instant::now();
        let began = Instant::now();
        while self.cx.background_executor().tick() && began.elapsed() < Duration::from_millis(4) {}
    }

    fn frame(&mut self) {
        let wait = FRAME.saturating_sub(self.last.elapsed());
        std::thread::sleep(wait);
        self.last = Instant::now();
        self.pump();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame();
        }
    }

    /// Runs frames for `seconds` with `each` called every frame, and times them.
    fn measure(&mut self, seconds: f64, each: impl FnMut(&mut Run, usize)) -> Measured {
        self.measure_while(|began, _| began.elapsed().as_secs_f64() < seconds, each)
    }

    /// Times `count` frames, doing `each` before every one, so the work done is the same however
    /// fast the machine draws.
    fn steps(&mut self, count: usize, each: impl FnMut(&mut Run, usize)) -> Measured {
        self.measure_while(|_, index| index < count, each)
    }

    fn measure_while(
        &mut self,
        more: impl Fn(Instant, usize) -> bool,
        mut each: impl FnMut(&mut Run, usize),
    ) -> Measured {
        DRAWS.with_borrow_mut(Vec::clear);
        COUNTED.with_borrow_mut(Vec::clear);
        let (began, cpu) = (Instant::now(), thread_cpu_ms());
        let mut index = 0;
        while more(began, index) {
            each(self, index);
            self.frame();
            index += 1;
        }
        let wall = began.elapsed().as_secs_f64();
        Measured {
            frames: DRAWS.with_borrow_mut(std::mem::take),
            busy: (thread_cpu_ms() - cpu) / (wall * 1e3) * 100.,
            wall,
        }
    }

    fn act(&mut self, id: &str) {
        let id = id.to_owned();
        self.cx
            .update_window(self.window, |_, window, cx| {
                if let Ok(action) = cx.build_action(&id, None) {
                    window.dispatch_action(action, cx);
                }
            })
            .ok();
    }

    /// Presses `key`, typing its letter as the platform would when it types one.
    fn key(&mut self, key: &str) {
        let os = self.cx.update(|cx| cx.platform_info().os);
        let mut keystroke = Keystroke::parse_with_os(key, os).expect("a keystroke");
        if keystroke.key_char.is_none() && !keystroke.modifiers.modified() {
            keystroke.key_char = match keystroke.key.as_str() {
                "space" => Some(" ".into()),
                single if single.chars().count() == 1 => Some(single.into()),
                _ => None,
            };
        }
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx)
            })
            .ok();
    }

    fn wheel(&mut self, at: (f32, f32), lines: f32) {
        let event = ScrollEvent {
            position: point(px(at.0), px(at.1)),
            delta: ScrollDelta::Lines(point(0., lines)),
            ..Default::default()
        };
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(PlatformInput::Scroll(event), cx)
            })
            .ok();
    }

    fn front_chat(&mut self) -> Option<String> {
        self.bench
            .read_with(&self.cx, |bench, _| bench.agent().map(|agent| agent.id))
    }

    fn draft(&mut self) -> String {
        self.bench
            .read_with(&self.cx, |bench, cx| {
                let agent = bench.agent()?;
                let composing = bench.chats.open.get(&agent.id)?.composer.as_ref()?;
                Some(composing.draft.read(cx).value().to_string())
            })
            .unwrap_or_default()
    }

    fn focus_draft(&mut self) -> bool {
        let bench = self.bench.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                let Some(agent) = bench.read(cx).agent() else {
                    return false;
                };
                let Some(draft) = bench
                    .read(cx)
                    .chats
                    .open
                    .get(&agent.id)
                    .and_then(|chat| chat.composer.as_ref())
                    .map(|composing| composing.draft.clone())
                else {
                    return false;
                };
                draft.read(cx).focus_handle(cx).focus(window, cx);
                true
            })
            .unwrap_or(false)
    }

    fn draft_focused(&mut self) -> bool {
        let bench = self.bench.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                let bench = bench.read(cx);
                bench
                    .agent()
                    .and_then(|agent| bench.chats.open.get(&agent.id))
                    .and_then(|chat| chat.composer.as_ref())
                    .is_some_and(|composing| {
                        composing.draft.read(cx).focus_handle(cx).is_focused(window)
                    })
            })
            .unwrap_or(false)
    }

    /// Types `count` letters into the front chat's composer a few frames apart, each
    /// timed from the key to the end of the first frame that shows it.
    fn typing(&mut self, count: usize) -> Option<Vec<f64>> {
        if !self.focus_draft() {
            return None;
        }
        self.frames(10);
        let mut latencies = Vec::with_capacity(count);
        for _ in 0..count {
            let before = self.draft().len();
            let began = Instant::now();
            self.key("a");
            while self.draft().len() == before && began.elapsed() < Duration::from_secs(1) {
                self.pump();
            }
            self.cx
                .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
                .ok();
            latencies.push(began.elapsed().as_secs_f64() * 1e3);
            self.last = Instant::now();
            self.frames(4);
        }
        for _ in 0..count {
            self.key("backspace");
        }
        self.frames(10);
        Some(latencies)
    }

    fn type_while(&mut self, load: &str) {
        match self.typing(150) {
            Some(latencies) => println!(
                "typing in the composer while {load}, key to echo: {} [headless, in-app]",
                spread(latencies)
            ),
            None => println!("typing while {load}: no chat composer in front"),
        }
    }

    /// Saves what the window shows now, to check a phase showed what it meant to.
    fn shot(&mut self, name: &str) {
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
        if let Ok(image) = self.cx.capture_screenshot(self.window) {
            image.save(self.shots.join(format!("{name}.png"))).ok();
        }
    }

    fn memory(&mut self) -> (f64, f64) {
        (
            rss_mb(self.pid).unwrap_or(0.),
            self.core.and_then(rss_mb).unwrap_or(0.),
        )
    }

    /// Waits up to `seconds` for `ready`, drawing as it goes.
    fn until(&mut self, seconds: u64, mut ready: impl FnMut(&mut Run) -> bool) -> bool {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(seconds) {
            if ready(self) {
                return true;
            }
            self.frame();
        }
        false
    }

    fn rows(&mut self, chat: &str) -> usize {
        let chat = chat.to_owned();
        self.bench.read_with(&self.cx, |bench, cx| {
            bench
                .chats
                .open
                .get(&chat)
                .map_or(0, |chat| chat.transcript.read(cx).rows().len())
        })
    }

    /// Waits until the chat's last row has not changed for three seconds: the app holds only the
    /// chat's end, so how many rows it holds does not say a fill is done.
    fn settle(&mut self, chat: &str) {
        let (mut last, mut since) = (None, Instant::now());
        self.until(120, |run| {
            let now = run.row_ids(chat).pop();
            if now != last {
                (last, since) = (now, Instant::now());
            }
            since.elapsed() > Duration::from_secs(3)
        });
    }

    /// Whether the last row held is in view, where the reader waits for the next page.
    fn at_last_held(&mut self, chat: &str) -> bool {
        let chat = chat.to_owned();
        self.bench.read_with(&self.cx, |bench, cx| {
            bench.chats.open.get(&chat).is_some_and(|chat| {
                let transcript = chat.transcript.read(cx);
                !transcript.rows().is_empty() && transcript.visible().end >= transcript.rows().len()
            })
        })
    }

    fn row_ids(&mut self, chat: &str) -> Vec<String> {
        let chat = chat.to_owned();
        self.bench.read_with(&self.cx, |bench, cx| {
            bench.chats.open.get(&chat).map_or_else(Vec::new, |chat| {
                let transcript = chat.transcript.read(cx);
                transcript
                    .rows()
                    .iter()
                    .map(|row| row.id().to_owned())
                    .collect()
            })
        })
    }

    fn pane_of(&mut self, target: &str) -> Option<(String, String)> {
        let target = target.to_owned();
        self.bench.read_with(&self.cx, |bench, _| {
            let strip = bench.strip()?;
            bench.tabs(&strip).iter().find_map(|tab| match &tab.pane {
                Pane::View {
                    id, target: shown, ..
                } if *shown == target => Some((tab.id.clone(), id.clone())),
                _ => None,
            })
        })
    }
}

/// A chat's message: text, a long code block, or a command whose output is long.
fn long_chat_prompt(index: usize) -> String {
    match index % 20 {
        0 => "run seq 1 3000".into(),
        7 | 14 => {
            let body: String = (0..160)
                .map(|line| format!("    let value_{line} = rows.put(message({line}), &mut cache);\n"))
                .collect();
            format!("block {index}:\n```rust\nfn build_{index}() {{\n{body}}}\n```")
        }
        3 => "work 1".into(),
        _ => format!(
            "message {index}: the core folds each event into rows and sends only what changed, so a long chat stays cheap to draw"
        ),
    }
}

fn agents(run: &mut Run, driver: &Arc<Driver>, minutes: Option<f64>) {
    let stop = Arc::new(AtomicBool::new(false));
    let chats: Vec<String> = (0..10).map(|_| driver.chat()).collect();
    run.until(30, |run| run.front_chat().is_some());
    for chat in &chats {
        driver.keep_busy(chat.clone(), 2_500, stop.clone());
    }
    run.frames(240);
    println!(
        "{}",
        run.measure(30., |_, _| {})
            .line("10 chats streaming, one in front")
    );
    let changing = {
        let (driver, chats) = (driver.clone(), chats.clone());
        run.measure(30., move |_, index| {
            if index % 360 == 0 {
                driver.front(&chats[(index / 360) % chats.len()], "agent");
            }
        })
    };
    println!(
        "{}",
        changing.line("10 chats streaming, the front one changing every 3 s")
    );
    let front = run.front_chat().unwrap_or_default();
    let (open, rows) = (
        run.bench
            .read_with(&run.cx, |bench, _| bench.chats.open.len()),
        run.rows(&front),
    );
    println!("  {open} chats held by the window, {rows} rows in the one in front");
    run.shot("agents");
    run.type_while("10 chats stream");
    if let Some(minutes) = minutes {
        soak(run, minutes);
    }
    stop.store(true, Ordering::Relaxed);
}

/// Opens new chats with ⌘N beside three already open, timing from the key to the new chat in
/// front, to its composer holding the keys, and the frames of the switch. With
/// FAKE_ACP_START_GATE set, the fake agent's start is held `STRESS_START_MS` (1500 by default),
/// as long as a real agent's program takes to start.
fn new_chats(run: &mut Run, driver: &Arc<Driver>) {
    for _ in 0..3 {
        driver.chat();
    }
    run.until(30, |run| run.front_chat().is_some());
    run.frames(240);
    let gate = std::env::var_os("FAKE_ACP_START_GATE").map(PathBuf::from);
    let hold: u64 = std::env::var("STRESS_START_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or(1_500);
    let (mut fronts, mut keys, mut switching) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..6 {
        let before = run.front_chat();
        if let Some(gate) = gate.clone() {
            std::fs::write(&gate, b"").ok();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(hold));
                std::fs::remove_file(gate).ok();
            });
        }
        let began = Instant::now();
        run.act("agent.new");
        if !run.until(15, |run| run.front_chat() != before) {
            println!("new chat: none came to the front in 15 s");
            return;
        }
        fronts.push(began.elapsed().as_secs_f64() * 1e3);
        let mut focused = None;
        let switch = run.measure(0.4, |run, _| {
            if focused.is_none() && run.draft_focused() {
                focused = Some(began.elapsed());
            }
        });
        switching.extend(switch.frames);
        if focused.is_none() && run.until(15, |run| run.draft_focused()) {
            focused = Some(began.elapsed());
        }
        match focused {
            Some(focused) => keys.push(focused.as_secs_f64() * 1e3),
            None => println!("new chat: its composer never held the keys"),
        }
        run.frames(120);
    }
    println!(
        "new chat, key to the chat in front: {} [headless, in-app]",
        spread(fronts)
    );
    println!(
        "new chat, key to its composer holding the keys: {} [headless, in-app]",
        spread(keys)
    );
    println!(
        "new chat, frames of the switch's first 400 ms: {} [headless, in-app]",
        spread(switching)
    );
    let newest = run.front_chat().unwrap_or_default();
    driver.prompt(&newest, "stream 40".into());
    if !run.until(15, |run| run.rows(&newest) > 1) {
        println!("new chat: its reply never showed");
    }
}

/// Keeps drawing for `minutes`, printing the app's and the core's memory each
/// minute, and whether the app's last half held flat.
fn soak(run: &mut Run, minutes: f64) {
    let mut samples = Vec::new();
    let began = Instant::now();
    let mut next = Duration::ZERO;
    let mut frames = Vec::new();
    while began.elapsed().as_secs_f64() < minutes * 60. {
        if began.elapsed() >= next {
            let (app, core) = run.memory();
            println!(
                "  {:>4.1} min: app {app:.0} MB, core {core:.0} MB",
                began.elapsed().as_secs_f64() / 60.
            );
            samples.push(app);
            next += Duration::from_secs(60);
        }
        let measured = run.measure(10., |_, _| {});
        frames.extend(measured.frames);
    }
    let half = &samples[samples.len() / 2..];
    let (low, high) = half.iter().fold((f64::MAX, 0f64), |(low, high), mb| {
        (low.min(*mb), high.max(*mb))
    });
    println!(
        "soak {minutes} min: {} · app memory over the last half {low:.0}–{high:.0} MB [headless, in-app]",
        spread(frames)
    );
}

const FIRST_PHRASE: &str = "where the windowed chat began";

/// Fills a chat to `rows` rows with the window showing it, then prints the memory both hold once
/// it settles, and what the core's find answers for a phrase only the first message holds.
fn windowed(run: &mut Run, driver: &Arc<Driver>) {
    let rows: usize = std::env::var("STRESS_WINDOWED_ROWS")
        .ok()
        .and_then(|rows| rows.parse().ok())
        .unwrap_or(10_000);
    let chat = driver.chat();
    run.until(30, |run| run.front_chat().is_some());
    let filled = Instant::now();
    for index in 0..rows / 2 {
        // The long chat's mix of text and code, without its command and tool turns, which take
        // seconds each.
        let text = match index {
            0 => format!("message 0: {FIRST_PHRASE}"),
            _ if matches!(index % 20, 0 | 3) => long_chat_prompt(1),
            _ => long_chat_prompt(index),
        };
        driver.prompt(&chat, text);
        std::thread::sleep(Duration::from_millis(25));
        if index % 50 == 0 {
            run.frames(2);
        }
    }
    // The app may hold only the chat's end, so the fill is also done once its last row stops changing.
    let (mut last, mut since) = (None, Instant::now());
    run.until(180, |run| {
        let ids = run.row_ids(&chat);
        if ids.len() >= rows {
            return true;
        }
        let now = ids.last().cloned();
        if now != last {
            (last, since) = (now, Instant::now());
        }
        since.elapsed() > Duration::from_secs(5)
    });
    run.frames(240);
    let (app, core) = (
        footprint_mb(run.pid).unwrap_or(0.),
        run.core.and_then(footprint_mb).unwrap_or(0.),
    );
    let held = run.row_ids(&chat);
    println!(
        "windowed {rows} rows: footprint app {app:.0} MB, core {core:.0} MB, the app holds {} rows, filled in {:.0}s",
        held.len(),
        filled.elapsed().as_secs_f64()
    );
    let feed = match driver
        .runtime
        .block_on(driver.client.acp_attach(chat.clone()))
    {
        Ok(ChatAttachment::Live { mark, .. }) => mark.feed,
        Ok(_) => {
            println!("windowed find: the attach answered without a mark");
            return;
        }
        Err(error) => {
            println!("windowed find: the attach failed: {error}");
            return;
        }
    };
    // The core scans a slice of the chat per call, newest first, and hands back where to go on.
    let (mut hits, mut from, mut steps) = (Vec::new(), Value::Null, 0);
    let began = Instant::now();
    loop {
        let asked = json!({ "agentId": chat, "feed": feed, "query": FIRST_PHRASE, "limit": 5, "from": from });
        let answer = match driver
            .runtime
            .block_on(driver.client.call("chatFind", asked))
        {
            Ok(answer) => answer,
            Err(error) => {
                println!("windowed find: chatFind failed: {error}");
                return;
            }
        };
        steps += 1;
        hits.extend(
            answer["hits"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|hit| hit["row"].as_str().map(str::to_owned)),
        );
        from = answer["next"].clone();
        if !hits.is_empty() || from.is_null() || steps == 500 {
            break;
        }
    }
    let loaded = hits.iter().filter(|row| held.contains(row)).count();
    println!(
        "windowed find: {} hits for the first message, {loaded} in rows the app holds, after {steps} calls in {:.1}s",
        hits.len(),
        began.elapsed().as_secs_f64()
    );
    if std::env::var_os("STRESS_WINDOWED_SLEEP").is_some() {
        driver.call("sleepAgent", json!({ "agentId": chat }));
        run.frames(120);
        println!("windowed sleep: the chat sleeps before the find bar looks");
    }
    if let Some(first) = hits.first() {
        find_jump(run, &chat, first, held.last().cloned());
    }
}

/// Finds the first message from the chat's find bar, which brings in the rows around it, then
/// goes back to the latest rows, timing each.
fn find_jump(run: &mut Run, chat: &str, row: &str, latest: Option<String>) {
    run.key("cmd-f");
    for key in FIRST_PHRASE.chars() {
        run.key(&if key == ' ' {
            "space".into()
        } else {
            key.to_string()
        });
    }
    run.frames(120);
    let began = Instant::now();
    run.key("enter");
    let came = run.until(15, |run| run.row_ids(chat).iter().any(|id| id == row));
    println!(
        "windowed jump: the first message {} in {:.0} ms, the app holds {} rows",
        if came { "came" } else { "never came" },
        began.elapsed().as_secs_f64() * 1e3,
        run.row_ids(chat).len()
    );
    run.key("escape");
    // Scrolls down from the first message for 20 pages. A page that came before the reader reached
    // the last row held cost no wait; otherwise the wait runs from reaching it to the page coming.
    let middle = (WINDOW_SIZE.0 / 2., WINDOW_SIZE.1 / 2.);
    let done = Cell::new(false);
    let (mut waits, mut stuck_since, mut stalled) = (Vec::new(), None::<Instant>, false);
    let mut last = run.row_ids(chat).last().cloned();
    // Before each step: the frames drawn so far, when, and the first and last rows held, to name
    // what a slow frame was doing.
    let mut held = Vec::new();
    let scrolling = Instant::now();
    let scrolled = run.measure_while(
        |began, _| !done.get() && began.elapsed() < Duration::from_secs(120),
        |run, _| {
            run.wheel(middle, -12.);
            let ids = run.row_ids(chat);
            let now = ids.last().cloned();
            held.push((
                DRAWS.with_borrow(Vec::len),
                scrolling.elapsed().as_secs_f64() * 1e3,
                ids.first().cloned(),
                now.clone(),
            ));
            if now != last {
                waits.push(
                    stuck_since
                        .take()
                        .map_or(0., |since| since.elapsed().as_secs_f64() * 1e3),
                );
                last = now;
                if waits.len() == 20 || last == latest {
                    done.set(true);
                }
            } else if let Some(since) = stuck_since {
                if since.elapsed() > Duration::from_secs(5) {
                    stalled = true;
                    done.set(true);
                }
            } else if run.at_last_held(chat) {
                stuck_since = Some(Instant::now());
            }
        },
    );
    if stalled {
        println!(
            "windowed down: a page never came within 5 s of reaching the last row held, after {} pages",
            waits.len()
        );
    } else {
        println!(
            "windowed down: {} pages{}, longest wait at the last row held {:.0} ms, slowest frame {:.1} ms · {}",
            waits.len(),
            if last == latest { ", reaching the latest rows" } else { "" },
            waits.iter().copied().fold(0., f64::max),
            scrolled.frames.iter().copied().fold(0., f64::max),
            spread(scrolled.frames.clone())
        );
    }
    let slow = scrolled
        .frames
        .iter()
        .enumerate()
        .filter(|(_, ms)| **ms > 16.);
    for (frame, ms) in slow.take(10) {
        // The window may draw more than once between two steps; a frame belongs to the last step
        // that came before it, and what changed shows in the step after.
        let step = held
            .partition_point(|seen| seen.0 <= frame)
            .saturating_sub(1);
        let (Some(before), after) = (held.get(step), held.get(step + 1)) else {
            continue;
        };
        let mut doing = Vec::new();
        if after.is_some_and(|after| after.3 != before.3) {
            doing.push("page insert");
        }
        if after.is_some_and(|after| after.2 != before.2) {
            doing.push("trim");
        }
        if doing.is_empty() {
            doing.push("scroll only (no rows came or went)");
        }
        println!(
            "windowed slow frame: #{frame} at {:.0} ms into the scroll took {ms:.1} ms, during {}{}",
            before.1,
            doing.join(" and "),
            counted_in(frame)
        );
    }
    #[cfg(feature = "counts")]
    COUNTED.with_borrow(|counted| {
        let total = |at: &[(&'static str, u64)], name: &str| {
            at.iter()
                .find(|(counter, _)| *counter == name)
                .map_or(0, |(_, value)| *value)
        };
        let (Some(start), end) = (counted.first(), gpui_transcript::counts::now()) else {
            return;
        };
        let lines = total(&end, "NEW_CODE_LINES") - total(start, "NEW_CODE_LINES");
        let micros = total(&end, "NEW_CODE_US") - total(start, "NEW_CODE_US");
        println!(
            "windowed code: {lines} lines laid out for the first time in {:.1} ms, {:.0} µs a line",
            micros as f64 / 1e3,
            micros as f64 / lines.max(1) as f64
        );
    });
    let began = Instant::now();
    let to = chat.to_owned();
    run.bench
        .update(&mut run.cx, |bench, cx| bench.show_latest(&to, cx));
    let back = run.until(15, |run| run.row_ids(chat).last() == latest.as_ref());
    println!(
        "windowed latest: {} in {:.0} ms, the app holds {} rows",
        if back { "back" } else { "never back" },
        began.elapsed().as_secs_f64() * 1e3,
        run.row_ids(chat).len()
    );
}

fn long(run: &mut Run, driver: &Arc<Driver>) {
    let chat = driver.chat();
    run.until(30, |run| run.front_chat().is_some());
    let filled = Instant::now();
    let rows: usize = std::env::var("STRESS_LONG_ROWS")
        .ok()
        .and_then(|rows| rows.parse().ok())
        .unwrap_or(2_800);
    for index in 0..rows / 2 {
        driver.prompt(&chat, long_chat_prompt(index));
        std::thread::sleep(Duration::from_millis(match index % 20 {
            0 | 3 => 400,
            _ => 25,
        }));
        if index % 50 == 0 {
            run.frames(2);
        }
    }
    run.settle(&chat);
    let (app, core) = run.memory();
    println!(
        "long chat: {rows} rows, {} held, in {:.0}s · app {app:.0} MB, core {core:.0} MB",
        run.rows(&chat),
        filled.elapsed().as_secs_f64()
    );
    let transcript = (WINDOW_SIZE.0 / 2., WINDOW_SIZE.1 / 2.);
    let up = run.steps(2_000, move |run, _| run.wheel(transcript, 6.));
    println!("{}", up.line("scrolling up through the long chat"));
    run.shot("long-up");
    let down = run.steps(2_000, move |run, _| run.wheel(transcript, -6.));
    println!("{}", down.line("scrolling down again"));
    run.focus_draft();
    run.key("cmd-f");
    for key in ["f", "n", "space", "b", "u", "i", "l", "d"] {
        run.key(key);
    }
    let found = run.steps(600, |run, index| {
        if index % 12 == 0 {
            run.key("enter");
        }
    });
    println!("{}", found.line("stepping through find matches"));
    run.shot("long-find");
    run.key("escape");
    driver.prompt(&chat, "stream 20000".into());
    run.frames(60);
    let streaming = run.measure(20., |_, _| {});
    println!(
        "{}",
        streaming.line("streaming into the long chat, following the end")
    );
    let scrolled = run.steps(800, move |run, index| {
        run.wheel(transcript, if index % 240 < 120 { 4. } else { -4. })
    });
    println!("{}", scrolled.line("scrolling while it streams"));
    run.type_while("the long chat streams");
}

fn flood(run: &mut Run, driver: &Arc<Driver>, log: &Path) {
    let chat = driver.chat();
    run.until(30, |run| run.front_chat().is_some());
    let session = driver.terminal(&format!("while :; do cat '{}'; done", log.display()));
    let Some((tab, pane)) = run.pane_of(&chat) else {
        println!("flood: the chat's tab never showed");
        return;
    };
    driver.call(
        "splitPane",
        json!({ "project": PROJECT, "tab": tab, "pane": pane, "side": "right", "kind": "terminal", "target": session }),
    );
    let stop = Arc::new(AtomicBool::new(false));
    driver.keep_busy(chat, 4_000, stop.clone());
    run.frames(240);
    println!(
        "{}",
        run.measure(30., |_, _| {})
            .line("a terminal flooding beside a streaming chat")
    );
    run.shot("flood");
    run.type_while("a terminal floods beside the chat");
    stop.store(true, Ordering::Relaxed);
}

fn repo(run: &mut Run, driver: &Arc<Driver>) {
    run.frames(120);
    run.act("window.git");
    run.frames(10);
    let git = run.measure(15., |_, _| {});
    println!("{}", git.line("the Git tab on the large repository"));
    run.shot("repo-git");
    run.act("window.files");
    let files = run.measure(10., move |run, index| {
        if index % 30 == 0 {
            run.wheel((WINDOW_SIZE.0 - 150., WINDOW_SIZE.1 / 2.), -8.)
        }
    });
    println!("{}", files.line("the Files rail on the large repository"));
    run.shot("repo-files");
    run.act("window.search");
    run.frames(20);
    for key in ["n", "e", "e", "d", "l", "e"] {
        run.key(key);
    }
    let searched = run.measure(15., |_, _| {});
    println!(
        "{}",
        searched.line("searching the large repository for \"needle\"")
    );
    run.shot("repo-search");
    run.act("palette.files");
    run.frames(20);
    let finder = run.measure(10., |run, index| {
        let typed = ["m", "o", "d", "4", "2", "backspace", "7"];
        if index % 20 == 0 && index / 20 < typed.len() {
            run.key(typed[index / 20]);
        }
    });
    println!("{}", finder.line("typing in the file finder"));
    run.shot("repo-finder");
    run.key("escape");
    let chat = driver.chat();
    let _ = chat;
    run.until(30, |run| run.front_chat().is_some());
    run.type_while("the large repository is open");
}

/// A generated repository of `files` small files, committed, then with 2,000
/// changed and 500 new, for the Git tab to show.
fn make_repo(folder: &Path, files: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        folder.starts_with("/tmp") || folder.starts_with("/private/tmp"),
        "the generated repository goes under /tmp"
    );
    std::fs::create_dir_all(folder)?;
    let per_folder = 20;
    for index in 0..files {
        let dir = folder
            .join(format!("pkg{:03}", index / (per_folder * 50)))
            .join(format!("mod{:02}", (index / per_folder) % 50));
        if index % per_folder == 0 {
            std::fs::create_dir_all(&dir)?;
        }
        let needle = if index % 97 == 0 {
            "\n// needle here\n"
        } else {
            ""
        };
        std::fs::write(
            dir.join(format!("file{index:06}.rs")),
            format!("pub fn value_{index}() -> u64 {{\n    {index}\n}}\n{needle}"),
        )?;
    }
    let git = |args: &[&str]| -> anyhow::Result<()> {
        let status = sikemux_process::user_environment::command("git")
            .current_dir(folder)
            .args(args)
            .status()?;
        anyhow::ensure!(status.success(), "git {args:?} failed");
        Ok(())
    };
    git(&["init", "-q"])?;
    // Sikemux's git refuses `add -A`, as other agents may share a checkout; this one is
    // only ours, so its folders are named.
    let folders: Vec<String> = (0..files.div_ceil(per_folder * 50))
        .map(|pkg| format!("pkg{pkg:03}"))
        .collect();
    let mut add = vec!["add", "--"];
    add.extend(folders.iter().map(String::as_str));
    git(&add)?;
    git(&[
        "-c",
        "user.name=stress",
        "-c",
        "user.email=stress@example.invalid",
        "commit",
        "-qm",
        "generated",
    ])?;
    for index in (0..files).step_by(files / 2_000) {
        let dir = folder
            .join(format!("pkg{:03}", index / (per_folder * 50)))
            .join(format!("mod{:02}", (index / per_folder) % 50));
        let path = dir.join(format!("file{index:06}.rs"));
        let text = std::fs::read_to_string(&path)?;
        std::fs::write(&path, format!("{text}// changed\n"))?;
    }
    std::fs::create_dir_all(folder.join("fresh"))?;
    for index in 0..500 {
        std::fs::write(
            folder.join("fresh").join(format!("new{index}.rs")),
            "// new\n",
        )?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let scenario = args.next().unwrap_or_else(|| "agents".into());
    if scenario == "make-repo" {
        let folder = PathBuf::from(args.next().unwrap_or_else(|| "/tmp/sx-stress/repo".into()));
        let files = args
            .next()
            .and_then(|count| count.parse().ok())
            .unwrap_or(100_000);
        let began = Instant::now();
        make_repo(&folder, files)?;
        println!(
            "wrote {files} files to {} in {:.0}s",
            folder.display(),
            began.elapsed().as_secs_f64()
        );
        return Ok(());
    }
    let socket = std::env::var(sikemux_core::SOCKET_ENV)
        .map_err(|_| anyhow::anyhow!("set SIKEMUX_CORE_SOCKET and a HOME of its own"))?;
    let agent = std::env::var("STRESS_AGENT")
        .map_err(|_| anyhow::anyhow!("set STRESS_AGENT to sikemux-fake-acp-agent"))?;
    let base = Path::new(&socket)
        .parent()
        .map_or_else(|| PathBuf::from("/tmp/sx-stress"), Path::to_path_buf);
    let project_folder = match scenario.as_str() {
        "repo" => PathBuf::from(args.next().unwrap_or_else(|| "/tmp/sx-stress/repo".into())),
        _ => base.join("work"),
    };
    std::fs::create_dir_all(&project_folder)?;
    let minutes: Option<f64> = args.next().and_then(|minutes| minutes.parse().ok());

    // The app's main thread runs at this priority; without it a busy machine can put the
    // window's thread on its slower cores and make every number several times larger.
    // SAFETY: it only changes the calling thread's own scheduling class.
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
    };
    let link = core::start(core::Home::default());
    let runtime = tokio::runtime::Runtime::new()?;
    let started = Instant::now();
    let client = loop {
        match runtime.block_on(CoreClient::connect(Path::new(&socket))) {
            Ok((client, _events)) => break client,
            Err(_) if started.elapsed() < Duration::from_secs(30) => {
                std::thread::sleep(Duration::from_millis(200))
            }
            Err(error) => anyhow::bail!("no core answers at {socket}: {error}"),
        }
    };
    let driver = Arc::new(Driver {
        runtime,
        client,
        agent,
        folder: project_folder.clone(),
    });
    driver.setup(&project_folder);

    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        memory::log(cx);
    });
    let held = Rc::new(RefCell::new(None));
    let kept = held.clone();
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
                })),
                focus: true,
                show: false,
                ..Default::default()
            },
            |window, cx| {
                let bench = cx.new(|cx| Workbench::new(link, window, cx));
                kept.replace(Some(bench.clone()));
                let probe = cx.new(|_| Probe { bench });
                cx.new(|cx| gpui_kit::base::Root::new(probe, window, cx))
            },
        )
    })?;
    let bench = held.take().expect("the workbench");
    cx.update_window(window.into(), |_, window, _| window.activate_window())
        .ok();
    let mut run = Run {
        cx,
        window: window.into(),
        bench,
        last: Instant::now(),
        clock: Instant::now(),
        pid: std::process::id(),
        core: core_pid(&socket),
        shots: base.join("shots"),
    };
    std::fs::create_dir_all(&run.shots)?;
    run.frames(120);
    let (app, core) = run.memory();
    println!("start: app {app:.0} MB, core {core:.0} MB");

    match scenario.as_str() {
        "agents" => agents(&mut run, &driver, None),
        "soak" => agents(&mut run, &driver, Some(minutes.unwrap_or(10.))),
        "long" => long(&mut run, &driver),
        "newchat" => new_chats(&mut run, &driver),
        "windowed" => windowed(&mut run, &driver),
        "flood" => {
            let log = base.join("flood.log");
            if !log.exists() {
                let lines: String = (0..5_000)
                    .map(|line| format!("   Compiling crate-{line} v0.{line}.0 (/tmp/sx-stress/work/crate-{line})\r\n"))
                    .collect();
                std::fs::write(&log, lines)?;
            }
            flood(&mut run, &driver, &log)
        }
        "repo" => repo(&mut run, &driver),
        other => anyhow::bail!("no scenario named {other}"),
    }
    let (app, core) = run.memory();
    println!("end: app {app:.0} MB, core {core:.0} MB");
    Ok(())
}
