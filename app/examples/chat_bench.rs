//! Times the app's own chat pane the way `pnpm perf chat` times today's, so the
//! two compare: opening 2,000 rows, scrolling them to the top, scrolling up
//! from the last 200 while earlier pages arrive, and a 4,000-token answer
//! streaming in at 32 characters every 16 ms. The rows and every change to
//! them are the fold's own, made from the same words `showcase/perf/chat.ts`
//! uses, and reach the window the way the core's do. Only the window's layout
//! comes from a core, which must have a chat in its tab in front:
//!   SIKEMUX_CORE_SOCKET=/tmp/sx/core.sock cargo run --release --example chat_bench
//! Without a core it lays out one project with the chat in front itself.
//! It draws offscreen on a simulated clock at 120 Hz and prints, for each phase,
//! the CPU each frame's draw of the whole window took.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    point, px, size, AnyWindowHandle, AppContext, Entity, HeadlessAppContext, Modifiers,
    PlatformInput, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase,
};
use gpui_transcript::Row;
use serde_json::{json, Value};
use sikemux_client::client::{ClientEvent, CoreClient};
use sikemux_fold::{ChatEvent, Fold, PageOf, PAGE_BYTES};
use sikemux_gpui::core::{Ask, ChatAsk, ChatNews, Link, Update};
use sikemux_gpui::workbench::Workbench;
use sikemux_gpui::{icons, theme, WINDOW_SIZE};
use sikemux_wire::chat::{ChatChange, ChatHead, ChatMessage, ChatPage, ChatView};
use sikemux_wire::protocol::{ChatAttachment, ChatMark, DeviceView, Event, PermissionMode};
use tokio::sync::mpsc;

const ROWS: usize = 2_000;
const PAGE: u64 = 200;
const TOKENS: usize = 4_000;
const STREAM_TAIL: usize = 40;
const CHARS_PER_FLUSH: usize = 32;
const FRAME: Duration = Duration::from_micros(8_333);
/// A flush every 16 ms is one every other frame at 120 Hz.
const FRAMES_PER_FLUSH: usize = 2;
/// A page takes 30 ms to come back, as the WebKit run waits.
const FRAMES_PER_PAGE: usize = 4;
const SCROLL_STEP: f32 = 60.;
const FEED: &str = "perf-chat";

const WORDS: [&str; 24] = [
    "the", "core", "folds", "each", "event", "into", "rows", "and", "sends", "only", "what",
    "changed", "since", "last", "frame", "so", "a", "long", "chat", "stays", "cheap", "to", "draw",
    "here",
];

/// `showcase/perf/chat.ts`'s generator, so both runs read the same words.
struct Words(u64);

