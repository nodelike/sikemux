//! What clients are sent of a fold: the changes since the last flush, a
//! window of the latest rows for a client opening the chat, what a client
//! coming back missed, and pages of earlier rows.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Map, Value};
use sikemux_markdown::{Doc, Op, Options};

use sikemux_wire::chat::{
    ChatChange, ChatMessage, ChatPage, ChatPart, ChatRole, ChatView, PartTree, SubagentCard,
};

use super::event::ChatEvent;
use super::{tools, Fold, Row, Side, Transcript};

/// How an agent's answers are read as markdown.
const ANSWER: Options = Options {
    gfm: true,
    html_as_text: false,
    file_links: true,
    html_images: false,
    file_paths: true,
};

/// What the person sent reads as the agent's text does, but shows any markup they typed as they
/// typed it.
const TYPED: Options = Options {
    html_as_text: true,
    ..ANSWER
};

/// How a role's texts are read into trees.
fn reading(role: ChatRole) -> Options {
    match role {
        ChatRole::Assistant => ANSWER,
        ChatRole::User => TYPED,
    }
}

/// A client opening a chat is sent its last 40 turns or 1 MB of their JSON, whichever is
/// less, and pages back through the rest. A turn longer than that, such as one full of
/// screenshots, is cut between its rows; the last row always goes whole.
pub const TAIL_TURNS: usize = 40;
const TAIL_BYTES: usize = 1024 * 1024;
/// The most of a subagent's rows a client opening the chat is sent: its latest.
const SUBAGENT_TAIL_ROWS: usize = 40;
/// The most of a long chat the fold holds; older rows page back from the
/// chat's events.
const FOLD_BYTES: usize = 8 * 1024 * 1024;
/// What a part costs beyond its text, roughly.
const PART_BYTES: usize = 160;

/// Which rows of a chat a page holds, by the event that opened each.
#[derive(Clone, Copy, Debug)]
pub enum PageOf {
    /// Those opened before the event.
    Before(u64),
    /// Those opened after the event.
    After(u64),
    /// Those opened by the event or after it.
    From(u64),
}

impl PageOf {
    fn keeps(self, index: u64) -> bool {
        match self {
            PageOf::Before(before) => index < before,
            PageOf::After(after) => index > after,
            PageOf::From(from) => index >= from,
        }
    }
}

/// The most of a page's JSON a client is sent at once, as of the window it opened with.
pub const PAGE_BYTES: usize = TAIL_BYTES;

/// Keeps as many of `rows` as stay under `budget` bytes of JSON, whole, counted from the
/// newest when `newest` and from the oldest otherwise; at least one stays. Says whether any went.
fn within(rows: &mut Vec<ChatMessage>, budget: usize, newest: bool) -> bool {
    let sizes: Vec<usize> = rows
        .iter()
        .map(|row| serde_json::to_vec(row).map_or(0, |json| json.len()))
        .collect();
    let mut total = 0;
    let mut kept = 0;
    let order: Box<dyn Iterator<Item = &usize>> = match newest {
        true => Box::new(sizes.iter().rev()),
        false => Box::new(sizes.iter()),
    };
    for size in order {
        if kept > 0 && total + size > budget {
            break;
        }
        total += size;
        kept += 1;
    }
    let gone = rows.len() - kept;
    match newest {
        true => drop(rows.drain(..gone)),
        false => rows.truncate(kept),
    }
    gone > 0
}

/// The head's fields a client drops when they are absent, so a catch-up
/// names each one.
const OPTIONAL_HEAD: [&str; 11] = [
    "sessionId",
    "turnOpening",
    "retrying",
    "error",
    "failure",
    "stopReason",
    "title",
    "usage",
    "plan",
    "activeTool",
    "ended",
];

/// A flush's changes and the rows they touched.
pub type Flushed = Vec<ChatChange>;

fn tree_of(patch: Vec<Op>, n: usize) -> PartTree {
    let blocks = patch
        .into_iter()
        .next()
        .and_then(|op| match op {
            Op::List { c, .. } => Some(Value::Array(c)),
            Op::Text { .. } => None,
        })
        .unwrap_or_else(|| Value::Array(Vec::new()));
    PartTree {
        blocks,
        n: n as u64,
    }
}

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The whole tree of the agent's `text`, read from scratch.
pub(super) fn read(text: &str) -> PartTree {
    read_as(text, ANSWER)
}

