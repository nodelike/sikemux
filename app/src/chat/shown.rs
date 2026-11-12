//! What finished work shows in a chat: the core's pictures of the page a turn changed, before
//! and after at a phone's, a tablet's and a desktop's width under the reply that ended it, and in
//! a lead's chat a row for each worker's finished work with Try it and Undo.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, App, FontWeight, Global, Image, InteractiveElement, IntoElement,
    ObjectFit, ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    WeakEntity,
};
use sikemux_core::server::call;
use sikemux_core::server::pictures::TurnPictureParams;
use sikemux_core::server::turn_state::TakeStepParams;
use sikemux_view::shown::{PageShown, Picture as Shown, ResultRow, TurnShown, WidthShown};
use sikemux_view::turn::{TurnPhase, TurnState, TurnStep};

use super::{ChatRow, Looks};
use crate::icons::Icon;
use crate::picture::Picture;
use crate::theme::{self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_5, TEXT_XS_H};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// How each width's pair is drawn: its name and each picture's size, from the product mockup.
/// The page each width lays out at is this much taller than wide, so a picture keeps its shape
/// and is cut from the top.
fn tallness(width: u32) -> f32 {
    match width {
        390 => 844. / 390.,
        820 => 1180. / 820.,
        _ => 900. / 1440.,
    }
}

const WIDTHS: [(u32, &str, f32, f32); 3] = [
    (390, n!("Phone"), 80., 150.),
    (820, n!("Tablet"), 112., 116.),
    (1440, n!("Desktop"), 150., 94.),
];
const PAIR_GAP: f32 = 8.;
const ROW_HEIGHT: f32 = 30.;
/// How many fetched pictures are kept before all are let go and fetched again as needed.
const FETCHED_KEPT: usize = 240;

