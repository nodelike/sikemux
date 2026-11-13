//! While you were away: on the chief's chat, pinned at its end until the core takes it away, the
//! calls that wait on the person as a queue, what agents decided for them and what landed.

use super::*;
use crate::ui::Clicks;
use sikemux_view::chat::{AwayDecision, AwayLanded, ChiefAway};
use sikemux_view::turn::TurnStep;

/// The block as the core sent it, with its calls as the device view lists them: those answered
/// since it began, then those open.
#[derive(Clone, PartialEq)]
pub(crate) struct Away {
    pub(crate) head: ChiefAway,
    pub(crate) calls: Vec<PersonCall>,
}

/// The call of the block drawn open: the one the person opened, else its first still open.
pub(super) fn open_of<'a>(
    ids: &[String],
    opened: Option<&str>,
    call: impl Fn(&str) -> Option<&'a PersonCall>,
) -> Option<&'a PersonCall> {
    let open = |id: &str| call(id).filter(|call| call.state == CallState::Open);
    (opened.filter(|id| ids.iter().any(|shown| shown == id)))
        .and_then(open)
        .or_else(|| ids.iter().find_map(|id| open(id)))
}

pub(super) fn block(
    away: &Away,
    through: Through,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(bench) = through.bench.upgrade() else {
        return div().into_any_element();
    };
    let (cards, target) = (through.cards.clone(), through.target.clone());
    let opened = cards.read(cx).opened.clone();
    let open = open_of(&away.head.calls, opened.as_deref(), |id| {
        away.calls.iter().find(|call| call.id == id)
    });
    let waiting: Vec<&PersonCall> = (away.calls.iter())
        .filter(|call| call.state == CallState::Open)
        .collect();
    let queue = open.map(|open| {
        let mark = asker_mark(bench.read(cx), open, FACE, cx);
        let at = 1
            + (away.calls.iter())
                .position(|call| call.id == open.id)
                .unwrap_or_default();
        let seat = Seat::Queue {
            at,
            of: away.calls.len(),
        };
        let shown = card(open, mark, seat, through, window, cx);
        let answered: Vec<AnyElement> = (away.calls.iter())
            .filter(|call| call.state != CallState::Open)
            .filter_map(|call| answered_row(call, &t))
            .collect();
        let rest: Vec<AnyElement> = (waiting.iter())
            .filter(|call| call.id != open.id)
            .map(|call| folded(bench.read(cx), &cards, call, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .when(!answered.is_empty(), |queue| {
                queue.child(div().w(px(CARD_WIDTH)).max_w_full().children(answered))
            })
            .child(panel(&t).child(shown))
            .child(div().w(px(CARD_WIDTH)).max_w_full().children(rest))
    });
    let needs = match away.calls.len() - waiting.len() {
        0 => waiting.len().to_string(),
        _ => t!("{count} left", count = waiting.len()),
    };
    let decided = (away.head.decided.iter())
        .map(|decision| decided_row(decision, &bench, &target, &t))
        .collect::<Vec<_>>();
    let landed = (away.head.landed.iter())
        .map(|landed| landed_row(landed, &bench, &t, cx))
        .collect::<Vec<_>>();
    let gone = away.head.back.map(|back| {
        let gone = back.saturating_sub(away.head.since) / 60_000;
        match (gone / 60, gone % 60) {
            (0, minutes) => t!("{minutes} min", minutes = minutes),
            (hours, 0) => t!("{hours} h", hours = hours),
            (hours, minutes) => t!("{hours} h {minutes} min", hours = hours, minutes = minutes),
        }
    });
    let head = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .text_size(theme::ui_text(13.))
        .child(ui::chief_mark(16.))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .child(t!("While you were away")),
        )
        .children(gone.map(|gone| {
            div()
                .text_size(theme::ui_text(11.5))
                .text_color(t.text_tertiary)
                .child(format!("· {gone}"))
        }));
    div()
        .w_full()
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .bg(t.surface_raised)
        .border_1()
        .border_color(t.line)
        .rounded(RADIUS_4)
        .child(head)
        .when_some(queue, |block, queue| {
            block
                .child(section(t!("Needs you"), needs, &t, cx))
                .child(queue)
        })
        .when(!decided.is_empty(), |block| {
            block
                .child(section(
                    t!("Decided for you"),
                    decided.len().to_string(),
                    &t,
                    cx,
                ))
                .children(decided)
        })
        .when(!landed.is_empty(), |block| {
            block
                .child(section(t!("Landed"), landed.len().to_string(), &t, cx))
                .children(landed)
        })
        .into_any_element()
}

/// A part's label in spaced capitals, how many it holds, and a rule to the edge.
fn section(label: &str, count: String, t: &Tokens, cx: &App) -> AnyElement {
    div()
        .pt(SPACE_1)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::caps(label, cx))
        .child(
            div()
                .text_size(theme::ui_text(11.))
                .text_color(t.text_tertiary)
                .child(count),
        )
        .child(div().flex_1().h(px(1.)).bg(t.line))
        .into_any_element()
}