impl Words {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as usize % below.max(1)
    }

    fn sentence(&mut self) -> String {
        let count = 6 + self.next(14);
        let words: Vec<&str> = (0..count).map(|_| WORDS[self.next(WORDS.len())]).collect();
        let sentence = words.join(" ");
        format!("{}{}. ", sentence[..1].to_uppercase(), &sentence[1..])
    }

    /// Markdown, and how many characters of it a reader sees.
    fn inline(&mut self) -> (String, usize) {
        let first = self.sentence();
        let (md, seen) = match self.next(4) {
            0 => ("`src/chat/rows.rs`".to_owned(), "src/chat/rows.rs".len()),
            1 => {
                let strong = self.sentence().trim_end().to_owned();
                (format!("**{strong}**"), strong.len())
            }
            2 => (
                "[the docs](https://sikemux.com)".to_owned(),
                "the docs".len(),
            ),
            _ => ("*once per frame*".to_owned(), "once per frame".len()),
        };
        let last = format!(" {}", self.sentence());
        (
            format!("{first}{md}{last}").trim_end().to_owned(),
            first.len() + seen + last.len(),
        )
    }

    fn items(&mut self, count: usize, marker: impl Fn(usize) -> String) -> (String, usize) {
        let items: Vec<(String, usize)> = (0..count).map(|_| self.inline()).collect();
        let md = items
            .iter()
            .enumerate()
            .map(|(ix, (md, _))| format!("{} {md}", marker(ix)))
            .collect::<Vec<_>>()
            .join("\n");
        (md, items.iter().map(|(_, seen)| seen).sum())
    }

    fn block(&mut self) -> (String, usize) {
        match self.next(10) {
            0 => {
                let level = 2 + self.next(2);
                let sentence = self.sentence();
                let text = &sentence[..sentence.len() - 2];
                (format!("{} {text}", "#".repeat(level)), text.len())
            }
            1 | 2 => {
                let count = 2 + self.next(4);
                self.items(count, |_| "-".to_owned())
            }
            3 => {
                let count = 2 + self.next(3);
                self.items(count, |ix| format!("{}.", ix + 1))
            }
            4 => {
                let lines: String = (0..4 + self.next(20))
                    .map(|ix| format!("    let row_{ix} = rows.put(message({ix}));\n"))
                    .collect();
                let code = format!("fn main() {{\n{lines}}}\n");
                (format!("```rust\n{code}```"), code.len())
            }
            5 => {
                let count = 3 + self.next(4);
                let rows: Vec<[String; 3]> = (0..count)
                    .map(|ix| {
                        [
                            format!("step {ix}"),
                            (100 + self.next(900)).to_string(),
                            self.sentence().trim_end().to_owned(),
                        ]
                    })
                    .collect();
                let line = |cells: &[String]| format!("| {} |", cells.join(" | "));
                let head = [
                    line(&["Phase".into(), "Frames".into(), "Note".into()]),
                    line(&["---".into(), "---:".into(), "---".into()]),
                ];
                let md = head
                    .into_iter()
                    .chain(rows.iter().map(|row| line(row)))
                    .collect::<Vec<_>>()
                    .join("\n");
                (md, rows.iter().flatten().map(String::len).sum())
            }
            6 => {
                let (md, seen) = self.inline();
                (format!("> {md}"), seen)
            }
            _ => self.inline(),
        }
    }

    /// A short prompt and an answer of one to five blocks, by turns.
    fn rows(&mut self, count: usize) -> Vec<String> {
        (0..count)
            .map(|ix| {
                if ix % 2 == 0 {
                    return self.sentence().trim_end().to_owned();
                }
                let blocks = 1 + self.next(5);
                (0..blocks)
                    .map(|_| self.block().0)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .collect()
    }

    /// About `tokens` tokens of answer, at four characters a reader sees to a token.
    fn answer(&mut self, tokens: usize) -> String {
        let (mut blocks, mut seen) = (Vec::new(), 0);
        while seen < tokens * 4 {
            let (md, more) = self.block();
            blocks.push(md);
            seen += more;
        }
        blocks.join("\n\n")
    }
}

fn event(kind: &str, payload: Value) -> ChatEvent {
    serde_json::from_value(json!({ "kind": kind, "payload": payload })).expect("a chat event")
}

fn said(kind: &str, message: &str, text: &str) -> ChatEvent {
    let update = json!({ "sessionUpdate": kind, "messageId": message, "content": { "type": "text", "text": text } });
    event(
        "session_update",
        json!({ "sessionId": format!("session-{FEED}"), "update": update }),
    )
}

/// The chat folded as the core folds it: every event with its index, its head,
/// all its rows, and the answer's changes, one list for each flush.
struct Folded {
    fold: Fold,
    events: Vec<(u64, ChatEvent)>,
    head: ChatHead,
    rows: Vec<ChatMessage>,
    flushes: Vec<Vec<ChatChange>>,
    chars: usize,
}

impl Folded {
    fn new() -> Folded {
        let mut fold = Fold::new(PermissionMode::WorkspaceWrite);
        let ready = event(
            "ready",
            json!({ "sessionId": format!("session-{FEED}"), "capabilities": {}, "setup": { "configOptions": [] } }),
        );
        let mut events = vec![(0, ready)];
        for (ix, text) in Words(7).rows(ROWS).iter().enumerate() {
            let kind = if ix % 2 == 0 {
                "user_message_chunk"
            } else {
                "agent_message_chunk"
            };
            events.push((ix as u64 + 1, said(kind, &format!("{FEED}-{ix}"), text)));
        }
        for (index, event) in &events {
            fold.apply(*index, event.kind, &event.payload, 0);
        }
        fold.flush();
        let head = fold.view(false).head;
        let rows = fold
            .page_of(&events, PageOf::Before(u64::MAX), usize::MAX, None, None)
            .rows;
        assert_eq!(rows.len(), ROWS, "the fold made {} rows", rows.len());
        let answer = Words(11).answer(TOKENS);
        let mut folded = Folded {
            fold,
            events,
            head,
            rows,
            flushes: Vec::new(),
            chars: answer.len(),
        };
        folded.flush(vec![
            event("turn_started", json!({ "at": 0 })),
            said(
                "user_message_chunk",
                &format!("{FEED}-ask"),
                "Write it all up.",
            ),
        ]);
        let chunks: Vec<String> = answer
            .as_bytes()
            .chunks(CHARS_PER_FLUSH)
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect();
        for chunk in chunks {
            folded.flush(vec![said(
                "agent_message_chunk",
                &format!("{FEED}-answer"),
                &chunk,
            )]);
        }
        folded.flush(vec![event(
            "turn_completed",
            json!({ "stopReason": "end_turn", "at": 0 }),
        )]);
        folded
    }

    fn flush(&mut self, events: Vec<ChatEvent>) {
        for event in events {
            let index = self.events.len() as u64;
            self.fold.apply(index, event.kind, &event.payload, 0);
            self.events.push((index, event));
        }
        self.flushes.push(self.fold.flush());
    }

    /// The view a client opens on: the rows from `start`, with earlier ones paged in.
    fn view(&self, start: usize) -> Value {
        let older_before = (start > 0).then_some(start as u64 + 1);
        let view = ChatView {
            head: self.head.clone(),
            rows: self.rows[start..].to_vec(),
            subagents: BTreeMap::new(),
            older_before,
        };
        serde_json::to_value(view).expect("a view")
    }

    /// The page before event `before`, as the core answers `AcpHistory`.
    fn page(&self, before: u64) -> ChatPage {
        let start = before.saturating_sub(PAGE).max(1);
        let events: Vec<(u64, ChatEvent)> = self
            .events
            .iter()
            .filter(|(index, _)| (start..before).contains(index))
            .cloned()
            .collect();
        let older_before = (start > 1).then_some(start);
        (self.fold).page_of(
            &events,
            PageOf::Before(before),
            PAGE_BYTES,
            older_before,
            None,
        )
    }
}

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

#[derive(Default)]
struct Place {
    row: Option<String>,
    first: bool,
    offset: gpui::Pixels,
    held: usize,
    following: bool,
}

struct Bench {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    bench: Entity<Workbench>,
    updates: mpsc::UnboundedSender<Update>,
    asks: mpsc::UnboundedReceiver<Ask>,
    folded: Folded,
    agent: Option<String>,
    /// The view an attach is answered with, and the pages asked for, by the frame they come back.
    shown: Value,
    pages: Vec<(usize, u64)>,
    served: usize,
    frames: Vec<f64>,
    /// The CPU spent taking in what the core sent, before each frame's draw, in ms.
    taking_in: f64,
    frame_ix: usize,
}

impl Bench {
    fn frame(&mut self) {
        self.frame_ix += 1;
        self.answer();
        self.cx.advance_clock(FRAME);
        let cpu = thread_cpu_ms();
        self.cx.run_until_parked();
        self.taking_in += thread_cpu_ms() - cpu;
        let cpu = thread_cpu_ms();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
        self.frames.push(thread_cpu_ms() - cpu);
    }

    /// Answers what the window asked of the core.
    fn answer(&mut self) {
        while let Ok(ask) = self.asks.try_recv() {
            match ask {
                Ask::Chat(ChatAsk::Attach { agent, .. }) => {
                    self.agent = Some(agent.clone());
                    self.attach();
                }
                Ask::Chat(ChatAsk::History { before, .. }) => {
                    self.pages.push((self.frame_ix + FRAMES_PER_PAGE, before))
                }
                _ => {}
            }
        }
        let due: Vec<u64> = self
            .pages
            .iter()
            .filter(|(at, _)| *at <= self.frame_ix)
            .map(|(_, before)| *before)
            .collect();
        self.pages.retain(|(at, _)| *at > self.frame_ix);
        for before in due {
            self.served += 1;
            let page = self.folded.page(before);
            self.send(ChatNews::Page(Some(page)));
        }
    }

    fn send(&mut self, news: ChatNews) {
        if let Some(agent) = self.agent.clone() {
            let _ = self.updates.send(Update::Chat(agent, news));
        }
    }

    fn attach(&mut self) {
        let view = self.shown.clone();
        self.send(ChatNews::Attached(ChatAttachment::Live {
            view,
            mark: ChatMark {
                feed: FEED.into(),
                seq: 0,
            },
        }));
    }

    /// Opens the chat on the rows from `start`, as the core does when it tells the window to attach again.
    fn open(&mut self, start: usize) {
        self.shown = self.folded.view(start);
        self.attach();
    }

    /// The row at the top of the view, how far into it the view starts, how many rows are held, and whether the end is in view.
    fn place(&mut self) -> Place {
        let agent = self.agent.clone().unwrap_or_default();
        self.bench.read_with(&self.cx, |bench, cx| {
            let Some(chat) = bench.chats.open.get(&agent) else {
                return Place::default();
            };
            let transcript = chat.transcript.read(cx);
            let list = transcript.list();
            let top = list.logical_scroll_top();
            Place {
                row: transcript
                    .rows()
                    .get(top.item_ix)
                    .map(|row| row.id().to_owned()),
                first: top.item_ix == 0,
                offset: top.offset_in_item,
                held: transcript.rows().len(),
                following: list.is_following_tail(),
            }
        })
    }

    fn wheel(&mut self) {
        let wheel = ScrollEvent {
            position: point(px(900.), px(450.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(SCROLL_STEP))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        };
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(PlatformInput::Scroll(wheel), cx)
            })
            .ok();
    }

    /// Scrolls up a step a frame until the first row is at the top and every row is held,
    /// counting the pages that arrived and those that left the reader's place where it was.
    fn scroll_to_top(&mut self, all: usize) -> (usize, usize) {
        let (mut pages, mut kept) = (0, 0);
        while self.frames.len() < 40_000 {
            let before = self.place();
            if before.first && before.offset == px(0.) && before.held >= all {
                break;
            }
            self.wheel();
            let scrolled = self.place();
            self.frame();
            let after = self.place();
            if after.held > scrolled.held {
                pages += 1;
                kept += usize::from(after.row == scrolled.row && after.offset == scrolled.offset);
            }
        }
        (pages, kept)
    }

    fn report(&mut self, phase: &str, extra: String) {
        let mut frames = std::mem::take(&mut self.frames);
        frames.sort_by(f64::total_cmp);
        let at = |q: f64| {
            frames
                .get(((frames.len() as f64 - 1.) * q).round() as usize)
                .copied()
                .unwrap_or(0.)
        };
        let total: f64 = frames.iter().sum();
        let over = frames.iter().filter(|frame| **frame > 8.33).count();
        let simulated = frames.len() as f64 * FRAME.as_secs_f64();
        println!(
            "{phase}: {} frames · draw CPU p50/p90/p99/max {:.2}/{:.2}/{:.2}/{:.2} ms, {over} over 8.3 ms · {:.0}% of one core at 120 Hz · peak RSS {:.0} MB{extra}",
            frames.len(),
            at(0.5),
            at(0.9),
            at(0.99),
            at(1.),
            total / 1e3 / simulated.max(1e-9) * 100.,
            peak_resident_mb()
        );
    }
}

