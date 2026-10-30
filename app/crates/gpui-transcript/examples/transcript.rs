//! A transcript of 2,000 rows and an answer streaming into it, timed.
//!
//! ```sh
//! FONTS=path/to/fonts cargo run --release --example transcript -- --shots out/
//! ```
//!
//! It draws offscreen on a simulated clock at 120 Hz: it opens the chat with
//! every row held and scrolls it to the top, opens it again with the last 200
//! and scrolls up while earlier pages arrive, then streams a 4,000-token answer
//! at 500 tokens a second. After each it prints how long frames took to draw,
//! the CPU that costs and its memory. `FONTS` names folders of `.ttf` files to
//! load, colon-separated; `--shots` saves pictures of the window along the way.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::{
    div, point, px, relative, rgb, rgba, size, AnyElement, AnyWindowHandle, App, AppContext,
    Bounds, Context, Entity, HeadlessAppContext, IntoElement, KeyDownEvent, Keystroke, ListState,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    PlatformInput, Point, Render, ScrollDelta, ScrollEvent, SharedString, Styled, Window,
    WindowBounds, WindowOptions,
};
use gpui_transcript::{Change, Markdown, Pen, Row, Style, Transcript};
use serde_json::{json, Value};

const ROWS: usize = 2_000;
const PAGE: usize = 200;
const CHARS_PER_FLUSH: usize = 32;
const FLUSH: Duration = Duration::from_millis(16);
const SCROLL_STEP: f32 = 60.;
const FRAME: Duration = Duration::from_micros(8_333);

struct Message {
    id: SharedString,
    user: bool,
    md: Markdown,
    units: usize,
}

impl Row for Message {
    fn id(&self) -> &str {
        &self.id
    }

    fn docs(&self) -> Vec<&Markdown> {
        vec![&self.md]
    }
}

struct Spec {
    id: SharedString,
    user: bool,
    blocks: Value,
}

impl Spec {
    fn message(&self) -> Message {
        Message {
            id: self.id.clone(),
            user: self.user,
            md: Markdown::new(&self.blocks, 0),
            units: 0,
        }
    }
}

/// The same words every run, so runs compare.
struct Words(u64);

impl Words {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % below.max(1)
    }

    fn sentence(&mut self) -> String {
        const WORDS: [&str; 24] = [
            "the", "core", "folds", "each", "event", "into", "rows", "and", "sends", "only",
            "what", "changed", "since", "last", "frame", "so", "a", "long", "chat", "stays",
            "cheap", "to", "draw", "here",
        ];
        let count = 6 + self.next(14);
        let mut words: Vec<&str> = (0..count).map(|_| WORDS[self.next(WORDS.len())]).collect();
        let first = words[0].to_owned();
        let mut sentence = first[..1].to_uppercase() + &first[1..];
        words.remove(0);
        for word in words {
            sentence.push(' ');
            sentence.push_str(word);
        }
        sentence + ". "
    }

    fn inline(&mut self) -> Value {
        let mut nodes = vec![json!(self.sentence())];
        match self.next(4) {
            0 => nodes.push(json!({ "t": "code", "v": "src/chat/rows.rs" })),
            1 => nodes.push(json!({ "t": "strong", "c": [self.sentence().trim_end()] })),
            2 => nodes.push(json!({ "t": "a", "href": "https://sikemux.com", "c": ["the docs"] })),
            _ => nodes.push(json!({ "t": "em", "c": ["once per frame"] })),
        }
        nodes.push(json!(format!(" {}", self.sentence())));
        Value::Array(nodes)
    }

    fn block(&mut self) -> Value {
        match self.next(10) {
            0 => {
                json!({ "t": "h", "l": 2 + self.next(2), "c": [self.sentence().trim_end_matches(". ")] })
            }
            1 | 2 => json!({ "t": "ul", "c": (0..2 + self.next(4))
                .map(|_| json!({ "t": "li", "c": self.inline() })).collect::<Vec<_>>() }),
            3 => json!({ "t": "ol", "c": (0..2 + self.next(3))
                .map(|_| json!({ "t": "li", "c": self.inline() })).collect::<Vec<_>>() }),
            4 => {
                let lines: String = (0..4 + self.next(20))
                    .map(|ix| format!("    let row_{ix} = rows.put(message({ix}));\n"))
                    .collect();
                json!({ "t": "pre", "lang": "rust", "v": format!("fn main() {{\n{lines}}}\n") })
            }
            5 => json!({
                "t": "table",
                "align": [null, "right", null],
                "head": [["Phase"], ["Frames"], ["Note"]],
                "rows": (0..3 + self.next(4)).map(|ix| json!([
                    [format!("step {ix}")], [format!("{}", 100 + self.next(900))], [self.sentence().trim_end()]
                ])).collect::<Vec<_>>(),
            }),
            6 => json!({ "t": "quote", "c": [{ "t": "p", "c": self.inline() }] }),
            _ => json!({ "t": "p", "c": self.inline() }),
        }
    }

    fn specs(&mut self, count: usize) -> Vec<Spec> {
        (0..count)
            .map(|ix| {
                let user = ix % 2 == 0;
                let blocks = if user {
                    json!([{ "t": "p", "c": [self.sentence().trim_end()] }])
                } else {
                    Value::Array((0..1 + self.next(5)).map(|_| self.block()).collect())
                };
                Spec {
                    id: format!("m{ix}").into(),
                    user,
                    blocks,
                }
            })
            .collect()
    }

    /// About `tokens` tokens of answer, at four characters a token.
    fn answer(&mut self, tokens: usize) -> Vec<Value> {
        let mut blocks = Vec::new();
        let mut chars = 0;
        while chars < tokens * 4 {
            let block = self.block();
            chars += visible(&block);
            blocks.push(block);
        }
        blocks
    }
}

