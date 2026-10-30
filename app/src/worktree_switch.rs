//! A chat's worktree switch: before its first message it says whether the chat starts
//! in a git worktree on a branch of its own, then which branch it works on.

use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use sikemux_wire::protocol::AgentWorktree;

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_0, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS};
use crate::ui;

/// Where the chat stands with a worktree.
#[derive(Clone, PartialEq)]
pub enum Switch {
    /// It has not started, in a repository: the first message starts it in a worktree when `on`.
    Choosing { on: bool },
    /// The worktree is being made, at `step`.
    Preparing { step: String },
    /// It works in the worktree on `branch` at `path`, which reads as `shown`.
    In {
        branch: String,
        path: String,
        shown: String,
    },
}

/// The switch a chat shows, or none for one that has started outside a worktree or is not
/// in a repository.
pub fn state(
    worktree: Option<&AgentWorktree>,
    is_repo: bool,
    started: bool,
    preparing: Option<&str>,
    on: bool,
    home: Option<&str>,
) -> Option<Switch> {
    if let Some(step) = preparing {
        return Some(Switch::Preparing {
            step: step.to_owned(),
        });
    }
    if let Some(worktree) = worktree {
        let path = worktree.path.to_string_lossy();
        return Some(Switch::In {
            branch: worktree.branch.clone(),
            shown: crate::ui::path::tilde(&path, home),
            path: path.into_owned(),
        });
    }
    (is_repo && !started).then_some(Switch::Choosing { on })
}

fn about(switch: &Switch) -> String {
    match switch {
        Switch::Choosing { on: true } => "The first message starts this chat in a new git worktree on its own branch. Click to stay in the project folder.".into(),
        Switch::Choosing { on: false } => "Click to start this chat in a new git worktree on its own branch.".into(),
        Switch::Preparing { step } => format!("{step}…"),
        Switch::In { branch, path, .. } => format!("Working in worktree {branch} at {path}"),
    }
}

/// Turns the worktree on or off for the first message.
pub type Flip = Rc<dyn Fn(&mut Window, &mut App)>;

/// The switch, in the strip docked on the composer beside the project it works in.
pub fn toggle(id: impl Into<SharedString>, switch: &Switch, flip: Flip, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let on = !matches!(switch, Switch::Choosing { on: false });
    let settable = matches!(switch, Switch::Choosing { .. });
    let label = match switch {
        Switch::In { branch, .. } => branch.clone(),
        _ => "worktree".into(),
    };
    BaseButton::new(id.into())
        .accessibility_label("Worktree")
        .selected(on)
        .disabled(!settable)
        .h(px(26.))
        .max_w(px(220.))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded_full()
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .text_color(if on { t.text_primary } else { t.text_tertiary })
        .when(settable, |toggle| {
            toggle.hover(move |style| {
                style.bg(Hsla {
                    a: 0.07,
                    ..t.text_primary
                })
            })
        })
        .tooltip(ui::tooltip(about(switch)))
        .on_click(move |_, window, cx| flip(window, cx))
        .child(ui::icon(Icon::Git, 12.).text_color(if on { t.accent } else { t.text_tertiary }))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(label),
        )
        .into_any_element()
}

/// The line above a chat's first message saying the worktree is being made, or which
/// worktree the chat works in: its branch, then its folder more quietly.
pub fn note(switch: &Switch, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let note = div()
        .w_full()
        .max_w(px(800.))
        .px(px(22.))
        .pt(SPACE_3)
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary);
    match switch {
        Switch::Choosing { .. } => None,
        Switch::Preparing { step } => Some(note.child(format!("{step}…")).into_any_element()),
        Switch::In { branch, shown, .. } => {
            let tile = div()
                .flex_none()
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(crate::theme::RADIUS_2)
                .bg(Hsla {
                    a: 0.16,
                    ..t.accent
                })
                .child(ui::icon(Icon::Git, 14.).text_color(t.accent));
            let lines = div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .id("worktree-note-branch")
                        .min_w_0()
                        .flex()
                        .tooltip(ui::tooltip(branch.clone()))
                        .child(
                            div()
                                .flex_none()
                                .whitespace_nowrap()
                                .child("Working in a worktree on\u{a0}"),
                        )
                        .child(
                            ui::mono(branch.clone(), TEXT_2XS, cx)
                                .min_w_0()
                                .truncate()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(t.text_primary),
                        ),
                )
                .child(
                    div()
                        .id("worktree-note-path")
                        .min_w_0()
                        .tooltip(ui::tooltip(shown.clone()))
                        .child(ui::mono(shown.clone(), TEXT_2XS, cx).truncate()),
                );
            Some(
                note.flex()
                    .items_center()
                    .gap(SPACE_2H)
                    .child(tile)
                    .child(lines)
                    .into_any_element(),
            )
        }
    }
}
