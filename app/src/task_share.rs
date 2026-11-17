//! The Tasks view's Share button: who the project's board is shared with, picked from the orgs
//! whose Team plan is paid.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::{div, px, Anchor, AnyElement, App, Context, IntoElement, ParentElement, Styled};
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

/// The button, while the board is shared or there is an org to share it with.
pub(crate) fn button(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let sharing = sharing(bench)?.clone();
    let label: String = match &sharing.shared {
        None => t!("Share").to_owned(),
        Some(shared) if !shared.on => t!("{org} sharing paused", org = shared.name),
        Some(shared) => t!(
            "Shared with {org} · {people}",
            org = shared.name,
            people = people(shared.people.len())
        ),
    };
    let handle = cx.weak_entity();
    let asks = bench.asks.clone();
    let t = Tokens::get(cx);
    let trigger = ui::Button::new("tasks-share-button", label, ui::Variant::Ghost)
        .icon(Icon::Share)
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
    Some(menu.into_any_element())
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
