//! The line under a message while the pointer is on it: write a sent message again, copy
//! it, when it was sent or how long its turn took, and how fast the answer came.

use chrono::{DateTime, Datelike, Local, TimeZone};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClipboardItem, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled,
};
use sikemux_wire::chat::{ChatMessage, ChatPart, ChatRole};

use super::{duration, ChatRow, Drawn};
use crate::icons::Icon;
use crate::theme::Tokens;
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
/// "Yesterday, 14:02", "29 Sep, 09:15".
fn sent(at: u64) -> Option<String> {
    let at: DateTime<Local> = Local.timestamp_millis_opt(at as i64).single()?;
    let now = Local::now();
    let days = now
        .date_naive()
        .signed_duration_since(at.date_naive())
        .num_days();
    let time = at.format("%H:%M");
    Some(match days {
        0 => format!("Today, {time}"),
        1 => format!("Yesterday, {time}"),
        _ if at.year() == now.year() => format!("{}, {time}", at.format("%-d %b")),
        _ => format!("{}, {time}", at.format("%-d %b %Y")),
    })
}

fn rate(rate: f64) -> String {
    if rate < 10. {
        format!("~{rate:.1} tok/s")
    } else {
        format!("~{} tok/s", rate.round())
    }
}

/// The line under row `ix`, shown while the pointer is on `group`; none for an agent's
/// row that does not close an answer.
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
    if !user && meta.is_none() {
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
        .and_then(sent);
    let took = meta.and_then(|meta| meta.took).filter(|_| !user);
    let when = at.map(|at| match took {
        Some(took) => format!("{at} · took {}", duration(took)),
        None => at,
    });
    let speed = meta.and_then(|meta| meta.rate).filter(|_| !user).map(rate);
    Some(
        div()
            .h(px(24.))
            .mt(px(4.))
            .flex()
            .items_center()
            .gap(px(8.))
            .when(user, |line| line.justify_end())
            .invisible()
            .group_hover(group.clone(), |line| line.visible())
            .children(edit)
            .children(copy)
            .children(when.map(|when| ui::mono(when, 10.5, cx).text_color(t.text_disabled)))
            .children(speed.map(|speed| ui::mono(speed, 10.5, cx).text_color(t.text_disabled)))
            .into_any_element(),
    )
}
