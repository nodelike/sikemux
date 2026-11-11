//! A page an agent showed with `page_show`, drawn in its reply at the height the
//! page reports, or as a card for a page shown in a tab on the agent's desk. The core keeps
//! the page; its view keeps no storage and never navigates, and a link the person clicks in
//! it opens on the agent's desk.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    deferred, div, img, linear_color_stop, linear_gradient, px, AnyElement, App, FontWeight,
    Global, Image, InteractiveElement, IntoElement, ObjectFit, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Window,
};
use gpui_native_view::{native_view, NativeSurface};
use sikemux_view::chat::PageRef;

use super::Looks;
use crate::icons::Icon;
use crate::page_view::{self, PageEvent, Room};
use crate::theme::{self, Tokens};
use crate::ui;
use sikemux_core::server::call;
use sikemux_core::server::harness::pages::PageParams;

/// A page's height before it reports its own, when the agent gave none.
const DEFAULT_HEIGHT: f32 = 320.;
const MAX_HEIGHT: f32 = sikemux_core::pages::MAX_HEIGHT as f32;
/// How long Open in tab stays after the pointer leaves the page, so it can be reached.
const CHIP_LINGER: Duration = Duration::from_millis(400);
const FADE_HEIGHT: f32 = 72.;
/// On X11 a page is a window of its own, and anything drawn over it reshapes that window on
/// every frame, which froze the app; there Open in tab sits under the page instead.
const DRAWS_OVER_PAGES: bool = cfg!(target_os = "macos");
const CARD_HEIGHT: f32 = 300.;
const CARD_BARE_HEIGHT: f32 = 64.;
/// How wide the picture a card shows is kept, sharp on a Retina reply column.
const THUMB_KEPT_WIDTH: u32 = 1280;
/// The picture keeps only the top of the page, as wide as this is to its height.
const THUMB_KEPT_SHAPE: f32 = 2.4;

/// Every page the window has drawn, by id, so a chat drawn again keeps each one's view and height.
#[derive(Default)]
struct Pages {
    shown: HashMap<String, Shown>,
}

impl Global for Pages {}

#[derive(Clone, Default)]
struct Pointer {
    over_page: Rc<Cell<bool>>,
    over_chip: Rc<Cell<bool>>,
    left_at: Rc<Cell<Option<Instant>>>,
}

impl Pointer {
    fn shows_chip(&self) -> bool {
        self.over_page.get()
            || self.over_chip.get()
            || self
                .left_at
                .get()
                .is_some_and(|left| left.elapsed() < CHIP_LINGER)
    }
}

struct Shown {
    height: Rc<Cell<f32>>,
    surface: Option<NativeSurface>,
    picture: Option<Arc<Image>>,
    pointer: Pointer,
}

