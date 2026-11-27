//! The panes' grain on a real window's ground, timed with GPUI's frame stats: how often the
//! ground layer is drawn, what each pass costs the GPU, and the frames around it.
//!
//!   cargo run --release --example ground_bench -- [stream|moving|still] [seconds] [kind]
//!   cargo run --release --example ground_bench -- offscreen [frames]
//!
//! `stream` (the default) moves the grain under twenty panes that each take a line every 8 ms,
//! as agents streaming do; `moving` moves it under panes that hold still; `still` shows it
//! without moving. `kind` is the backdrop drawn, grain (the default), gradient, stars or grid,
//! in the default theme's inks. The window must stay in front, since a covered window does not draw. It
//! prints one line of means over the run, after the first two seconds.
//!
//! `offscreen` opens no window: it draws a window-sized pane of each kind, and of no backdrop,
//! `frames` times on the headless renderer, waiting for each frame to be read back, and prints
//! each kind's mean and p95 milliseconds per frame over the empty pane's.

use anyhow::Context as _;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{
    div, px, size, AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions,
};
use sikemux_gpui::theme::{self, Tokens};
use sikemux_gpui::ui::grain;
use sikemux_gpui::WINDOW_SIZE;
use sikemux_view::backdrop::{BackdropKind, ThemeBackdrop};
use sikemux_view::prefs::Preferences;

const PANES: usize = 20;
const COLUMNS: usize = 5;
const LINES: usize = 16;
const LINE_EVERY: Duration = Duration::from_millis(8);
/// The grain's own tick on power. The app's waits for the window to be active, which a window
/// opened from a terminal is not always.
const GRAIN_EVERY: Duration = Duration::from_millis(1000 / 30);
const WARM_UP_SECONDS: f64 = 2.;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Stream,
    Moving,
    Still,
}

struct Bench {
    panes: Vec<VecDeque<String>>,
    written: u64,
}

impl Bench {
    fn write_line(&mut self) {
        self.written += 1;
        for (pane, lines) in self.panes.iter_mut().enumerate() {
            if lines.len() == LINES {
                lines.pop_front();
            }
            lines.push_back(format!(
                "pane {pane:>2}  line {:>6}  compiling sikemux-core v0.6.1 (crates/sikemux-core)",
                self.written
            ));
        }
    }
}

impl Render for Bench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        window.set_ground(Some(t.surface_sunken));
        let mut rows = Vec::new();
        for row in self.panes.chunks(COLUMNS) {
            let mut cells = Vec::new();
            for lines in row {
                let ground = grain::pane_ground(None, 0., cx);
                cells.push(
                    div()
                        .relative()
                        .flex_1()
                        .overflow_hidden()
                        .rounded(theme::radius_3())
                        .border_1()
                        .border_color(t.border)
                        .child(ground)
                        .child(
                            div()
                                .p(px(8.))
                                .font_family(theme::mono_font())
                                .text_size(px(11.))
                                .text_color(t.text_secondary)
                                .children(lines.iter().map(|line| div().child(line.clone()))),
                        ),
                );
            }
            rows.push(div().flex().flex_1().gap(px(6.)).children(cells));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(6.))
            .children(rows)
    }
}

/// The number after `key`, looking from where `section` first appears.
fn after(words: &[&str], section: &str, key: &str) -> f64 {
    let from = words.iter().position(|word| *word == section).unwrap_or(0);
    words
        .get(from..)
        .unwrap_or_default()
        .windows(2)
        .find(|pair| matches!(pair, [word, _] if *word == key))
        .and_then(|pair| pair.last()?.parse().ok())
        .unwrap_or(0.)
}

