//! The tab rail's list of the mods that show here, in sections by where each shows.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, SharedString};
use sikemux_calls::mods::ModInfo;

use crate::agents::searched;
use crate::agents::{new_row, page_body, page_header, rail_empty};
use crate::icons::Icon;
use crate::rail::with_menu;
use crate::theme::Tokens;
use crate::ui::{self, Row, Tone};
use crate::workbench::Workbench;

pub(crate) fn page(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let body = page_body().child(page_header(t!("Mods"), bench, cx));
    let words = bench.mod_search.words(cx);
    let found: Vec<&ModInfo> = (bench.mods_here())
        .filter(|shown| searched::holds(&format!("{} {}", shown.name, shown.description), &words))
        .collect();
    let empty = found.is_empty().then(|| match words.is_empty() {
        true => rail_empty(Icon::Puzzle, t!("No mods yet"), None, cx),
        false => rail_empty(Icon::Search, t!("No mod matches"), None, cx),
    });
    let front = bench.front_mod(cx);
    let front = (front.as_deref()).filter(|_| !bench.settings.read(cx).is_open());
    let project = (bench.project()).map(|project| SharedString::from(project.name.clone()));
    let (everywhere, here): (Vec<&ModInfo>, Vec<&ModInfo>) =
        (found.into_iter()).partition(|shown| shown.project.is_none());
    let mut rows = |mods: Vec<&ModInfo>| {
        (mods.into_iter())
            .map(|shown| row(bench, shown, front == Some(shown.id.as_str()), cx))
            .collect()
    };
    let sections = [
        (t!("Every project").into(), rows(everywhere)),
        (project.unwrap_or_default(), rows(here)),
    ];
    let new = new_row("new-mod", Icon::Plus, t!("New mod"), None, cx)
        .on_click(cx.listener(|bench, _, _, cx| bench.ask_for_mod(None, cx)));
    searched::list(
        body,
        "mods-scroll",
        &bench.mod_search,
        Some(new.into_any_element()),
        empty,
        sections,
        cx,
    )
}

/// A mod, which opens as a tab of the desk, with the few words its page shows beside its name.
fn row(
    bench: &Workbench,
    shown: &ModInfo,
    chosen: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let ink = Tokens::get(cx).text_secondary;
    let id = shown.id.clone();
    let project = bench.project().map(|project| project.name.clone());
    let row = Row::new(SharedString::from(format!("mod-{id}")), shown.name.clone())
        .mark(22., ui::picture(super::icon(shown), 15., ink))
        .compact(bench.rail_compact())
        .chosen(chosen)
        .when_some(shown.badge.clone(), |row, badge| row.detail(badge))
        .when(shown.problem.is_some(), |row| {
            row.status(Some(ui::dot(Tone::Failed, cx)))
        })
        .when(shown.problem.is_none() && shown.note.is_some(), |row| {
            row.status(Some(ui::dot(Tone::Pending, cx)))
        })
        .on_click(cx.listener(move |bench, _, _, cx| bench.open_mod(&id, cx)));
    let (bench, shown, project) = (cx.weak_entity(), shown.clone(), project);
    with_menu(
        SharedString::from(format!("mod-{}-menu", shown.id)),
        move |popup, _, _| {
            let pinned = shown.project.is_some();
            let pin = match (pinned, &project) {
                (true, _) => t!("Show in every project").to_owned(),
                (false, Some(project)) => t!("Show only in {project}", project = project),
                (false, None) => t!("Show only in this project").to_owned(),
            };
            let (pinning, editing, removing) = (bench.clone(), bench.clone(), bench.clone());
            let (id, edited, removed) = (shown.id.clone(), shown.clone(), shown.clone());
            popup
                .item(
                    PopupMenuItem::new(pin)
                        .icon(Icon::Pin)
                        .on_click(move |_, _, cx| {
                            let _ = pinning.update(cx, |bench, _| bench.pin_mod(&id, pinned));
                        }),
                )
                .item(
                    PopupMenuItem::new(t!("Edit with an agent"))
                        .icon(Icon::Sparkle)
                        .on_click(move |_, _, cx| {
                            let edited = Some(edited.clone());
                            let _ = editing.update(cx, |bench, cx| bench.ask_for_mod(edited, cx));
                        }),
                )
                .separator()
                .item(PopupMenuItem::new(t!("Remove")).icon(Icon::Trash).on_click(
                    move |_, _, cx| {
                        let _ = removing.update(cx, |bench, cx| bench.remove_mod(&removed, cx));
                    },
                ))
        },
        row,
        cx,
    )
}
