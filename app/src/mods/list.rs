//! The tab rail's list of mods: active mods by scope, turned-off mods, shipped examples,
//! and the mods folder path at the foot of the rail.

use std::path::Path;

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, ClipboardItem, Context, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_calls::call;
use sikemux_calls::mods::{AllowParams, ModExample, ModInfo};

use crate::agents::searched;
use crate::agents::{new_row, page_body, page_header, rail_empty};
use crate::icons::Icon;
use crate::rail::with_menu;
use crate::theme::{self, Tokens, SPACE_1, SPACE_2, SPACE_2H, TEXT_2XS};
use crate::toast;
use crate::ui::{self, Row, Tone};
use crate::workbench::Workbench;

pub(crate) fn page(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let body = page_body().child(page_header(t!("Mods"), bench, cx));
    let words = bench.mod_search.words(cx);
    let list = bench.feeds.in_front().mods.as_ref();
    let found: Vec<&ModInfo> = (bench.mods_here())
        .filter(|shown| searched::holds(&format!("{} {}", shown.name, shown.description), &words))
        .collect();
    let off_mods: Vec<&ModInfo> = list
        .into_iter()
        .flat_map(|list| list.mods.iter())
        .filter(|shown| {
            shown.off && searched::holds(&format!("{} {}", shown.name, shown.description), &words)
        })
        .collect();
    let examples: Vec<&ModExample> = list
        .into_iter()
        .flat_map(|list| list.examples.iter())
        .filter(|example| {
            searched::holds(&format!("{} {}", example.name, example.description), &words)
        })
        .collect();
    let empty =
        (found.is_empty() && off_mods.is_empty() && examples.is_empty()).then(|| {
            match words.is_empty() {
                true => rail_empty(Icon::Puzzle, t!("No mods yet"), None, cx),
                false => rail_empty(Icon::Search, t!("No mod matches"), None, cx),
            }
        });
    let front = bench.front_mod(cx);
    let front = (front.as_deref()).filter(|_| !bench.settings.read(cx).is_open());
    let project = (bench.project()).map(|project| SharedString::from(project.name.clone()));
    let (everywhere, here): (Vec<&ModInfo>, Vec<&ModInfo>) =
        (found.into_iter()).partition(|shown| shown.project.is_none());
    let every_rows: Vec<AnyElement> = everywhere
        .into_iter()
        .map(|shown| row(bench, shown, front == Some(shown.id.as_str()), cx))
        .collect();
    let here_rows: Vec<AnyElement> = here
        .into_iter()
        .map(|shown| row(bench, shown, front == Some(shown.id.as_str()), cx))
        .collect();
    let off_rows: Vec<AnyElement> = off_mods
        .into_iter()
        .map(|shown| row(bench, shown, front == Some(shown.id.as_str()), cx))
        .collect();
    let example_rows: Vec<AnyElement> = examples
        .into_iter()
        .map(|example| example_row(bench, example, cx))
        .collect();
    let sections = [
        (t!("Every project").into(), every_rows),
        (project.unwrap_or_default(), here_rows),
        (t!("Turned off").into(), off_rows),
        (t!("Examples").into(), example_rows),
    ];
    let new = new_row("new-mod", Icon::Plus, t!("New mod"), None, cx)
        .on_click(cx.listener(|bench, _, _, cx| bench.ask_for_mod(None, cx)));
    let listed = searched::list(
        body,
        "mods-scroll",
        &bench.mod_search,
        Some(new.into_any_element()),
        empty,
        sections,
        cx,
    );
    let footer = list.map(|list| folder_footer(&list.folder, cx));
    div()
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(div().flex_1().min_h_0().child(listed))
        .children(footer)
        .into_any_element()
}

fn shorten_home(path: &Path) -> String {
    let full = path.display().to_string();
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            if let Some(rest) = full.strip_prefix(&home) {
                return format!("~{rest}");
            }
        }
    }
    full
}

fn folder_footer(folder: &Path, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx);
    let full = folder.display().to_string();
    let shown = shorten_home(folder);
    let copy_path = full.clone();
    let reveal_path = folder.to_path_buf();
    let this = cx.weak_entity();
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .px(SPACE_2H)
        .py(SPACE_2)
        .border_t(crate::theme::border())
        .border_color(t.border)
        .child(ui::icon(Icon::Folder, 13.).text_color(t.text_tertiary))
        .child(
            div()
                .id("mods-folder-path")
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(theme::mono_font())
                .text_size(theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .tooltip(ui::tooltip(full))
                .child(shown),
        )
        .child(
            ui::IconButton::new("mods-folder-copy", Icon::Copy, t!("Copy path"))
                .compact()
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()));
                    let _ = this.update(cx, |bench, cx| {
                        bench.notify(toast::success(t!("Copied mods folder path")), cx)
                    });
                }),
        )
        .child(
            ui::IconButton::new(
                "mods-folder-reveal",
                Icon::ExternalLink,
                crate::i18n::tr(ui::REVEAL),
            )
            .compact()
            .on_click(move |_, _, cx| cx.reveal_path(&reveal_path)),
        )
        .into_any_element()
}