/// The pictures of `pages`, each width's before and after side by side under its name.
pub(super) fn pictures(pages: &[PageShown], key: &str, looks: &Looks, cx: &mut App) -> AnyElement {
    let mut drawn = Vec::new();
    for page in pages {
        let mut pairs = Vec::new();
        for &(width, name, w, h) in &WIDTHS {
            if let Some(shown) = page.widths.iter().find(|shown| shown.width == width) {
                let key = format!("{key}:{}:{width}", page.page);
                pairs.push(pair(shown, name, w, h, key, looks, cx));
            }
        }
        drawn.push(
            div()
                .flex()
                .flex_wrap()
                .items_start()
                .gap(px(18.))
                .children(pairs),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .pt(SPACE_2)
        .pb(SPACE_1)
        .children(drawn)
        .into_any_element()
}

fn pair(
    shown: &WidthShown,
    name: &str,
    width: f32,
    height: f32,
    key: String,
    looks: &Looks,
    cx: &mut App,
) -> AnyElement {
    let pictures: Vec<(&str, &Shown)> = [
        (n!("Before"), shown.before.as_ref()),
        (n!("After"), Some(&shown.after)),
    ]
    .into_iter()
    .filter_map(|(word, picture)| Some((word, picture?)))
    .collect();
    let frames: Vec<AnyElement> = (pictures.iter())
        .map(|(word, picture)| {
            let key = format!("{key}:{word}");
            framed(picture, shown.width, width, height, key, looks, cx)
        })
        .collect();
    let t = Tokens::get(cx);
    let caption = |word: &str| ui::caps(crate::i18n::tr(word), cx).w(px(width)).flex_none();
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(ui::caps(crate::i18n::tr(name), cx).text_color(t.text_secondary))
        .child(div().flex().gap(px(PAIR_GAP)).children(frames))
        .child(
            div()
                .flex()
                .gap(px(PAIR_GAP))
                .children(pictures.iter().map(|(word, _)| caption(word))),
        )
        .into_any_element()
}

/// One picture, taken at `picture_width`, cut to `width` by `height` from its top, opening full
/// size on a click; an empty frame while it is fetched.
fn framed(
    picture: &Shown,
    picture_width: u32,
    width: f32,
    height: f32,
    key: String,
    looks: &Looks,
    cx: &mut App,
) -> AnyElement {
    let reader = looks.reader.clone();
    let redraw = move |cx: &mut App| reader.update(cx, |_, cx| cx.notify());
    let image = turn_picture(&picture.id, &looks.agent, &looks.bench, cx, redraw);
    let t = Tokens::get(cx);
    let bench = looks.bench.clone();
    let name = SharedString::from(picture.id.clone());
    div()
        .id(SharedString::from(key))
        .w(px(width))
        .h(px(height))
        .flex_none()
        .overflow_hidden()
        .rounded(px(6.))
        .border_1()
        .border_color(t.border)
        .bg(t.surface_raised)
        .cursor_pointer()
        .hover(move |frame| frame.border_color(t.border_strong))
        .when_some(image, |frame, image| {
            let shown = Picture::Sent { name, image };
            let source = shown.source_at(px(width.max(height)));
            frame
                .child(
                    img(source)
                        .w(px(width))
                        .h(px(width * tallness(picture_width)))
                        .flex_none()
                        .object_fit(ObjectFit::Fill),
                )
                .on_click(move |_, window, cx| {
                    bench
                        .update(cx, |bench, cx| {
                            bench.show_picture(shown.clone(), window, cx)
                        })
                        .ok();
                })
        })
        .into_any_element()
}

/// The pictures fetched from the core, by id: none while one is on its way or did not come.
#[derive(Default)]
struct Fetched(HashMap<String, Option<Arc<Image>>>);

impl Global for Fetched {}

/// The picture `id` names, once it has come from the core.
pub(crate) fn turn_picture_held(id: &str, cx: &App) -> Option<Arc<Image>> {
    cx.try_global::<Fetched>()?.0.get(id)?.clone()
}

/// The picture `id` names, of a turn of `agent`'s, asking the core for it the first time and
/// calling `redraw` once it came.
pub(crate) fn turn_picture(
    id: &str,
    agent: &str,
    bench: &WeakEntity<Workbench>,
    cx: &mut App,
    redraw: impl FnOnce(&mut App) + 'static,
) -> Option<Arc<Image>> {
    if !cx.has_global::<Fetched>() {
        cx.set_global(Fetched::default());
    }
    if let Some(known) = cx.global::<Fetched>().0.get(id) {
        return known.clone();
    }
    let kept = &mut cx.global_mut::<Fetched>().0;
    if kept.len() >= FETCHED_KEPT {
        kept.clear();
    }
    kept.insert(id.to_owned(), None);
    let target = bench.upgrade()?.read(cx).chat_target(agent);
    let asked = target.request_typed::<call::TurnPicture>(TurnPictureParams { id: id.to_owned() });
    let id = id.to_owned();
    cx.spawn(async move |cx| {
        let Ok(answer) = asked.await else { return };
        let image = crate::picture::decode(&answer.mime_type, &answer.data);
        cx.update(|cx| {
            if let Some(kept) = cx.global_mut::<Fetched>().0.get_mut(&id) {
                *kept = image;
            }
            redraw(cx);
        });
    })
    .detach();
    None
}

/// A worker's finished work: its mark and name, what it did, where its work is and how its
/// checks stand, Try it and Undo, and a chevron that opens the worker's own chat. Its pages'
/// pictures sit under it.
pub(super) fn result_row(row: &ResultRow, looks: &Looks, cx: &mut App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = format!("result:{}:{}", row.agent_id, row.at);
    let worker = looks
        .bench
        .upgrade()
        .and_then(|bench| bench.read(cx).agent_by_id(&row.agent_id));
    let mark = match &worker {
        Some(worker) => worker.mark(17., cx),
        None => ui::mark(&looks.provider, 17.),
    };
    let maker = crate::turn_line::maker(
        worker
            .as_ref()
            .map_or(&looks.provider, |worker| &worker.provider),
    );
    let turn = TurnState {
        state: TurnPhase::Done,
        since: row.at,
        on: None,
        wait: None,
        call: None,
        done: Some(row.done.clone()),
        stopped: None,
        failed: None,
    };
    let facts = crate::turn_line::said(&turn, maker, ui::time::now_ms())
        .map(|said| said.parts.join(" · "))
        .filter(|facts| !facts.is_empty());
    let tries = row
        .done
        .next
        .iter()
        .any(|next| next.step == TurnStep::TryIt);
    let step = |step: TurnStep, variant: Variant| {
        let label = crate::turn_line::step_label(step).unwrap_or_default();
        let (bench, agent) = (looks.bench.clone(), row.agent_id.clone());
        Button::new(SharedString::from(format!("{key}:{label}")), label, variant)
            .sized(22., 10., 12.)
            .on_click(move |_, _, cx| take(&bench, &agent, step, cx))
    };
    let name = row
        .name
        .as_ref()
        .map_or_else(|| row.title.clone(), |name| format!("@{name}"));
    let (bench, worker_id) = (looks.bench.clone(), row.agent_id.clone());
    let summary = div()
        .id(SharedString::from(key.clone()))
        .h(px(ROW_HEIGHT))
        .pl(SPACE_3)
        .pr(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_3)
        .bg(t.surface_raised)
        .text_size(theme::ui_text(TEXT_XS_H))
        .child(div().flex_none().child(mark))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(looks.ink)
                .child(name),
        )
        .when(row.name.is_some(), |summary| {
            summary.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(looks.quiet)
                    .child(row.title.clone()),
            )
        })
        .children(facts.map(|said| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(div().size(px(7.)).rounded_full().bg(t.accent))
                .child(div().text_color(looks.quiet).child(said))
        }))
        .child(div().flex_1())
        .when(tries, |summary| {
            summary.child(step(TurnStep::TryIt, Variant::Primary))
        })
        .when(row.undo, |summary| {
            summary.child(step(TurnStep::Undo, Variant::Ghost))
        })
        .child(
            div()
                .id(SharedString::from(format!("{key}:open")))
                .flex_none()
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_2)
                .cursor_pointer()
                .hover(move |chevron| chevron.bg(t.surface_active))
                .child(ui::icon(Icon::Chevron, 12.).text_color(looks.quiet))
                .on_click(move |_, _, cx| {
                    bench
                        .update(cx, |bench, cx| bench.open_agent(&worker_id, cx))
                        .ok();
                }),
        );
    div()
        .w_full()
        .pt(SPACE_2)
        .child(summary)
        .when(!row.pages.is_empty(), |result| {
            result.child(
                div()
                    .pl(SPACE_3)
                    .child(pictures(&row.pages, &key, looks, cx)),
            )
        })
        .pb(if row.pages.is_empty() {
            SPACE_1
        } else {
            SPACE_5
        })
        .into_any_element()
}