pub(super) fn page(page: &PageRef, looks: &Looks, window: &mut Window, cx: &mut App) -> AnyElement {
    if !cx.has_global::<Pages>() {
        cx.set_global(Pages::default());
    }
    if !cx.global::<Pages>().shown.contains_key(&page.id) {
        let height = page.height.map_or(DEFAULT_HEIGHT, |height| height as f32);
        cx.global_mut::<Pages>().shown.insert(
            page.id.clone(),
            Shown {
                height: Rc::new(Cell::new(height)),
                surface: None,
                picture: None,
                pointer: Pointer::default(),
            },
        );
        fetch(page, looks, window, cx);
    }
    let shown = &cx.global::<Pages>().shown[&page.id];
    let (height, surface, picture, pointer) = (
        shown.height.get(),
        shown.surface.clone(),
        shown.picture.clone(),
        shown.pointer.clone(),
    );
    if page.tab {
        return card(page, picture, looks, cx);
    }
    let t = Tokens::get(cx);
    let cut = height > MAX_HEIGHT;
    let chip = (DRAWS_OVER_PAGES && pointer.shows_chip()).then(|| {
        let reader = looks.reader.clone();
        let over_chip = pointer.over_chip.clone();
        deferred(
            div()
                .id(SharedString::from(format!("page-chip-{}", page.id)))
                .absolute()
                .top(px(8.))
                .right(px(8.))
                .on_hover(move |hovered, _, cx| {
                    over_chip.set(*hovered);
                    reader.update(cx, |_, cx| cx.notify());
                })
                .child(open_in_tab(page, looks, "page-chip", cx)),
        )
    });
    let fade = (DRAWS_OVER_PAGES && cut).then(|| {
        let ground = theme::page_ground(t);
        deferred(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(FADE_HEIGHT))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(gpui_kit::Hsla { a: 0., ..ground }, 0.),
                    linear_color_stop(ground, 0.9),
                )),
        )
    });
    let drawn = div()
        .relative()
        .w_full()
        .h(px(height.clamp(1., MAX_HEIGHT)))
        .children(crate::ui::spot(|| format!("chat-page-{}", page.id)))
        .children(surface.map(|surface| {
            native_view(SharedString::from(format!("page-{}", page.id)), &surface).size_full()
        }))
        .children(fade)
        .children(chip);
    if DRAWS_OVER_PAGES && !cut {
        return drawn.into_any_element();
    }
    let shows = cut.then(|| {
        ui::meta(
            format!(
                "Showing {} of {} px",
                ui::numbers::whole(f64::from(MAX_HEIGHT)),
                ui::numbers::whole(f64::from(height))
            ),
            cx,
        )
    });
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(drawn)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .children(shows)
                .child(open_in_tab(page, looks, "page-under", cx)),
        )
        .into_any_element()
}

/// A page shown in a tab on the desk: the top of it fading out under its title, opening the tab
/// when clicked.
fn card(page: &PageRef, picture: Option<Arc<Image>>, looks: &Looks, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let height = if picture.is_some() {
        CARD_HEIGHT
    } else {
        CARD_BARE_HEIGHT
    };
    let fade = gpui_kit::Hsla {
        a: 0.,
        ..t.surface_raised
    };
    div()
        .id(SharedString::from(format!("page-card-{}", page.id)))
        .relative()
        .w_full()
        .h(px(height))
        .rounded(px(10.))
        .overflow_hidden()
        .border_1()
        .border_color(t.border)
        .bg(t.surface_raised)
        .cursor_pointer()
        .hover(|card| card.border_color(t.border_strong))
        .on_click(opens_tab(page, looks))
        .children(picture.map(|picture| {
            img(picture)
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Cover)
        }))
        .child(div().absolute().size_full().bg(linear_gradient(
            180.,
            linear_color_stop(fade, 0.3),
            linear_color_stop(t.surface_raised, 0.8),
        )))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .flex()
                .items_end()
                .gap(px(10.))
                .px(px(14.))
                .pb(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(ui::text(page.title.clone(), 14., FontWeight::MEDIUM).truncate())
                        .child(ui::meta("In a tab on the desk", cx)),
                )
                .child(
                    ui::Button::new(
                        SharedString::from(format!("page-card-open-{}", page.id)),
                        "Open",
                        ui::Variant::Outline,
                    )
                    .icon(Icon::Tabs)
                    .sized(24., 10., 11.5),
                ),
        )
        .into_any_element()
}

fn open_in_tab(page: &PageRef, looks: &Looks, key: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    ui::Button::new(
        SharedString::from(format!("{key}-{}", page.id)),
        "Open in tab",
        ui::Variant::Outline,
    )
    .icon(Icon::Tabs)
    .sized(22., 8., 11.5)
    .solid(t.surface_raised, t.surface_hover)
    .on_click(opens_tab(page, looks))
    .into_any_element()
}

fn opens_tab(
    page: &PageRef,
    looks: &Looks,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
    let (id, agent, bench) = (page.id.clone(), looks.agent.clone(), looks.bench.clone());
    move |_, _, cx| {
        let _ = bench.update(cx, |bench, cx| bench.open_page_in_tab(&agent, &id, cx));
    }
}

