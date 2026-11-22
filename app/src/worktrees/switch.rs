//! A chat's worktree switch: before its first message it says whether the chat starts
//! in a git worktree on a branch of its own, then which branch it works on.

use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use sikemux_view::workspace::AgentWorktree;

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_2, TEXT_2XS};
use crate::ui;

/// Where the chat stands with a worktree.
#[derive(Clone, PartialEq)]
pub enum Switch {
    /// It has not started, in a repository: the first message starts it in a worktree when `on`.
    Choosing { on: bool },
    /// The work always takes a checkout of its own, so there is nothing to choose.
    Always,
    /// The worktree is being made, at `step`.
    Preparing { step: String },
    /// It works in the worktree on `branch` at `path`.
    In { branch: String, path: String },
}

/// The switch a chat shows, or none for one that has started outside a worktree or is not
/// in a repository.
pub fn state(
    worktree: Option<&AgentWorktree>,
    is_repo: bool,
    started: bool,
    preparing: Option<&str>,
    on: bool,
) -> Option<Switch> {
    if let Some(step) = preparing {
        return Some(Switch::Preparing {
            step: step.to_owned(),
        });
    }
    if let Some(worktree) = worktree {
        return Some(Switch::In {
            branch: worktree.branch.clone(),
            path: worktree.path.to_string_lossy().into_owned(),
        });
    }
    (is_repo && !started).then_some(Switch::Choosing { on })
}

fn about(switch: &Switch) -> String {
    match switch {
        Switch::Choosing { on: true } => t!("The first message starts this chat in a new git worktree on its own branch. Click to stay in the project folder.").into(),
        Switch::Choosing { on: false } => t!("Click to start this chat in a new git worktree on its own branch.").into(),
        Switch::Always => t!("Hand off always works in a git worktree of its own.").into(),
        Switch::Preparing { step } => format!("{step}…"),
        Switch::In { branch, path } => {
            t!("Working in worktree {branch} at {path}", branch = branch, path = path)
        }
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
        _ => t!("worktree").into(),
    };
    BaseButton::new(id.into())
        .accessibility_label(t!("Worktree"))
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
        .text_color(match switch {
            Switch::Always => t.accent,
            _ if on => t.text_primary,
            _ => t.text_tertiary,
        })
        .when(matches!(switch, Switch::Always), |toggle| {
            toggle.bg(Hsla {
                a: 0.16,
                ..t.accent
            })
        })
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
