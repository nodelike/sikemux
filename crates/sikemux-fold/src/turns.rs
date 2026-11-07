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
    ) || matches!(part, ChatPart::AgentNotice { notice, .. } if notice.goal.is_some())
}

/// A page the agent showed, a GIF it sent, where a goal began or ended and a subagent still at
/// work stay in view when the turn around them folds.
fn outlives(part: &ChatPart) -> bool {
    match part {
        ChatPart::Tool { page, gif, .. } => page.is_some() || gif.is_some(),
        ChatPart::Subagent { subagent, .. } => subagent.state == SubagentState::Running,
        ChatPart::AgentNotice { notice, .. } => notice.goal.is_some(),
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
fn run_start(parts: &[(usize, usize, &ChatPart)], end: usize) -> usize {
    let mut start = end;
    while start > 0 && is_reply(parts[start - 1].2) {
        start -= 1;
    }
    start
}

fn written(part: &ChatPart) -> u64 {
    match part {
        ChatPart::Text { text, .. } => text.encode_utf16().count() as u64,
        _ => 0,
    }
}

/// An agent often answers, makes more calls, such as waiting on a check or
/// stopping a server, and closes with a line about them, often in a message
/// of its own. A stretch of writing longer than everything written after it
/// is still the reply, so the reply starts at the earliest such stretch.
fn reply_from(parts: &[(usize, usize, &ChatPart)]) -> usize {
    let mut from = run_start(parts, parts.len());
    let mut after: u64 = parts[from..].iter().map(|(_, _, part)| written(part)).sum();
    let mut end = from;
    while end > 0 {
        while end > 0 && !is_reply(parts[end - 1].2) {
            end -= 1;
        }
        let start = run_start(parts, end);
        let run: u64 = parts[start..end]
            .iter()
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

/// A finished turn's fold, and the row and part its reply starts at.
struct TurnFold {
    fold: WorkFold,
    reply: (usize, usize),
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
            .filter(|(at, part)| outlives(part) || (*at >= from && is_reply(part)))
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
    let closing = run_start(parts, parts.len());
    parts[closing..]
        .iter()
        .any(|(_, _, part)| matches!(part, ChatPart::Text { .. }))
}

fn work_fold(rows: &[Row], start: usize, end: usize) -> Option<TurnFold> {
    let parts = turn_parts(rows, start, end);
    if !closes_with_writing(&parts) {
        return None;
    }
    let from = reply_from(&parts);
    let (mut calls, mut worked, mut acted) = (0, false, false);
    for (index, (_, _, part)) in parts.iter().enumerate() {
        if index >= from && is_reply(part) {
            continue;
        }
        if matches!(part, ChatPart::Tool { gif: None, .. }) {
            calls += 1;
        }
        worked |= !is_reply(part);
        acted |= !is_reply(part) && !matches!(part, ChatPart::Thought { .. });
    }
    worked.then(|| TurnFold {
        fold: WorkFold {
            id: rows[start].msg.id.clone(),
            last: rows[end].msg.id.clone(),
            from: 0,
            shown: Vec::new(),
            calls,
            took: turn_span(rows, start, end),
            thought: !acted,
        },
        reply: (parts[from].0, parts[from].1),
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
            let turn = (!live).then(|| work_fold(rows, at, end)).flatten();
            let meta = (!live).then(|| answer_meta(rows, at, end));
            let read_from = rows[at.saturating_sub(1)..=end]
                .iter()
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