/// The top of a desk tab's picture, as a card shows it, in base64.
pub(crate) fn thumbnail(jpeg: &[u8]) -> Option<String> {
    use base64::Engine as _;
    let decoded = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
    let width = decoded.width();
    let top = (width as f32 / THUMB_KEPT_SHAPE) as u32;
    let kept = decoded.crop_imm(0, 0, width, top.min(decoded.height()));
    let small = if width > THUMB_KEPT_WIDTH {
        kept.resize(
            THUMB_KEPT_WIDTH,
            u32::MAX,
            image::imageops::FilterType::Triangle,
        )
    } else {
        kept
    }
    .into_rgb8();
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 80)
        .encode_image(&small)
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Asks the core for the page, then shows it in a view of its own, or keeps its picture for a
/// page shown in a tab.
fn fetch(page: &PageRef, looks: &Looks, window: &mut Window, cx: &mut App) {
    let Some(target) = looks
        .bench
        .upgrade()
        .map(|bench| bench.read(cx).chat_target(&looks.agent))
    else {
        return;
    };
    let answer = target.request_typed::<call::ReadPage>(PageParams {
        id: page.id.clone(),
    });
    let theme = theme::page_rule(Tokens::get(cx), opaque_ground(Tokens::get(cx)));
    let (id, reader, tab) = (page.id.clone(), looks.reader.clone(), page.tab);
    let (bench, agent) = (looks.bench.clone(), looks.agent.clone());
    window
        .spawn(cx, async move |cx| {
            let Ok(answer) = answer.await else {
                return;
            };
            if tab {
                let picture = answer
                    .picture
                    .and_then(|picture| crate::picture::decode("image/jpeg", &picture));
                let _ = cx.update(|_, cx| {
                    if let Some(shown) = cx.global_mut::<Pages>().shown.get_mut(&id) {
                        shown.picture = picture;
                    }
                    reader.update(cx, |_, cx| cx.notify());
                });
                return;
            }
            let html = sikemux_core::pages::themed(&answer.html, &theme, None);
            let (events, mut heard) = tokio::sync::mpsc::unbounded_channel();
            let _ = cx.update(|window, cx| {
                let surface = page_view::open(&html, Room::Grows, events, window, cx)
                    .map(|view| view.surface);
                if let Some(shown) = cx.global_mut::<Pages>().shown.get_mut(&id) {
                    shown.surface = surface;
                }
                reader.update(cx, |_, cx| cx.notify());
            });
            while let Some(event) = heard.recv().await {
                let applied = cx.update(|window, cx| match event {
                    PageEvent::Height(height) => {
                        if let Some(shown) = cx.global::<Pages>().shown.get(&id) {
                            shown.height.set(height as f32);
                        }
                        reader.update(cx, |_, cx| cx.notify());
                    }
                    PageEvent::Pointer(_) if !DRAWS_OVER_PAGES => {}
                    PageEvent::Pointer(over) => {
                        if let Some(shown) = cx.global::<Pages>().shown.get(&id) {
                            shown.pointer.over_page.set(over);
                            shown.pointer.left_at.set((!over).then(Instant::now));
                        }
                        reader.update(cx, |_, cx| cx.notify());
                        if !over {
                            let reader = reader.clone();
                            window
                                .spawn(cx, async move |cx| {
                                    cx.background_executor().timer(CHIP_LINGER).await;
                                    let _ =
                                        cx.update(|_, cx| reader.update(cx, |_, cx| cx.notify()));
                                })
                                .detach();
                        }
                    }
                    PageEvent::Link(url) => {
                        let _ = bench.update(cx, |bench, cx| bench.open_on_desk(&agent, &url, cx));
                    }
                    PageEvent::Wheel(delta) => {
                        let _ = bench.update(cx, |bench, cx| {
                            if let Some(chat) = bench.chats.open.get(&agent) {
                                chat.transcript.update(cx, |transcript, cx| {
                                    transcript.list().scroll_by(px(delta as f32));
                                    cx.notify();
                                });
                            }
                        });
                    }
                    PageEvent::Mod(_) => {}
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
}

/// A page on Linux is a window of its own, which has nothing behind it to show through, so it
/// draws the window's ground itself.
fn opaque_ground(t: &Tokens) -> Option<gpui_kit::Hsla> {
    cfg!(target_os = "linux").then(|| theme::page_ground(t))
}
