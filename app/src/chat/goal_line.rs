//! A chat and its goal, as the composer shows them: the suggestion to track its work, the Undo
//! right after, and the goal on the strip. The core decides all of it; this only draws it.

use std::rc::Rc;

use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use sikemux_core::server::{call, ChatGoalParams};
use sikemux_wire::chat::{ChatGoal, ChatHead};
use sikemux_wire::protocol::NoticeGoal;
use sikemux_wire::slice::SliceState;

use super::composer::{Act, Notice};
use crate::theme::{Tokens, SPACE_1, SPACE_1H, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

/// A step of the strip's progress, as wide as the design has it.
const STEP_WIDTH: f32 = 14.;
const STEP_GAP: f32 = 3.;
const STRIP_MOST: f32 = 120.;

fn goal_act<C: sikemux_core::server::call::Call<Params = ChatGoalParams> + 'static>(
    this: &WeakEntity<Workbench>,
    agent: &str,
) -> Act {
    let (this, agent) = (this.clone(), agent.to_owned());
    Rc::new(move |_: &mut Window, cx: &mut App| {
        let agent_id = agent.clone();
        this.update(cx, |bench, _| {
            bench
                .chat_target(&agent)
                .call_typed::<C>(ChatGoalParams { agent_id });
        })
        .ok();
    })
}

/// The line under the composer: right after tracking, how to take it back; else the suggestion.
pub(super) fn notice(
    head: Option<&ChatHead>,
    agent: &str,
    this: &WeakEntity<Workbench>,
) -> Option<Notice> {
    let head = head?;
    if let Some(goal) = head.goal.as_ref().filter(|goal| goal.undoable) {
        return Some(Notice::Done(
            format!("Tracking as “{}”", goal.name).into(),
            ("Undo".into(), goal_act::<call::ChatGoalUndo>(this, agent)),
        ));
    }
    let reason = head.goal_offer.as_ref()?;
    Some(Notice::Suggest(
        format!("This looks like real work: {reason}").into(),
        (
            "Track as goal".into(),
            goal_act::<call::ChatGoalTrack>(this, agent),
        ),
        (
            "Not now".into(),
            goal_act::<call::ChatGoalNotNow>(this, agent),
        ),
    ))
}

/// How far along a goal is, in the words its steps count in.
fn progress(goal: &ChatGoal) -> Option<String> {
    let total = goal.steps.len();
    if total == 0 {
        return None;
    }
    let done = (goal.steps.iter())
        .filter(|step| **step == SliceState::Done)
        .count();
    Some(match goal.of_gates {
        true => format!("{done} of {total} gates"),
        false => format!("{done} of {total} done"),
    })
}

/// The goal at the strip's right end, which opens it.
pub(super) fn on_strip(
    goal: &ChatGoal,
    bench: Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let opens = NoticeGoal {
        project: goal.project.clone(),
        card: goal.card,
    };
    let steps = goal.steps.len() as f32;
    let width = (steps * STEP_WIDTH + (steps - 1.).max(0.) * STEP_GAP).min(STRIP_MOST);
    div()
        .id(SharedString::from(format!("composer-goal-{}", goal.card)))
        .min_w_0()
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded_full()
        .cursor_pointer()
        .hover(move |style| style.bg(t.surface_hover))
        .tooltip(ui::tooltip("Open goal"))
        .on_click(move |_, _, cx| {
            bench.update(cx, |bench, cx| bench.open_goal(&opens, cx));
        })
        .child(crate::goals::state_mark(goal.state, cx))
        .child(
            ui::text(goal.name.clone(), TEXT_XS, FontWeight::MEDIUM)
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .children(
            (!goal.steps.is_empty())
                .then(|| crate::goals::state_strip(goal.steps.iter().copied(), px(width), cx)),
        )
        .children(progress(goal).map(|words| ui::meta(words, cx).flex_none().mx(SPACE_1)))
        .into_any_element()
}