/// Means of the frame stats' one-second lines after the warm-up.
fn summary(stats: &str) -> Option<String> {
    let seconds: Vec<Vec<&str>> = stats
        .lines()
        .filter(|line| line.starts_with("S "))
        .map(|line| line.split_whitespace().collect())
        .collect();
    let time = |words: &Vec<&str>| {
        words
            .get(1)
            .and_then(|word| word.parse::<f64>().ok())
            .unwrap_or(0.)
    };
    let start = seconds
        .iter()
        .position(|words| time(words) >= WARM_UP_SECONDS)?;
    let (first, counted) = seconds.get(start..)?.split_first()?;
    let span = time(seconds.last()?) - time(first);
    if counted.is_empty() || span <= 0. {
        return None;
    }
    let total = |section, key| {
        counted
            .iter()
            .map(|words| after(words, section, key))
            .sum::<f64>()
    };
    let mean = |section, key| total(section, key) / counted.len() as f64;
    let with_ground: Vec<_> = counted
        .iter()
        .filter(|words| after(words, "S", "ground_draws") > 0.)
        .collect();
    let ground = |key| {
        with_ground
            .iter()
            .map(|words| after(words, "ground_gpu_ms", key))
            .sum::<f64>()
            / with_ground.len().max(1) as f64
    };
    let ground_max = with_ground
        .iter()
        .map(|words| after(words, "ground_gpu_ms", "max"))
        .fold(0., f64::max);
    Some(format!(
        "frames drawn {:.1}/s presented {:.1}/s, ground {:.1} passes/s at {:.3} ms GPU \
         (p95 {:.3}, max {:.3}), tick p50 {:.2} p95 {:.2} ms, per drawn frame present {:.2} \
         encode {:.2} ms",
        total("S", "drawn") / span,
        total("S", "presented") / span,
        total("S", "ground_draws") / span,
        ground("p50"),
        ground("p95"),
        ground_max,
        mean("tick_ms", "p50"),
        mean("tick_ms", "p95"),
        mean("per_drawn_ms", "present"),
        mean("per_drawn_ms", "encode"),
    ))
}

struct Field;

impl Render for Field {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().children(grain::field(cx))
    }
}

struct WelcomeView;

impl Render for WelcomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        window.set_ground(Some(t.surface_sunken));
        grain::welcome_stage_begin();
        div()
            .relative()
            .size_full()
            .child(grain::pane_ground(None, 1., cx))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(24.))
                    .child(grain::welcome_mark(116.))
                    .child(
                        div()
                            .relative()
                            .text_size(px(20.))
                            .text_color(t.text_primary)
                            .child(
                                gpui::canvas(
                                    |bounds, _, _| grain::welcome_note_quiet(bounds),
                                    |_, (), _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .child("What are we making in sikemux?"),
                    )
                    .child(
                        div()
                            .w(px(620.))
                            .h(px(110.))
                            .rounded(theme::radius_3())
                            .border_1()
                            .border_color(t.border)
                            .bg(t.surface_composer),
                    ),
            )
    }
}

