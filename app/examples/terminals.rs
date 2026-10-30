//! Terminal panes against a real core, drawn offscreen and timed: typing alone,
//! typing beside one and then four terminals flooding, and scrolling back
//! through a long history.
//!
//!   SIKEMUX_CORE_SOCKET=/tmp/x/core.sock SIKEMUX_SIDECAR_PATH=path/to/sikemux-host HOME=/tmp/x/home \
//!     cargo run --release --example terminals -- [log to flood with] [shots folder]
//!
//! It starts its own core at that socket, so run it with a HOME of its own. A
//! frame's time is render, layout, prepaint and paint, everything but handing
//! the frame to the GPU; typing is from the key to the end of the first frame
//! that shows its echo.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, point, px, relative, size, AnyElement, AnyWindowHandle, App, AppContext, Bounds, Context,
    Element, ElementId, Entity, GlobalElementId, HeadlessAppContext, InspectorElementId,
    IntoElement, Keystroke, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, PlatformInput, Render, ScrollDelta, ScrollEvent, Styled, Window,
    WindowBounds, WindowOptions,
};
use sikemux_client::client::CoreClient;
use sikemux_gpui::hosts::Place;
use sikemux_gpui::terminal;
use sikemux_gpui::workbench::Workbench;
use sikemux_gpui::{core, icons, theme, WINDOW_SIZE};
use sikemux_wire::protocol::{LaunchIdentity, SessionId, SpawnTarget, TerminalSpawn};

const FRAME: Duration = Duration::from_micros(8_333);

struct Panes {
    bench: Entity<Workbench>,
    shown: Vec<SessionId>,
}

impl Render for Panes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        DRAWS.with_borrow_mut(|draws| draws.push(0.));
        let entity = self.bench.clone();
        let panes: Vec<AnyElement> = self
            .shown
            .iter()
            .map(|id| {
                self.bench.update(cx, |bench, cx| {
                    terminal::pane(bench, &entity, &Place::This, &id.to_string(), cx)
                })
            })
            .collect();
        let columns = self.shown.len().clamp(1, 3);
        let rows = self.shown.len().div_ceil(columns).max(1);
        div()
            .size_full()
            .flex()
            .flex_wrap()
            .bg(gpui::rgb(0x0f0f13))
            .font_family(theme::MONO)
            .children(panes.into_iter().map(|pane| {
                div()
                    .w(relative(1. / columns as f32))
                    .h(relative(1. / rows as f32))
                    .child(Timed(pane))
            }))
    }
}

thread_local! {
    /// How long each window draw spent laying out and painting all its panes, in ms.
    static DRAWS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
}

/// A pane whose prepaint and paint are timed. The test platform also draws
/// whenever the window changes, so draws are timed wherever they happen.
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
        (self.0.request_layout(window, cx), ())
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

struct Run {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    bench: Entity<Workbench>,
    panes: Entity<Panes>,
    last: Instant,
    clock: Instant,
}

impl Run {
    /// Runs what is ready, as a window's thread would between frames. Never
    /// waits to park: while terminals flood, more is always arriving. The test
    /// clock is moved on with the real one, so the window's timers fire.
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

    fn show(&mut self, shown: Vec<SessionId>) {
        self.panes.update(&mut self.cx, |panes, cx| {
            panes.shown = shown;
            cx.notify();
        });
        for _ in 0..120 {
            self.frame();
        }
        DRAWS.with_borrow_mut(Vec::clear);
    }

    fn frame_number(&mut self, id: SessionId) -> u32 {
        self.bench.read_with(&self.cx, |bench, _| {
            bench
                .terminals
                .screen(bench.terminals.key(&Place::This, id))
                .map_or(0, |screen| screen.header.frame)
        })
    }

    fn cursor(&mut self, id: SessionId) -> (u16, u16) {
        self.bench.read_with(&self.cx, |bench, _| {
            let key = bench.terminals.key(&Place::This, id);
            bench.terminals.screen(key).map_or((0, 0), |screen| {
                (screen.header.cursor.row, screen.header.cursor.col)
            })
        })
    }

