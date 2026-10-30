//! How the chat's turns read: where each answer starts and ends, how long its
//! turn took and how fast it was written, and which finished turns fold their
//! work away behind the reply they closed with.

use sikemux_wire::chat::{ChatPart, ChatRole, RowMeta, WorkFold};

use super::{Fold, Row};

/// Nothing in the session reports how many tokens the agent wrote, so the
/// rate is read off the characters: four per token is where English prose and
/// code both land.
const CHARS_PER_TOKEN: f64 = 4.0;
/// A burst shorter than this is one network hiccup away from any answer at
/// all, so it gets no rate rather than a wrong one.
const MIN_SAMPLE_MS: u64 = 400;

fn is_reply(part: &ChatPart) -> bool {
    matches!(
        part,
        ChatPart::Text { .. } | ChatPart::Content { .. } | ChatPart::Tool { gif: Some(_), .. }
    )
}

fn reply_start(parts: &[ChatPart], end: usize) -> usize {
    let mut start = end;
    while start > 0 && is_reply(&parts[start - 1]) {
        start -= 1;
    }
    start
}

fn written_length(parts: &[ChatPart], start: usize, end: usize) -> u64 {
    parts[start..end]
        .iter()
        .map(|part| match part {
            ChatPart::Text { text, .. } => text.encode_utf16().count() as u64,
            _ => 0,
        })
        .sum()
}

/// An agent often answers, makes one more call, such as stopping a server,
/// and closes with a line about it. The answer before that call is still the
/// reply.
fn answer_before_aside(parts: &[ChatPart], from: usize) -> usize {
    let mut aside = from;
    while aside > 0 && !is_reply(&parts[aside - 1]) {
        aside -= 1;
    }
    if aside == from || aside == 0 {
        return from;
    }
    let answer = reply_start(parts, aside);
    if written_length(parts, answer, aside) > written_length(parts, from, parts.len()) {
        answer
    } else {
        from
    }
}

/// How long the run `start..=end` took. A prompted turn starts with its
/// prompt; one the agent began by itself starts when the core said it did.
fn turn_span(rows: &[Row], start: usize, end: usize) -> Option<u64> {
    let before = start.checked_sub(1).map(|at| &rows[at].msg);
    let began = before
        .filter(|message| message.role == ChatRole::User)
        .and_then(|message| message.sent_at)
        .or_else(|| {
            rows[start]
                .msg
                .opens_turn
                .as_ref()
                .and_then(|opens| opens.at)
        })?;
    let ended = rows[end].msg.ended_at?;
    (ended > began).then(|| ended - began)
}

fn work_fold(rows: &[Row], start: usize, end: usize) -> Option<WorkFold> {
    let parts = &rows[end].msg.parts;
    let closing = reply_start(parts, parts.len());
    if !parts[closing..]
        .iter()
        .any(|part| matches!(part, ChatPart::Text { .. }))
    {
        return None;
    }
    let from = answer_before_aside(parts, closing);
    let (mut calls, mut worked, mut acted) = (0, false, false);
    for (index, row) in rows.iter().enumerate().take(end + 1).skip(start) {
        for (at, part) in row.msg.parts.iter().enumerate() {
            if index == end && at >= from && is_reply(part) {
                continue;
            }
            if matches!(part, ChatPart::Tool { gif: None, .. }) {
                calls += 1;
            }
            worked |= !is_reply(part);
            acted |= !is_reply(part) && !matches!(part, ChatPart::Thought { .. });
        }
    }
    worked.then(|| WorkFold {
        id: rows[start].msg.id.clone(),
        last: rows[end].msg.id.clone(),
        from: from as u64,
        calls,
        took: turn_span(rows, start, end),
        thought: !acted,
    })
}

