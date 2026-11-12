//! How an agent stands, as the mark beside its name: nine cells twinkling while it works, a ring
//! filling while it waits, a dot once it has something for the person or failed, a hollow ring
//! once it stopped short. A change of state is said once, as on the Mac:
//! the new mark pops in, and one that now needs the person sends two rings out. A mark drawn
//! first in a state just shows it, so a list drawing itself again replays nothing.

use gpui::{
    div, px, AnyElement, App, Context, Hsla, IntoElement, ParentElement, Render, SharedString,
    Styled, Window,
};
use sikemux_view::turn::{TurnPhase, TurnState};
use sikemux_wire::protocol::AgentStatus;

use super::clock::{self, Slot};
use super::mark_box;
use crate::motion::{self, Curve};
use crate::theme::Tokens;
use crate::theme::TEXT_2XS;
use crate::turn_line::{self, Mark};

/// Each cell's own beat and where in it the cell starts, in seconds, as the Mac staggers them.
const BEATS: [(f32, f32); 9] = [
    (0.9, 0.),
    (1.24, 0.52),
    (1.58, 1.04),
    (1.07, 0.39),
    (1.41, 0.91),
    (0.9, 0.26),
    (1.24, 0.78),
    (1.58, 0.13),
    (1.07, 0.65),
];
/// The share of a beat a cell takes to brighten; it dims over the rest.
const BRIGHTEN: f32 = 0.4;
const DIMMEST: f32 = 0.2;

/// `agent`'s mark for `status`, or none while it sits idle.
pub fn status_mark(agent: &str, status: AgentStatus, cx: &App) -> Option<AnyElement> {
    let mark = match status {
        AgentStatus::Working => Mark::Working,
        AgentStatus::Blocked => Mark::NeedsYou,
        AgentStatus::Done => Mark::Done,
        _ => return None,
    };
    Some(mark_of(agent, mark, cx))
}

/// An agent's row mark for how its turn stands; none before the agent's first turn. A ring
/// draws itself again each second on its own while its wait runs.
pub fn turn_mark(agent: &str, turn: Option<&TurnState>) -> Option<AnyElement> {
    let turn = turn
        .filter(|turn| turn_line::mark(turn, 0).is_some())?
        .clone();
    Some(
        Slot::new(
            SharedString::from(format!("turn-mark:{agent}")),
            TurnMark {
                agent: agent.to_owned(),
                turn,
            },
        )
        .into_any_element(),
    )
}

/// The time a row shows before its mark while one runs: the minutes a wait has left, or how
/// long it has waited on the person. It draws itself again each second on its own.
pub fn turn_time(agent: &str, turn: Option<&TurnState>) -> Option<AnyElement> {
    let turn = turn.filter(|turn| turn_line::short(turn, super::time::now_ms()).is_some())?;
    Some(
        Slot::new(
            SharedString::from(format!("turn-time:{agent}")),
            TurnTime { turn: turn.clone() },
        )
        .into_any_element(),
    )
}

struct TurnMark {
    agent: String,
    turn: TurnState,
}

impl Render for TurnMark {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.turn.state == TurnPhase::Waiting {
            clock::second(window, cx);
        }
        match turn_line::mark(&self.turn, super::time::now_ms()) {
            Some(mark) => mark_of(&self.agent, mark, cx),
            None => div().into_any_element(),
        }
    }
}

struct TurnTime {
    turn: TurnState,
}

impl Render for TurnTime {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        clock::second(window, cx);
        let t = Tokens::get(cx);
        let ink = match turn_line::mark(&self.turn, 0) {
            Some(Mark::NeedsYou) => t.warn,
            Some(Mark::Timer) => t.cmd,
            _ => t.text_tertiary,
        };
        div()
            .flex_none()
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .font_features(crate::theme::tabular())
            .text_color(ink)
            .children(turn_line::short(&self.turn, super::time::now_ms()))
    }
}