fn read_as(text: &str, options: Options) -> PartTree {
    tree_of(Doc::read(text, options), units(text))
}

/// A part as clients get it, with the tree of its text when the agent or the person wrote it.
fn shown(part: &ChatPart, role: ChatRole, side: Option<&mut Side>, open: bool) -> ChatPart {
    let mut part = part.clone();
    let options = reading(role);
    if let ChatPart::Text { text, md, .. } | ChatPart::Thought { text, md, .. } = &mut part {
        *md = Some(match side {
            Some(side) if open => {
                let mut doc = Doc::new(options);
                doc.push(text);
                let tree = tree_of(doc.patch(), doc.units());
                side.doc = Some(doc);
                tree
            }
            _ => read_as(text, options),
        });
    }
    part
}

fn text_len(part: &ChatPart) -> Option<usize> {
    match part {
        ChatPart::Text { text, .. } | ChatPart::Thought { text, .. } => Some(text.len()),
        _ => None,
    }
}

fn row_bytes(row: &Row) -> usize {
    row.msg
        .parts
        .iter()
        .map(|part| PART_BYTES + text_len(part).unwrap_or(0))
        .sum::<usize>()
        + PART_BYTES
}

/// Only the text being written keeps its tree between flushes: the last part
/// of the last row, while it is text.
fn open_part(transcript: &Transcript, position: usize) -> Option<usize> {
    if position + 1 != transcript.rows.len() {
        return None;
    }
    let row = &transcript.rows[position];
    if row.msg.role != ChatRole::Assistant {
        return None;
    }
    let last = row.msg.parts.len().checked_sub(1)?;
    text_len(&row.msg.parts[last]).map(|_| last)
}

/// The row with every tree read, for a client that holds none of it.
fn whole_row(row: &Row) -> ChatMessage {
    fit_frame(with_trees(row.msg.clone()))
}

/// The most of a row that goes in one frame, leaving room for what travels with it.
const ROW_FRAME_BYTES: usize = sikemux_wire::protocol::MAX_FRAME_BYTES * 3 / 4;
/// What a text keeps of itself in a row too big for a frame.
const CUT_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// `msg` as it fits in a frame: a row too big for one keeps the first 4 MB of each long text,
/// saying how much it left off, rather than not going at all.
fn fit_frame(mut msg: ChatMessage) -> ChatMessage {
    if serde_json::to_vec(&msg).map_or(0, |json| json.len()) <= ROW_FRAME_BYTES {
        return msg;
    }
    let options = reading(msg.role);
    for part in &mut msg.parts {
        if let ChatPart::Text {
            text,
            md,
            cut_bytes,
            ..
        }
        | ChatPart::Thought {
            text,
            md,
            cut_bytes,
            ..
        } = part
        {
            if text.len() <= CUT_TEXT_BYTES {
                continue;
            }
            let mut keep = CUT_TEXT_BYTES;
            while !text.is_char_boundary(keep) {
                keep -= 1;
            }
            *cut_bytes = Some((text.len() - keep) as u64);
            text.truncate(keep);
            *md = Some(read_as(text, options));
        }
    }
    msg
}

/// `msg` with the tree of each text the agent or the person wrote read, as clients are sent it.
pub fn with_trees(mut msg: ChatMessage) -> ChatMessage {
    let options = reading(msg.role);
    for part in &mut msg.parts {
        if let ChatPart::Text { text, md, .. } | ChatPart::Thought { text, md, .. } = part {
            *md = Some(read_as(text, options));
        }
    }
    msg
}

fn field(row: &ChatMessage, name: &str) -> Value {
    let value = match name {
        "sentAt" => serde_json::to_value(row.sent_at),
        "endedAt" => serde_json::to_value(row.ended_at),
        "opensTurn" => serde_json::to_value(&row.opens_turn),
        "meta" => serde_json::to_value(&row.meta),
        "fold" => serde_json::to_value(&row.fold),
        "context" => serde_json::to_value(&row.context),
        "letters" => serde_json::to_value(&row.letters),
        _ => Ok(Value::Null),
    };
    value.unwrap_or_default()
}

fn fields(row: &ChatMessage, names: impl IntoIterator<Item = &'static str>) -> Value {
    Value::Object(
        names
            .into_iter()
            .map(|name| (name.to_owned(), field(row, name)))
            .collect(),
    )
}

