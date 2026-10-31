//! The rules a goal's agents follow: the goal's own, then those kept for the project and the
//! folders above it. A project's rule can move into its repository's AGENTS.md, where it is
//! versioned with the code; the core says when one can't.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{div, AnyElement, Context, IntoElement, ParentElement, SharedString, Styled};
use sikemux_core::server::CardView;

use super::goal::section;
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

pub(super) fn rules(
    pane: &GoalsPane,
    goal: &CardView,
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
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
        folder.rules.iter().map(move |rule| Listed {
            text: rule.text.clone(),
            kept: match rule.sensitive {
                true => format!("{place} · sensitive"),
                false => place.clone(),
            },
            movable: (!rule.sensitive).then_some(rule.id),
        })
    });
    let listed: Vec<Listed> = own.chain(kept).collect();
    if listed.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let busy = pane.busy;
    Some(
        div()
            .flex()
            .flex_col()
            .child(section("Rules", listed.len(), cx))
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
            .into_any_element(),
    )
}
