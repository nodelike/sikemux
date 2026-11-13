//! The line a fresh session under a chat leaves: when, why, and, opened, the brief it started
//! from as one contained block.

use super::*;
use sikemux_core::server::call;
use sikemux_core::server::chat::rotation::{RotationBriefParams, RotationBriefRead};
use sikemux_view::chat::{BriefSection, BriefSectionKind, RotationReason, SessionRotation};

use crate::theme::{SPACE_1, SPACE_2, SPACE_3, TEXT_2XS_H, TEXT_XS_H};

pub(super) fn line(rotation: &SessionRotation, looks: &Drawn, cx: &App) -> AnyElement {
    use chrono::TimeZone;
    let key = format!("rotation:{}", rotation.at);
    let open = looks.reading.opened.contains(&key);
    let why = match rotation.reason {
        RotationReason::Full => Some(t!("the old one was full")),
        RotationReason::Away => Some(t!("you came back after the old one went cold")),
        RotationReason::Model => Some(t!("the chat moved to another model")),
        RotationReason::Account => Some(t!("the chat moved to another account")),
        RotationReason::Unknown => None,
    };
    let at = (chrono::Local
        .timestamp_millis_opt(rotation.at as i64)
        .single())
    .map(|at| at.format("%H:%M").to_string())
    .unwrap_or_default();
    let mut words = t!("New session").to_owned();
    for part in why.into_iter().map(str::to_owned).chain([at]) {
        words.push_str(" · ");
        words.push_str(&part);
    }
    let reader = looks.reader.clone();
    let toggled = key.clone();
    let head = notice_box(looks)
        .id(SharedString::from(key))
        .cursor_pointer()
        .child(ui::icon(Icon::Refresh, 12.).text_color(looks.quiet))
        .child(div().min_w_0().truncate().child(words))
        .child(motion::turn(
            format!("rotation-chevron:{}", rotation.at),
            motion::fold_angle(open),
            motion::DETAIL_FOLD,
            ui::icon(Icon::Chevron, 10.).text_color(looks.quiet),
        ))
        .on_click(move |_, _, cx| {
            reader.update(cx, |reading, cx| {
                reading.toggle(&toggled);
                cx.notify();
            })
        });
    div()
        .flex()
        .flex_col()
        .child(head)
        .when(open, |line| line.child(brief(rotation, looks, cx)))
        .into_any_element()
}

fn brief(rotation: &SessionRotation, looks: &Drawn, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let whole = looks.reading.briefs.get(&rotation.at);
    let read = whole.and_then(|whole| whole.as_ref().ok());
    let sections = (rotation.brief.sections.iter()).map(|section| {
        let text = (read
            .and_then(|brief| brief.sections.iter().find(|read| read.kind == section.kind)))
        .map(|read| read.text.clone())
        .filter(|text| !text.is_empty());
        div()
            .py(SPACE_1)
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .flex_none()
                            .text_color(looks.ink)
                            .child(title(section)),
                    )
                    .children(counts(section).map(|counts| {
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(looks.quiet)
                            .child(counts)
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .font_features(theme::tabular())
                            .text_color(looks.faint)
                            .child(tokens(section.tokens)),
                    ),
            )
            .children(text.map(|text| div().pl(SPACE_3).text_color(looks.quiet).child(text)))
    });
    let (bench, reader, at) = (looks.bench.clone(), looks.reader.clone(), rotation.at);
    let agent = looks.agent.clone();
    let action = match whole {
        Some(Err(error)) => div()
            .text_color(t.danger)
            .child(error.clone())
            .into_any_element(),
        Some(Ok(_)) => div().into_any_element(),
        None => div()
            .id(SharedString::from(format!("rotation-read:{at}")))
            .cursor_pointer()
            .text_color(looks.ink)
            .child(t!("Read the whole brief"))
            .on_click(move |_, _, cx| read_brief(&bench, &reader, &agent, at, cx))
            .into_any_element(),
    };
    div()
        .ml(SPACE_3)
        .mt(SPACE_1)
        .mb(SPACE_2)
        .px(SPACE_3)
        .py(SPACE_2)
        .flex()
        .flex_col()
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .bg(t.surface_raised)
        .border_1()
        .border_color(t.line)
        .rounded(theme::RADIUS_3)
        .children(sections)
        .child(div().my(SPACE_1).h(px(1.)).bg(t.line))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .child(div().text_color(looks.faint).child(t!(
                    "{tokens} in all",
                    tokens = tokens(rotation.brief.tokens)
                )))
                .child(div().flex_1())
                .child(action),
        )
        .into_any_element()
}

fn title(section: &BriefSection) -> String {
    match (section.kind, section.counts.exchanges) {
        (BriefSectionKind::Exchanges, Some(1)) => {
            return t!("Your last exchange, word for word").to_owned()
        }
        (BriefSectionKind::Exchanges, Some(n)) => {
            return t!("Your last {n} exchanges, word for word", n = n)
        }
        _ => {}
    }
    match section.kind {
        BriefSectionKind::Seat => t!("How the chief works, and your rules"),
        BriefSectionKind::Decisions => t!("Decisions"),
        BriefSectionKind::Work => t!("Work in hand"),
        BriefSectionKind::Handoff => t!("Handoff note from the old session"),
        BriefSectionKind::Exchanges => t!("Your last exchanges, word for word"),
        BriefSectionKind::Unknown => t!("More"),
    }
    .to_owned()
}

/// What a section holds, as "9 rules" or "4 workers, 2 waits, 1 open call".
fn counts(section: &BriefSection) -> Option<String> {
    let counts = &section.counts;
    let say = |count: Option<u32>, one: &str, many: &str| {
        let count = count?;
        let said = match count {
            1 => crate::i18n::tr(one),
            _ => crate::i18n::tr(many),
        };
        Some(crate::i18n::fill(said, &[("n", &count)]))
    };
    let decisions = match (counts.decisions, counts.days) {
        (Some(decisions), Some(1)) => Some(t!("{n}, the last day", n = decisions)),
        (Some(decisions), Some(days)) => {
            Some(t!("{n}, the last {days} days", n = decisions, days = days))
        }
        (decisions, None) => say(decisions, n!("{n} decision"), n!("{n} decisions")),
        (None, _) => None,
    };
    let said: Vec<String> = [
        say(counts.rules, n!("{n} rule"), n!("{n} rules")),
        decisions,
        say(counts.workers, n!("{n} worker"), n!("{n} workers")),
        say(counts.waits, n!("{n} wait"), n!("{n} waits")),
        say(counts.calls, n!("{n} open call"), n!("{n} open calls")),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!said.is_empty()).then(|| said.join(", "))
}

/// About how many tokens, as "2.1K".
fn tokens(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        _ => format!("{:.1}K", tokens as f64 / 1000.),
    }
}

fn read_brief(
    bench: &WeakEntity<Workbench>,
    reader: &Entity<Reading>,
    agent: &str,
    at: u64,
    cx: &mut App,
) {
    let Some(bench) = bench.upgrade() else {
        return;
    };
    let asked = (bench.read(cx).chat_target(agent)).request_typed::<call::RotationBrief>(
        RotationBriefParams {
            agent_id: agent.to_owned(),
            at,
        },
    );
    let reader = reader.downgrade();
    cx.spawn(async move |cx| {
        let read = asked.await.map(|read: RotationBriefRead| read.brief);
        let _ = reader.update(cx, |reading, cx| {
            reading.briefs.insert(at, read);
            cx.notify();
        });
    })
    .detach();
}