/// `agent`'s mark for `mark`, in a 16pt box. A new state pops in, and one that now needs the
/// person sends two rings out.
pub fn mark_of(agent: &str, mark: Mark, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let tint = match mark {
        Mark::Working => t.live,
        Mark::NeedsYou => t.warn,
        Mark::Done => t.accent,
        Mark::Failed => t.danger,
        Mark::Timer => t.cmd,
        Mark::Waiting { .. } => t.text_secondary,
        Mark::Stopped => t.text_secondary,
    };
    let (track, kind) = (t.border, kind_of(mark));
    let twinkle = format!("twinkle:{agent}");
    let rings = (mark == Mark::NeedsYou).then(|| {
        motion::replay(
            format!("needs-you:{agent}"),
            kind,
            motion::NEEDS_YOU,
            move |through| needs_you(through, tint),
        )
    });
    let drawn = motion::replay(
        format!("state:{agent}"),
        kind,
        motion::STATE_POP,
        move |popped| {
            let scale = pop_scale(popped);
            let shown = match mark {
                Mark::Working => Slot::new(twinkle, Twinkle { tint, scale }).into_any_element(),
                Mark::Waiting { through } => super::ring(
                    through.map(|through| (f64::from(through) * 100., tint)),
                    11. * scale,
                    1.6,
                    track,
                )
                .into_any_element(),
                Mark::Timer => super::icon(crate::icons::Icon::Timer, 11. * scale)
                    .text_color(tint)
                    .into_any_element(),
                Mark::Stopped => div()
                    .size(px(7. * scale))
                    .rounded_full()
                    .border(px(1.5))
                    .border_color(tint)
                    .into_any_element(),
                Mark::NeedsYou | Mark::Done | Mark::Failed => div()
                    .size(px(7. * scale))
                    .rounded_full()
                    .bg(tint)
                    .into_any_element(),
            };
            div()
                .opacity((popped / 0.6).min(1.))
                .child(shown)
                .into_any_element()
        },
    );
    mark_box(16., drawn)
        .relative()
        .children(rings)
        .into_any_element()
}

/// Which mark it is, apart from how far a ring has filled, so a ring filling never pops.
fn kind_of(mark: Mark) -> u8 {
    match mark {
        Mark::Working => 0,
        Mark::Waiting { .. } => 1,
        Mark::Timer => 2,
        Mark::NeedsYou => 3,
        Mark::Done => 4,
        Mark::Failed => 5,
        Mark::Stopped => 6,
    }
}

/// How large a popping mark is: from 0.4 up past its size by the time it is 60% in, then back.
fn pop_scale(popped: f32) -> f32 {
    match popped {
        p if p < 0.6 => 0.4 + (1.18 - 0.4) * (p / 0.6),
        p => 1.18 - 0.18 * ((p - 0.6) / 0.4),
    }
}

/// The two rings leaving a mark that now needs the person, `through` of the way through both.
fn needs_you(through: f32, tint: Hsla) -> AnyElement {
    let total = motion::NEEDS_YOU.duration.as_secs_f32();
    let ring_time = motion::RING.duration.as_secs_f32();
    let elapsed = through * total;
    let rings = [0., motion::SECOND_RING.as_secs_f32()].map(|delay| {
        let local = ((elapsed - delay) / ring_time).clamp(0., 1.);
        let grown = motion::RING.at(local);
        let size = 12. * (0.6 + 1.8 * grown);
        (local > 0. && local < 1.).then(|| {
            div()
                .absolute()
                .top(px(8. - size / 2.))
                .left(px(8. - size / 2.))
                .size(px(size))
                .rounded_full()
                .border(px(1.5))
                .border_color(tint)
                .opacity(0.9 * (1. - grown))
        })
    });
    div()
        .absolute()
        .inset_0()
        .children(rings.into_iter().flatten())
        .into_any_element()
}

/// The nine cells of a working mark, each brightening and dimming on its own beat.
struct Twinkle {
    tint: Hsla,
    scale: f32,
}

impl Render for Twinkle {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let moving = !cx.reduce_motion() && clock::tick(window, cx);
        let now = clock::seconds();
        let ease = motion::Ease::new(0, Curve::Bezier(0.42, 0., 0.58, 1.));
        let (tint, scale) = (self.tint, self.scale);
        div()
            .w(px(10.5 * scale))
            .flex()
            .flex_wrap()
            .gap(px(1.5 * scale))
            .children(BEATS.iter().map(move |&(beat, ahead)| {
                let lit = match moving {
                    false => 1.,
                    true => {
                        let phase = ((now + ahead) / beat).fract();
                        let rise = match phase < BRIGHTEN {
                            true => ease.at(phase / BRIGHTEN),
                            false => 1. - ease.at((phase - BRIGHTEN) / (1. - BRIGHTEN)),
                        };
                        DIMMEST + (1. - DIMMEST) * rise
                    }
                };
                div()
                    .size(px(2.5 * scale))
                    .rounded(px(0.5 * scale))
                    .bg(tint)
                    .opacity(lit)
            }))
    }
}
