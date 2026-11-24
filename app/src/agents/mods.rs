//! The Mods section at the foot of every page of the tab rail.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled,
};

use crate::agents::limits::{section_header, small_button};
use crate::icons::Icon;
use crate::rail::with_menu;
use crate::theme::{Tokens, SPACE_0, SPACE_2H};
use crate::ui::{self, Row, Tone};
use crate::workbench::Workbench;

/// The rows past this height scroll, so many mods leave the list above its room.
const ROWS_TALLEST: f32 = 240.;

pub(crate) fn mods(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    let compact = bench.rail_compact();
    let rows = rows(bench, cx);
    let add = small_button("add-mod", Icon::Plus, t!("Make a mod with an agent").into())
        .on_click(cx.listener(|bench, _, _, cx| bench.ask_for_mod(None, cx)));
    div()
        .flex_none()
        .mt(px(if compact { 10. } else { 12. }))
        .mx(SPACE_2H)
        .mb(px(if compact { 8. } else { 12. }))
        .child(section_header(t!("MODS"), cx).child(add))
        .when(!rows.is_empty(), |section| {
            section.child(
                div()
                    .id("mods-rows")
                    .max_h(px(ROWS_TALLEST))
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .overflow_y_scroll()
                    .children(rows),
            )
        })
}

/// The mods of the host in front that show in the project in front, each opening as a tab of
/// the desk, with the few words its page shows beside its name.
fn rows(bench: &Workbench, cx: &mut Context<Workbench>) -> Vec<AnyElement> {
    let ink = Tokens::get(cx).text_secondary;
    let compact = bench.rail_compact();
    let front = bench.front_mod(cx);
    let settings_open = bench.settings.read(cx).is_open();
    let project = bench.project().map(|project| project.name.clone());
    bench
        .mods_here()
        .map(|shown| {
            let id = shown.id.clone();
            let row = Row::new(SharedString::from(format!("mod-{id}")), shown.name.clone())
                .mark(22., ui::picture(crate::mods::icon(shown), 15., ink))
                .compact(compact)
                .quiet()
                .chosen(!settings_open && front.as_ref() == Some(&id))
                .when_some(shown.badge.clone(), |row, badge| row.detail(badge))
                .when(shown.problem.is_some(), |row| {
                    row.status(Some(ui::dot(Tone::Failed, cx)))
                })
                .when(shown.problem.is_none() && shown.note.is_some(), |row| {
                    row.status(Some(ui::dot(Tone::Pending, cx)))
                })
                .on_click(cx.listener(move |bench, _, _, cx| bench.open_mod(&id, cx)));
            let (bench, shown, project) = (cx.weak_entity(), shown.clone(), project.clone());
            with_menu(
                SharedString::from(format!("mod-{}-menu", shown.id)),
                move |popup, _, _| {
                    let pinned = shown.project.is_some();
                    let pin = match (pinned, &project) {
                        (true, _) => t!("Show in every project").to_owned(),
                        (false, Some(project)) => t!("Show only in {project}", project = project),
                        (false, None) => t!("Show only in this project").to_owned(),
                    };
                    let (pinning, editing, removing) =
                        (bench.clone(), bench.clone(), bench.clone());
                    let (id, edited, removed) = (shown.id.clone(), shown.clone(), shown.clone());
                    popup
                        .item(
                            PopupMenuItem::new(pin)
                                .icon(Icon::Pin)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        pinning.update(cx, |bench, _| bench.pin_mod(&id, pinned));
                                }),
                        )
                        .item(
                            PopupMenuItem::new(t!("Edit with an agent"))
                                .icon(Icon::Sparkle)
                                .on_click(move |_, _, cx| {
                                    let edited = Some(edited.clone());
                                    let _ = editing
                                        .update(cx, |bench, cx| bench.ask_for_mod(edited, cx));
                                }),
                        )
                        .separator()
                        .item(PopupMenuItem::new(t!("Remove")).icon(Icon::Trash).on_click(
                            move |_, _, cx| {
                                let _ =
                                    removing.update(cx, |bench, cx| bench.remove_mod(&removed, cx));
                            },
                        ))
                },
                row,
                cx,
            )
        })
        .collect()
}