/// The changes to one row since the last flush.
fn row_changes(
    transcript: &mut Transcript,
    position: usize,
    within: &Option<String>,
    seq: u64,
    out: &mut Vec<ChatChange>,
) {
    let open = open_part(transcript, position);
    let row = &mut transcript.rows[position];
    row.v = seq;
    let role = row.msg.role;
    if row.fresh {
        let mut msg = row.msg.clone();
        for (at, part) in msg.parts.iter_mut().enumerate() {
            *part = shown(
                &row.msg.parts[at],
                role,
                row.sides.get_mut(at),
                open == Some(at),
            );
        }
        for (side, part) in row.sides.iter_mut().zip(&row.msg.parts) {
            side.sent = text_len(part).unwrap_or(0);
        }
        out.push(ChatChange::Row {
            row: fit_frame(msg),
            within: within.clone(),
        });
        row.fresh = false;
        row.parts.clear();
        row.fields.clear();
        return;
    }
    let id = row.msg.id.clone();
    let mut replaced = std::mem::take(&mut row.parts);
    replaced.sort_unstable();
    for at in 0..row.msg.parts.len() {
        if replaced.contains(&at) {
            let part = shown(
                &row.msg.parts[at],
                role,
                row.sides.get_mut(at),
                open == Some(at),
            );
            row.sides[at].sent = text_len(&row.msg.parts[at]).unwrap_or(0);
            out.push(ChatChange::Part {
                row: id.clone(),
                part,
                within: within.clone(),
            });
            continue;
        }
        let Some(len) = text_len(&row.msg.parts[at]) else {
            continue;
        };
        let side = &mut row.sides[at];
        if len <= side.sent {
            continue;
        }
        let (ChatPart::Text { id: part, text, .. } | ChatPart::Thought { id: part, text, .. }) =
            &row.msg.parts[at]
        else {
            continue;
        };
        let more = text[side.sent..].to_owned();
        let md = Some({
            let doc = side.doc.get_or_insert_with(|| {
                let mut doc = Doc::new(reading(role));
                doc.push(text);
                doc
            });
            json!({ "n": doc.units(), "o": doc.patch() })
        });
        side.sent = len;
        out.push(ChatChange::Text {
            row: id.clone(),
            part: part.clone(),
            text: more,
            md,
            within: within.clone(),
        });
    }
    if !row.fields.is_empty() {
        let names: Vec<&'static str> = std::mem::take(&mut row.fields).into_iter().collect();
        out.push(ChatChange::Set {
            row: id,
            set: fields(&row.msg, names),
            within: within.clone(),
        });
    }
}

fn flush_transcript(
    transcript: &mut Transcript,
    within: &Option<String>,
    seq: u64,
    out: &mut Vec<ChatChange>,
) {
    let mut first = transcript.rows.len().saturating_sub(2);
    for id in std::mem::take(&mut transcript.touched) {
        if let Some(position) = transcript.position(&id) {
            first = first.min(position);
            row_changes(transcript, position, within, seq, out);
            let row = &mut transcript.rows[position];
            let bytes = row_bytes(row);
            transcript.bytes = transcript.bytes + bytes - row.bytes;
            row.bytes = bytes;
        }
    }
    let last = transcript.rows.len().checked_sub(1);
    let open = last.and_then(|last| open_part(transcript, last).map(|part| (last, part)));
    for (position, row) in transcript.rows.iter_mut().enumerate().skip(first) {
        for (at, side) in row.sides.iter_mut().enumerate() {
            if open != Some((position, at)) {
                side.doc = None;
            }
        }
    }
}