fn inside(node: &Value) -> Vec<&Value> {
    if let Some(list) = node.as_array() {
        return list.iter().collect();
    }
    if let Some(list) = node.get("c").and_then(Value::as_array) {
        return list.iter().collect();
    }
    node.get("rows")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().flat_map(|row| inside(row)).collect())
        .unwrap_or_default()
}

fn visible(node: &Value) -> usize {
    match node {
        Value::String(text) => text.chars().count(),
        _ => match node.get("v").and_then(Value::as_str) {
            Some(code) => code.chars().count(),
            None => inside(node).into_iter().map(visible).sum(),
        },
    }
}

/// The blocks as far as their first `chars` visible characters, as a reader sees them written.
fn reveal(nodes: &[Value], mut chars: usize) -> Vec<Value> {
    let mut kept = Vec::new();
    for node in nodes {
        if chars == 0 {
            break;
        }
        let length = visible(node);
        kept.push(if length <= chars {
            node.clone()
        } else {
            cut(node, chars)
        });
        chars = chars.saturating_sub(length);
    }
    kept
}

fn cut(node: &Value, chars: usize) -> Value {
    match node {
        Value::String(text) => json!(text.chars().take(chars).collect::<String>()),
        Value::Array(list) => Value::Array(reveal(list, chars)),
        _ => {
            let mut node = node.clone();
            if let Some(code) = node.get("v").and_then(Value::as_str) {
                node["v"] = json!(code.chars().take(chars).collect::<String>());
            } else if let Some(list) = node.get("c").and_then(Value::as_array) {
                node["c"] = Value::Array(reveal(list, chars));
            }
            node
        }
    }
}

/// The core's patch from one tree to the next: shared entries kept, the first
/// one that differs grown in place when it can be, the rest sent whole.
fn diff(path: &[usize], old: &[Value], new: &[Value], first: usize, ops: &mut Vec<Value>) {
    let same = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if same == old.len() && same == new.len() {
        return;
    }
    let at: Vec<usize> = path.iter().copied().chain([first + same]).collect();
    let grew = match (old.get(same), new.get(same)) {
        (Some(a), Some(b)) => grown(&at, a, b, ops),
        _ => false,
    };
    let from = same + usize::from(grew);
    if from < old.len().max(new.len()) {
        ops.push(
            json!({ "p": path, "at": first + from, "c": new.get(from..).unwrap_or_default() }),
        );
    }
}

