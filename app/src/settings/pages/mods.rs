//! The mods of the host in front: what each reads and may change, to open, turn off, take its
//! writes back from, or remove.

use gpui_kit::component::switch::Switch;
use gpui_kit::{
    div, AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Styled, Window,
};
use sikemux_calls::mods::ModInfo;

use crate::icons::Icon;
use crate::settings::index::Page::Mods;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_1H};
use crate::ui;
use crate::workbench::Workbench;
use sikemux_calls::call;
use sikemux_calls::mods::AllowParams;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(Mods, "Mods", "mods mod panel panels html page dashboard widget custom make own approve allow writes take back turn off remove pin"),
];

/// What a mod reads and may change, as its row says it.
fn about(shown: &ModInfo) -> SharedString {
    if let Some(problem) = &shown.problem {
        return t!("Cannot show: {problem}", problem = problem).into();
    }
    let mut said = Vec::new();
    if !shown.reads.is_empty() {
        said.push(t!("reads {names}", names = shown.reads.join(", ")));
    }
    if !shown.writes.is_empty() {
        let names = shown.writes.join(", ");
        said.push(match shown.allowed {
            true => t!("changes {names} (allowed)", names = names),
            false => t!("changes {names} (asks first)", names = names),
        });
    }
    if shown.project.is_some() {
        said.push(t!("shows in one project").to_owned());
    }
    match said.is_empty() {
        true => SharedString::from(shown.description.clone()),
        false => said.join(" · ").into(),
    }
}

fn controls(shown: &ModInfo, this: &Entity<Workbench>) -> AnyElement {
    let id = shown.id.clone();
    let button = |what: &str, label: &'static str| {
        ui::Button::new(
            SharedString::from(format!("mod-{what}-{id}")),
            label,
            ui::Variant::Outline,
        )
    };
    let (opening, open) = (this.clone(), (shown.id.clone(), shown.name.clone()));
    let open = button("open", t!("Open"))
        .disabled(shown.off || shown.problem.is_some())
        .on_click(move |_, window, cx| {
            opening.update(cx, |bench, cx| {
                crate::settings::close(bench, window, cx);
                bench.open_mod(&open.0, &open.1);
            })
        });
    let (taking, taken) = (this.clone(), shown.id.clone());
    let take_back = (shown.allowed && !shown.writes.is_empty()).then(|| {
        button("take-back", t!("Take back writes")).on_click(move |_, _, cx| {
            let id = taken.clone();
            taking.update(cx, |bench, _| {
                bench.target().call_typed::<call::AllowMod>(AllowParams {
                    id: id.clone(),
                    writes: None,
                })
            })
        })
    });
    let (removing, removed) = (this.clone(), shown.clone());
    let remove = button("remove", t!("Remove"))
        .icon(Icon::Trash)
        .on_click(move |_, _, cx| removing.update(cx, |bench, cx| bench.remove_mod(&removed, cx)));
    let (switching, switched) = (this.clone(), shown.id.clone());
    let on = Switch::new(SharedString::from(format!("mod-on-{id}")))
        .checked(!shown.off)
        .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
            let off = !*on;
            switching.update(cx, |bench, _| bench.turn_mod_off(&switched, off))
        });
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(open)
        .children(take_back)
        .child(remove)
        .child(on)
        .into_any_element()
}

pub fn render(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let ink = Tokens::get(cx).text_secondary;
    let list = bench.feeds.in_front().mods.as_ref();
    let mods = list.map(|list| list.mods.as_slice()).unwrap_or_default();
    let body = match mods.is_empty() {
        true => rows::note(
            t!("No mods yet. Ask an agent for one with the + beside Mods in the rail."),
            cx,
        ),
        false => div()
            .children(mods.iter().map(|shown| {
                rows::row_with(
                    Some(ui::picture(crate::mods::icon(shown), 16., ink)),
                    shown.name.clone(),
                    Some(about(shown)),
                    controls(shown, this),
                    cx,
                )
            }))
            .into_any_element(),
    };
    let folder = list.map(|list| {
        t!(
            "Each mod is a folder of {folder}. A mod reads only what its mod.json names, and asks you once before it changes anything.",
            folder = list.folder.display()
        )
    });
    let examples = list
        .map(|list| list.examples.as_slice())
        .unwrap_or_default();
    let examples = div().children(examples.iter().map(|example| {
        let (adding, id, name) = (this.clone(), example.id.clone(), example.name.clone());
        let add = ui::Button::new(
            SharedString::from(format!("mod-example-{}", example.id)),
            if example.added {
                t!("Added")
            } else {
                t!("Add")
            },
            ui::Variant::Outline,
        )
        .disabled(example.added)
        .on_click(move |_, window, cx| {
            adding.update(cx, |bench, cx| {
                crate::settings::close(bench, window, cx);
                bench.add_example_mod(&id, &name, cx);
            })
        });
        rows::row_with(
            Some(ui::picture(Icon::Dashboard, 16., ink)),
            example.name.clone(),
            Some(example.description.clone().into()),
            add,
            cx,
        )
    }));
    vec![
        rows::section(
            n!("Mods"),
            None,
            Some(folder.unwrap_or_default().into()),
            lit("Mods"),
            body,
            cx,
        ),
        rows::section(
            n!("Examples"),
            None,
            Some(t!("Mods Sikemux ships for you to copy into your mods folder and change.").into()),
            lit("Examples"),
            examples,
            cx,
        ),
    ]
}
