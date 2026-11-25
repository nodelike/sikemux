//! How the chat's turns read: where each answer starts and ends, how long its
//! turn took and how fast it was written, and which finished turns fold their
//! work away behind the reply they closed with.

use sikemux_view::chat::{ChatPart, ChatRole, RowMeta, SubagentState, WorkFold};

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
    ) || matches!(part, ChatPart::AgentNotice { notice, .. } if notice.goal.is_some() || notice.decided.is_some())
}

/// A message that went to another agent, which is what a turn that wrote nothing at its end came to.
fn is_sent(part: &ChatPart) -> bool {
    matches!(
        part,
        ChatPart::Tool {
            sent: Some(_),
            failure: None,
            ..
        }
    )
}

fn is_reply_or_sent(part: &ChatPart) -> bool {
    is_reply(part) || is_sent(part)
}

/// A page the agent or one of its workers showed, an MCP app a call opened, a GIF it sent, where a goal began or ended, a rule kept from the
/// person's words, a decision or its Overrule, a week's count, a subagent still at work and the line of background
/// tasks that ended stay in view when the turn around them folds.
fn outlives(part: &ChatPart) -> bool {
    match part {
        ChatPart::Tool { page, app, gif, .. } => page.is_some() || app.is_some() || gif.is_some(),
        ChatPart::Subagent { subagent, .. } => subagent.state == SubagentState::Running,
        ChatPart::Tasks { .. } => true,
        ChatPart::AgentNotice { notice, .. } => {
            notice.goal.is_some()
                || notice.rule.is_some()
                || notice.overruled.is_some()
                || notice.decided.is_some()
                || notice.week.is_some()
                || notice.rule_change.is_some()
                || notice.worker_page.is_some()
        }
        _ => false,
    }
}

/// The turn's parts in order, each with the row it is in and where in that row.
fn turn_parts(rows: &[Row], start: usize, end: usize) -> Vec<(usize, usize, &ChatPart)> {
    rows.iter()
        .enumerate()
        .take(end + 1)
        .skip(start)
        .flat_map(|(row, it)| {
            it.msg
                .parts
                .iter()
                .enumerate()
                .map(move |(at, part)| (row, at, part))
        })
        .collect()
}

/// Where the run of reply parts ending at `end` starts.
fn run_start(
    parts: &[(usize, usize, &ChatPart)],
    end: usize,
    is_reply: fn(&ChatPart) -> bool,
) -> usize {
    let replies = (parts.iter().take(end).rev())
        .take_while(|(_, _, part)| is_reply(part))
        .count();
    end - replies
}

fn written(part: &ChatPart) -> u64 {
    match part {
        ChatPart::Text { text, .. } => text.encode_utf16().count() as u64,
        ChatPart::Tool {
            sent: Some(sent), ..
        } => sent.n,
        _ => 0,
    }
}

/// An agent often answers, makes more calls, such as waiting on a check or
/// stopping a server, and closes with a line about them, often in a message
/// of its own. A stretch of writing longer than everything written after it
/// is still the reply, so the reply starts at the earliest such stretch.
fn reply_from(parts: &[(usize, usize, &ChatPart)], is_reply: fn(&ChatPart) -> bool) -> usize {
    let mut from = run_start(parts, parts.len(), is_reply);
    let mut after: u64 = (parts.iter().skip(from))
        .map(|(_, _, part)| written(part))
        .sum();
    let mut end = from;
    while end > 0 {
        end -= (parts.iter().take(end).rev())
            .take_while(|(_, _, part)| !is_reply(part))
            .count();
        let start = run_start(parts, end, is_reply);
        let run: u64 = (parts.iter().take(end).skip(start))
            .map(|(_, _, part)| written(part))
            .sum();
        if run > after {
            from = start;
        }
        after += run;
        end = start;
    }
    from
}

/// How long the run `start..=end` took. A prompted turn starts with its
/// prompt; one the agent began by itself starts when the core said it did.
fn turn_span(rows: &[Row], start: usize, end: usize) -> Option<u64> {
    let before = (start.checked_sub(1))
        .and_then(|at| rows.get(at))
        .map(|row| &row.msg);
    let began = before
        .filter(|message| message.role == ChatRole::User)
        .and_then(|message| message.sent_at)
        .or_else(|| {
            (rows.get(start))
                .and_then(|row| row.msg.opens_turn.as_ref())
                .and_then(|opens| opens.at)
        })?;
    let ended = rows.get(end)?.msg.ended_at?;
    (ended > began).then(|| ended - began)
}

/// A finished turn's fold, and the row and part its reply starts at.
struct TurnFold {
    fold: WorkFold,
    reply: (usize, usize),
    /// The turn wrote nothing at its end, so the messages it sent stay in view.
    sent_shown: bool,
}

impl TurnFold {
    /// The fold as the row at `index` carries it, saying where the reply starts among its parts.
    fn for_row(&self, index: usize, parts: &[ChatPart]) -> WorkFold {
        let (row, at) = self.reply;
        let from = match index.cmp(&row) {
            std::cmp::Ordering::Less => parts.len(),
            std::cmp::Ordering::Equal => at,
            std::cmp::Ordering::Greater => 0,
        };
        let shown = parts
            .iter()
            .enumerate()
            .filter(|(at, part)| {
                outlives(part)
                    || (*at >= from && is_reply(part))
                    || (self.sent_shown && is_sent(part))
            })
            .map(|(at, _)| at as u64)
            .collect();
        WorkFold {
            from: from as u64,
            shown,
            ..self.fold.clone()
        }
    }
}

