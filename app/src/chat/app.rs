//! An MCP app an agent's tool call opened, drawn in its reply in a card that names its server.
//! A web view costs the app a process and its memory, so apps borrow one of a few views kept for
//! the window, never more however long the chat, and only while in sight. An app out of sight
//! for a moment is kept as a still picture of itself and hands its view back; once it is in sight
//! again and the chat has rested, it borrows one again and is set up afresh from its call. A view
//! no app has wanted for a while is let go, with its process.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    canvas, div, img, px, AnyElement, App, AsyncWindowContext, FocusHandle, FontWeight, Global,
    Image, ImageFormat, InteractiveElement, IntoElement, KeyDownEvent, ObjectFit, ParentElement,
    SharedString, Styled, StyledImage, Window,
};
use gpui_native_view::native_view;
use serde_json::{json, Value};
use sikemux_core::server::call;
use sikemux_core::server::mcp_apps::{McpApp, McpAppParams, McpAppRequestParams};
use sikemux_view::chat::McpAppRef;

use super::Looks;
use crate::icons::Icon;
use crate::page_view::{self, PageEvent, Room};
use crate::theme::{self, Tokens};
use crate::ui;

/// The most web views kept for apps, which is the most apps running at once.
const MAX_VIEWS: usize = 3;
const DEFAULT_HEIGHT: f32 = 240.;
const MAX_HEIGHT: f32 = 2000.;
const HEADER_HEIGHT: f32 = 30.;
/// How long the chat must rest before an app in sight borrows a view, so scrolling past apps,
/// or flicking through a chat while reading it, never waits on WebKit.
const SETTLE: Duration = Duration::from_millis(300);
/// How long an app's card must have gone undrawn before another app in sight may take its
/// view, which an app asks for only once the frame that saw it is drawn; taking a view whose
/// card is still in sight would pass views round and round when more apps are in sight than
/// there are views.
const UNSEEN: Duration = Duration::from_millis(150);
/// How long an app may be out of sight before it hands its view back.
const GRACE: Duration = Duration::from_millis(1500);
/// How long a view no app borrows is kept before it is let go; making one again holds the main
/// thread for a sixth of a second.
const IDLE: Duration = Duration::from_secs(120);
/// How long an app must be still before a fresh picture of it is taken.
const QUIET: Duration = Duration::from_secs(1);
/// How often a running app is looked at, to take a fresh still or hand its view back.
const TICK: Duration = Duration::from_millis(500);
/// How long the person must have rested before a view is made for apps.
const REST: Duration = Duration::from_millis(1200);
const HOST: &str = include_str!("app_host.html");
const BLANK: &str = "<!doctype html><html><body></body></html>";

#[derive(Default)]
struct Apps {
    shown: HashMap<String, Shown>,
    views: Vec<Kept>,
    next_view: u64,
    /// Views are being made, one each time the person rests.
    warming: bool,
    /// When a card last moved, as the chat scrolled or its agent wrote.
    moved: Option<Instant>,
    /// Whether a turn is running in each chat that drew an app.
    turns: Vec<Weak<Cell<bool>>>,
    /// The chats with an app in sight waiting for a view, told when one is made.
    waiting: Vec<Wake>,
    /// The app grown over its whole chat, as MCP Apps' fullscreen display.
    expanded: Option<Expanded>,
}

struct Expanded {
    id: String,
    label: SharedString,
    wake: Wake,
    focus: FocusHandle,
}

impl Global for Apps {}

struct Shown {
    label: SharedString,
    height: Rc<Cell<f32>>,
    /// Where the card was last drawn in sight, and when it last moved there.
    sight: Rc<Sight>,
    still: Option<Arc<Image>>,
    /// The view the app has borrowed.
    view: Option<u64>,
    wake: Option<Wake>,
    /// Counts the times the app was set up, so a loop kept for an earlier one stops.
    round: u64,
    starting: bool,
    failed: Option<SharedString>,
}

#[derive(Default)]
struct Sight {
    top: Cell<f32>,
    moved: Cell<Option<Instant>>,
    /// When the card was last drawn in sight.
    seen: Cell<Option<Instant>>,
    /// The chat rested long enough after this app last moved in sight to give it a view.
    settled: Cell<bool>,
    waiting: Cell<bool>,
}

