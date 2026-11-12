//! An MCP app an agent's tool call opened, drawn in its reply in a card that names its server.
//! A web view costs the app a process and its memory, so only apps in sight get one, and never
//! more than a few however long the chat. An app out of sight for a moment is kept as a still
//! picture of itself and gives its view back; once it is in sight again and the chat has stopped
//! scrolling, it is set up again from its call.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    canvas, div, img, px, AnyElement, App, AsyncWindowContext, FontWeight, Global, Image,
    ImageFormat, IntoElement, ObjectFit, ParentElement, SharedString, Styled, StyledImage, Window,
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

/// The most apps with a view of their own at once.
const MAX_LIVE: usize = 3;
const DEFAULT_HEIGHT: f32 = 240.;
const MAX_HEIGHT: f32 = 2000.;
const HEADER_HEIGHT: f32 = 30.;
/// How long the chat must rest before an app in sight gets a view, so scrolling past apps never
/// waits on WebKit.
const SETTLE: Duration = Duration::from_millis(160);
/// How long an app may be out of sight before its view goes.
const GRACE: Duration = Duration::from_millis(1500);
/// How often a live app is looked at, to take a fresh still or let its view go.
const TICK: Duration = Duration::from_millis(500);
const HOST: &str = include_str!("app_host.html");

#[derive(Default)]
struct Apps {
    shown: HashMap<String, Shown>,
}

impl Global for Apps {}

struct Shown {
    height: Rc<Cell<f32>>,
    /// Where the card was last drawn in sight, and when it last moved there.
    sight: Rc<Sight>,
    still: Option<Arc<Image>>,
    live: Option<Live>,
    starting: bool,
    failed: Option<SharedString>,
}

#[derive(Default)]
struct Sight {
    top: Cell<f32>,
    moved: Cell<Option<Instant>>,
    /// The chat rested long enough after this app last moved in sight to give it a view.
    settled: Cell<bool>,
    waiting: Cell<bool>,
}

struct Live {
    view: page_view::View,
    /// Something changed in the app since its last still.
    changed: Rc<Cell<bool>>,
    pointer_over: Rc<Cell<bool>>,
}