/// Each kind's frames drawn offscreen, against frames with no backdrop.
fn offscreen(frames: usize) -> anyhow::Result<()> {
    use gpui::HeadlessAppContext;
    use sikemux_view::prefs::PaneBackdrop;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        std::sync::Arc::new(sikemux_gpui::icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
    });
    let palette = sikemux_corekit::themes::default_palette();
    let tokens = palette.tokens;
    let magenta = palette.theme.terminal.magenta;
    let kinds = [
        ("none", None),
        ("grain", Some(BackdropKind::Grain)),
        ("gradient", Some(BackdropKind::Gradient)),
        ("stars", Some(BackdropKind::Stars)),
        ("grid", Some(BackdropKind::Grid)),
    ];
    let mut empty = 0.;
    for (name, kind) in kinds {
        let preferences = Preferences {
            pane_backdrop: match kind {
                None => PaneBackdrop::None,
                Some(_) => PaneBackdrop::Theme,
            },
            ..Preferences::default()
        };
        let look = ThemeBackdrop {
            kind: kind.unwrap_or_default(),
            ..ThemeBackdrop::default()
        };
        let worn = sikemux_corekit::themes::backdrop::worn(&look, &tokens, &magenta);
        cx.update(|cx| {
            grain::set(&preferences, cx);
            grain::wear(&worn, cx);
        });
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: gpui::point(px(0.), px(0.)),
                        size: size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Field),
            )
        })?;
        cx.run_until_parked();
        let mut took = Vec::with_capacity(frames);
        for frame in 0..frames + 10 {
            let started = std::time::Instant::now();
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx)
            })?;
            cx.capture_screenshot(window.into())?;
            if frame >= 10 {
                took.push(started.elapsed().as_secs_f64() * 1000.);
            }
        }
        took.sort_by(f64::total_cmp);
        let mean = took.iter().sum::<f64>() / took.len() as f64;
        let p95 = *took
            .get(took.len() * 95 / 100)
            .context("no frames were timed")?;
        if kind.is_none() {
            empty = mean;
        }
        println!(
            "{name:>8}: {mean:.3} ms a frame (p95 {p95:.3}), {:+.3} ms over no backdrop",
            mean - empty
        );
        cx.update_window(window.into(), |_, window, _| window.remove_window())?;
    }
    let empty_4k = {
        let preferences = Preferences {
            pane_backdrop: PaneBackdrop::None,
            ..Preferences::default()
        };
        cx.update(|cx| grain::set(&preferences, cx));
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: gpui::point(px(0.), px(0.)),
                        size: size(px(2560.), px(1440.)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Field),
            )
        })?;
        cx.run_until_parked();
        let mut took = Vec::with_capacity(frames);
        for frame in 0..frames + 10 {
            let started = std::time::Instant::now();
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx)
            })?;
            cx.capture_screenshot(window.into())?;
            if frame >= 10 {
                took.push(started.elapsed().as_secs_f64() * 1000.);
            }
        }
        let mean = took.iter().sum::<f64>() / took.len() as f64;
        println!(
            "{:>10}: {mean:.3} ms a frame (baseline at 2560x1440)",
            "none-4k"
        );
        cx.update_window(window.into(), |_, window, _| window.remove_window())?;
        mean
    };
    let prefs = Preferences::default();
    let pal = sikemux_corekit::themes::palette(&sikemux_corekit::themes::Shelf::default(), &prefs);
    cx.update(|cx| {
        theme::apply(&pal, cx);
        grain::set(&prefs, cx);
        grain::glow_shown(false, cx);
        grain::glow_shown(true, cx);
    });
    for (label, dims, base_ms) in [
        ("welcome", WINDOW_SIZE, empty),
        ("welcome-4k", (2560.0_f32, 1440.0_f32), empty_4k),
    ] {
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: gpui::point(px(0.), px(0.)),
                        size: size(px(dims.0), px(dims.1)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| WelcomeView),
            )
        })?;
        cx.run_until_parked();
        let mut took = Vec::with_capacity(frames);
        let mut draw_took = Vec::with_capacity(frames);
        for frame in 0..frames + 10 {
            let angle = frame as f32 * 0.14;
            let ptr = [
                dims.0 * 0.5 + angle.cos() * 140.,
                dims.1 * 0.38 + angle.sin() * 90.,
            ];
            cx.update(|cx| sikemux_gpui::ui::welcome_dots::bench_pointer_move(ptr, cx));
            let started = std::time::Instant::now();
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx)
            })?;
            let draw_ms = started.elapsed().as_secs_f64() * 1000.;
            cx.capture_screenshot(window.into())?;
            if frame >= 10 {
                draw_took.push(draw_ms);
                took.push(started.elapsed().as_secs_f64() * 1000.);
            }
        }
        let (field_n, draw_n, awake, step_us, collect_us, emit_us) =
            cx.update(|cx| sikemux_gpui::ui::welcome_dots::bench_stats(cx));
        let packed_n = cx.update(|cx| sikemux_gpui::ui::welcome_dots::bench_packed_count(cx));
        took.sort_by(f64::total_cmp);
        draw_took.sort_by(f64::total_cmp);
        let mean = took.iter().sum::<f64>() / took.len() as f64;
        let p95 = *took
            .get(took.len() * 95 / 100)
            .context("no frames were timed")?;
        let draw_mean = draw_took.iter().sum::<f64>() / draw_took.len() as f64;
        println!(
            "{label:>10}: {mean:.3} ms a frame (p95 {p95:.3}, draw {draw_mean:.3} ms), {:+.3} ms over no backdrop ({field_n} field, {draw_n} dots -> {packed_n} quads, awake={awake}, step={step_us}us, collect={collect_us}us, emit={emit_us}us)",
            mean - base_ms
        );
        cx.update_window(window.into(), |_, window, _| window.remove_window())?;
        cx.update(sikemux_gpui::ui::welcome_dots::clear_all);
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    if std::env::args().nth(1).as_deref() == Some("offscreen") {
        let frames = std::env::args()
            .nth(2)
            .and_then(|frames| frames.parse().ok())
            .unwrap_or(300);
        return offscreen(frames).context("the frames draw offscreen");
    }
    let mode = match args.next().as_deref() {
        None | Some("stream") => Mode::Stream,
        Some("moving") => Mode::Moving,
        Some("still") => Mode::Still,
        Some(other) => anyhow::bail!("no mode {other}: stream, moving or still"),
    };
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);
    let kind = match args.next().as_deref() {
        None | Some("grain") => BackdropKind::Grain,
        Some("gradient") => BackdropKind::Gradient,
        Some("stars") => BackdropKind::Stars,
        Some("grid") => BackdropKind::Grid,
        Some(other) => anyhow::bail!("no kind {other}: grain, gradient, stars or grid"),
    };
    let look = ThemeBackdrop {
        kind,
        ..ThemeBackdrop::default()
    };
    let palette = sikemux_corekit::themes::default_palette();
    let worn = sikemux_corekit::themes::backdrop::worn(
        &look,
        &palette.tokens,
        &palette.theme.terminal.magenta,
    );
    let stats = std::env::var_os(gpui::frame_stats::ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("ground-bench-{}.txt", std::process::id()))
        });
    let _ = std::fs::remove_file(&stats);
    std::env::set_var(gpui::frame_stats::ENV_VAR, &stats);

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        grain::set(&Preferences::default(), cx);
        grain::wear(&worn, cx);
        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        #[expect(
            clippy::expect_used,
            reason = "the bench has nothing to measure without its window"
        )]
        let (handle, bench) = gpui_kit::open_window(options, cx, |_, cx| {
            cx.new(|_| {
                let mut bench = Bench {
                    panes: vec![VecDeque::with_capacity(LINES); PANES],
                    written: 0,
                };
                for _ in 0..LINES {
                    bench.write_line();
                }
                bench
            })
        })
        .expect("window opens");
        cx.activate(true);
        if mode != Mode::Still {
            cx.spawn(async move |cx| loop {
                cx.background_executor().timer(GRAIN_EVERY).await;
                let _ = handle.update(cx, |_, window, _| window.present_ground());
            })
            .detach();
        }
        if mode == Mode::Stream {
            cx.spawn(async move |cx| loop {
                cx.background_executor().timer(LINE_EVERY).await;
                bench.update(cx, |bench, cx| {
                    bench.write_line();
                    cx.notify();
                });
            })
            .detach();
        }
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(seconds))
                .await;
            report(mode, seconds, &stats);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    Ok(())
}

/// Prints the run's means. Quitting ends the process, so this runs before it.
fn report(mode: Mode, seconds: u64, stats: &Path) {
    let read = std::fs::read_to_string(stats).unwrap_or_default();
    let mode = match mode {
        Mode::Stream => "stream",
        Mode::Moving => "moving",
        Mode::Still => "still",
    };
    match summary(&read) {
        Some(line) => println!("{mode} {seconds}s: {line}"),
        None if mode == "still" => println!(
            "{mode} {seconds}s: frames drawn 0.0/s presented 0.0/s, ground 0.0 passes/s at 0.000 ms GPU, \
             nothing drawn after the warm-up"
        ),
        None => {
            eprintln!("{mode}: no frame stats past the warm-up in {stats:?}; was the window in front?");
            std::process::exit(1);
        }
    }
}