/// A web view kept for apps, and the app borrowing it.
struct Kept {
    id: u64,
    view: page_view::View,
    app: Option<String>,
    /// When the app last said anything, which a fresh still waits to be past.
    stirred: Rc<Cell<Option<Instant>>>,
    pointer_over: Rc<Cell<bool>>,
    freed: Option<Instant>,
}

fn apps(cx: &mut App) -> &mut Apps {
    if !cx.has_global::<Apps>() {
        cx.set_global(Apps::default());
    }
    cx.global_mut::<Apps>()
}

impl Apps {
    fn kept(&self, app: &str) -> Option<&Kept> {
        let view = self.shown.get(app)?.view?;
        self.views
            .iter()
            .find(|kept| kept.id == view && kept.app.as_deref() == Some(app))
    }

    fn view_of(&self, app: &str) -> Option<page_view::View> {
        self.kept(app).map(|kept| kept.view.clone())
    }

    /// Hands the app's view back, blank, for another app to borrow.
    fn hand_back(&mut self, app: &str) {
        let Some(view) = self.shown.get_mut(app).and_then(|shown| shown.view.take()) else {
            return;
        };
        if let Some(kept) = self.views.iter_mut().find(|kept| kept.id == view) {
            kept.app = None;
            kept.freed = Some(Instant::now());
            kept.view.fill(false);
            kept.view.show(BLANK);
        }
    }

    /// A view for one more app: one no app borrows, or the view of the app whose card has gone
    /// undrawn longest; with every view showing an app in sight, or none made yet, there is none.
    fn spare(&mut self) -> Option<u64> {
        if let Some(kept) = self.views.iter().find(|kept| kept.app.is_none()) {
            return Some(kept.id);
        }
        let out_of_sight = self
            .views
            .iter()
            .filter_map(|kept| {
                let app = kept.app.clone()?;
                let seen = self.shown.get(&app)?.sight.seen.get();
                seen.is_none_or(|seen| seen.elapsed() >= UNSEEN)
                    .then_some((app, seen))
            })
            .min_by_key(|(_, seen)| *seen)
            .map(|(app, _)| app)?;
        self.hand_back(&out_of_sight);
        self.spare()
    }
}

/// What the card's header names: the server and its tool, from Claude Code's `mcp__server__tool`.
fn label(title: &str) -> SharedString {
    match title
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
    {
        Some((server, tool)) => format!("{server} · {tool}").into(),
        None => title.to_owned().into(),
    }
}

