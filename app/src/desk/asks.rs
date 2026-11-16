//! The Asks board pinned on a chief's desk: each thing the person asked in its project as one
//! card, set in four lanes by how far it has gone. The core says where each ask stands and which
//! agents wait on the person; this only sorts those into lanes and draws them.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, App, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    ObjectFit, ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    WeakEntity,
};
use std::path::PathBuf;

use sikemux_view::now::{AskStand, NowAsk, NowGroup, ProjectNow};

use crate::icons::Icon;
use crate::picture::Picture;
use crate::theme::{
    self, radius_2, radius_3, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_3H, SPACE_6,
    TEXT_2XS, TEXT_XS,
};
use crate::ui::{self, Clicks};
use crate::workbench::Workbench;

const THUMB_H: f32 = 132.;
const FOOT_H: f32 = 36.;
const LANE_MIN: f32 = 232.;
const EMPTY_LANE: f32 = 132.;

#[derive(Clone, Copy, PartialEq)]
enum Lane {
    Nobody,
    AsksYou,
    Moving,
    Done,
}

impl Lane {
    const ALL: [Lane; 4] = [Lane::Nobody, Lane::AsksYou, Lane::Moving, Lane::Done];

    fn of(ask: &NowAsk, now: &ProjectNow) -> Option<Lane> {
        match &ask.stand {
            AskStand::Nobody => Some(Lane::Nobody),
            AskStand::Working { agents } if agents.iter().any(|agent| waits(now, agent)) => {
                Some(Lane::AsksYou)
            }
            AskStand::Working { .. } => Some(Lane::Moving),
            AskStand::OnBranch | AskStand::InYourBuild => Some(Lane::Done),
            AskStand::Unknown => None,
        }
    }

    fn words(self) -> &'static str {
        match self {
            Lane::Nobody => t!("Nobody on it"),
            Lane::AsksYou => t!("Asks you"),
            Lane::Moving => t!("Moving"),
            Lane::Done => t!("Done"),
        }
    }
}

fn waits(now: &ProjectNow, agent: &str) -> bool {
    (now.rows.iter()).any(|row| row.agent == agent && row.group == NowGroup::WaitingOnYou)
}

/// The board for `chief`'s project, filling the desk below its tabs.
pub(super) fn board(bench: &Workbench, chief: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let now = (bench.agent_by_id(chief))
        .and_then(|chief| {
            let view = bench.view_at(&bench.place())?;
            view.now
                .iter()
                .find(|now| now.project == chief.project)
                .cloned()
        })
        .unwrap_or_else(|| ProjectNow {
            project: String::new(),
            rows: Vec::new(),
            release: None,
            closed: Vec::new(),
            asks: Vec::new(),
        });
    let lanes = Lane::ALL.map(|lane| {
        let asks: Vec<&NowAsk> = (now.asks.iter())
            .filter(|ask| Lane::of(ask, &now) == Some(lane))
            .collect();
        (lane, asks)
    });
    let handle = cx.weak_entity();
    let lane_w = |asks: &[&NowAsk]| match asks.is_empty() {
        true => EMPTY_LANE,
        false => LANE_MIN,
    };
    let min_w = (lanes.iter()).map(|(_, asks)| lane_w(asks)).sum::<f32>();
    let body = match now.asks.is_empty() {
        true => div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(ui::meta(t!("Nothing you asked is open"), cx))
            .into_any_element(),
        false => div()
            .min_w(px(min_w) + SPACE_3H * 3. + SPACE_6 * 2.)
            .flex()
            .gap(SPACE_3H)
            .px(SPACE_6)
            .pt(SPACE_3)
            .pb(SPACE_6)
            .children(lanes.iter().map(|(lane, asks)| {
                div()
                    .map(|column| match asks.is_empty() {
                        true => column.flex_none().w(px(EMPTY_LANE)),
                        false => column.flex_1().min_w(px(LANE_MIN)),
                    })
                    .flex()
                    .flex_col()
                    .gap(SPACE_2)
                    .child(lane_label(lane.words(), asks.len(), cx))
                    .children(
                        asks.iter()
                            .map(|ask| card(bench, ask, chief, *lane, &handle, cx)),
                    )
            }))
            .into_any_element(),
    };
    div()
        .id(SharedString::from(format!("asks-board-{chief}")))
        .size_full()
        .flex()
        .flex_col()
        .overflow_scroll()
        .child(head(&lanes, &t))
        .child(body)
        .into_any_element()
}