fn grown(path: &[usize], old: &Value, new: &Value, ops: &mut Vec<Value>) -> bool {
    let text = |a: &str, b: &str, ops: &mut Vec<Value>| {
        let Some(added) = b.strip_prefix(a) else {
            return false;
        };
        ops.push(json!({ "p": path, "at": a.encode_utf16().count(), "s": added }));
        true
    };
    match (old, new) {
        (Value::String(a), Value::String(b)) => text(a, b, ops),
        (Value::Array(a), Value::Array(b)) => {
            diff(path, a, b, 0, ops);
            true
        }
        (Value::Object(a), Value::Object(b)) => {
            let Some(key) = ["c", "rows", "v"]
                .into_iter()
                .find(|key| a.contains_key(*key))
            else {
                return false;
            };
            if a.len() != b.len()
                || a.iter()
                    .any(|(name, value)| name != key && b.get(name) != Some(value))
            {
                return false;
            }
            match (&a[key], &b[key]) {
                (Value::String(a), Value::String(b)) => text(a, b, ops),
                (Value::Array(a), Value::Array(b)) => {
                    diff(path, a, b, 0, ops);
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn cpu_seconds() -> f64 {
    // SAFETY: getrusage only writes the struct it is handed.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1e6;
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}

/// The time this thread has run, which other processes busy on the machine do not inflate.
fn thread_cpu_ms() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime only writes the struct it is handed.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time.tv_sec as f64 * 1e3 + time.tv_nsec as f64 / 1e6
}

fn peak_resident_mb() -> f64 {
    // SAFETY: getrusage only writes the struct it is handed.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    usage.ru_maxrss as f64 / 1024. / 1024.
}

fn draw_row(
    rows: &[Message],
    ix: usize,
    pen: &Pen,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(message) = rows.get(ix) else {
        return div().into_any_element();
    };
    let body = pen.markdown(message.id.clone(), &message.md, window, cx);
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(px(800.))
                .px(px(22.))
                .map(|row| {
                    if message.user {
                        row.pt(px(17.)).pb(px(10.))
                    } else {
                        row.py(px(5.))
                    }
                })
                .when(message.user, |row| row.flex().justify_end())
                .child(if message.user {
                    div()
                        .max_w(relative(0.8))
                        .px(px(12.))
                        .py(px(8.))
                        .border_1()
                        .border_color(rgba(0x24242bcc))
                        .rounded(px(10.))
                        .bg(rgba(0x19191eb8))
                        .child(body)
                        .into_any_element()
                } else {
                    body
                }),
        )
        .into_any_element()
}

struct Shell(Entity<Transcript<Message>>);

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0x0f0f13)).child(self.0.clone())
    }
}

/// Draws frames on a simulated clock at 120 Hz and times each draw: render,
/// layout, prepaint and paint, everything but handing the frame to the GPU.
struct Bench {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    transcript: Entity<Transcript<Message>>,
    shots: Option<PathBuf>,
    frames: Vec<f64>,
    walls: Vec<f64>,
    rows_drawn: Rc<Cell<usize>>,
    cpu: f64,
    simulated: Duration,
}

impl Bench {
    fn frame(&mut self) {
        self.cx.advance_clock(FRAME);
        self.simulated += FRAME;
        self.cx.run_until_parked();
        let began = Instant::now();
        let cpu = thread_cpu_ms();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
        self.frames.push(thread_cpu_ms() - cpu);
        self.walls.push(began.elapsed().as_secs_f64() * 1e3);
    }

