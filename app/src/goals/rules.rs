//! The rules a goal's agents follow: the goal's own, then those kept for the project and the
//! folders above it. A project's rule can move into its repository's AGENTS.md, where it is
//! versioned with the code; the core says when one can't. The rules the person kept saying in
//! the project's past sessions are offered here to keep, ticked, once the core has found them.

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{div, AnyElement, Context, IntoElement, ParentElement, SharedString, Styled};
use sikemux_core::server::CardView;
use sikemux_view::memory::Seeding;

use super::goal::quiet_section;
use super::GoalsPane;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, TEXT_XS_H};
use crate::ui::{self, Button, Variant};

/// One rule, as the section lists it.
struct Listed {
    text: String,
    /// Where it is kept, in a word or two.
    kept: String,
    /// The memory rule the person may move into AGENTS.md.
    movable: Option<u64>,
}

pub(super) fn rules(pane: &GoalsPane, goal: &CardView, cx: &mut Context<GoalsPane>) -> AnyElement {
    let own = goal
        .rules
        .iter()
        .filter(|rule| rule.retired.is_none())
        .map(|rule| Listed {
            text: rule.text.clone(),
            kept: "this goal".into(),
            movable: None,
        });
    let folders = pane.memory.iter().flat_map(|memory| memory.folders.iter());
    let kept = folders.enumerate().flat_map(|(at, folder)| {
        let place = match at {
            0 => "the project".to_owned(),
            _ => format!(
                "for {}",
                folder
                    .folder
                    .rsplit('/')
                    .find(|part| !part.is_empty())
                    .unwrap_or(&folder.folder)
            ),
        };
        folder.rules.iter().map(move |listed| Listed {
            text: listed.rule.text.clone(),
            kept: match listed.rule.sensitive {
                true => format!("{place} · sensitive"),
                false => place.clone(),
            },
            movable: listed.stays.is_none().then_some(listed.rule.id),
        })
    });
    let listed: Vec<Listed> = own.chain(kept).collect();
    let t = Tokens::get(cx).clone();
    let busy = pane.busy;
    let seeding = pane
        .memory
        .as_ref()
        .and_then(|memory| memory.seeding.as_ref());
    let idle = !matches!(
        seeding,
        Some(Seeding::Reading { .. } | Seeding::Offered { .. })
    );
    div()
        .flex()
        .flex_col()
        .child(quiet_section("Rules", listed.len(), cx))
        .children(offer(pane, true, cx))
        .children(listed.into_iter().map(|rule| {
            div()
                .py(SPACE_1H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(theme::ui_text(TEXT_XS_H))
                        .text_color(t.text_secondary)
                        .child(rule.text),
                )
                .child(ui::meta(rule.kept, cx))
                .when_some(rule.movable, |row, id| {
                    row.child(
                        Button::new(
                            SharedString::from(format!("rule-move-{id}")),
                            "Move to AGENTS.md",
                            Variant::Outline,
                        )
                        .disabled(busy)
                        .on_click(cx.listener(move |pane, _, _, cx| pane.promote(id, cx))),
                    )
                })
        }))
        .when(idle && pane.memory.is_some(), |section| {
            section.child(
                div()
                    .pt(SPACE_2)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        Button::new(
                            "rules-seed",
                            "Find rules in past sessions",
                            Variant::Outline,
                        )
                        .disabled(busy)
                        .on_click(cx.listener(|pane, _, _, cx| pane.seed_rules(cx))),
                    )
                    .children(match seeding {
                        Some(Seeding::Ran { at, kept: 0 }) => {
                            Some(ui::meta(format!("Looked {}", ui::time::ago(*at)), cx))
                        }
                        Some(Seeding::Ran { at, kept }) => Some(ui::meta(
                            format!("Looked {} · kept {kept}", ui::time::ago(*at)),
                            cx,
                        )),
                        _ => None,
                    }),
            )
        })
        .into_any_element()
}

/// The rules the core found the person repeating in the project's past sessions, each ticked
/// to keep unless they untick it, with Keep and Dismiss; or that it is reading them now. Asks
/// whether to keep them when `asked`, where nothing above asks already.
pub(super) fn offer(
    pane: &GoalsPane,
    asked: bool,
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let rules = match pane.memory.as_ref()?.seeding.as_ref()? {
        Seeding::Reading { .. } => {
            return Some(
                div()
                    .py(SPACE_1H)
                    .child(ui::meta(
                        "Reading your past sessions for rules you repeat…",
                        cx,
                    ))
                    .into_any_element(),
            )
        }
        Seeding::Offered { rules, .. } => rules,
        Seeding::Ran { .. } => return None,
    };
    let pane_entity = cx.entity();
    let kept = rules
        .iter()
        .filter(|rule| !pane.unticked.contains(&rule.id))
        .count();
    Some(
        div()
            .py(SPACE_1H)
            .flex()
            .flex_col()
            .when(asked, |offer| {
                offer.child(
                    div()
                        .pb(SPACE_1H)
                        .text_size(theme::ui_text(TEXT_XS_H))
                        .text_color(t.text_primary)
                        .child("You've said these rules in past sessions. Keep them?"),
                )
            })
            .children(rules.iter().map(|rule| {
                let (id, entity) = (rule.id, pane_entity.clone());
                div()
                    .py(SPACE_1H)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        Checkbox::new(SharedString::from(format!("seeded-{id}")))
                            .checked(!pane.unticked.contains(&id))
                            .on_click(move |on: &bool, _, cx| {
                                let on = *on;
                                entity.update(cx, |pane, cx| pane.tick_seeded(id, on, cx));
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme::ui_text(TEXT_XS_H))
                            .text_color(t.text_secondary)
                            .child(rule.text.clone()),
                    )
                    .child(ui::meta(format!("{} sessions", rule.sessions), cx))
            }))
            .child(
                div()
                    .pt(SPACE_2)
                    .flex()
                    .gap(SPACE_2)
                    .child(
                        Button::new("seeded-keep", format!("Keep {kept}"), Variant::Primary)
                            .disabled(pane.busy || kept == 0)
                            .on_click(cx.listener(|pane, _, _, cx| pane.keep_seeded(true, cx))),
                    )
                    .child(
                        Button::new("seeded-dismiss", "Dismiss", Variant::Outline)
                            .disabled(pane.busy)
                            .on_click(cx.listener(|pane, _, _, cx| pane.keep_seeded(false, cx))),
                    ),
            )
            .into_any_element(),
    )
}