/// The page's name, then how many stand in each lane.
fn head(lanes: &[(Lane, Vec<&NowAsk>); 4], t: &Tokens) -> Div {
    let tint = |lane: Lane| match lane {
        Lane::Nobody => t.warn,
        Lane::AsksYou => t.accent,
        Lane::Moving | Lane::Done => t.text_secondary,
    };
    let counts = (lanes.iter())
        .filter(|(_, asks)| !asks.is_empty())
        .enumerate();
    div()
        .flex()
        .items_baseline()
        .gap(SPACE_3H)
        .px(SPACE_6)
        .pt(px(22.))
        .pb(SPACE_1H)
        .child(ui::text(t!("Asks"), 18., FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(5.))
                .text_size(theme::ui_text(12.5))
                .text_color(t.text_tertiary)
                .children(counts.flat_map(|(at, (lane, asks))| {
                    let dot = (at > 0).then(|| div().child("·").into_any_element());
                    let count = ui::text(asks.len().to_string(), 12.5, FontWeight::MEDIUM)
                        .font_features(theme::tabular())
                        .text_color(tint(*lane))
                        .into_any_element();
                    let words = div().child(lane.words().to_lowercase()).into_any_element();
                    dot.into_iter().chain([count, words])
                })),
        )
}

/// A lane's spaced-capitals name, its count and a hairline to the lane's edge.
fn lane_label(words: &str, count: usize, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .h(px(22.))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::caps(words, cx).flex_none())
        .child(
            ui::text(count.to_string(), TEXT_2XS, FontWeight::MEDIUM)
                .font_features(theme::tabular())
                .text_color(t.text_disabled),
        )
        .child(div().flex_1().h(px(1.)).bg(t.line_soft))
}

/// One ask: what the person wrote, the first picture they sent with it and how many more, then
/// the agents on it, how far it has gone and since when. A click opens it.
fn card(
    bench: &Workbench,
    ask: &NowAsk,
    chief: &str,
    lane: Lane,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let link = format!("sikemux://ask/{}", ask.id);
    let open = {
        let handle = handle.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            let _ = handle.update(cx, |bench, cx| crate::links::follow(bench, &link, cx));
        }
    };
    let agents: Vec<AnyElement> = match &ask.stand {
        AskStand::Working { agents } => (agents.iter())
            .filter_map(|id| bench.agent_by_id(id))
            .map(|agent| crate::chat::agent_pill(&agent, 12., handle, cx).into_any_element())
            .collect(),
        _ => Vec::new(),
    };
    div()
        .id(SharedString::from(format!("ask-card-{}", ask.id)))
        .flex()
        .flex_col()
        .rounded(radius_3())
        .bg(t.surface_raised)
        .border_1()
        .border_color(t.border)
        .overflow_hidden()
        .cursor_pointer()
        .hover(move |card| card.border_color(t.border_strong))
        .clicks(open)
        .child(
            div()
                .px(SPACE_3)
                .pt(px(11.))
                .pb(SPACE_2)
                .text_size(theme::ui_text(13.5))
                .font_weight(FontWeight::MEDIUM)
                .line_height(px(19.))
                .text_color(t.text_primary)
                .line_clamp(2)
                .child(ask.title.clone()),
        )
        .children(thumbnail(ask, &t))
        .children(links(bench, ask, chief, handle, cx))
        .child(
            div()
                .h(px(FOOT_H))
                .mt(px(2.))
                .px(SPACE_3)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .border_t_1()
                .border_color(t.line_soft)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(SPACE_1)
                        .overflow_hidden()
                        .children(agents),
                )
                .child(stand(ask, lane, &t))
                .child(
                    ui::text(ui::time::age(ask.since), 11.5, FontWeight::NORMAL)
                        .font_features(theme::tabular())
                        .text_color(t.text_disabled),
                ),
        )
        .into_any_element()
}