/// A call of the block the person answered, folded to one dim line: what was asked, how they
/// answered, and how long ago.
fn answered_row(call: &PersonCall, t: &Tokens) -> Option<AnyElement> {
    let answer = call.answer.as_ref()?;
    Some(
        line(t)
            .pr(px(10.))
            .text_color(t.text_tertiary)
            .child(
                div()
                    .w(px(20.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(ui::icon(Icon::Check, 12.).text_color(t.text_tertiary)),
            )
            .child(div().min_w_0().truncate().child(call.question.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(60.))
                    .truncate()
                    .text_size(theme::ui_text(11.5))
                    .child(answer_words(call, answer)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(theme::ui_text(11.5))
                    .font_features(theme::tabular())
                    .child(ui::time::age(answer.at)),
            )
            .into_any_element(),
    )
}

fn line(t: &Tokens) -> gpui_kit::Div {
    div()
        .h(px(30.))
        .pl(SPACE_2)
        .flex()
        .items_center()
        .gap(px(9.))
        .text_size(theme::ui_text(13.))
        .text_color(t.text_primary)
}

fn decided_row(
    decision: &AwayDecision,
    bench: &Entity<Workbench>,
    target: &Target,
    t: &Tokens,
) -> AnyElement {
    let after =
        (decision.why.clone()).or_else(|| decision.by.as_ref().map(|by| format!("@{}", by.name)));
    let (bench, target, overruled) = (bench.downgrade(), target.clone(), decision.clone());
    let id = format!(
        "away-overrule-{}-{}",
        decision.goal.unwrap_or_default(),
        decision.decision
    );
    line(t)
        .child(
            div()
                .w(px(20.))
                .flex_none()
                .flex()
                .justify_center()
                .child(ui::icon(Icon::Check, 12.).text_color(t.text_tertiary)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .when(decision.overruled, |text| {
                    text.line_through().text_color(t.text_tertiary)
                })
                .child(decision.text.clone()),
        )
        .children(after.map(|after| {
            div()
                .flex_1()
                .min_w(px(80.))
                .truncate()
                .text_color(t.text_tertiary)
                .child(format!("· {after}"))
        }))
        .when(!decision.overruled, |row| {
            row.child(
                div()
                    .id(SharedString::from(id))
                    .flex_none()
                    .pr(SPACE_2)
                    .text_size(theme::ui_text(12.))
                    .text_color(t.text_secondary)
                    .hover(|word| word.text_color(t.text_primary))
                    .child(t!("Overrule"))
                    .clicks(move |_, _, cx| {
                        let _ = bench.update(cx, |bench, cx| {
                            crate::playbook::overrule(bench, &target, &overruled, cx)
                        });
                    }),
            )
        })
        .into_any_element()
}

fn landed_row(landed: &AwayLanded, bench: &Entity<Workbench>, t: &Tokens, cx: &App) -> AnyElement {
    let mark = (bench.read(cx).agent_by_id(&landed.agent_id)).map(|agent| agent.mark(FACE, cx));
    let weak = bench.downgrade();
    let agent = landed.agent_id.clone();
    line(t)
        .child(
            div()
                .w(px(20.))
                .flex_none()
                .flex()
                .justify_center()
                .children(mark),
        )
        .child(div().min_w_0().truncate().child(landed.title.clone()))
        .children(landed.name.as_ref().map(|name| {
            div()
                .flex_none()
                .text_color(t.text_tertiary)
                .child(format!("· @{name}"))
        }))
        .children(landed.change.as_ref().map(|change| {
            div()
                .min_w_0()
                .flex_shrink(1000.)
                .truncate()
                .text_color(t.text_tertiary)
                .child(format!("· {change}"))
        }))
        .child(div().flex_1().min_w(SPACE_2))
        .when(landed.try_it, |row| {
            let label = crate::turn_line::step_label(TurnStep::TryIt).unwrap_or_default();
            row.child(
                Button::new(
                    SharedString::from(format!("away-try-{}-{}", landed.agent_id, landed.at)),
                    label,
                    Variant::Primary,
                )
                .sized(22., 10., 12.)
                .on_click(move |_, _, cx| {
                    crate::chat::take_step(&weak, &agent, TurnStep::TryIt, cx)
                }),
            )
        })
        .into_any_element()
}