pub(super) fn app(
    app: &McpAppRef,
    title: &str,
    looks: &Looks,
    running: &Rc<Cell<bool>>,
    cx: &mut App,
) -> AnyElement {
    let turn = Rc::downgrade(running);
    let turns = &mut apps(cx).turns;
    if !turns.iter().any(|known| known.ptr_eq(&turn)) {
        turns.push(turn);
    }
    let shown = apps(cx)
        .shown
        .entry(app.id.clone())
        .or_insert_with(|| Shown {
            label: label(title),
            height: Rc::new(Cell::new(DEFAULT_HEIGHT)),
            sight: Rc::default(),
            still: None,
            view: None,
            wake: None,
            round: 0,
            starting: false,
            failed: None,
        });
    let (height, sight, still, failed) = (
        shown.height.get(),
        shown.sight.clone(),
        shown.still.clone(),
        shown.failed.clone(),
    );
    let surface = apps(cx)
        .view_of(&app.id)
        .map(|view| view.surface)
        .filter(|_| !is_expanded(&app.id, cx));
    let t = Tokens::get(cx);
    let ground = theme::page_ground(t);
    let body: AnyElement = match (surface, still, failed) {
        (Some(surface), _, _) => {
            native_view(SharedString::from(format!("app-{}", app.id)), &surface)
                .size_full()
                .into_any_element()
        }
        (None, _, Some(failed)) => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(ui::meta(
                t!("Could not show this app: {failed}", failed = failed),
                cx,
            ))
            .into_any_element(),
        (None, Some(still), None) => img(still)
            .size_full()
            .object_fit(ObjectFit::Fill)
            .into_any_element(),
        (None, None, None) => div().size_full().into_any_element(),
    };
    let watch = {
        let (id, looks) = (app.id.clone(), Wake::of(looks));
        canvas(
            move |bounds, window, cx| {
                let in_sight = !bounds.intersect(&window.content_mask().bounds).is_empty();
                if in_sight {
                    seen(&id, &sight, f32::from(bounds.origin.y), &looks, window, cx);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .overflow_hidden()
        .border_1()
        .border_color(t.border)
        .bg(ground)
        .child(
            div()
                .h(px(HEADER_HEIGHT))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(10.))
                .border_b_1()
                .border_color(t.border)
                .text_color(t.text_secondary)
                .child(ui::icon(Icon::Plug, 13.).text_color(t.text_tertiary))
                .child(
                    ui::text(label(title), 11.5, FontWeight::NORMAL)
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                )
                .child({
                    let (id, label, wake) = (app.id.clone(), label(title), Wake::of(looks));
                    ui::Button::new(
                        SharedString::from(format!("app-expand-{}", app.id)),
                        t!("Expand"),
                        ui::Variant::Outline,
                    )
                    .icon(Icon::Resize)
                    .sized(22., 8., 11.5)
                    .on_click(move |_, window, cx| {
                        expand(id.clone(), label.clone(), wake.clone(), window, cx)
                    })
                }),
        )
        .child(
            div()
                .relative()
                .w_full()
                .h(px(height.clamp(1., MAX_HEIGHT)))
                .children(ui::spot(|| format!("chat-app-{}", app.id)))
                .child(body)
                .child(watch),
        )
        .into_any_element()
}

fn label_of(id: &str, cx: &mut App) -> SharedString {
    apps(cx)
        .shown
        .get(id)
        .map(|shown| shown.label.clone())
        .unwrap_or_default()
}

fn is_expanded(id: &str, cx: &mut App) -> bool {
    apps(cx)
        .expanded
        .as_ref()
        .is_some_and(|expanded| expanded.id == id)
}

/// Grows the app over its whole chat, the same view and state, until Escape or Collapse.
fn expand(id: String, label: SharedString, wake: Wake, window: &mut Window, cx: &mut App) {
    collapse(cx);
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    apps(cx).expanded = Some(Expanded {
        id: id.clone(),
        label,
        wake: wake.clone(),
        focus,
    });
    match apps(cx).view_of(&id) {
        Some(view) => shows(&view, true),
        None => start(id, wake.clone(), window, cx),
    }
    redraw(&wake, cx);
}

/// Puts the app grown over its chat back in its reply.
fn collapse(cx: &mut App) {
    let Some(expanded) = apps(cx).expanded.take() else {
        return;
    };
    if let Some(view) = apps(cx).view_of(&expanded.id) {
        shows(&view, false);
    }
    redraw(&expanded.wake, cx);
}

/// Tells the app's view, and the app, whether it fills its chat.
fn shows(view: &page_view::View, expanded: bool) {
    view.fill(expanded);
    let mode = if expanded { "fullscreen" } else { "inline" };
    view.run(&format!(
        "window.__sikemuxApp && window.__sikemuxApp.show({mode:?})"
    ));
}

fn redraw(wake: &Wake, cx: &mut App) {
    wake.reader.update(cx, |_, cx| cx.notify());
    let _ = wake.bench.update(cx, |_, cx| cx.notify());
}

/// The app of `agent`'s chat grown over the chat, drawn over its rows.
pub(super) fn expanded(agent: &str, cx: &mut App) -> Option<AnyElement> {
    let expanded = apps(cx)
        .expanded
        .as_ref()
        .filter(|expanded| expanded.wake.agent == agent)?;
    let (id, label, focus) = (
        expanded.id.clone(),
        expanded.label.clone(),
        expanded.focus.clone(),
    );
    let surface = apps(cx).view_of(&id).map(|view| view.surface);
    let t = Tokens::get(cx);
    let body = match surface {
        Some(surface) => native_view(SharedString::from(format!("app-{id}")), &surface)
            .size_full()
            .into_any_element(),
        None => div().size_full().into_any_element(),
    };
    Some(
        div()
            .id("app-expanded")
            .track_focus(&focus)
            .on_key_down(|event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    collapse(cx);
                }
            })
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .bg(theme::page_ground(t))
            .child(
                div()
                    .h(px(HEADER_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(10.))
                    .border_b_1()
                    .border_color(t.border)
                    .text_color(t.text_secondary)
                    .child(ui::icon(Icon::Plug, 13.).text_color(t.text_tertiary))
                    .child(
                        ui::text(label, 11.5, FontWeight::NORMAL)
                            .flex_1()
                            .min_w_0()
                            .truncate(),
                    )
                    .child(ui::meta("esc", cx))
                    .child(
                        ui::Button::new("app-collapse", t!("Collapse"), ui::Variant::Outline)
                            .icon(Icon::Resize)
                            .sized(22., 8., 11.5)
                            .on_click(|_, _, cx| collapse(cx)),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element(),
    )
}

/// What an app needs to reach its chat when it wakes.
#[derive(Clone)]
struct Wake {
    reader: gpui_kit::Entity<super::Reading>,
    bench: gpui_kit::WeakEntity<crate::workbench::Workbench>,
    agent: String,
}

impl Wake {
    fn of(looks: &Looks) -> Self {
        Self {
            reader: looks.reader.clone(),
            bench: looks.bench.clone(),
            agent: looks.agent.clone(),
        }
    }
}

/// The card was drawn in sight: once the chat rests with it there, the app gets a view.
fn seen(id: &str, sight: &Rc<Sight>, top: f32, wake: &Wake, window: &mut Window, cx: &mut App) {
    sight.seen.set(Some(Instant::now()));
    if (sight.top.get() - top).abs() > 0.5 || sight.moved.get().is_none() {
        sight.top.set(top);
        sight.moved.set(Some(Instant::now()));
        sight.settled.set(false);
        apps(cx).moved = Some(Instant::now());
    }
    warm(window, cx);
    let Some(shown) = apps(cx).shown.get(id) else {
        return;
    };
    if shown.view.is_some() || shown.starting || shown.failed.is_some() {
        return;
    }
    if sight.settled.get() {
        let (id, wake) = (id.to_owned(), wake.clone());
        window.defer(cx, move |window, cx| start(id, wake, window, cx));
        return;
    }
    if sight.waiting.replace(true) {
        return;
    }
    let (sight, reader) = (sight.clone(), wake.reader.clone());
    window
        .spawn(cx, async move |cx| {
            loop {
                cx.background_executor().timer(SETTLE).await;
                let rested = sight
                    .moved
                    .get()
                    .is_some_and(|moved| moved.elapsed() >= SETTLE);
                if rested {
                    break;
                }
            }
            sight.waiting.set(false);
            sight.settled.set(true);
            let _ = cx.update(|_, cx| reader.update(cx, |_, cx| cx.notify()));
        })
        .detach();
}

fn start(id: String, wake: Wake, window: &mut Window, cx: &mut App) {
    let Some(bench) = wake.bench.upgrade() else {
        return;
    };
    if apps(cx).spare().is_none() {
        let apps = apps(cx);
        if !apps
            .waiting
            .iter()
            .any(|waiting| waiting.agent == wake.agent)
        {
            apps.waiting.push(wake);
        }
        return;
    }
    let round = match apps(cx).shown.get_mut(&id) {
        Some(shown) => {
            shown.sight.settled.set(false);
            shown.starting = true;
            shown.wake = Some(wake.clone());
            shown.round += 1;
            shown.round
        }
        None => return,
    };
    let target = bench.read(cx).chat_target(&wake.agent);
    let read = target.request_typed::<call::ReadMcpApp>(McpAppParams {
        agent_id: wake.agent.clone(),
        id: id.clone(),
    });
    window
        .spawn(cx, async move |cx| {
            let read = read.await;
            let set_up = cx.update(|_, cx| {
                if let Some(shown) = apps(cx).shown.get_mut(&id) {
                    shown.starting = false;
                }
                let lent = read.map(|app| lend(&id, &app, cx));
                if let Some(shown) = apps(cx).shown.get_mut(&id) {
                    match lent {
                        Ok(Some(view)) => shown.view = Some(view),
                        Ok(None) => {}
                        Err(error) => shown.failed = Some(error.into()),
                    }
                }
                if let Some(view) = apps(cx).view_of(&id).filter(|_| is_expanded(&id, cx)) {
                    shows(&view, true);
                }
                wake.reader.update(cx, |_, cx| cx.notify());
                apps(cx).kept(&id).is_some()
            });
            if matches!(set_up, Ok(true)) {
                keep(id, round, cx).await;
            }
        })
        .detach();
}

/// Lends the app a kept view with its page loaded.
fn lend(id: &str, app: &McpApp, cx: &mut App) -> Option<u64> {
    let height = apps(cx)
        .shown
        .get(id)
        .map_or(DEFAULT_HEIGHT, |shown| shown.height.get());
    let html = host_page(id, app, height, cx);
    let apps = apps(cx);
    let kept = apps.spare()?;
    let view = apps.views.iter_mut().find(|view| view.id == kept)?;
    view.app = Some(id.to_owned());
    view.freed = None;
    view.stirred.set(Some(Instant::now()));
    view.view.show(&html);
    Some(kept)
}

/// Makes the window's views for apps while the person rests, one at a time. Making one holds the
/// main thread, the first in a while for most of a second as WebKit starts its page process, so
/// it waits until nothing has been typed, clicked or scrolled for a moment; an app waits for one
/// with its card drawn.
fn warm(window: &mut Window, cx: &mut App) {
    let pool = apps(cx);
    if pool.warming || pool.views.len() >= MAX_VIEWS {
        return;
    }
    pool.warming = true;
    window
        .spawn(cx, async move |cx| loop {
            cx.background_executor().timer(TICK).await;
            let made = cx.update(|window, cx| {
                if writing(apps(cx)) {
                    return Some(false);
                }
                let scrolled = apps(cx).moved.is_some_and(|moved| moved.elapsed() < REST);
                if scrolled || input_rest() < REST {
                    return Some(false);
                }
                let (events, heard) = tokio::sync::mpsc::unbounded_channel();
                let view = page_view::open(BLANK, Room::App, events, window, cx)?;
                let pool = apps(cx);
                pool.next_view += 1;
                let kept = pool.next_view;
                pool.views.push(Kept {
                    id: kept,
                    view,
                    app: None,
                    stirred: Rc::default(),
                    pointer_over: Rc::default(),
                    freed: None,
                });
                listen(kept, heard, window, cx);
                for wake in std::mem::take(&mut apps(cx).waiting) {
                    wake.reader.update(cx, |_, cx| cx.notify());
                }
                Some(true)
            });
            let done = match made {
                Ok(Some(_)) => apps_full(cx),
                Ok(None) | Err(_) => true,
            };
            if done {
                let _ = cx.update(|_, cx| apps(cx).warming = false);
                return;
            }
        })
        .detach();
}

/// Whether an agent is writing in a chat that drew an app, which holding the main thread would
/// stall; the time the chat moved is kept up to date while it does.
fn writing(pool: &mut Apps) -> bool {
    pool.turns.retain(|turn| turn.strong_count() > 0);
    let writing = pool
        .turns
        .iter()
        .any(|turn| turn.upgrade().is_some_and(|running| running.get()));
    if writing {
        pool.moved = Some(Instant::now());
    }
    writing
}

fn apps_full(cx: &mut AsyncWindowContext) -> bool {
    cx.update(|_, cx| apps(cx).views.len() >= MAX_VIEWS)
        .unwrap_or(true)
}

/// How long since the person last typed, clicked or moved anything, anywhere.
#[cfg(target_os = "macos")]
fn input_rest() -> Duration {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, kind: u32) -> f64;
    }
    // SAFETY: a plain query of the combined session's input; both arguments are its constants.
    let seconds = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
    Duration::from_secs_f64(seconds.max(0.))
}

#[cfg(not(target_os = "macos"))]
fn input_rest() -> Duration {
    Duration::MAX
}

/// The page that holds the app in a sandboxed frame of its own and speaks for it to the chat.
fn host_page(id: &str, app: &McpApp, height: f32, cx: &App) -> String {
    let t = Tokens::get(cx);
    let ground = theme::css(theme::page_ground(t));
    let variables = json!({
        "--color-background-primary": ground,
        "--color-background-secondary": theme::css(t.surface_raised),
        "--color-background-tertiary": theme::css(t.surface_hover),
        "--color-text-primary": theme::css(t.text_primary),
        "--color-text-secondary": theme::css(t.text_secondary),
        "--color-text-tertiary": theme::css(t.text_tertiary),
        "--color-text-danger": theme::css(t.danger),
        "--color-border-primary": theme::css(t.border),
        "--color-border-secondary": theme::css(t.border_strong),
        "--color-ring-primary": theme::css(t.accent),
        "--font-sans": format!("{}, system-ui, sans-serif", theme::UI),
        "--font-mono": format!("{}, ui-monospace, monospace", theme::MONO),
        "--border-radius-sm": "4px",
        "--border-radius-md": "6px",
        "--border-radius-lg": "10px",
    });
    let host = json!({
        "html": app.html,
        "input": app.input,
        "result": app.result,
        "version": env!("CARGO_PKG_VERSION"),
        "maxHeight": MAX_HEIGHT,
        "context": {
            "toolInfo": { "id": id, "tool": app.tool },
            "theme": if t.dark { "dark" } else { "light" },
            "styles": { "variables": variables },
            "displayMode": "inline",
            "availableDisplayModes": ["inline", "fullscreen"],
            "containerDimensions": { "maxHeight": MAX_HEIGHT },
            "platform": "desktop",
            "userAgent": "Sikemux",
            "deviceCapabilities": { "touch": false, "hover": true },
        },
    });
    let host = serde_json::to_string(&host)
        .unwrap_or_else(|_| "{}".into())
        .replace('<', "\\u003c");
    HOST.replace("__GROUND__", &ground)
        .replace("__HEIGHT__", &format!("{}", height.round()))
        .replace("__HOST__", &host)
}

/// Hears what the page in the kept view `kept` says, for whichever app borrows it. It holds no
/// view itself, so a view let go ends it.
fn listen(
    kept: u64,
    mut heard: tokio::sync::mpsc::UnboundedReceiver<PageEvent>,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            while let Some(event) = heard.recv().await {
                let applied = cx.update(|window, cx| {
                    let apps = apps(cx);
                    let Some(view) = apps.views.iter().find(|view| view.id == kept) else {
                        return;
                    };
                    view.stirred.set(Some(Instant::now()));
                    let (pointer_over, Some(id)) = (view.pointer_over.clone(), view.app.clone())
                    else {
                        return;
                    };
                    let Some(wake) = apps.shown.get(&id).and_then(|shown| shown.wake.clone())
                    else {
                        return;
                    };
                    heard_from(&id, event, &pointer_over, &wake, window, cx);
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
}

fn heard_from(
    id: &str,
    event: PageEvent,
    pointer_over: &Cell<bool>,
    wake: &Wake,
    window: &mut Window,
    cx: &mut App,
) {
    match event {
        PageEvent::Height(_) if is_expanded(id, cx) => {}
        PageEvent::Height(height) => {
            if let Some(shown) = apps(cx).shown.get(id) {
                shown.height.set(height as f32);
            }
            wake.reader.update(cx, |_, cx| cx.notify());
        }
        PageEvent::Escape if is_expanded(id, cx) => collapse(cx),
        PageEvent::Escape => {}
        PageEvent::Pointer(over) => pointer_over.set(over),
        PageEvent::Link(url) => {
            let _ = wake
                .bench
                .update(cx, |bench, cx| bench.open_on_desk(&wake.agent, &url, cx));
        }
        PageEvent::Wheel(delta) => {
            let _ = wake.bench.update(cx, |bench, cx| {
                if let Some(chat) = bench.chats.open.get(&wake.agent) {
                    chat.transcript.update(cx, |transcript, cx| {
                        transcript.list().scroll_by(px(delta as f32));
                        cx.notify();
                    });
                }
            });
        }
        PageEvent::App(message)
            if message.get("method").and_then(Value::as_str) == Some("ui/request-display-mode") =>
        {
            match message.pointer("/params/mode").and_then(Value::as_str) {
                Some("fullscreen") if !is_expanded(id, cx) => {
                    let label = label_of(id, cx);
                    expand(id.to_owned(), label, wake.clone(), window, cx)
                }
                Some("inline") if is_expanded(id, cx) => collapse(cx),
                _ => {}
            }
        }
        PageEvent::App(message) => ask(id, message, wake, cx),
        PageEvent::Mod(_) => {}
    }
}

/// Passes a request the app made of its server to the core, and its answer back to the app,
/// if it still has its view by then.
fn ask(id: &str, message: Value, wake: &Wake, cx: &mut App) {
    let Some(bench) = wake.bench.upgrade() else {
        return;
    };
    let (Some(method), Some(asked)) = (
        message.get("method").and_then(Value::as_str),
        message.get("id").cloned(),
    ) else {
        return;
    };
    let answer = bench
        .read(cx)
        .chat_target(&wake.agent)
        .request_typed::<call::McpAppRequest>(McpAppRequestParams {
            agent_id: wake.agent.clone(),
            id: id.to_owned(),
            method: method.to_owned(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
        });
    let id = id.to_owned();
    cx.spawn(async move |cx| {
        let (ok, value) = match answer.await {
            Ok(result) => (true, result),
            Err(error) => (false, Value::String(error)),
        };
        cx.update(|cx| {
            if let Some(view) = apps(cx).view_of(&id) {
                view.run(&format!(
                    "window.__sikemuxApp && window.__sikemuxApp.reply({asked}, {ok}, {value})"
                ));
            }
        });
    })
    .detach();
}

/// Keeps a running app's still fresh once it has been quiet a moment, and hands its view back
/// once it has been out of sight a while; a view no app then borrows is let go after a while.
async fn keep(id: String, round: u64, cx: &mut AsyncWindowContext) {
    let mut hidden_since: Option<Instant> = None;
    loop {
        cx.background_executor().timer(TICK).await;
        let looked = cx.update(|_, cx| {
            let apps = apps(cx);
            let shown = apps.shown.get(&id).filter(|shown| shown.round == round)?;
            let has_still = shown.still.is_some();
            let wake = shown.wake.clone();
            let kept = apps.kept(&id)?;
            if kept.view.surface.placed().is_some() {
                hidden_since = None;
                let quiet = kept
                    .stirred
                    .get()
                    .is_none_or(|stirred| stirred.elapsed() >= QUIET);
                if quiet && !kept.pointer_over.get() && (kept.stirred.get().is_some() || !has_still)
                {
                    kept.stirred.set(None);
                    return Some(Look::Still(kept.view.clone()));
                }
                return Some(Look::Rest);
            }
            let since = *hidden_since.get_or_insert_with(Instant::now);
            if since.elapsed() < GRACE {
                return Some(Look::Rest);
            }
            let kept = kept.id;
            apps.hand_back(&id);
            if let Some(wake) = wake {
                wake.reader.update(cx, |_, cx| cx.notify());
            }
            Some(Look::HandedBack(kept))
        });
        match looked {
            Ok(Some(Look::Still(view))) => {
                let (sent, picture) = tokio::sync::oneshot::channel();
                view.still(Box::new(move |jpeg| {
                    let _ = sent.send(jpeg);
                }));
                if let Ok(Some(jpeg)) = picture.await {
                    let still = Arc::new(Image::from_bytes(ImageFormat::Jpeg, jpeg));
                    let _ = cx.update(|_, cx| {
                        if let Some(shown) = apps(cx).shown.get_mut(&id) {
                            shown.still = Some(still);
                        }
                    });
                }
            }
            Ok(Some(Look::Rest)) => {}
            Ok(Some(Look::HandedBack(kept))) => {
                cx.background_executor().timer(IDLE).await;
                let _ = cx.update(|_, cx| {
                    apps(cx).views.retain(|view| {
                        view.id != kept
                            || view.app.is_some()
                            || view.freed.is_none_or(|freed| freed.elapsed() < IDLE)
                    })
                });
                return;
            }
            Ok(None) | Err(_) => return,
        }
    }
}

enum Look {
    Still(page_view::View),
    Rest,
    HandedBack(u64),
}
