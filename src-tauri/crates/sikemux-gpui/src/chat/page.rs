//! A page an agent showed with `page_show`, drawn in its reply at the height the
//! page reports. The core keeps the page; its view keeps no storage and never
//! navigates, and a link the person clicks in it opens on the agent's desk.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    div, px, AnyElement, App, Global, IntoElement, ParentElement, SharedString, Styled, Window,
};
use gpui_native_view::{native_view, NativeSurface};
use serde_json::json;
use sikemux_wire::chat::PageRef;

use super::Looks;
use crate::theme::{self, Tokens};

/// A page's height before it reports its own, when the agent gave none.
const DEFAULT_HEIGHT: f32 = 320.;
const MAX_HEIGHT: f32 = 4000.;

/// Every page the window has drawn, by id, so a chat drawn again keeps each one's view and height.
#[derive(Default)]
struct Pages {
    shown: HashMap<String, Shown>,
}

impl Global for Pages {}

struct Shown {
    height: Rc<Cell<f32>>,
    surface: Option<NativeSurface>,
}

pub(super) fn page(page: &PageRef, looks: &Looks, window: &mut Window, cx: &mut App) -> AnyElement {
    if !cx.has_global::<Pages>() {
        cx.set_global(Pages::default());
    }
    let known = cx
        .global::<Pages>()
        .shown
        .get(&page.id)
        .map(|shown| (shown.height.get(), shown.surface.clone()));
    let (height, surface) = match known {
        Some(known) => known,
        None => {
            let height = page.height.map_or(DEFAULT_HEIGHT, |height| height as f32);
            cx.global_mut::<Pages>().shown.insert(
                page.id.clone(),
                Shown {
                    height: Rc::new(Cell::new(height)),
                    surface: None,
                },
            );
            fetch(page, looks, window, cx);
            (height, None)
        }
    };
    div()
        .w_full()
        .h(px(height.clamp(1., MAX_HEIGHT)))
        .children(surface.map(|surface| {
            native_view(SharedString::from(format!("page-{}", page.id)), &surface).size_full()
        }))
        .into_any_element()
}

/// Asks the core for the page, then shows it in a view of its own.
fn fetch(page: &PageRef, looks: &Looks, window: &mut Window, cx: &mut App) {
    let Some(target) = looks
        .bench
        .upgrade()
        .map(|bench| bench.read(cx).chat_target(&looks.agent))
    else {
        return;
    };
    let answer = target.request("page", json!({ "id": page.id }));
    let theme = theme::page_rule(Tokens::get(cx), None);
    let (id, reader, bench, agent) = (
        page.id.clone(),
        looks.reader.clone(),
        looks.bench.clone(),
        looks.agent.clone(),
    );
    window
        .spawn(cx, async move |cx| {
            let Ok(answer) = answer.await else {
                return;
            };
            let Some(html) = answer["html"].as_str() else {
                return;
            };
            let html = sikemux_core::pages::themed(html, &theme, None);
            let (events, mut heard) = tokio::sync::mpsc::unbounded_channel();
            let _ = cx.update(|window, cx| {
                let surface = shown_view(&html, events, window);
                if let Some(shown) = cx.global_mut::<Pages>().shown.get_mut(&id) {
                    shown.surface = surface;
                }
                reader.update(cx, |_, cx| cx.notify());
            });
            while let Some(event) = heard.recv().await {
                let applied = cx.update(|_, cx| match event {
                    #[cfg(target_os = "macos")]
                    sikemux_browser::page_view::PageEvent::Height(height) => {
                        if let Some(shown) = cx.global::<Pages>().shown.get(&id) {
                            shown.height.set((height as f32).clamp(1., MAX_HEIGHT));
                        }
                        reader.update(cx, |_, cx| cx.notify());
                    }
                    #[cfg(target_os = "macos")]
                    sikemux_browser::page_view::PageEvent::Link(url) => {
                        let _ = bench.update(cx, |bench, _| bench.open_on_desk(&agent, &url));
                    }
                    #[cfg(target_os = "macos")]
                    sikemux_browser::page_view::PageEvent::Wheel(delta) => {
                        let _ = bench.update(cx, |bench, cx| {
                            if let Some(chat) = bench.chats.get(&agent) {
                                chat.transcript.update(cx, |transcript, cx| {
                                    transcript.list().scroll_by(px(delta as f32));
                                    cx.notify();
                                });
                            }
                        });
                    }
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
}

#[cfg(target_os = "macos")]
type Events = tokio::sync::mpsc::UnboundedSender<sikemux_browser::page_view::PageEvent>;
#[cfg(not(target_os = "macos"))]
type Events = tokio::sync::mpsc::UnboundedSender<std::convert::Infallible>;

#[cfg(target_os = "macos")]
fn shown_view(html: &str, events: Events, window: &Window) -> Option<NativeSurface> {
    use sikemux_browser::page_view::PageView;
    let mtm = objc2::MainThreadMarker::new()?;
    let page = PageView::new(mtm, html, move |event| {
        let _ = events.send(event);
    });
    let view = gpui_native_view::OwnedView::new(page.view(), window)?;
    Some(NativeSurface::new(Shows { _page: page, view }))
}

#[cfg(not(target_os = "macos"))]
fn shown_view(_: &str, _: Events, _: &Window) -> Option<NativeSurface> {
    None
}

/// The page's view placed in its row, kept with the page that fills it.
#[cfg(target_os = "macos")]
struct Shows {
    _page: sikemux_browser::page_view::PageView,
    view: gpui_native_view::OwnedView,
}

#[cfg(target_os = "macos")]
impl gpui_native_view::Surface for Shows {
    fn place(&self, frame: Option<gpui_native_view::Frame>) {
        self.view.place(frame);
    }

    fn responder(&self) -> Option<gpui_native_view::Retained<gpui_native_view::NSView>> {
        self.view.responder()
    }
}
