//! What finished work shows in a chat: the core's pictures of the page a turn changed, before
//! and after at a phone's, a tablet's and a desktop's width under the reply that ended it.

use crate::ui::Clicks;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, App, Global, Image, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, SharedString, Styled, StyledImage, WeakEntity,
};
use sikemux_calls::call;
use sikemux_calls::pictures::TurnPictureParams;
use sikemux_calls::turn_state::TakeStepParams;
use sikemux_view::shown::{PageShown, Picture as Shown, TurnShown, WidthShown};
use sikemux_view::turn::TurnStep;

use super::{ChatRow, Looks};
use crate::picture::Picture;
use crate::theme::{Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3};
use crate::ui;
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
/// How many fetched pictures are kept before all are let go and fetched again as needed.
const FETCHED_KEPT: usize = 240;
const RETRY_AFTER: Duration = Duration::from_secs(30);

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
        .border(crate::theme::border())
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
                .clicks(move |_, window, cx| {
                    bench
                        .update(cx, |bench, cx| {
                            bench.show_picture(shown.clone(), window, cx)
                        })
                        .ok();
                })
        })
        .into_any_element()
}

/// The pictures fetched from the core, by id, bounded by LRU eviction.
#[derive(Default)]
struct Fetched {
    entries: HashMap<String, Option<Arc<Image>>>,
    order: VecDeque<String>,
    loading: HashSet<String>,
    failed: HashMap<String, Instant>,
}

impl Fetched {
    fn get(&mut self, id: &str) -> Option<Option<Arc<Image>>> {
        let value = self.entries.get(id)?.clone();
        self.order.retain(|kept| kept != id);
        self.order.push_back(id.to_owned());
        Some(value)
    }

    fn insert(&mut self, id: String, value: Option<Arc<Image>>) {
        self.loading.remove(&id);
        self.failed.remove(&id);
        self.order.retain(|kept| kept != &id);
        while self.entries.len() >= FETCHED_KEPT {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.order.push_back(id.clone());
        self.entries.insert(id, value);
    }
}

impl Global for Fetched {}

/// The picture `id` names, once it has come from the core.
pub(crate) fn turn_picture_held(id: &str, cx: &App) -> Option<Arc<Image>> {
    cx.try_global::<Fetched>()?.entries.get(id)?.clone()
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
    let fetched = cx.global_mut::<Fetched>();
    if let Some(known) = fetched.get(id) {
        return known;
    }
    if fetched.loading.contains(id)
        || fetched
            .failed
            .get(id)
            .is_some_and(|at| at.elapsed() < RETRY_AFTER)
    {
        return None;
    }
    fetched.loading.insert(id.to_owned());
    let target = bench.upgrade()?.read(cx).chat_target(agent);
    let asked = target.request_typed::<call::TurnPicture>(TurnPictureParams { id: id.to_owned() });
    let id = id.to_owned();
    cx.spawn(async move |cx| {
        let Ok(answer) = asked.await else {
            cx.update(|cx| {
                let fetched = cx.global_mut::<Fetched>();
                fetched.loading.remove(&id);
                fetched.failed.insert(id, Instant::now());
            });
            return;
        };
        let image = cx
            .background_executor()
            .spawn(async move { crate::picture::decode(&answer.mime_type, &answer.data) })
            .await;
        cx.update(|cx| {
            cx.global_mut::<Fetched>().insert(id, image);
            redraw(cx);
        });
    })
    .detach();
    None
}

/// Takes `step` on `agent`'s work, as the person clicked it, saying so when the core refuses.
pub(crate) fn take(
    bench: &gpui_kit::WeakEntity<crate::workbench::Workbench>,
    agent: &str,
    step: TurnStep,
    cx: &mut App,
) -> gpui_kit::Task<bool> {
    let Some(workbench) = bench.upgrade() else {
        return gpui_kit::Task::ready(false);
    };
    let target = workbench.read(cx).chat_target(agent);
    let asked = target.request_typed::<call::TakeStep>(TakeStepParams {
        agent_id: agent.to_owned(),
        step,
    });
    let label = crate::turn_line::step_label(step).unwrap_or_default();
    let bench = bench.clone();
    cx.spawn(async move |cx| {
        let (taken, note) = match asked.await {
            Ok(_) => (
                true,
                crate::toast::info(format!("{label}: sent to the agent")),
            ),
            Err(error) => (false, crate::toast::error(error.to_string())),
        };
        cx.update(|cx| crate::toast::show(&bench, note, cx));
        taken
    })
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
    let before = (rows.iter().take(ix).rev())
        .find_map(|row| row.message.ended_at)
        .unwrap_or_default();
    (shown.iter())
        .filter(|turn| turn.turn > before && turn.turn <= end)
        .collect()
}
