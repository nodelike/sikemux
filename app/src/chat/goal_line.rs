//! A chat and its goal: a toast with Undo right after its agent tracks one, and how far along
//! the goal is, as its milestone rows in the transcript draw it.

use std::rc::Rc;

use gpui_kit::{px, AnyElement, App, WeakEntity, Window};
use sikemux_core::server::board::chat_goal::ChatGoalParams;
use sikemux_core::server::call;
use sikemux_view::chat::ChatGoal;
use sikemux_view::slice::SliceState;

use super::composer::Act;
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
    (!steps.is_empty())
        .then(|| crate::ui::slice_state::state_strip(steps.iter().copied(), px(width), cx))
}
