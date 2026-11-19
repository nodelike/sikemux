//! Times a cold launch into a chat that already has messages: how soon a key typed into it
//! stays, when its rows arrive, whether it drew as an empty chat before them, and whether
//! anything typed was lost when they came. The core must have just started with that chat in
//! its tab in front, as `fake_chat` leaves it once the core is restarted:
//!   SIKEMUX_CORE_SOCKET=/tmp/sx/core.sock cargo run --release --example cold_chat
//! It types a key every 100 ms from the window's first frame for `COLD_CHAT_SECONDS`, 6 by
//! default, the way a person starts writing as soon as the window is up. With
//! `COLD_CHAT_SHOTS=<prefix>` it saves the window at one second and at the end, as
//! `<prefix>-early.png` and `<prefix>-late.png`. A debug core trusts it as its window only as
//! `examples/remote` beside the core, as `fake_chat` says.

use anyhow::Context as _;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{point, px, size, AppContext, HeadlessAppContext, Keystroke};
use sikemux_gpui::workbench::Workbench;
use sikemux_gpui::{core, icons, theme, WINDOW_SIZE};

const FRAME: Duration = Duration::from_micros(8_333);
const KEY_EVERY: Duration = Duration::from_millis(100);
const SHOT_AT_MS: f64 = 1_000.;

/// The chat in front as one frame drew it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Seen {
    NoChat,
    NoComposer,
    /// Drawn as a new chat, the composer in the middle under the welcome.
    Empty,
    /// Its composer at the bottom, waiting for its rows.
    Waiting,
    Rows,
}

/// Marks the throwaway core's first run done, so no welcome or release notes cover the chat.
fn settle_first_run() -> anyhow::Result<()> {
    let socket = std::env::var("SIKEMUX_CORE_SOCKET")?;
    tokio::runtime::Runtime::new()?.block_on(async {
        let (client, _events) =
            sikemux_client::client::CoreClient::connect(socket.as_ref()).await?;
        let version = client.hello().build.version.clone();
        for (key, value) in [
            ("onboardingComplete", serde_json::json!(true)),
            ("lastSeenVersion", serde_json::json!(version)),
        ] {
            client
                .call(
                    "setPreference",
                    serde_json::json!({ "key": key, "value": value }),
                )
                .await?;
        }
        anyhow::Ok(())
    })
}

fn main() -> anyhow::Result<()> {
    core::search_path_only_for_own_core();
    let seconds: f64 = std::env::var("COLD_CHAT_SECONDS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or(6.);
    settle_first_run()?;
    let started = Instant::now();
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    let link = core::start(core::Home::of(sikemux_buildinfo::identity()));
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
                focus: true,
                show: false,
                ..Default::default()
            },
            |window, cx| {
                let bench = cx.new(|cx| Workbench::new(link, window, cx));
                kept.set(Some(bench.clone()));
                cx.new(|cx| gpui_kit::base::Root::new(bench, window, cx))
            },
        )
    })?;
    let bench = held.take().context("the workbench")?;
    cx.update_window(window.into(), |_, window, _| window.activate_window())
        .ok();
    let ms = |at: Instant| at.duration_since(started).as_secs_f64() * 1e3;
    let shots = std::env::var("COLD_CHAT_SHOTS").ok();
    let mut seen = Vec::<(f64, Seen)>::new();
    let (mut typed, mut stuck_at, mut last_key) = (0usize, None, None::<Instant>);
    let (mut longest_frame, mut early_shot) = (0f64, false);
    while started.elapsed().as_secs_f64() < seconds {
        let frame = Instant::now();
        cx.advance_clock(FRAME);
        cx.run_until_parked();
        longest_frame = longest_frame.max(frame.elapsed().as_secs_f64() * 1e3);
        let (now, written) = bench.read_with(&cx, |bench: &Workbench, cx| {
            let Some(chat) = bench
                .agent()
                .and_then(|agent| bench.chats.open.get(&agent.id))
            else {
                return (Seen::NoChat, None);
            };
            let Some(composing) = chat.composer.as_ref() else {
                return (Seen::NoComposer, None);
            };
            let written = composing.draft.read(cx).value().chars().count();
            let shown = match (
                chat.welcome_gone(cx),
                chat.transcript.read(cx).rows().is_empty(),
            ) {
                (Some(_), _) => Seen::Empty,
                (None, true) => Seen::Waiting,
                (None, false) => Seen::Rows,
            };
            (shown, Some(written))
        });
        if seen.last().map(|(_, last)| *last) != Some(now) {
            seen.push((ms(frame), now));
        }
        if stuck_at.is_none() && written.is_some_and(|written| written > 0) {
            stuck_at = Some(ms(frame));
        }
        if let Some(prefix) = shots
            .as_ref()
            .filter(|_| !early_shot && ms(frame) > SHOT_AT_MS)
        {
            early_shot = true;
            cx.capture_screenshot(window.into())?
                .save(format!("{prefix}-early.png"))?;
        }
        if last_key.is_none_or(|last| last.elapsed() >= KEY_EVERY) {
            last_key = Some(Instant::now());
            cx.update_window(window.into(), |_, window, cx| {
                if let Ok(mut key) = Keystroke::parse("a") {
                    key.key_char = Some("a".into());
                    window.dispatch_keystroke(key, cx);
                }
            })
            .ok();
            typed += usize::from(written.is_some());
        }
        std::thread::sleep(FRAME.saturating_sub(frame.elapsed()));
    }
    if let Some(prefix) = &shots {
        cx.capture_screenshot(window.into())?
            .save(format!("{prefix}-late.png"))?;
    }
    let kept = bench.read_with(&cx, |bench: &Workbench, cx| {
        let agent = bench.agent()?;
        let composing = bench.chats.open.get(&agent.id)?.composer.as_ref()?;
        Some(composing.draft.read(cx).value().chars().count())
    });
    for (at, shown) in &seen {
        println!("{at:>8.0} ms  {shown:?}");
    }
    let rows_at = seen.iter().find(|(_, shown)| *shown == Seen::Rows);
    println!(
        "first key that stayed: {} · rows: {} · drew as a new chat: {} · keys typed into a composer: {typed}, kept: {} · longest frame {longest_frame:.0} ms",
        stuck_at.map_or("never".into(), |at| format!("{at:.0} ms")),
        rows_at.map_or("never".into(), |(at, _)| format!("{at:.0} ms")),
        seen.iter().any(|(_, shown)| *shown == Seen::Empty),
        kept.map_or("no composer".into(), |kept| kept.to_string()),
    );
    Ok(())
}
