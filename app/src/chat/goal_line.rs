//! A chat and its goal: a toast with Undo right after its agent tracks one, and the goal at
//! the head of the dock over the composer. The core decides all of it; this only draws it.

use std::rc::Rc;

use gpui_kit::{
    div, px, AnyElement, App, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use sikemux_core::server::board::chat_goal::ChatGoalParams;
use sikemux_core::server::call;
use sikemux_view::chat::ChatGoal;
use sikemux_view::slice::SliceState;
use sikemux_wire::protocol::NoticeGoal;

use super::composer::Act;
use crate::theme::{self, Tokens, SPACE_2, SPACE_2H, TEXT_XS_H};
use crate::ui;
use crate::workbench::Workbench;

/// A step of the goal's progress, as wide as the design has it.
const STEP_WIDTH: f32 = 14.;
const STEP_GAP: f32 = 3.;
const STRIP_MOST: f32 = 120.;

fn undo(this: &WeakEntity<Workbench>, agent: &str) -> Act {
    let (this, agent) = (this.clone(), agent.to_owned());
    Rc::new(move |_: &mut Window, cx: &mut App| {
        let agent_id = agent.clone();
        this.update(cx, |bench, _| {
            bench
                .chat_target(&agent)
                .call_typed::<call::ChatGoalUndo>(ChatGoalParams { agent_id });
        })
        .ok();
    })
}

/// Says so in a toast, with Undo, when the chat's agent has just tracked its work as a goal.
pub(super) fn tracked(goal: &ChatGoal, agent: &str, this: &WeakEntity<Workbench>, cx: &mut App) {
    let undo = undo(this, agent);
    let note = crate::toast::undoable(
        format!("Tracking as “{}”", goal.name),
        move |window, cx| undo(window, cx),
    );
    crate::toast::show(this, note, cx);
}

/// How far along a goal is, in the words its steps count in.
pub(super) fn progress(steps: &[SliceState], of_gates: bool) -> Option<String> {
    let total = steps.len();
    if total == 0 {
        return None;
    }
    let done = (steps.iter())
        .filter(|step| **step == SliceState::Done)
        .count();
    Some(match of_gates {
        true => format!("{done} of {total} gates"),
        false => format!("{done} of {total} done"),
    })
}

/// A goal's steps as the strip of segments that shows them, as wide as the design has it.
pub(super) fn steps(steps: &[SliceState], cx: &App) -> Option<AnyElement> {
    let count = steps.len() as f32;
    let width = (count * STEP_WIDTH + (count - 1.).max(0.) * STEP_GAP).min(STRIP_MOST);
    (!steps.is_empty()).then(|| crate::goals::state_strip(steps.iter().copied(), px(width), cx))
}

/// The goal heading the dock over the composer: its state, name, steps and how far along,
/// and a chevron; the row opens it.
pub(super) fn in_dock(goal: &ChatGoal, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opens = NoticeGoal {
        project: goal.project.clone(),
        card: goal.card,
    };
    div()
        .id(SharedString::from(format!("dock-goal-{}", goal.card)))
        .h(px(34.))
        .pl(SPACE_2H)
        .pr(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .hover(move |style| style.bg(t.surface_hover))
        .on_click(cx.listener(move |bench, _, _, cx| bench.open_goal(&opens, cx)))
        .child(crate::goals::state_mark(goal.state, cx))
        .child(
            ui::text(goal.name.clone(), TEXT_XS_H, FontWeight::MEDIUM)
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .children(steps(&goal.steps, cx))
        .children(progress(&goal.steps, goal.of_gates).map(|words| ui::meta(words, cx).flex_none()))
        .child(div().flex_1())
        .child(
            div()
                .w(px(18.))
                .flex_none()
                .flex()
                .justify_center()
                .child(ui::icon(crate::icons::Icon::Chevron, 10.).text_color(t.text_tertiary)),
        )
        .into_any_element()
}