fn apps(cx: &mut App) -> &mut Apps {
    if !cx.has_global::<Apps>() {
        cx.set_global(Apps::default());
    }
    cx.global_mut::<Apps>()
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

pub(super) fn app(app: &McpAppRef, title: &str, looks: &Looks, cx: &mut App) -> AnyElement {
    let shown = apps(cx)
        .shown
        .entry(app.id.clone())
        .or_insert_with(|| Shown {
            height: Rc::new(Cell::new(DEFAULT_HEIGHT)),
            sight: Rc::default(),
            still: None,
            live: None,
            starting: false,
            failed: None,
        });
    let (height, sight, still, surface, failed) = (
        shown.height.get(),
        shown.sight.clone(),
        shown.still.clone(),
        shown.live.as_ref().map(|live| live.view.surface.clone()),
        shown.failed.clone(),
    );
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
            .child(ui::meta(format!("Could not show this app: {failed}"), cx))
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
                .child(ui::text(label(title), 11.5, FontWeight::NORMAL).truncate()),
        )
        .child(
            div()
                .relative()
                .w_full()
                .h(px(height.clamp(1., MAX_HEIGHT)))
                .child(body)
                .child(watch),
        )
        .into_any_element()
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
    if (sight.top.get() - top).abs() > 0.5 || sight.moved.get().is_none() {
        sight.top.set(top);
        sight.moved.set(Some(Instant::now()));
        sight.settled.set(false);
    }
    let Some(shown) = apps(cx).shown.get(id) else {
        return;
    };
    if shown.live.is_some() || shown.starting || shown.failed.is_some() {
        return;
    }
    if sight.settled.get() {
        sight.settled.set(false);
        start(id.to_owned(), wake.clone(), window, cx);
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

/// Makes room for one more live app, letting go of the one longest out of sight; with every
/// live app in sight there is no room.
fn room(apps: &mut Apps) -> bool {
    let live: Vec<&String> = apps
        .shown
        .iter()
        .filter(|(_, shown)| shown.live.is_some() || shown.starting)
        .map(|(id, _)| id)
        .collect();
    if live.len() < MAX_LIVE {
        return true;
    }
    let out_of_sight = apps
        .shown
        .iter()
        .filter_map(|(id, shown)| {
            let live = shown.live.as_ref()?;
            live.view
                .surface
                .placed()
                .is_none()
                .then(|| (id.clone(), shown.sight.moved.get()))
        })
        .min_by_key(|(_, moved)| *moved)
        .map(|(id, _)| id);
    match out_of_sight {
        Some(id) => {
            if let Some(shown) = apps.shown.get_mut(&id) {
                shown.live = None;
            }
            true
        }
        None => false,
    }
}

fn start(id: String, wake: Wake, window: &mut Window, cx: &mut App) {
    let Some(bench) = wake.bench.upgrade() else {
        return;
    };
    if !room(apps(cx)) {
        return;
    }
    if let Some(shown) = apps(cx).shown.get_mut(&id) {
        shown.starting = true;
    }
    let target = bench.read(cx).chat_target(&wake.agent);
    let read = target.request_typed::<call::ReadMcpApp>(McpAppParams {
        agent_id: wake.agent.clone(),
        id: id.clone(),
    });
    window
        .spawn(cx, async move |cx| {
            let opened = match read.await {
                Ok(app) => cx.update(|window, cx| open(&id, &app, &wake, window, cx)),
                Err(error) => Ok(Err(error)),
            };
            let opened = match opened {
                Ok(opened) => opened,
                Err(_) => return,
            };
            let _ = cx.update(|_, cx| {
                if let Some(shown) = apps(cx).shown.get_mut(&id) {
                    shown.starting = false;
                    match opened {
                        Ok(live) => shown.live = Some(live),
                        Err(error) => shown.failed = Some(error.into()),
                    }
                }
                wake.reader.update(cx, |_, cx| cx.notify());
            });
            keep(id, wake, cx).await;
        })
        .detach();
}

fn open(
    id: &str,
    app: &McpApp,
    wake: &Wake,
    window: &mut Window,
    cx: &mut App,
) -> Result<Live, String> {
    let height = apps(cx)
        .shown
        .get(id)
        .map_or(DEFAULT_HEIGHT, |shown| shown.height.get());
    let html = host_page(id, app, height, cx);
    let (events, heard) = tokio::sync::mpsc::unbounded_channel();
    let view = page_view::open(&html, Room::App, events, window, cx)
        .ok_or("this window cannot show web pages")?;
    let live = Live {
        view: view.clone(),
        changed: Rc::new(Cell::new(true)),
        pointer_over: Rc::default(),
    };
    listen(
        id.to_owned(),
        view,
        live.changed.clone(),
        live.pointer_over.clone(),
        heard,
        wake.clone(),
        window,
        cx,
    );
    Ok(live)
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
            "availableDisplayModes": ["inline"],
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

#[allow(clippy::too_many_arguments)]
fn listen(
    id: String,
    view: page_view::View,
    changed: Rc<Cell<bool>>,
    pointer_over: Rc<Cell<bool>>,
    mut heard: tokio::sync::mpsc::UnboundedReceiver<PageEvent>,
    wake: Wake,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            while let Some(event) = heard.recv().await {
                changed.set(true);
                let applied = cx.update(|_, cx| match event {
                    PageEvent::Height(height) => {
                        if let Some(shown) = apps(cx).shown.get(&id) {
                            shown.height.set(height as f32);
                        }
                        wake.reader.update(cx, |_, cx| cx.notify());
                    }
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
                    PageEvent::App(message) => ask(&id, message, &view, &wake, cx),
                    PageEvent::Mod(_) => {}
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
}

/// Passes a request the app made of its server to the core, and its answer back to the app.
fn ask(id: &str, message: Value, view: &page_view::View, wake: &Wake, cx: &mut App) {
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
    let view = view.clone();
    cx.spawn(async move |_| {
        let (ok, value) = match answer.await {
            Ok(result) => (true, result),
            Err(error) => (false, Value::String(error)),
        };
        view.run(&format!(
            "window.__sikemuxApp && window.__sikemuxApp.reply({asked}, {ok}, {value})"
        ));
    })
    .detach();
}

/// Keeps a live app's still fresh while it is in sight, and lets go of its view once it has been
/// out of sight a while.
async fn keep(id: String, wake: Wake, cx: &mut AsyncWindowContext) {
    let mut hidden_since: Option<Instant> = None;
    loop {
        cx.background_executor().timer(TICK).await;
        let looked = cx.update(|_, cx| {
            let shown = apps(cx).shown.get_mut(&id)?;
            let live = shown.live.as_ref()?;
            let in_sight = live.view.surface.placed().is_some();
            if in_sight {
                hidden_since = None;
                if live.changed.replace(false) || live.pointer_over.get() || shown.still.is_none() {
                    return Some(Look::Still(live.view.clone()));
                }
                return Some(Look::Rest);
            }
            let since = *hidden_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= GRACE {
                shown.live = None;
                wake.reader.update(cx, |_, cx| cx.notify());
                return None;
            }
            Some(Look::Rest)
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
            Ok(None) | Err(_) => return,
        }
    }
}

enum Look {
    Still(page_view::View),
    Rest,
}