/// Takes `step` on `agent`'s work, as the person clicked it, saying so when the core refuses.
pub(crate) fn take(
    bench: &gpui_kit::WeakEntity<crate::workbench::Workbench>,
    agent: &str,
    step: TurnStep,
    cx: &mut App,
) {
    let Some(bench) = bench.upgrade() else {
        return;
    };
    let target = bench.read(cx).chat_target(agent);
    let asked = target.request_typed::<call::TakeStep>(TakeStepParams {
        agent_id: agent.to_owned(),
        step,
    });
    let bench = bench.downgrade();
    cx.spawn(async move |cx| {
        if let Err(error) = asked.await {
            bench
                .update(cx, |bench, cx| {
                    bench.notify(crate::toast::error(error.to_string()), cx)
                })
                .ok();
        }
    })
    .detach();
}

/// When a row happened: when its turn ended, or when the person sent it.
fn time_of(row: &ChatRow) -> Option<u64> {
    row.message.ended_at.or(row.message.sent_at)
}

/// The turns whose pictures go under row `ix`: those its reply ended.
pub(super) fn ended_at<'a>(
    rows: &[ChatRow],
    ix: usize,
    shown: &'a [TurnShown],
) -> Vec<&'a TurnShown> {
    let Some(end) = rows.get(ix).and_then(|row| row.message.ended_at) else {
        return Vec::new();
    };
    let before = (rows[..ix].iter().rev())
        .find_map(|row| row.message.ended_at)
        .unwrap_or_default();
    (shown.iter())
        .filter(|turn| turn.turn > before && turn.turn <= end)
        .collect()
}

/// The workers' finished work that goes after row `ix`: what finished after it and before the
/// next row that has a time.
pub(super) fn results_after<'a>(
    rows: &[ChatRow],
    ix: usize,
    results: &'a [ResultRow],
) -> Vec<&'a ResultRow> {
    let Some(at) = rows.get(ix).and_then(time_of) else {
        return Vec::new();
    };
    let next = rows
        .get(ix + 1..)
        .and_then(|rest| rest.iter().find_map(time_of));
    (results.iter())
        .filter(|row| row.at >= at && next.is_none_or(|next| row.at < next))
        .collect()
}