    fn focus(&mut self, id: SessionId) {
        let bench = self.bench.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                let terminals = &bench.read(cx).terminals;
                let focus = terminals.focus(terminals.key(&Place::This, id)).cloned();
                if let Some(focus) = focus {
                    window.focus(&focus, cx);
                }
            })
            .ok();
    }

    fn key(&mut self, key: &str) {
        let os = self.cx.update(|cx| cx.platform_info().os);
        let keystroke = Keystroke::parse_with_os(key, os).expect("a keystroke");
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx)
            })
            .ok();
    }

    /// Types `count` keys a few frames apart, timing each to its echo.
    fn typing(&mut self, id: SessionId, count: usize) -> Vec<f64> {
        self.focus(id);
        let mut latencies = Vec::new();
        for index in 0..count {
            let key = if index % 50 == 49 { "enter" } else { "a" };
            let before = self.cursor(id);
            let began = Instant::now();
            self.key(key);
            while self.cursor(id) == before && began.elapsed() < Duration::from_secs(1) {
                self.pump();
                std::thread::sleep(Duration::from_micros(200));
            }
            self.cx
                .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
                .ok();
            latencies.push(began.elapsed().as_secs_f64() * 1e3);
            if latencies.len() >= 3
                && latencies[latencies.len() - 3..]
                    .iter()
                    .all(|latency| *latency >= 1000.)
            {
                eprintln!("three keys went unanswered: GPUI's test scheduler lost the window's wake-up; run again");
                std::process::exit(3);
            }
            self.last = Instant::now();
            for _ in 0..5 {
                self.frame();
            }
        }
        latencies
    }

    fn wheel(&mut self, lines: f32) {
        let event = ScrollEvent {
            position: point(px(300.), px(300.)),
            delta: ScrollDelta::Lines(point(0., lines)),
            ..Default::default()
        };
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(PlatformInput::Scroll(event), cx)
            })
            .ok();
    }

    /// Moves the mouse over a cell of a lone pane, then clicks it `clicks` times.
    fn point(&mut self, row: u16, col: u16, clicks: Option<usize>) {
        let cell = self
            .bench
            .read_with(&self.cx, |bench, _| bench.terminals.cell())
            .unwrap_or_default();
        let position = point(
            px(16.) + cell.width * (f32::from(col) + 0.5),
            px(8.) + cell.height * (f32::from(row) + 0.5),
        );
        let mut events = vec![PlatformInput::MouseMove(MouseMoveEvent {
            position,
            ..Default::default()
        })];
        for click_count in 1..=clicks.unwrap_or(0) {
            events.push(PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                click_count,
                ..Default::default()
            }));
            events.push(PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position,
                click_count,
                ..Default::default()
            }));
        }
        for event in events {
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.dispatch_event(event, cx)
                })
                .ok();
        }
        for _ in 0..10 {
            self.frame();
        }
    }

    fn right_click(&mut self, row: u16, col: u16) {
        let cell = self
            .bench
            .read_with(&self.cx, |bench, _| bench.terminals.cell())
            .unwrap_or_default();
        let position = point(
            px(16.) + cell.width * (f32::from(col) + 0.5),
            px(8.) + cell.height * (f32::from(row) + 0.5),
        );
        let event = PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Right,
            position,
            click_count: 1,
            ..Default::default()
        });
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(event, cx)
            })
            .ok();
        for _ in 0..10 {
            self.frame();
        }
    }

    fn shot(&mut self, folder: &Option<PathBuf>, name: &str) {
        let Some(folder) = folder else {
            return;
        };
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
        if let Ok(image) = self.cx.capture_screenshot(self.window) {
            image.save(folder.join(format!("{name}.png"))).ok();
        }
    }
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
        "p50 {:.2} ms, p90 {:.2}, p99 {:.2}, max {:.2} ({} samples)",
        at(0.5),
        at(0.9),
        at(0.99),
        at(1.),
        values.len()
    )
}

/// The most memory this process has held, in MB.
fn peak_rss_mb() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    usage.ru_maxrss as f64 / 1024. / 1024.
}

fn spawn(runtime: &tokio::runtime::Runtime, client: &CoreClient, script: &str) -> SessionId {
    let target = SpawnTarget::Terminal(TerminalSpawn {
        cols: 120,
        rows: 40,
        cwd: Some("/tmp".into()),
        startup: Some(script.into()),
        ..TerminalSpawn::default()
    });
    let launch = LaunchIdentity {
        version: "0.0.0-bench".into(),
        ..LaunchIdentity::default()
    };
    runtime
        .block_on(client.spawn(launch, target))
        .expect("a terminal starts")
}

