//! The panes' grain on a real window's ground, timed with GPUI's frame stats: how often the
//! ground layer is drawn, what each pass costs the GPU, and the frames around it.
//!
//!   cargo run --release --example ground_bench -- [stream|moving|still] [seconds]
//!
//! `stream` (the default) moves the grain under twenty panes that each take a line every 8 ms,
//! as agents streaming do; `moving` moves it under panes that hold still; `still` shows it
//! without moving. The window must stay in front, since a covered window does not draw. It
//! prints one line of means over the run, after the first two seconds.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{
    div, px, size, AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions,
};
use sikemux_gpui::ui::grain;
use sikemux_gpui::theme::{self, Tokens};
use sikemux_gpui::WINDOW_SIZE;
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
        let ground = grain::pane_ground(cx);
        let rows = self.panes.chunks(COLUMNS).map(|row| {
            div()
                .flex()
                .flex_1()
                .gap(px(6.))
                .children(row.iter().map(|lines| {
                    div()
                        .relative()
                        .flex_1()
                        .overflow_hidden()
                        .rounded(theme::radius_3())
                        .border_1()
                        .border_color(t.border)
                        .child(ground(0.))
                        .child(
                            div()
                                .p(px(8.))
                                .font_family(theme::MONO)
                                .text_size(px(11.))
                                .text_color(t.text_secondary)
                                .children(lines.iter().map(|line| div().child(line.clone()))),
                        )
                }))
        });
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
    words[from..]
        .windows(2)
        .find(|pair| pair[0] == key)
        .and_then(|pair| pair[1].parse().ok())
        .unwrap_or(0.)
}

/// Means of the frame stats' one-second lines after the warm-up.
fn summary(stats: &str) -> Option<String> {
    let seconds: Vec<Vec<&str>> = stats
        .lines()
        .filter(|line| line.starts_with("S "))
        .map(|line| line.split_whitespace().collect())
        .collect();
    let time = |words: &Vec<&str>| words[1].parse::<f64>().unwrap_or(0.);
    let start = seconds
        .iter()
        .position(|words| time(words) >= WARM_UP_SECONDS)?;
    let counted = &seconds[start + 1..];
    let span = time(seconds.last()?) - time(&seconds[start]);
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

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = match args.next().as_deref() {
        None | Some("stream") => Mode::Stream,
        Some("moving") => Mode::Moving,
        Some("still") => Mode::Still,
        Some(other) => panic!("no mode {other}: stream, moving or still"),
    };
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);
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
        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
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
