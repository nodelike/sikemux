//! A page an agent showed with `page_show`, as a card in its reply for the tab it opened on the
//! agent's desk. The core keeps the page and a picture of the tab; a click brings the tab forward.

use crate::ui::Clicks;
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, App, FontWeight, Global, Image, InteractiveElement, IntoElement,
    ObjectFit, ParentElement, SharedString, Styled, StyledImage, Window,
};
use sikemux_view::chat::{PageRef, WorkerPage};

use super::Looks;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::ui;
use sikemux_calls::call;
use sikemux_calls::harness::PageParams;

const ROW_HEIGHT: f32 = 72.;
const THUMB_WIDTH: f32 = 136.;
/// How wide the picture a row shows is kept, sharp on a Retina screen.
const THUMB_KEPT_WIDTH: u32 = 408;
/// The picture keeps only the top of the page, as wide as this is to its height.
const THUMB_KEPT_SHAPE: f32 = 2.4;

/// Every page card the window has drawn, by id, so a chat drawn again asks the core once.
#[derive(Default)]
struct Pages {
    shown: HashMap<String, Shown>,
}

impl Global for Pages {}

#[derive(Default)]
struct Shown {
    picture: Option<Arc<Image>>,
    failed: Option<SharedString>,
}

fn shown<'a>(page: &PageRef, looks: &Looks, window: &mut Window, cx: &'a mut App) -> &'a Shown {
    if !cx.has_global::<Pages>() {
        cx.set_global(Pages::default());
    }
    if !cx.global::<Pages>().shown.contains_key(&page.id) {
        cx.global_mut::<Pages>()
            .shown
            .insert(page.id.clone(), Shown::default());
        fetch(page, looks, window, cx);
    }
    &cx.global::<Pages>().shown[&page.id]
}

/// Pages shown in tabs on the desk, one row each in a shared frame: the top of the page, its
/// title, and Open, which brings its tab forward, as a click anywhere on the row does.
pub(super) fn tabs(
    pages: &[&PageRef],
    looks: &Looks,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let rows: Vec<AnyElement> = pages
        .iter()
        .enumerate()
        .map(|(at, page)| {
            let shown = shown(page, looks, window, cx);
            let (picture, failed) = (shown.picture.clone(), shown.failed.clone());
            let place: SharedString = match failed {
                Some(failed) => t!("Could not show it: {failed}", failed = failed).into(),
                None => t!("In a tab on the desk").into(),
            };
            tab_row(
                page,
                picture,
                place,
                at > 0,
                (&looks.agent, &looks.bench),
                cx,
            )
        })
        .collect();
    tab_frame(rows, cx)
}

/// A page one of the agent's workers showed, as the row a page in a tab gets, named as the
/// worker's; Open brings it forward on the worker's desk.
pub(super) fn worker_page(
    page: &WorkerPage,
    looks: &Looks,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let page_ref = PageRef {
        id: page.id.clone(),
        title: page.title.clone(),
    };
    let picture = shown(&page_ref, looks, window, cx).picture.clone();
    let place = t!("@{name}'s page", name = page.name).into();
    let row = tab_row(
        &page_ref,
        picture,
        place,
        false,
        (&page.agent_id, &looks.bench),
        cx,
    );
    tab_frame(vec![row], cx)
}

/// The shared frame page rows sit in.
pub(crate) fn tab_frame(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .overflow_hidden()
        .border(crate::theme::border())
        .border_color(t.border)
        .bg(t.surface_raised)
        .children(rows)
        .into_any_element()
}

/// A page's row: the top of the page, its title over `place`, and Open, which brings its tab
/// forward on `agent`'s desk, as a click anywhere on the row does.
pub(crate) fn tab_row(
    page: &PageRef,
    picture: Option<Arc<Image>>,
    place: SharedString,
    below_another: bool,
    (agent, bench): (&str, &gpui_kit::WeakEntity<crate::workbench::Workbench>),
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let thumb = div()
        .flex_none()
        .w(px(THUMB_WIDTH))
        .h(px(THUMB_WIDTH / THUMB_KEPT_SHAPE))
        .rounded(px(6.))
        .overflow_hidden()
        .border(crate::theme::border())
        .border_color(t.border)
        .bg(t.surface)
        .flex()
        .items_center()
        .justify_center()
        .child(match picture {
            Some(picture) => img(picture)
                .size_full()
                .rounded(px(6.))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => ui::icon(Icon::Tabs, 16.)
                .text_color(t.text_tertiary)
                .into_any_element(),
        });
    let place = ui::meta(place, cx).truncate();
    div()
        .id(SharedString::from(format!("page-card-{}", page.id)))
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(14.))
        .pl(px(8.))
        .pr(px(14.))
        .when(below_another, |row| {
            row.border_t(crate::theme::border()).border_color(t.border)
        })
        .hover(|row| row.bg(t.surface_hover))
        .clicks(opens_tab(page, agent, bench))
        .children(ui::spot(|| format!("page-card-{}", page.id)))
        .child(thumb)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(ui::text(page.title.clone(), 14., FontWeight::MEDIUM).truncate())
                .child(place),
        )
        .child(
            ui::Button::new(
                SharedString::from(format!("page-card-open-{}", page.id)),
                t!("Open"),
                ui::Variant::Outline,
            )
            .icon(Icon::Tabs)
            .sized(24., 10., 11.5)
            .on_click(opens_tab(page, agent, bench)),
        )
        .into_any_element()
}

fn opens_tab(
    page: &PageRef,
    agent: &str,
    bench: &gpui_kit::WeakEntity<crate::workbench::Workbench>,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
    let (id, agent, bench) = (page.id.clone(), agent.to_owned(), bench.clone());
    move |_, _, cx| {
        let _ = bench.update(cx, |bench, cx| bench.open_page_in_tab(&agent, &id, cx));
    }
}

/// The top of a desk tab's picture, as its row in the reply shows it, in base64.
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

/// Asks the core for the picture of the page's tab.
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
    let (id, reader) = (page.id.clone(), looks.reader.clone());
    window
        .spawn(cx, async move |cx| {
            let answer = answer.await;
            let _ = cx.update(|_, cx| {
                if let Some(shown) = cx.global_mut::<Pages>().shown.get_mut(&id) {
                    match answer {
                        Ok(answer) => {
                            shown.picture = answer
                                .picture
                                .and_then(|picture| crate::picture::decode("image/jpeg", &picture));
                        }
                        Err(error) => shown.failed = Some(error.into()),
                    }
                }
                reader.update(cx, |_, cx| cx.notify());
            });
        })
        .detach();
}