    fn input(&mut self, input: PlatformInput) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(input, cx)
            })
            .ok();
    }

    fn wheel(&mut self) {
        self.input(PlatformInput::Scroll(ScrollEvent {
            position: point(px(450.), px(500.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(SCROLL_STEP))),
            ..Default::default()
        }));
    }

    /// Drags from one point to another, shift-clicks a third, copying after each.
    fn select(&mut self, from: Point<Pixels>, to: Point<Pixels>, then: Point<Pixels>) {
        let copy = |bench: &mut Bench| {
            let keystroke = Keystroke::parse("cmd-c").expect("a keystroke");
            let key = KeyDownEvent {
                keystroke,
                is_held: false,
                prefer_character_input: false,
            };
            bench.input(PlatformInput::KeyDown(key));
            let copied = bench.cx.update(|cx| cx.read_from_clipboard());
            let copied = copied.and_then(|item| item.text()).unwrap_or_default();
            let start: String = copied.chars().take(90).collect();
            println!("Copied {} characters: {start:?}", copied.chars().count());
        };
        let left = MouseButton::Left;
        let down = |position, modifiers| MouseDownEvent {
            button: left,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        };
        let up = |position, modifiers| MouseUpEvent {
            button: left,
            position,
            modifiers,
            click_count: 1,
        };
        self.input(PlatformInput::MouseDown(down(from, Modifiers::default())));
        for step in 1..=10 {
            let position = from + (to - from) * (step as f32 / 10.);
            let pressed_button = Some(left);
            self.input(PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button,
                ..Default::default()
            }));
            self.frame();
        }
        self.input(PlatformInput::MouseUp(up(to, Modifiers::default())));
        self.frame();
        self.shot("selected");
        copy(self);
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        self.input(PlatformInput::MouseDown(down(then, shift)));
        self.input(PlatformInput::MouseUp(up(then, shift)));
        self.frame();
        self.shot("extended");
        copy(self);
    }

    fn push(&mut self, changes: Vec<Change<Message>>) {
        self.transcript.update(&mut self.cx, |transcript, cx| {
            transcript.extend(changes, cx)
        });
    }

    fn list(&mut self) -> ListState {
        self.transcript
            .read_with(&self.cx, |transcript, _| transcript.list().clone())
    }

    fn held(&mut self) -> usize {
        self.transcript
            .read_with(&self.cx, |transcript, _| transcript.rows().len())
    }

    fn shot(&mut self, name: &str) {
        let Some(dir) = self.shots.clone() else {
            return;
        };
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
        if let Ok(image) = self.cx.capture_screenshot(self.window) {
            image.save(dir.join(format!("{name}.png"))).ok();
        }
    }

    fn report(&mut self, phase: &str) {
        let cpu = cpu_seconds() - self.cpu;
        let mut frames = std::mem::take(&mut self.frames);
        frames.sort_by(f64::total_cmp);
        let mut walls = std::mem::take(&mut self.walls);
        walls.sort_by(f64::total_cmp);
        let wall = walls.get(walls.len() / 2).copied().unwrap_or(0.);
        let at = |q: f64| {
            frames
                .get(((frames.len() as f64 - 1.) * q).round() as usize)
                .copied()
                .unwrap_or(0.)
        };
        let mean = frames.iter().sum::<f64>() / frames.len().max(1) as f64;
        let over = frames.iter().filter(|frame| **frame > 8.33).count();
        let rows = self.rows_drawn.replace(0) as f64 / frames.len().max(1) as f64;
        println!(
            "{phase}: {} frames over {:.1}s · draw CPU mean {:.2} ms, p50 {:.2}, p95 {:.2}, p99 {:.2}, max {:.2}, {} over 8.3 ms, {:.0} rows laid out a frame, wall p50 {:.2} ms · CPU {:.0}% of one core at 120 Hz · peak RSS {:.0} MB",
            frames.len(),
            self.simulated.as_secs_f64(),
            mean,
            at(0.5),
            at(0.95),
            at(0.99),
            at(1.),
            over,
            rows,
            wall,
            cpu / self.simulated.as_secs_f64().max(1e-9) * 100.,
            peak_resident_mb()
        );
        self.cpu = cpu_seconds();
        self.simulated = Duration::ZERO;
    }

    fn at_top(&mut self) -> bool {
        let top = self.list().logical_scroll_top();
        top.item_ix == 0 && top.offset_in_item == px(0.)
    }

    /// The row at the top of the view and how far into it the view starts.
    fn place(&mut self) -> (Option<SharedString>, Pixels) {
        let top = self.list().logical_scroll_top();
        let id = self.transcript.read_with(&self.cx, |transcript, _| {
            transcript.rows().get(top.item_ix).map(|row| row.id.clone())
        });
        (id, top.offset_in_item)
    }

    fn scroll_to_top(&mut self, all: usize) {
        let mut shot = true;
        let (mut pages, mut kept) = (0, 0);
        while !(self.at_top() && self.held() == all) && self.frames.len() < 20_000 {
            self.wheel();
            let (held, place) = (self.held(), self.place());
            self.frame();
            if self.held() != held {
                pages += 1;
                kept += usize::from(self.place() == place);
            }
            if shot && self.list().logical_scroll_top().item_ix < all / 2 {
                shot = false;
                self.shot("scrolled");
            }
        }
        if pages > 0 {
            println!("The view stayed on the same row at the same offset for {kept} of {pages} pages added above it");
        }
    }
}

fn load_fonts(cx: &mut App) {
    let Ok(dirs) = std::env::var("FONTS") else {
        return;
    };
    let fonts = dirs
        .split(':')
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ttf"))
        .filter_map(|path| std::fs::read(path).ok())
        .map(std::borrow::Cow::Owned)
        .collect();
    cx.text_system().add_fonts(fonts).ok();
}