fn example_row(bench: &Workbench, example: &ModExample, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx);
    let id = example.id.clone();
    let add = ui::Button::new(
        SharedString::from(format!("mod-example-{}", example.id)),
        if example.added {
            t!("Added")
        } else {
            t!("Add")
        },
        ui::Variant::Outline,
    )
    .sized(22., 8., TEXT_2XS)
    .disabled(example.added)
    .on_click(cx.listener(move |bench, _, _, cx| bench.add_example_mod(&id, cx)));
    Row::new(
        SharedString::from(format!("mod-example-row-{}", example.id)),
        example.name.clone(),
    )
    .mark(22., ui::picture(Icon::Puzzle, 15., t.text_tertiary))
    .compact(bench.rail_compact())
    .detail(example.description.clone())
    .status(Some(add))
    .into_any_element()
}

/// A mod, which opens as a tab of the desk, with the few words its page shows beside its name.
fn row(
    bench: &Workbench,
    shown: &ModInfo,
    chosen: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx);
    let ink = if shown.off {
        t.text_tertiary
    } else {
        t.text_secondary
    };
    let id = shown.id.clone();
    let off = shown.off;
    let project = bench.project().map(|project| project.name.clone());
    let row = Row::new(SharedString::from(format!("mod-{id}")), shown.name.clone())
        .mark(22., ui::picture(super::icon(shown), 15., ink))
        .compact(bench.rail_compact())
        .chosen(chosen)
        .when_some(shown.badge.clone(), |row, badge| row.detail(badge))
        .when(off, |row| row.status(Some(ui::dot(Tone::Idle, cx))))
        .when(!off && shown.problem.is_some(), |row| {
            row.status(Some(ui::dot(Tone::Failed, cx)))
        })
        .when(
            !off && shown.problem.is_none() && shown.note.is_some(),
            |row| row.status(Some(ui::dot(Tone::Pending, cx))),
        )
        .on_click(cx.listener(move |bench, _, _, cx| {
            if off {
                bench.turn_mod_off(&id, false);
                bench.notify(toast::success(t!("Turned mod on")), cx);
            } else {
                bench.open_mod(&id, cx);
            }
        }));
    let (bench, shown, project) = (cx.weak_entity(), shown.clone(), project);
    with_menu(
        SharedString::from(format!("mod-{}-menu", shown.id)),
        move |mut popup, _, _| {
            let pinned = shown.project.is_some();
            let pin = match (pinned, &project) {
                (true, _) => t!("Show in every project").to_owned(),
                (false, Some(project)) => t!("Show only in {project}", project = project),
                (false, None) => t!("Show only in this project").to_owned(),
            };
            let (pinning, editing, switching, removing) =
                (bench.clone(), bench.clone(), bench.clone(), bench.clone());
            let (id, edited, switch_id, removed) = (
                shown.id.clone(),
                shown.clone(),
                shown.id.clone(),
                shown.clone(),
            );
            let is_off = shown.off;
            popup = popup
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
                .item(
                    PopupMenuItem::new(if is_off {
                        t!("Turn on")
                    } else {
                        t!("Turn off")
                    })
                    .icon(Icon::Power)
                    .on_click(move |_, _, cx| {
                        let _ = switching.update(cx, |bench, cx| {
                            bench.turn_mod_off(&switch_id, !is_off);
                            bench.notify(
                                toast::info(if is_off {
                                    t!("Turned mod on")
                                } else {
                                    t!("Turned mod off")
                                }),
                                cx,
                            );
                        });
                    }),
                );
            let asks = !shown.writes.is_empty() || !shown.connect.is_empty();
            if shown.allowed && asks {
                let (taking, taken) = (bench.clone(), shown.id.clone());
                popup = popup.item(
                    PopupMenuItem::new(t!("Take back approval"))
                        .icon(Icon::Shield)
                        .on_click(move |_, _, cx| {
                            let id = taken.clone();
                            let _ = taking.update(cx, |bench, cx| {
                                bench.target().call_typed::<call::AllowMod>(AllowParams {
                                    id: id.clone(),
                                    writes: None,
                                    connect: Vec::new(),
                                });
                                bench.notify(toast::info(t!("Approval taken back")), cx);
                            });
                        }),
                );
            }
            popup
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