fn main() {
    let Ok(socket) = std::env::var(sikemux_core::SOCKET_ENV) else {
        eprintln!(
            "set SIKEMUX_CORE_SOCKET and a HOME of its own, so the core it starts is not yours"
        );
        std::process::exit(2);
    };
    let log = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/termbench/buildlog-crlf.txt".into());
    let shots = std::env::args().nth(2).map(PathBuf::from);
    let link = core::start(core::Home::default());
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let started = Instant::now();
    let client = loop {
        match runtime.block_on(CoreClient::connect(std::path::Path::new(&socket))) {
            Ok((client, _events)) => break client,
            Err(_) if started.elapsed() < Duration::from_secs(20) => {
                std::thread::sleep(Duration::from_millis(200))
            }
            Err(error) => panic!("no core answers at {socket}: {error}"),
        }
    };
    let typist = spawn(&runtime, &client, "stty -icanon; exec cat");
    let flood = format!("while :; do cat '{log}'; done");
    let floods: Vec<SessionId> = (0..4).map(|_| spawn(&runtime, &client, &flood)).collect();
    let history = spawn(
        &runtime,
        &client,
        "seq 1 20000; echo 'docs live at https://sikemux.com/docs/terminal.'; exec cat",
    );

    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
    });
    let mut parts = None;
    let window = cx
        .update(|cx| {
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
                    let panes = cx.new(|cx: &mut Context<Panes>| {
                        cx.observe(&bench, |_, _, cx| cx.notify()).detach();
                        Panes {
                            bench: bench.clone(),
                            shown: Vec::new(),
                        }
                    });
                    parts = Some((bench, panes.clone()));
                    panes
                },
            )
        })
        .expect("window opens")
        .into();
    let (bench, panes) = parts.expect("window parts");
    cx.update_window(window, |_, window, _| window.activate_window())
        .ok();
    let mut run = Run {
        cx,
        window,
        bench,
        panes,
        last: Instant::now(),
        clock: Instant::now(),
    };

    run.show(vec![typist]);
    let idle = run.typing(typist, 100);
    println!("typing alone: {}", spread(idle));
    run.focus(typist);
    DRAWS.with_borrow_mut(Vec::clear);
    let (began, cpu) = (Instant::now(), thread_cpu_ms());
    while began.elapsed() < Duration::from_secs(3) {
        run.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    let blinks = DRAWS.with_borrow_mut(std::mem::take);
    println!("idle 3 s with a blinking cursor: {} draws, window thread {:.1} ms; panes laid out and painted {}", blinks.len(), thread_cpu_ms() - cpu, spread(blinks));
    run.shot(&shots, "typing");

    for count in [1, 4] {
        let mut shown = vec![typist];
        shown.extend(&floods[..count]);
        run.show(shown);
        let before: Vec<u32> = floods[..count]
            .iter()
            .map(|id| run.frame_number(*id))
            .collect();
        let (began, cpu) = (Instant::now(), thread_cpu_ms());
        for _ in 0..360 {
            run.frame();
        }
        let busy = (thread_cpu_ms() - cpu) / (began.elapsed().as_secs_f64() * 1e3) * 100.;
        let applied: Vec<u32> = floods[..count]
            .iter()
            .map(|id| run.frame_number(*id))
            .zip(&before)
            .map(|(after, before)| after - before)
            .collect();
        let painted = DRAWS.with_borrow_mut(std::mem::take);
        println!(
            "{count} flooding: each pane took {applied:?} frames in {:.1}s; window thread {busy:.0}% busy; panes laid out and painted {}",
            began.elapsed().as_secs_f64(),
            spread(painted)
        );
        let typed = run.typing(typist, 100);
        DRAWS.with_borrow_mut(Vec::clear);
        println!("typing beside {count} flooding: {}", spread(typed));
        run.shot(&shots, &format!("flood-{count}"));
    }

    run.show(vec![history]);
    for _ in 0..400 {
        run.wheel(4.);
        run.frame();
    }
    println!(
        "scrolling back 1,600 lines of 20,000, 4 lines a frame: panes laid out and painted {}",
        spread(DRAWS.with_borrow_mut(std::mem::take))
    );
    run.shot(&shots, "history");
    for _ in 0..500 {
        run.wheel(-8.);
    }
    for _ in 0..30 {
        run.frame();
    }
    let (row, _) = run.cursor(history);
    run.point(row.saturating_sub(1), 20, None);
    run.shot(&shots, "link");
    run.point(row.saturating_sub(1), 1, Some(2));
    run.shot(&shots, "word");
    run.point(row.saturating_sub(1), 1, Some(3));
    run.shot(&shots, "line");
    run.key("cmd-f");
    for key in ["9", "9", "9", "8"] {
        run.key(key);
    }
    for _ in 0..30 {
        run.frame();
    }
    run.shot(&shots, "find");
    run.key("escape");
    run.right_click(row.saturating_sub(1), 30);
    run.shot(&shots, "menu");
    let pick = serde_json::json!({ "key": "themeId", "value": "dracula" });
    if let Err(error) = runtime.block_on(client.call("setPreference", pick)) {
        eprintln!("the core would not change the theme: {error}");
    }
    for _ in 0..60 {
        run.frame();
    }
    run.shot(&shots, "dracula");
    println!("app peak RSS {:.0} MB", peak_rss_mb());
    drop(client);
}