fn main() {
    let shots = std::env::args()
        .skip_while(|arg| arg != "--shots")
        .nth(1)
        .map(PathBuf::from);
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    cx.update(load_fonts);
    let specs = Rc::new(Words(7).specs(ROWS));
    let style = Rc::new(Style::default());
    let held = Rc::new(RefCell::new(ROWS - PAGE));
    let first_held = held.clone();
    let rows_drawn = Rc::new(Cell::new(0));
    let drawn = rows_drawn.clone();
    let transcript = cx.update(|cx| {
        gpui_transcript::init(cx);
        let pages = specs.clone();
        cx.new(|cx: &mut Context<Transcript<Message>>| {
            let this = cx.weak_entity();
            Transcript::new(
                Vec::new(),
                style,
                move |rows, ix, pen, window, cx| {
                    drawn.set(drawn.get() + 1);
                    draw_row(rows, ix, pen, window, cx)
                },
                cx,
            )
            .on_reach_top(move |_, cx| {
                let end = *held.borrow();
                let start = end.saturating_sub(PAGE);
                if start == end {
                    return;
                }
                *held.borrow_mut() = start;
                let page: Vec<Message> = pages[start..end].iter().map(Spec::message).collect();
                let this = this.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(30))
                        .await;
                    this.update(cx, |this, cx| this.push(Change::Prepend(page), cx))
                        .ok();
                })
                .detach();
            })
        })
    });
    let window = cx
        .update(|cx| {
            let shell = transcript.clone();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(900.), px(1000.)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Shell(shell)),
            )
        })
        .expect("window opens")
        .into();
    let mut bench = Bench {
        cx,
        window,
        transcript,
        shots,
        frames: Vec::new(),
        walls: Vec::new(),
        rows_drawn,
        cpu: cpu_seconds(),
        simulated: Duration::ZERO,
    };
    bench.frame();
    bench.frames.clear();
    bench.cpu = cpu_seconds();
    bench.simulated = Duration::ZERO;

    bench.push(vec![Change::Reset(
        specs.iter().map(Spec::message).collect(),
    )]);
    bench.frame();
    bench.report("Open 2,000 rows (first frame)");
    bench.shot("open");
    for _ in 0..120 {
        bench.frame();
    }
    bench.report("Idle at the end, 1 s");
    bench.select(
        point(px(120.), px(300.)),
        point(px(600.), px(560.)),
        point(px(400.), px(900.)),
    );
    bench.frames.clear();

    bench.scroll_to_top(ROWS);
    bench.report("Scroll 2,000 rows to the top");

    *first_held.borrow_mut() = ROWS - PAGE;
    bench.push(vec![Change::Reset(
        specs[ROWS - PAGE..].iter().map(Spec::message).collect(),
    )]);
    bench.frame();
    bench.scroll_to_top(ROWS);
    bench.report("Scroll up from the last 200 as pages of 200 arrive");
    bench.shot("paged");

    for tokens in std::env::args()
        .skip_while(|arg| arg != "--tokens")
        .nth(1)
        .unwrap_or_else(|| "4000".into())
        .split(',')
        .filter_map(|tokens| tokens.parse().ok())
    {
        stream(&mut bench, &specs, tokens);
    }
}

/// Streams an answer of `tokens` tokens at 500 a second, as the core sends it:
/// a flush every 16 ms re-reads the last two blocks and sends what changed.
fn stream(bench: &mut Bench, specs: &[Spec], tokens: usize) {
    let answer = Words(11).answer(tokens);
    let lengths: Vec<usize> = answer.iter().map(visible).collect();
    let total: usize = lengths.iter().sum();
    bench.push(vec![
        Change::Reset(specs[ROWS - 40..].iter().map(Spec::message).collect()),
        Change::Put(Message {
            id: "ask".into(),
            user: true,
            md: Markdown::new(&json!([{ "t": "p", "c": ["Write it all up."] }]), 0),
            units: 0,
        }),
        Change::Put(Message {
            id: "answer".into(),
            user: false,
            md: Markdown::empty(),
            units: 0,
        }),
    ]);
    bench.frame();
    bench.frames.clear();
    bench.walls.clear();
    bench.rows_drawn.set(0);
    bench.cpu = cpu_seconds();
    bench.simulated = Duration::ZERO;
    let mut held: Vec<Value> = Vec::new();
    let mut written = 0;
    let mut since_flush = Duration::ZERO;
    while written < total {
        since_flush += FRAME;
        if since_flush >= FLUSH {
            since_flush -= FLUSH;
            written = (written + CHARS_PER_FLUSH).min(total);
            let settled = held.len().saturating_sub(2);
            let before: usize = lengths[..settled].iter().sum();
            let tail = reveal(&answer[settled..], written - before);
            let mut ops = Vec::new();
            diff(&[], &held[settled..], &tail, settled, &mut ops);
            held.truncate(settled);
            held.extend(tail);
            let patch = json!({ "n": written, "o": ops });
            bench.push(vec![Change::Update(
                "answer".into(),
                Box::new(move |message: &mut Message| {
                    message.units = written;
                    message.md.write(&patch, written);
                }),
            )]);
            if written * 2 >= total && written * 2 < total + 2 * CHARS_PER_FLUSH {
                bench.shot(&format!("streaming-{tokens}"));
            }
        }
        bench.frame();
    }
    bench.report(&format!("Stream a {tokens}-token answer at 500 tokens/s"));
    bench.shot(&format!("streamed-{tokens}"));
}
