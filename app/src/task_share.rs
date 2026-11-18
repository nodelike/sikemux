//! The Tasks view's Share button: who the project's board is shared with, picked from the orgs
//! whose Team plan is paid.

use gpui_kit::component::menu::PopupMenuItem;
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    div, px, Anchor, AnyElement, App, Context, FontWeight, IntoElement, ParentElement,
    SharedString, Styled,
};
use sikemux_calls::board::{BoardParams, ShareProjectParams};
use sikemux_calls::call;
use sikemux_view::teammates::{ProjectSharing, Teammate};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::Ask;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

fn sharing(bench: &Workbench) -> Option<&ProjectSharing> {
    let sharing = bench.feeds.at(&bench.place())?.sharing.as_ref()?;
    let folder = bench.project()?.path.to_string_lossy();
    (sharing.project == folder && (sharing.shared.is_some() || !sharing.orgs.is_empty()))
        .then_some(sharing)
}

fn people(count: usize) -> String {
    match count {
        1 => t!("1 person").to_owned(),
        count => t!("{count} people", count = count),
    }
}

/// What the button can say, longest first; with no room for the last it shows its icon alone.
fn labels(sharing: &ProjectSharing) -> Vec<SharedString> {
    match &sharing.shared {
        None => vec![t!("Share").into()],
        Some(shared) if !shared.on => vec![t!("{org} sharing paused", org = shared.name).into()],
        Some(shared) => vec![
            t!(
                "Shared with {org} · {people}",
                org = shared.name,
                people = people(shared.people.len())
            )
            .into(),
            t!("Shared with {org}", org = shared.name).into(),
            shared.name.clone().into(),
        ],
    }
}

/// The button, while the board is shared or there is an org to share it with, at the right of
/// agent `agent`'s Tasks bar, its label shortened to the room the counts leave it.
pub(crate) fn button(
    bench: &Workbench,
    agent: &str,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let sharing = sharing(bench)?.clone();
    let labels = labels(&sharing);
    let fit = bench.desks.update(cx, |desks, _| {
        (desks
            .task_views
            .entry(agent.to_owned())
            .or_default()
            .share_fit)
            .clone()
    });
    let shown = fit.get().min(labels.len());
    let handle = cx.weak_entity();
    let asks = bench.asks.clone();
    let t = Tokens::get(cx);
    let label = (labels.get(shown).or(labels.first()))
        .cloned()
        .unwrap_or_default();
    let trigger = ui::Button::new("tasks-share-button", label, ui::Variant::Ghost)
        .icon(Icon::Share)
        .icon_only(shown == labels.len())
        .solid(t.surface_raised, t.surface_hover)
        .sized(24., 8., theme::TEXT_XS_H);
    let menu = ui::menu_button(
        "tasks-share",
        Anchor::TopRight,
        trigger,
        move |mut popup, _, cx| {
            let choose = |org: Option<String>| {
                let handle = handle.clone();
                move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
                    let org = org.clone();
                    let _ = handle.update(cx, |bench, cx| share(bench, org, cx));
                }
            };
            match &sharing.shared {
                Some(shared) => {
                    popup = popup.label(match shared.on {
                        true => t!("Shared with {org}", org = shared.name),
                        false => t!("{org} sharing paused", org = shared.name),
                    });
                    for person in &shared.people {
                        let (person, asks) = (person.clone(), asks.clone());
                        popup = popup.item(
                            PopupMenuItem::element(move |_, cx| person_row(&person, &asks, cx))
                                .disabled(true),
                        );
                    }
                    popup = popup.separator().item(
                        ui::MenuRow::new(t!("Stop sharing"))
                            .icon(Icon::Close)
                            .item(cx)
                            .on_click(choose(None)),
                    );
                }
                None => {
                    popup = popup.label(t!("Share this board with")).item(
                        ui::MenuRow::new(t!("Only you"))
                            .item(cx)
                            .checked(true)
                            .on_click(choose(None)),
                    );
                    for org in &sharing.orgs {
                        popup = popup.item(
                            ui::MenuRow::new(org.name.clone())
                                .hint(people(org.people as usize))
                                .item(cx)
                                .on_click(choose(Some(org.org.clone()))),
                        );
                    }
                }
            }
            popup.item(PopupMenuItem::element(|_, cx| footnote(cx)).disabled(true))
        },
    );
    let drawn = Rc::new(Cell::new(0.));
    let told = drawn.clone();
    let right = div()
        .w_full()
        .flex()
        .justify_end()
        .child(menu)
        .on_children_prepainted(move |bounds, _, _| {
            told.set(bounds.first().map_or(0., |button| button.size.width.into()));
        });
    let room = div()
        .flex_1()
        .min_w_0()
        .pl(theme::SPACE_3)
        .child(right)
        .on_children_prepainted(move |bounds, window, cx| {
            let Some(room) = bounds.first().map(|right| f32::from(right.size.width)) else {
                return;
            };
            let mut width = |label: &SharedString| -> f32 {
                ui::text_width(label, theme::TEXT_XS_H, FontWeight::MEDIUM, window, cx).into()
            };
            let around = match labels.get(shown) {
                Some(label) => drawn.get() - width(label),
                None => drawn.get() + f32::from(theme::SPACE_2),
            };
            let best = (labels.iter())
                .position(|label| around + width(label) <= room)
                .unwrap_or(labels.len());
            if best != fit.get() {
                fit.set(best);
                ui::redraw_after_paint(window, cx);
            }
        });
    Some(room.into_any_element())
}

fn person_row(person: &Teammate, asks: &UnboundedSender<Ask>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let name = match person.you {
        true => t!("{name} (you)", name = person.name),
        false => person.name.clone(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_color(t.text_primary)
        .child(crate::task_list::face(person, 16., asks, cx))
        .child(name)
        .into_any_element()
}

fn footnote(cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .max_w(px(260.))
        .py(px(4.))
        .whitespace_normal()
        .text_size(theme::ui_text(theme::TEXT_2XS_H))
        .text_color(t.text_tertiary)
        .child(t!(
            "Teammates read the tasks; their agents can ask yours about one. Nobody else's project is shared by this."
        ))
        .into_any_element()
}

fn share(bench: &mut Workbench, org: Option<String>, cx: &mut Context<Workbench>) {
    let Some(project) = bench.project() else {
        return;
    };
    let project = project.path.to_string_lossy().into_owned();
    let shared = org.is_some();
    let target = bench.target();
    let asked: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ProjectSharing, String>>>,
    > = match org {
        Some(org) => Box::pin(
            target.request_typed::<call::ShareProject>(ShareProjectParams { project, org }),
        ),
        None => Box::pin(target.request_typed::<call::UnshareProject>(BoardParams { project })),
    };
    bench.report(
        asked,
        t!("The board's sharing did not change"),
        move |bench, sharing, cx| {
            let place = bench.place();
            let words = match (shared, &sharing.shared) {
                (true, Some(with)) => t!("Shared with {org}", org = with.name),
                _ => t!("Only you see this board").to_owned(),
            };
            bench.feeds.at_mut(&place).sharing = Some(sharing);
            bench.parts.redraw_stage(cx);
            bench.notify(toast::success(words), cx);
        },
        cx,
    );
}