/// The line under an answer's last message. The agent opens a fresh message
/// every time it comes back from a tool, so the answer is the whole run, and
/// its speed counts the time spent writing, not waiting on tools.
fn answer_meta(rows: &[Row], start: usize, end: usize) -> RowMeta {
    let (mut chars, mut written) = (0u64, 0u64);
    for row in &rows[start..=end] {
        if let Some(stream) = row.stream.filter(|stream| stream.ended > stream.started) {
            chars += stream.chars;
            written += stream.ended - stream.started;
        }
    }
    let rate = (chars > 0 && written >= MIN_SAMPLE_MS)
        .then(|| chars as f64 / CHARS_PER_TOKEN / (written as f64 / 1000.0));
    RowMeta {
        from: rows[start].msg.id.clone(),
        rate,
        at: rows[end].msg.ended_at,
        took: turn_span(rows, start, end),
    }
}

/// Whether the row at `at` opens a run of the agent's messages.
fn opens_run(rows: &[Row], at: usize) -> bool {
    rows[at].msg.role == ChatRole::Assistant
        && (at == 0
            || rows[at - 1].msg.role != ChatRole::Assistant
            || rows[at].msg.opens_turn.is_some())
}

fn set_meta(row: &mut Row, meta: Option<RowMeta>) -> bool {
    if row.msg.meta == meta {
        return false;
    }
    row.msg.meta = meta;
    row.set("meta");
    true
}

fn set_fold(row: &mut Row, fold: Option<WorkFold>) -> bool {
    if row.msg.fold == fold {
        return false;
    }
    row.msg.fold = fold;
    row.set("fold");
    true
}

impl Fold {
    /// Works out the folds and lines again for the runs from the first main
    /// row that changed, which is only ever the end of a long chat. A run's fold
    /// and line count as changed by the latest change to anything they are read from.
    pub(super) fn settle_runs(&mut self) {
        let Some(from) = self.settle_from.take() else {
            return;
        };
        let rows = &mut self.main.rows;
        if rows.is_empty() {
            return;
        }
        let mut start = from.min(rows.len() - 1);
        while start > 0 && !opens_run(rows, start) {
            start -= 1;
        }
        let running = self.head.running;
        let mut touched = Vec::new();
        let mut at = start;
        while at < rows.len() {
            if rows[at].msg.role != ChatRole::Assistant {
                if set_fold(&mut rows[at], None) {
                    touched.push(at);
                }
                at += 1;
                continue;
            }
            let mut end = at;
            while end + 1 < rows.len()
                && rows[end + 1].msg.role == ChatRole::Assistant
                && rows[end + 1].msg.opens_turn.is_none()
            {
                end += 1;
            }
            // The run still being written has no fold or line until its turn ends.
            let last = end == rows.len() - 1;
            let live = running && last;
            let fold = (!live).then(|| work_fold(rows, at, end)).flatten();
            let meta = (!live).then(|| answer_meta(rows, at, end));
            let read_from = rows[at.saturating_sub(1)..=end]
                .iter()
                .map(|row| row.v)
                .chain(last.then_some(self.running_v))
                .max()
                .unwrap_or_default();
            for (index, row) in rows.iter_mut().enumerate().take(end + 1).skip(at) {
                let mut changed = set_fold(row, fold.clone());
                changed |= set_meta(row, meta.clone().filter(|_| index == end));
                if changed {
                    row.v = row.v.max(read_from);
                    touched.push(index);
                }
            }
            at = end + 1;
        }
        for index in touched {
            self.main.queue(index);
        }
    }
}

/// The folds and lines of rows folded on their own, such as an earlier page.
pub(super) fn settle_all(rows: &mut [Row]) {
    let mut at = 0;
    while at < rows.len() {
        if rows[at].msg.role != ChatRole::Assistant {
            at += 1;
            continue;
        }
        let mut end = at;
        while end + 1 < rows.len()
            && rows[end + 1].msg.role == ChatRole::Assistant
            && rows[end + 1].msg.opens_turn.is_none()
        {
            end += 1;
        }
        let fold = work_fold(rows, at, end);
        let meta = answer_meta(rows, at, end);
        for (index, row) in rows.iter_mut().enumerate().take(end + 1).skip(at) {
            row.msg.fold = fold.clone();
            row.msg.meta = (index == end).then(|| meta.clone());
        }
        at = end + 1;
    }
}
