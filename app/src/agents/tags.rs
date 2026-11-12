//! The person's tags on chats: the Tags submenu that puts them on and takes them off, the
//! rail's button that lists only one tag's chats, and the pill that says which.

use std::collections::BTreeMap;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{
    div, px, Anchor, AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::workspace::{AgentPatch, UpdateAgentParams};

use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2H, SPACE_3};
use crate::ui::{self, IconButton};
use crate::workbench::Workbench;

impl Workbench {
    /// Each tag on the chats the rail can list, with how many have it, by name.
    pub(crate) fn tags_in_use(&self) -> BTreeMap<String, usize> {
        let mut tags = BTreeMap::new();
        let project = self.project().map(|project| project.id.clone());
        let agents = self.this_view().map(|view| &view.workspace.agents);
        for agent in agents.into_iter().flatten() {
            if !self.rails.all_projects && Some(&agent.project) != project.as_ref() {
                continue;
            }
            for tag in &agent.tags {
                *tags.entry(tag.clone()).or_default() += 1;
            }
        }
        tags
    }

    /// Lists only the chats tagged `tag`, or every chat again for none.
    pub(crate) fn show_tag(&mut self, tag: Option<String>, cx: &mut Context<Self>) {
        self.rails.tag = tag;
        cx.notify();
    }
}

/// Sets `agent`'s tags to `tags`.
fn retag(bench: &Workbench, place: &Place, agent: &str, tags: Vec<String>) {
    Target::new(&bench.asks, place.clone()).call_typed::<call::UpdateAgent>(UpdateAgentParams::new(
        agent.to_owned(),
        AgentPatch {
            tags: Some(tags),
            ..AgentPatch::default()
        },
    ))
}

/// The agent menu's Tags: each tag in use, checked when `agent` has it, and one to add a new
/// tag by name.
pub(crate) fn menu_item(
    popup: PopupMenu,
    bench: WeakEntity<Workbench>,
    place: Place,
    id: &str,
    had: Vec<String>,
    in_use: Vec<String>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let id = id.to_owned();
    popup.submenu_with_icon(
        Some(Icon::Tag.into()),
        t!("Tags"),
        window,
        cx,
        ui::menu(move |mut menu, _, _| {
            let (bench_new, id_new, had_new) = (bench.clone(), id.clone(), had.clone());
            let place_new = place.clone();
            menu = menu.item(
                PopupMenuItem::new(t!("New Tag…"))
                    .icon(Icon::Plus)
                    .on_click(move |_, window, cx| {
                        let (bench, id, had) = (bench_new.clone(), id_new.clone(), had_new.clone());
                        let place = place_new.clone();
                        window.defer(cx, move |_, cx| {
                            let _ = bench.update(cx, |bench, cx| {
                                let place = place.clone();
                                bench.prompt(
                                    t!("New tag"),
                                    "",
                                    move |bench, typed, _, _| {
                                        let typed = typed.trim().to_lowercase();
                                        if !typed.is_empty() && !had.contains(&typed) {
                                            let tags = had.iter().cloned().chain([typed]).collect();
                                            retag(bench, &place, &id, tags);
                                        }
                                    },
                                    cx,
                                )
                            });
                        });
                    }),
            );
            if !in_use.is_empty() {
                menu = menu.separator();
            }
            for tag in &in_use {
                let on = had.contains(tag);
                let (bench, id, had, tag) = (bench.clone(), id.clone(), had.clone(), tag.clone());
                let place = place.clone();
                menu = menu.item(PopupMenuItem::new(capitalised(&tag)).checked(on).on_click(
                    move |_, _, cx| {
                        let tags = match on {
                            true => had.iter().filter(|kept| **kept != tag).cloned().collect(),
                            false => had.iter().cloned().chain([tag.clone()]).collect(),
                        };
                        let _ = bench.update(cx, |bench, _| retag(bench, &place, &id, tags));
                    },
                ));
            }
            menu
        }),
    )
}

fn capitalised(tag: &str) -> String {
    let mut letters = tag.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// The rail head's button that lists the tags in use, each with its count, to show only one's
/// chats; none while no chat has a tag.
pub(crate) fn button(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let in_use = bench.tags_in_use();
    if in_use.is_empty() {
        return None;
    }
    let chosen = bench.rails.tag.clone();
    let this = cx.weak_entity();
    Some(
        ui::menu_button(
            "rail-tags",
            Anchor::TopRight,
            IconButton::new("rail-tags-button", Icon::Tag, t!("Show one tag's chats"))
                .pressed(chosen.is_some()),
            move |mut popup, _, cx| {
                for (tag, count) in &in_use {
                    let (this, tag) = (this.clone(), tag.clone());
                    let on = chosen.as_ref() == Some(&tag);
                    popup = popup.item(
                        ui::MenuRow::new(capitalised(&tag))
                            .hint(count.to_string())
                            .item(cx)
                            .checked(on)
                            .on_click(move |_, _, cx| {
                                let tag = (!on).then(|| tag.clone());
                                let _ = this.update(cx, |bench, cx| bench.show_tag(tag, cx));
                            }),
                    );
                }
                popup
            },
        )
        .into_any_element(),
    )
}

/// The pill under the rail's head naming the tag it shows only the chats of, which clears it.
pub(crate) fn pill(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let tag = bench.rails.tag.clone()?;
    let t = Tokens::get(cx).clone();
    Some(
        div()
            .flex_none()
            .mx(SPACE_3)
            .mt(SPACE_2H)
            .flex()
            .child(
                div()
                    .id("rail-tag-pill")
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .px(SPACE_1H)
                    .py(px(2.))
                    .rounded(theme::RADIUS_1)
                    .bg(t.surface_chosen)
                    .cursor_pointer()
                    .hover(move |style| style.bg(t.surface_active))
                    .child(ui::caps(&tag, cx).text_color(t.text_primary))
                    .child(ui::icon(Icon::Close, 9.).text_color(t.text_tertiary))
                    .tooltip(ui::tooltip(t!("Show every chat again")))
                    .on_click(cx.listener(|bench, _, _, cx| bench.show_tag(None, cx))),
            )
            .into_any_element(),
    )
}