fn closes_with_writing(parts: &[(usize, usize, &ChatPart)]) -> bool {
    let closing = run_start(parts, parts.len(), is_reply);
    (parts.iter().skip(closing)).any(|(_, _, part)| matches!(part, ChatPart::Text { .. }))
}

/// A finished turn folds behind the writing it closed with, or the messages it sent when it
/// closed with none. One that was stopped or failed before closing with any stays open.
fn work_fold(rows: &[Row], start: usize, end: usize) -> Option<TurnFold> {
    let parts = turn_parts(rows, start, end);
    let wrote = closes_with_writing(&parts);
    let finished = || {
        rows.iter()
            .take(end + 1)
            .skip(start)
            .any(|row| row.finished)
    };
    if !wrote && !finished() {
        return None;
    }
    let is_reply: fn(&ChatPart) -> bool = if wrote { is_reply } else { is_reply_or_sent };
    let from = reply_from(&parts, is_reply);
    let (first, last) = (rows.get(start)?, rows.get(end)?);
    let reply = (parts.get(from)).map_or((end + 1, 0), |(row, at, _)| (*row, *at));
    let (mut calls, mut worked, mut acted) = (0, false, false);
    for (index, (_, _, part)) in parts.iter().enumerate() {
        if (index >= from && is_reply(part)) || (!wrote && is_sent(part)) {
            continue;
        }
        if let ChatPart::Tool {
            tool, gif: None, ..
        } = part
        {
            calls += tool.row.calls();
        }
        worked |= !is_reply(part);
        acted |= !is_reply(part) && !matches!(part, ChatPart::Thought { .. });
    }
    worked.then(|| TurnFold {
        fold: WorkFold {
            id: first.msg.id.clone(),
            last: last.msg.id.clone(),
            from: 0,
            shown: Vec::new(),
            calls,
            took: turn_span(rows, start, end),
            thought: !acted,
        },
        reply,
        sent_shown: !wrote,
    })
}

/// The line under an answer's last message. The agent opens a fresh message
/// every time it comes back from a tool, so the answer is the whole run, and
/// its speed counts the time spent writing, not waiting on tools.
fn answer_meta(rows: &[Row], start: usize, end: usize) -> RowMeta {
    let (mut chars, mut written) = (0u64, 0u64);
    for row in rows.iter().take(end + 1).skip(start) {
        if let Some(stream) = row.stream {
            let (row_chars, row_ms) = stream.totals();
            chars += row_chars;
            written += row_ms;
        }
    }
    let rate = (chars > 0 && written >= MIN_SAMPLE_MS)
        .then(|| chars as f64 / CHARS_PER_TOKEN / (written as f64 / 1000.0));
    RowMeta {
        from: (rows.get(start))
            .map(|row| row.msg.id.clone())
            .unwrap_or_default(),
        rate,
        at: rows.get(end).and_then(|row| row.msg.ended_at),
        took: turn_span(rows, start, end),
    }
}

fn opens_run(rows: &[Row], at: usize) -> bool {
    let Some(row) = rows.get(at) else {
        return false;
    };
    let before = at.checked_sub(1).and_then(|before| rows.get(before));
    row.msg.role == ChatRole::Assistant
        && (before.is_none_or(|before| before.msg.role != ChatRole::Assistant)
            || row.msg.opens_turn.is_some())
}

/// The last row of the run of the agent's rows that starts at `at`.
fn run_end(rows: &[Row], at: usize) -> usize {
    let more = (rows.iter().skip(at + 1))
        .take_while(|row| row.msg.role == ChatRole::Assistant && row.msg.opens_turn.is_none())
        .count();
    at + more
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
        while let Some(row) = rows.get_mut(at) {
            if row.msg.role != ChatRole::Assistant {
                if set_fold(row, None) {
                    touched.push(at);
                }
                at += 1;
                continue;
            }
            let end = run_end(rows, at);
            // The run still being written has no fold or line until its turn ends.
            let last = end == rows.len() - 1;
            let live = running && last;
            let turn = (!live).then(|| work_fold(rows, at, end)).flatten();
            let meta = (!live).then(|| answer_meta(rows, at, end));
            let read_from = (rows.iter().take(end + 1).skip(at.saturating_sub(1)))
                .map(|row| row.v)
                .chain(last.then_some(self.running_v))
                .max()
                .unwrap_or_default();
            for (index, row) in rows.iter_mut().enumerate().take(end + 1).skip(at) {
                let fold = turn
                    .as_ref()
                    .map(|turn| turn.for_row(index, &row.msg.parts));
                let mut changed = set_fold(row, fold);
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
        self.settle_team(from);
    }
}

/// The folds and lines of rows folded on their own, such as an earlier page.
pub(super) fn settle_all(rows: &mut [Row]) {
    let mut at = 0;
    while let Some(row) = rows.get(at) {
        if row.msg.role != ChatRole::Assistant {
            at += 1;
            continue;
        }
        let end = run_end(rows, at);
        let turn = work_fold(rows, at, end);
        let meta = answer_meta(rows, at, end);
        for (index, row) in rows.iter_mut().enumerate().take(end + 1).skip(at) {
            row.msg.fold = turn
                .as_ref()
                .map(|turn| turn.for_row(index, &row.msg.parts));
            row.msg.meta = (index == end).then(|| meta.clone());
        }
        at = end + 1;
    }
    super::team::settle_page(rows);
}