/// The first picture sent with the ask across the card, and how many more came with it.
fn thumbnail(ask: &NowAsk, t: &Tokens) -> Option<Div> {
    let first = ask.images.first()?;
    let more = ask.images.len() - 1;
    let picture = Picture::File(first.into()).source_at(px(THUMB_H * 2.));
    Some(
        div().px(SPACE_1H).child(
            div()
                .relative()
                .h(px(THUMB_H))
                .rounded(radius_2())
                .overflow_hidden()
                .bg(t.surface_sunken)
                .child(img(picture).size_full().object_fit(ObjectFit::Cover))
                .when(more > 0, |shot| {
                    shot.child(
                        div()
                            .absolute()
                            .right(SPACE_1H)
                            .bottom(SPACE_1H)
                            .h(px(18.))
                            .px(SPACE_1H)
                            .rounded(radius_2())
                            .flex()
                            .items_center()
                            .bg(t.surface_overlay.opacity(0.85))
                            .border_1()
                            .border_color(t.border)
                            .child(
                                ui::text(format!("+{more}"), 11., FontWeight::SEMIBOLD)
                                    .font_features(theme::tabular())
                                    .text_color(t.text_primary),
                            ),
                    )
                }),
        ),
    )
}

enum Link {
    Page(String),
    File(PathBuf),
}

/// The pages and files the ask's agent opened on its desk while on it, the first two as chips
/// that open on `chief`'s desk with one click, and how many more.
fn links(
    bench: &Workbench,
    ask: &NowAsk,
    chief: &str,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> Option<Div> {
    const SHOWN: usize = 2;
    let t = Tokens::get(cx);
    let count = ask.pages.len() + ask.files.len();
    if count == 0 {
        return None;
    }
    let opener = |link: Link| {
        let (handle, chief) = (handle.clone(), chief.to_owned());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            cx.stop_propagation();
            let _ = handle.update(cx, |bench, cx| match &link {
                Link::Page(url) => {
                    bench.open_on_desk(&chief, url, cx);
                }
                Link::File(path) => bench.open_file_on_desk(&chief, path.clone(), None, None, cx),
            });
        }
    };
    let pages = ask.pages.iter().map(|page| {
        let chip = crate::chat::context_chip(
            format!("ask-{}-page-{}", ask.id, page.id),
            &page.url,
            &page.title,
            Some(&bench.asks),
            t,
        );
        (chip, opener(Link::Page(page.url.clone())))
    });
    let files = ask.files.iter().map(|path| {
        let path = PathBuf::from(path);
        let id = format!("ask-{}-file-{}", ask.id, path.display());
        let chip = crate::chat::file_chip(id, &path, t, cx);
        (chip, opener(Link::File(path)))
    });
    let chips = (pages.chain(files).take(SHOWN).enumerate()).map(|(at, (chip, open))| {
        div()
            .id(SharedString::from(format!("ask-{}-link-{at}", ask.id)))
            .min_w_0()
            .clicks(open)
            .child(chip)
    });
    let more = count.saturating_sub(SHOWN);
    Some(
        div()
            .px(SPACE_3)
            .pt(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .overflow_hidden()
            .children(chips)
            .when(more > 0, |row| {
                row.child(
                    ui::text(format!("+{more}"), TEXT_XS, FontWeight::MEDIUM)
                        .flex_none()
                        .font_features(theme::tabular())
                        .text_color(t.text_tertiary),
                )
            }),
    )
}

/// How far the ask has gone, in its lane's words and tint.
fn stand(ask: &NowAsk, lane: Lane, t: &Tokens) -> Div {
    let (icon, words, tint, weight): (Option<AnyElement>, &str, Hsla, FontWeight) =
        match (&ask.stand, lane) {
            (_, Lane::Nobody) => (None, t!("Nobody on it"), t.warn, FontWeight::MEDIUM),
            (_, Lane::AsksYou) => (None, t!("Asks you"), t.accent, FontWeight::MEDIUM),
            (AskStand::InYourBuild, _) => (
                Some(
                    ui::icon(Icon::Check, 12.)
                        .text_color(t.live)
                        .into_any_element(),
                ),
                t!("In your build"),
                t.live,
                FontWeight::NORMAL,
            ),
            (AskStand::OnBranch, _) => (
                None,
                t!("Not in your build yet"),
                t.text_tertiary,
                FontWeight::NORMAL,
            ),
            _ => (
                Some(ui::working(11., t.accent)),
                t!("Working"),
                t.text_secondary,
                FontWeight::NORMAL,
            ),
        };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.))
        .children(icon)
        .child(ui::text(words, TEXT_XS, weight).text_color(tint))
}