/// The layout of a core with one project and one chat in front, for a run with no core.
fn standalone() -> anyhow::Result<DeviceView> {
    Ok(serde_json::from_value(json!({
        "workspace": {
            "projects": [{ "id": "bench", "name": "bench", "path": "/tmp/chat-bench" }],
            "launchers": [],
            "palette": {},
            "backdrop": { "texture": false, "image": null },
            "agents": [{
                "id": "bench-agent", "provider": "claude", "project": "bench",
                "cwd": "/tmp/chat-bench", "permissionMode": "bypass", "title": "Bench"
            }],
        },
        "sessions": [],
        "chats": [{
            "agentId": "bench-agent", "provider": "claude", "title": "Bench",
            "cwd": "/tmp/chat-bench", "sessionId": null, "state": "ready", "running": false,
            "pendingPermissions": [], "startedBy": null, "launcher": null,
            "permissionMode": "bypass", "model": null, "effort": null, "unread": false,
            "activeAt": null
        }],
        "attentions": [],
        "layout": {
            "activeProject": "bench",
            "projects": { "bench": { "active": "tab", "tabs": [{
                "id": "tab",
                "pane": { "type": "view", "id": "view", "kind": "agent", "target": "bench-agent" }
            }] } }
        }
    }))?)
}

fn layout() -> anyhow::Result<DeviceView> {
    let Ok(socket) = std::env::var("SIKEMUX_CORE_SOCKET") else {
        return standalone();
    };
    let socket = PathBuf::from(socket);
    tokio::runtime::Runtime::new()?.block_on(async {
        let (client, mut events) = CoreClient::connect(&socket).await?;
        client.watch_view().await?;
        while let Some(event) = events.recv().await {
            if let ClientEvent::Event(Event::DeviceView { view }) = event {
                return Ok(*view);
            }
        }
        Err(anyhow::anyhow!("the core sent no view"))
    })
}