impl Fold {
    /// What changed since the last flush, as change number `seq`.
    pub fn flush(&mut self, seq: u64) -> Flushed {
        self.settle_runs();
        let mut out = Vec::new();
        if let Some(from) = self.dropped.take() {
            out.push(ChatChange::Drop { from });
        }
        if !self.head_dirty.is_empty() {
            let whole = serde_json::to_value(&self.head).unwrap_or_default();
            let set: Map<String, Value> = std::mem::take(&mut self.head_dirty)
                .into_iter()
                .map(|name| {
                    (
                        name.to_owned(),
                        whole.get(name).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect();
            out.push(ChatChange::Head {
                set: Value::Object(set),
            });
        }
        let active = self.active_tool();
        if active != self.head.active_tool {
            self.head.active_tool = active.clone();
            out.push(ChatChange::Head {
                set: json!({ "activeTool": active }),
            });
        }
        flush_transcript(&mut self.main, &None, seq, &mut out);
        let mut sessions: Vec<String> = self
            .subs
            .iter()
            .filter(|(_, sub)| !sub.touched.is_empty())
            .map(|(session, _)| session.clone())
            .collect();
        sessions.sort();
        for session in sessions {
            if let Some(sub) = self.subs.get_mut(&session) {
                flush_transcript(sub, &Some(session.clone()), seq, &mut out);
            }
        }
        self.seq = seq;
        self.trim();
        out
    }

    /// Whether anything waits to go out.
    pub fn is_dirty(&self) -> bool {
        !self.head_dirty.is_empty()
            || self.dropped.is_some()
            || self.settle_from.is_some()
            || !self.main.touched.is_empty()
            || self.subs.values().any(|sub| !sub.touched.is_empty())
    }

    fn active_tool(&self) -> Option<String> {
        let last = self.main.rows.last()?;
        last.msg.parts.iter().rev().find_map(|part| match part {
            ChatPart::Tool { id, tool, .. } => {
                Some(tools::running(tool.status.as_deref()).then(|| id.clone()))
            }
            _ => None,
        })?
    }

    /// Lets go of the oldest turns once the fold holds more than it should.
    /// They page back from the chat's events like any earlier turn.
    fn trim(&mut self) {
        let mut bytes = self.main.bytes;
        if bytes <= FOLD_BYTES {
            return;
        }
        let mut cut = 0;
        while cut < self.main.rows.len().saturating_sub(1) && bytes > FOLD_BYTES / 2 {
            bytes -= self.main.rows[cut].bytes;
            cut += 1;
        }
        while cut < self.main.rows.len() && !starts_turn(&self.main.rows, cut) {
            cut += 1;
        }
        let gone: Vec<Row> = self.main.rows.drain(..cut).collect();
        for row in &gone {
            self.main.bytes -= row.bytes;
            self.forget_row(&None, row);
        }
        if let Some(position) = self.settle_from.as_mut() {
            *position = position.saturating_sub(cut);
        }
    }

    /// The rows this fold holds that `wanted` asks for, as they stand, without their trees:
    /// the nearest to the event first, as many as fit in about `budget` bytes, at least one.
    /// Says whether they reach the last row on the far side.
    pub fn held(&self, wanted: PageOf, budget: usize) -> (Vec<ChatMessage>, bool) {
        let rows = &self.main.rows;
        let mut taken = Vec::new();
        let mut bytes = 0;
        let order: Box<dyn Iterator<Item = &Row>> = match wanted {
            PageOf::Before(_) => Box::new(rows.iter().rev()),
            PageOf::After(_) | PageOf::From(_) => Box::new(rows.iter()),
        };
        let mut all = true;
        for row in order.filter(|row| wanted.keeps(row.index)) {
            if !taken.is_empty() && bytes + row.bytes > budget {
                all = false;
                break;
            }
            bytes += row.bytes;
            taken.push(row.msg.clone());
        }
        (taken, all)
    }

    /// The index of the event the fold's first row came from.
    pub fn floor(&self) -> Option<u64> {
        self.main.rows.first().map(|row| row.index)
    }

    /// How many bytes `row` is as a client opening the chat is sent it.
    fn sent_bytes(&self, row: &Row) -> usize {
        let mut sizes = self.sent_bytes.borrow_mut();
        if let Some(&(_, bytes)) = sizes.get(&row.msg.id).filter(|(v, _)| *v == row.v) {
            return bytes;
        }
        let bytes = serde_json::to_vec(&whole_row(row)).map_or(0, |json| json.len());
        if sizes.len() > 2 * self.main.rows.len() + 64 {
            let held: std::collections::HashSet<&str> = self
                .main
                .rows
                .iter()
                .map(|row| row.msg.id.as_str())
                .collect();
            sizes.retain(|id, _| held.contains(id.as_str()));
        }
        sizes.insert(row.msg.id.clone(), (row.v, bytes));
        bytes
    }

    fn window_start(&self) -> usize {
        let rows = &self.main.rows;
        let mut turns = 0;
        let mut bytes = 0;
        let mut start = None;
        let mut piece = None;
        for (position, row) in rows.iter().enumerate().rev() {
            bytes += self.sent_bytes(row);
            if bytes > TAIL_BYTES && (start.is_some() || piece.is_some()) {
                break;
            }
            piece = Some(position);
            if starts_turn(rows, position) {
                start = Some(position);
                turns += 1;
                if turns >= TAIL_TURNS {
                    break;
                }
            }
        }
        start.or(piece).unwrap_or(0)
    }

    fn subagent_rows(&self, rows: &[&Row]) -> BTreeMap<String, Vec<ChatMessage>> {
        let mut found = BTreeMap::new();
        let mut sessions: Vec<String> = Vec::new();
        let note = |parts: &[ChatPart], sessions: &mut Vec<String>| {
            for part in parts {
                if let ChatPart::Subagent { subagent, .. } = part {
                    sessions.push(subagent.session_id.clone());
                }
            }
        };
        for row in rows {
            note(&row.msg.parts, &mut sessions);
        }
        sessions.extend(
            self.head
                .running_subagents
                .iter()
                .map(|card| card.session_id.clone()),
        );
        while let Some(session) = sessions.pop() {
            if found.contains_key(&session) {
                continue;
            }
            let Some(sub) = self.subs.get(&session) else {
                continue;
            };
            let latest = &sub.rows[sub.rows.len().saturating_sub(SUBAGENT_TAIL_ROWS)..];
            for row in latest {
                note(&row.msg.parts, &mut sessions);
            }
            found.insert(session, latest.iter().map(whole_row).collect());
        }
        found
    }

    /// The chat as a client opening it is sent. `older` says whether the
    /// chat's events hold turns before the fold's first row.
    pub fn view(&self, older: bool) -> ChatView {
        let start = self.window_start();
        let rows: Vec<&Row> = self.main.rows[start..].iter().collect();
        let older_before = rows
            .first()
            .filter(|_| start > 0 || older)
            .map(|row| row.index);
        let mut head = self.head.clone();
        head.active_tool = self.active_tool();
        ChatView {
            head,
            subagents: self.subagent_rows(&rows),
            rows: rows.iter().map(|row| whole_row(row)).collect(),
            older_before,
        }
    }

    /// What a client that held every change up to `since` missed: the head,
    /// which rows of the window stay, and each row changed since. None when
    /// the window starts past what it held, and it needs the view whole.
    pub fn catch_up(&self, since: u64) -> Option<Vec<ChatChange>> {
        let start = self.window_start();
        let rows = &self.main.rows[start..];
        if rows.first().is_some_and(|row| row.v > since) {
            return None;
        }
        let mut head = self.head.clone();
        head.active_tool = self.active_tool();
        let mut set = serde_json::to_value(&head).unwrap_or_default();
        if let Some(fields) = set.as_object_mut() {
            for name in OPTIONAL_HEAD {
                fields.entry(name).or_insert(Value::Null);
            }
        }
        let mut out = vec![
            ChatChange::Head { set },
            ChatChange::Keep {
                rows: rows.iter().map(|row| row.msg.id.clone()).collect(),
            },
        ];
        for row in rows.iter().filter(|row| row.v > since) {
            out.push(ChatChange::Row {
                row: whole_row(row),
                within: None,
            });
        }
        let window: Vec<&Row> = rows.iter().collect();
        for (session, sub_rows) in self.subagent_rows(&window) {
            let Some(sub) = self.subs.get(&session) else {
                continue;
            };
            for (row, msg) in sub.rows.iter().zip(sub_rows) {
                if row.v > since {
                    out.push(ChatChange::Row {
                        row: msg,
                        within: Some(session.clone()),
                    });
                }
            }
        }
        Some(out)
    }

    /// The rows `events` hold that `wanted` asks for, folded from the whole turns they ran
    /// in, so a call finished after them arrives finished and no row is cut. They stay under
    /// `budget` bytes of JSON, counted from the end nearest the event asked from; at least one
    /// row goes. `beyond` is where the chat goes on past the events, on the far side, when it
    /// does; a page that keeps no rows points there. With `session`, the rows are that
    /// subagent's rather than the chat's own.
    pub fn page_of(
        &self,
        events: &[(u64, ChatEvent)],
        wanted: PageOf,
        budget: usize,
        beyond: Option<u64>,
        session: Option<&str>,
    ) -> ChatPage {
        let mut fold = Fold::new(&self.head.permission_mode);
        fold.head.session_id = self.head.session_id.clone();
        for (index, event) in events {
            let at = event
                .payload
                .get("at")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            fold.apply(*index, event.kind, &event.payload, at);
        }
        let cards: HashMap<&str, &SubagentCard> = self.cards_by_session();
        let folded = match session {
            Some(session) => fold.subs.remove(session).map(|sub| sub.rows),
            None => Some(std::mem::take(&mut fold.main.rows)),
        };
        let mut rows: Vec<Row> = folded
            .unwrap_or_default()
            .into_iter()
            .filter(|row| wanted.keeps(row.index))
            .collect();
        for row in &mut rows {
            for part in &mut row.msg.parts {
                match part {
                    ChatPart::Subagent { subagent, .. } => {
                        if let Some(card) = cards.get(subagent.session_id.as_str()) {
                            *subagent = (*card).clone();
                        }
                    }
                    ChatPart::Tool { tool, .. } if tools::running(tool.status.as_deref()) => {
                        if let Some(known) = self.tool_part(&tool.tool_call_id) {
                            *part = known;
                        }
                    }
                    _ => {}
                }
            }
        }
        if session.is_none() {
            super::turns::settle_all(&mut rows);
        }
        let mut sent: Vec<ChatMessage> = rows.iter().map(whole_row).collect();
        let cut = within(&mut sent, budget, matches!(wanted, PageOf::Before(_)));
        let kept: Vec<&Row> = rows
            .iter()
            .filter(|row| sent.iter().any(|sent| sent.id == row.msg.id))
            .collect();
        let mut subagents = fold.subagent_rows(&kept);
        for (session, rows) in subagents.iter_mut() {
            if let Some(sub) = self.subs.get(session) {
                let latest = &sub.rows[sub.rows.len().saturating_sub(SUBAGENT_TAIL_ROWS)..];
                *rows = latest.iter().map(whole_row).collect();
            }
        }
        let more = cut || beyond.is_some();
        let (older_before, newer_after) = match wanted {
            PageOf::Before(_) => (
                sent.first()
                    .and_then(|row| row.from_event)
                    .or(beyond)
                    .filter(|_| more),
                None,
            ),
            PageOf::After(_) | PageOf::From(_) => (
                None,
                sent.last()
                    .and_then(|row| row.from_event)
                    .or(beyond)
                    .filter(|_| more),
            ),
        };
        ChatPage {
            rows: sent,
            subagents,
            older_before,
            newer_after,
        }
    }

    fn cards_by_session(&self) -> HashMap<&str, &SubagentCard> {
        let mut cards = HashMap::new();
        for transcript in std::iter::once(&self.main).chain(self.subs.values()) {
            for row in &transcript.rows {
                for part in &row.msg.parts {
                    if let ChatPart::Subagent { subagent, .. } = part {
                        cards.insert(subagent.session_id.as_str(), subagent);
                    }
                }
            }
        }
        cards
    }

    fn tool_part(&self, tool_call_id: &str) -> Option<ChatPart> {
        let place = self
            .tools
            .iter()
            .find(|((_, id), _)| id == tool_call_id)
            .map(|(_, place)| place)?;
        let transcript = match &place.within {
            None => &self.main,
            Some(session) => self.subs.get(session)?,
        };
        let row = &transcript.rows[transcript.position(&place.row)?];
        row.msg.parts.get(place.part).cloned()
    }

    /// Every row the fold holds now counts as changed, for a fold built again
    /// from the chat's events: a client coming back gets the rows whole.
    pub fn rebuilt(&mut self, seq: u64) {
        self.seq = seq;
        for transcript in std::iter::once(&mut self.main).chain(self.subs.values_mut()) {
            transcript.touched.clear();
            transcript.bytes = 0;
            for row in &mut transcript.rows {
                row.bytes = row_bytes(row);
                transcript.bytes += row.bytes;
                row.v = seq;
                row.fresh = false;
                row.parts.clear();
                row.fields.clear();
                for (side, part) in row.sides.iter_mut().zip(&row.msg.parts) {
                    side.sent = text_len(part).unwrap_or(0);
                    side.doc = None;
                }
            }
        }
        self.head_dirty.clear();
        self.dropped = None;
        self.settle_runs();
        for transcript in std::iter::once(&mut self.main).chain(self.subs.values_mut()) {
            transcript.touched.clear();
            for row in &mut transcript.rows {
                row.fields.clear();
            }
        }
        self.head.active_tool = self.active_tool();
    }
}

/// Where a turn starts: a prompt, or a message the agent began a turn with
/// by itself.
fn starts_turn(rows: &[Row], at: usize) -> bool {
    let message = &rows[at].msg;
    message.role == ChatRole::User
        || message.opens_turn.is_some()
            && at
                .checked_sub(1)
                .is_none_or(|before| rows[before].msg.role != ChatRole::User)
}
