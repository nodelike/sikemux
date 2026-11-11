//! The line under a message while the pointer is on it: write a sent message again, copy
//! it, when it was sent or how long its turn took, and how fast the answer came.

use chrono::{DateTime, Datelike, Local, TimeZone};
use gpui_kit::{
    div, px, AnyElement, App, ClipboardItem, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled,
};
use sikemux_view::chat::{ChatMessage, ChatPart, ChatRole};

use super::{ChatRow, Drawn};
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1, SPACE_2, TEXT_2XS, TEXT_3XS_H};
use crate::ui::{self, IconButton};

/// What a reader means by copying a message: its prose, the reasoning it showed, and the
/// files it was sent with. Tool rows stay behind.
fn prose(message: &ChatMessage) -> Vec<String> {
    let mut blocks: Vec<String> = message
        .parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Text { text, .. } | ChatPart::Thought { text, .. } => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .collect();
    blocks.extend(message.attachments.iter().cloned());
    blocks
}

/// A prompt's own text, or at the end of an answer, all of the answer's prose.
fn copied(rows: &[ChatRow], ix: usize) -> Option<String> {
    let message = &rows.get(ix)?.message;
    let from = if message.role == ChatRole::User {
        ix
    } else {
        let start = &message.meta.as_ref()?.from;
        rows.iter()
            .position(|row| &row.message.id == start)
            .unwrap_or(ix)
    };
    let text = rows[from..=ix]
        .iter()
        .flat_map(|row| prose(&row.message))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// When a message was sent, as a reader scanning the chat wants it: "Today, 21:07",
/// "Yesterday, 14:02", "29 Sep, 09:15", or under the person's own message today, "21:07".
fn sent(at: u64, mine: bool) -> Option<String> {
    let at: DateTime<Local> = Local.timestamp_millis_opt(at as i64).single()?;
    let now = Local::now();
    let days = now
        .date_naive()
        .signed_duration_since(at.date_naive())
        .num_days();
    let time = at.format("%H:%M");
    Some(match days {
        0 if mine => time.to_string(),
        0 => format!("Today, {time}"),
        1 => format!("Yesterday, {time}"),
        _ if at.year() == now.year() => format!("{}, {time}", at.format("%-d %b")),
        _ => format!("{}, {time}", at.format("%-d %b %Y")),
    })
}

/// When a message came, as the line over a teammate's bubble says it.
pub fn clock(at: u64) -> Option<String> {
    sent(at, true)
}

fn rate(rate: f64) -> String {
    if rate < 10. {
        format!("~{rate:.1} tok/s")
    } else {
        format!("~{} tok/s", rate.round())
    }
}

/// The line under row `ix`, shown while the pointer is on `group`. An agent's row has one only
/// where it closes an answer and no messages between agents follow, since the line's height is
/// held even while hidden and would open a hole in that run.
pub fn line(
    rows: &[ChatRow],
    ix: usize,
    group: &SharedString,
    looks: &Drawn,
    cx: &App,
) -> Option<AnyElement> {
    let message = &rows.get(ix)?.message;
    let user = message.role == ChatRole::User;
    let meta = message.meta.as_ref();
    let run_follows = rows
        .get(ix + 1)
        .is_some_and(|next| next.message.team.is_some());
    if !user && (meta.is_none() || run_follows) {
        return None;
    }
    let t = Tokens::get(cx);
    let copy = copied(rows, ix).map(|text| {
        let label = if user { "Copy message" } else { "Copy reply" };
        IconButton::new(
            SharedString::from(format!("copy:{}", message.id)),
            Icon::Copy,
            label,
        )
        .glyph(13.)
        .width(24.)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
    });
    let editable =
        user && looks.reading.editable && message.prompt_id.is_some() && message.context.is_empty();
    let edit = editable.then(|| {
        let text = super::edit::written(message);
        let (bench, agent, row) = (looks.bench.clone(), looks.agent.clone(), message.id.clone());
        IconButton::new(
            SharedString::from(format!("edit:{}", message.id)),
            Icon::Pencil,
            "Edit message",
        )
        .glyph(13.)
        .width(24.)
        .on_click(move |_, window, cx| {
            bench
                .update(cx, |bench, cx| {
                    bench.edit_message(&agent, &row, text.clone(), window, cx)
                })
                .ok();
        })
    });
    let at = meta
        .and_then(|meta| meta.at)
        .or(message.sent_at)
        .and_then(|at| sent(at, user));
    let took = meta.and_then(|meta| meta.took);
    let when = at.map(|at| match took {
        Some(took) => format!("{at} · took {}", crate::ui::time::took(took)),
        None => at,
    });
    let speed = meta.and_then(|meta| meta.rate).map(rate);
    if user {
        let actions = div()
            .flex()
            .items_center()
            .invisible()
            .group_hover(group.clone(), |actions| actions.visible())
            .children(edit)
            .children(copy);
        return Some(
            div()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_end()
                .gap(SPACE_1)
                .child(actions)
                .children(when.map(|when| {
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_disabled)
                        .child(when)
                }))
                .into_any_element(),
        );
    }
    Some(
        div()
            .h(px(24.))
            .mt(SPACE_1)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .invisible()
            .group_hover(group.clone(), |line| line.visible())
            .children(copy)
            .children(when.map(|when| ui::mono(when, TEXT_3XS_H, cx).text_color(t.text_disabled)))
            .children(
                speed.map(|speed| ui::mono(speed, TEXT_3XS_H, cx).text_color(t.text_disabled)),
            )
            .into_any_element(),
    )
}