fn main() -> anyhow::Result<()> {
    let view = layout()?;
    let made = Instant::now();
    let folded = Folded::new();
    println!(
        "Folded {} rows and an answer of {} characters in {} flushes, in {:.1}s",
        folded.rows.len(),
        folded.chars,
        folded.flushes.len(),
        made.elapsed().as_secs_f64()
    );
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    let (updates, updates_rx) = mpsc::unbounded_channel();
    let (asks_tx, asks) = mpsc::unbounded_channel();
    let held = Rc::new(Cell::new(None));
    let kept = held.clone();
    let window = cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
                })),
                focus: false,
                show: false,
                ..Default::default()
            },
            |window, cx| {
                let bench = cx.new(|cx| {
                    Workbench::new(
                        Link {
                            updates: updates_rx,
                            asks: asks_tx,
                        },
                        window,
                        cx,
                    )
                });
                kept.set(Some(bench.clone()));
                cx.new(|cx| gpui_kit::base::Root::new(bench, window, cx))
            },
        )
    })?;
    let bench = held.take().expect("the workbench");
    let mut run = Bench {
        cx,
        window: window.into(),
        bench,
        updates,
        asks,
        folded,
        agent: None,
        shown: Value::Null,
        pages: Vec::new(),
        served: 0,
        frames: Vec::new(),
        taking_in: 0.,
        frame_ix: 0,
    };
    run.shown = run.folded.view(ROWS);
    let _ = run.updates.send(Update::View(Box::new(view)));
    for _ in 0..30 {
        run.frame();
    }
    if run.agent.is_none() {
        anyhow::bail!("the core's tab in front holds no chat");
    }
    run.frames.clear();

    run.open(0);
    run.frame();
    let first = run.frames[0];
    for _ in 0..59 {
        run.frame();
    }
    run.report(
        &format!("open {ROWS} rows"),
        format!(" · first frame {first:.2} ms"),
    );

    run.scroll_to_top(ROWS);
    run.report("scroll to the top", String::new());

    run.open(ROWS - PAGE as usize);
    for _ in 0..30 {
        run.frame();
    }
    run.frames.clear();
    let (pages, kept) = run.scroll_to_top(ROWS);
    run.report(
        &format!("scroll up as {} pages arrive", run.served),
        format!(" · the view stayed put for {kept} of {pages}"),
    );

    run.open(ROWS - STREAM_TAIL);
    for _ in 0..30 {
        run.frame();
    }
    run.frames.clear();
    run.taking_in = 0.;
    let flushes = std::mem::take(&mut run.folded.flushes);
    let count = flushes.len();
    let mut lost = None;
    let mut decoding = Duration::ZERO;
    for (seq, changes) in flushes.into_iter().enumerate() {
        if lost.is_none() && !run.place().following {
            lost = Some(seq);
        }
        let changes = serde_json::to_value(changes)?;
        let read = Instant::now();
        let changes = sikemux_gpui::chat_changes::read(changes);
        decoding += read.elapsed();
        run.send(ChatNews::Changes {
            seq: seq as u64 + 1,
            changes,
        });
        for _ in 0..FRAMES_PER_FLUSH {
            run.frame();
        }
    }
    let end = match lost {
        None => "following the end throughout".to_owned(),
        Some(seq) => format!("stopped following the end at flush {seq}"),
    };
    if let Ok(path) = std::env::var("CHAT_BENCH_SHOT") {
        run.cx
            .update_window(run.window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
        run.cx.capture_screenshot(run.window)?.save(path)?;
    }
    run.report(
        &format!("stream {} characters in {count} flushes", run.folded.chars),
        format!(
            " · {end} after · taken in with {:.1} ms on the window's thread, read in {:.1} ms on the core's",
            run.taking_in,
            decoding.as_secs_f64() * 1e3
        ),
    );
    Ok(())
}
