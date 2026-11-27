//! What `agent_read` and `agent_wait` answer: what another agent said, in
//! short, and waiting until agents finish or need the person. What agents do
//! is journaled beside the project's task events, so a wait from a cursor
//! misses nothing that happened between two waits.

use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::Instant;

use crate::cli::protocol::HarnessRequest;
use sikemux_wire::protocol::AgentStatus;

use super::chat::feed::Feed;
use super::connection::blocking;
use super::roster::{self, Listed, Reach};
use super::Core;
use crate::acp::failure::{out_of_usage, PAUSED_FOR_AGENTS};
use crate::agent_catalog::AgentKind;
use crate::sessions::roots::account_root;
use crate::sessions::transcript::{self, Said};
use std::collections::BTreeMap;

const MAX_WAIT_MS: u64 = 30_000;
const MAX_AGENTS: usize = 8;
/// Turns read when no cursor says where to start.
const RECENT_TURNS: usize = 3;
/// Serialized turns budget per read, keeping the full JSON under MCP's 10 KB cap.
const MAX_READ_BYTES: usize = 9 * 1024;
const EXCERPT_BYTES: usize = 64 * 1024;
const MAX_ASKED_CHARS: usize = 600;
const MAX_SAID_BYTES: usize = 2 * 1024;
/// Total intermediate narration kept when a finished turn also carries a final answer.
const MAX_SAID_WITH_ANSWER_BYTES: usize = 3 * 1024;
const TURN_ASKED: usize = 6;
const TURN_SAID: usize = 4;
const MIN_ANSWER_CHUNK_BYTES: usize = 2 * 1024;
const TOOL_TITLES: usize = 6;
const MAX_TITLE_CHARS: usize = 80;
const SCREEN_LINES: usize = 40;
const REPORT_SCREEN_LINES: usize = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Happened {
    Working,
    Done,
    Stopped,
    Failed,
    NeedsYou,
    Gone,
}

const HAPPENINGS: [Happened; 6] = [
    Happened::Working,
    Happened::Done,
    Happened::Stopped,
    Happened::Failed,
    Happened::NeedsYou,
    Happened::Gone,
];

impl Happened {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Happened::Working => "agent.working",
            Happened::Done => "agent.done",
            Happened::Stopped => "agent.stopped",
            Happened::Failed => "agent.failed",
            Happened::NeedsYou => "agent.needs-you",
            Happened::Gone => "agent.gone",
        }
    }

    pub(crate) fn of(kind: &str) -> Option<Self> {
        HAPPENINGS
            .into_iter()
            .find(|happened| happened.kind() == kind)
    }

    pub(crate) fn words(self) -> &'static str {
        match self {
            Happened::Working => "started working",
            Happened::Done => "finished its turn",
            Happened::Stopped => "was stopped mid-turn",
            Happened::Failed => "ran into an error",
            Happened::NeedsYou => "waits on the person",
            Happened::Gone => "stopped running",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Until {
    /// No longer at work, for any reason.
    Idle,
    /// Its turn ended, or it stopped running.
    Done,
    NeedsYou,
    Any,
}

impl Until {
    fn of(params: &Value) -> Result<Self, String> {
        match params.get("until") {
            None | Some(Value::Null) => Ok(Until::Idle),
            Some(Value::String(until)) => Self::named(until),
            Some(_) => Err("until must be idle, done, needs_you or any".into()),
        }
    }

    pub(crate) fn named(until: &str) -> Result<Self, String> {
        match until {
            "idle" => Ok(Until::Idle),
            "done" => Ok(Until::Done),
            "needs_you" => Ok(Until::NeedsYou),
            "any" => Ok(Until::Any),
            other => Err(format!(
                "until must be idle, done, needs_you or any, not {other}"
            )),
        }
    }

    pub(crate) fn heard(self, happened: Happened) -> bool {
        match self {
            Until::Any => true,
            Until::NeedsYou => happened == Happened::NeedsYou,
            Until::Done => matches!(
                happened,
                Happened::Done | Happened::Stopped | Happened::Failed | Happened::Gone
            ),
            Until::Idle => happened != Happened::Working,
        }
    }

    /// Whether an agent standing at `status` already is where the wait would
    /// take it. `None` is an agent that is gone.
    pub(crate) fn already(self, status: Option<AgentStatus>) -> bool {
        let Some(status) = status else {
            return self != Until::NeedsYou && self != Until::Any;
        };
        match self {
            Until::Any => false,
            Until::NeedsYou => status == AgentStatus::Blocked,
            Until::Done => matches!(
                status,
                AgentStatus::Done | AgentStatus::Idle | AgentStatus::Asleep | AgentStatus::Stopped
            ),
            Until::Idle => !matches!(status, AgentStatus::Working | AgentStatus::Starting),
        }
    }
}

fn agent_param(params: &Value) -> Result<String, String> {
    match params.get("agent") {
        Some(Value::String(agent)) if !agent.trim().is_empty() && agent.len() <= 200 => {
            Ok(agent.clone())
        }
        _ => Err("agent must be the agent's name or id".into()),
    }
}

fn cursor_param(params: &Value) -> Result<Option<&str>, String> {
    match params.get("cursor") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(cursor)) if !cursor.is_empty() && cursor.len() <= 200 => {
            Ok(Some(cursor))
        }
        Some(_) => Err("cursor must be one an earlier call gave you".into()),
    }
}

fn omitted_label(bytes: usize) -> String {
    if bytes >= 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else if bytes == 1 {
        "1 byte".to_owned()
    } else {
        format!("{bytes} bytes")
    }
}

pub(crate) fn cut_bytes(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_owned();
    }
    let end = text.floor_char_boundary(max);
    let kept = text.get(..end).unwrap_or(text).trim_end();
    let omitted = text.len().saturating_sub(kept.len());
    format!("{kept} [truncated: {} omitted]", omitted_label(omitted))
}

fn json_char_bytes(ch: char) -> usize {
    match ch {
        '"' | '\\' | '\x08' | '\x0c' | '\n' | '\r' | '\t' => 2,
        ch if (ch as u32) < 0x20 => 6,
        ch => ch.len_utf8(),
    }
}

fn take_json_bytes(text: &str, max_json_bytes: usize) -> usize {
    let mut used = 0;
    for (idx, ch) in text.char_indices() {
        let need = json_char_bytes(ch);
        if used + need > max_json_bytes {
            return idx;
        }
        used += need;
    }
    text.len()
}

fn split_chunk(rest: &str, raw_end: usize) -> usize {
    let prefix = rest.get(..raw_end).unwrap_or(rest);
    let line_floor = raw_end.saturating_sub(512);
    if let Some(pos) = prefix.rfind('\n') {
        let after = pos + 1;
        if after > line_floor && after < rest.len() {
            return after;
        }
    }
    let word_floor = raw_end.saturating_sub(128);
    if let Some((pos, ch)) = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
    {
        let after = pos + ch.len_utf8();
        if after > word_floor && after < rest.len() {
            return after;
        }
    }
    if raw_end == 0 {
        rest.chars().next().map_or(0, char::len_utf8)
    } else {
        raw_end
    }
}

fn cut_chars(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let kept: String = line.chars().take(max).collect();
    format!("{}…", kept.trim_end())
}

fn without_tag(text: &str, tag: &str) -> String {
    let open = format!("<{tag} ");
    let mut text = text.to_owned();
    while let Some(start) = text.find(&open) {
        let Some(length) = text.get(start..).and_then(|rest| rest.find('>')) else {
            break;
        };
        text.replace_range(start..=start + length, "");
    }
    text.replace(&format!("</{tag}>"), "")
}

/// Who sent a prompt and what it said, without the labels Sikemux wraps
/// messages from agents and the person's answers in.
fn asked(text: &str) -> Value {
    let start = text.trim_start();
    let from = start
        .strip_prefix("<agent-message from=\"")
        .and_then(|rest| rest.split_once('"'))
        .map(|(name, _)| {
            let name = name.replace("&quot;", "\"").replace("&amp;", "&");
            match name.as_str() {
                "Board" | "Trigger" | "Auto" | "Sikemux" | "the person" => name,
                _ => super::names::at_name(&name),
            }
        })
        .or_else(|| {
            start
                .starts_with("<task-notification>")
                .then(|| "a background task".to_owned())
        });
    let body = without_tag(&without_tag(text, "agent-message"), "question-answer");
    json!({
        "from": from.unwrap_or_else(|| "the person".to_owned()),
        "text": cut_chars(&body, MAX_ASKED_CHARS),
    })
}

#[derive(PartialEq, Eq)]
enum Last {
    Nothing,
    Message,
    Other,
}

struct Turn {
    start: u64,
    asked: Vec<Value>,
    said: Vec<String>,
    tools: Vec<(String, Option<String>, bool)>,
    ended: Option<Value>,
    last: Last,
}

enum Next {
    Turn(u64),
    Answer {
        start: u64,
        offset: usize,
        omitted: usize,
    },
}

struct Viewed {
    view: Value,
    /// When the turn's answer did not fit in this read: `(next_byte_offset, remaining_bytes)`.
    truncated: Option<(usize, usize)>,
}

impl Turn {
    fn new(start: u64) -> Self {
        Self {
            start,
            asked: Vec::new(),
            said: Vec::new(),
            tools: Vec::new(),
            ended: None,
            last: Last::Nothing,
        }
    }

    fn tools_line(&self) -> Option<String> {
        if self.tools.is_empty() {
            return None;
        }
        let count = self.tools.len();
        let failed = self.tools.iter().filter(|(_, _, failed)| *failed).count();
        let titles: Vec<String> = self
            .tools
            .iter()
            .filter_map(|(_, title, _)| title.as_deref())
            .filter(|title| !title.trim().is_empty())
            .take(TOOL_TITLES)
            .map(|title| cut_chars(title, MAX_TITLE_CHARS))
            .collect();
        let mut line = format!("{count} tool call{}", if count == 1 { "" } else { "s" });
        if failed > 0 {
            line.push_str(&format!(", {failed} failed"));
        }
        if !titles.is_empty() {
            line.push_str(": ");
            line.push_str(&titles.join("; "));
            if count > titles.len() {
                line.push_str(&format!("; and {} more", count - titles.len()));
            }
        }
        Some(line)
    }

    fn view(mut self, answer_offset: Option<usize>, room: usize) -> Option<Viewed> {
        let finished = self.ended.as_ref().and_then(Value::as_str) == Some("finished");
        let answer = (finished && self.last == Last::Message)
            .then(|| self.said.pop())
            .flatten();
        if let Some(offset) = answer_offset.filter(|offset| *offset > 0) {
            let answer = answer?;
            let answer = answer.trim();
            let offset = answer.floor_char_boundary(offset.min(answer.len()));
            if offset >= answer.len() {
                return None;
            }
            let rest = answer.get(offset..).unwrap_or_default();
            let mut fields = serde_json::Map::new();
            fields.insert("state".into(), "finished".into());
            fields.insert("continued".into(), true.into());
            let budget = room.saturating_sub(160).max(MIN_ANSWER_CHUNK_BYTES);
            let truncated = insert_answer(&mut fields, rest, offset, budget);
            return Some(Viewed {
                view: Value::Object(fields),
                truncated,
            });
        }
        let tools = self.tools_line();
        let mut fields = serde_json::Map::new();
        fields.insert(
            "state".into(),
            self.ended.take().unwrap_or_else(|| "working".into()),
        );
        let mut omitted = Vec::new();
        let earlier_asked = self.asked.len().saturating_sub(TURN_ASKED);
        if earlier_asked > 0 {
            omitted.push(format!("{earlier_asked} earlier messages to it"));
        }
        if !self.asked.is_empty() {
            let asked: Vec<Value> = self.asked.drain(earlier_asked..).collect();
            fields.insert("asked".into(), asked.into());
        }
        let mut earlier_said = self.said.len().saturating_sub(TURN_SAID);
        if !self.said.is_empty() {
            let mut kept_said: Vec<String> = Vec::new();
            if answer.is_some() {
                let mut used = 0;
                for text in self.said.iter().skip(earlier_said).rev() {
                    let cut = cut_bytes(text, MAX_SAID_BYTES);
                    let len = cut.len();
                    if !kept_said.is_empty() && used + len > MAX_SAID_WITH_ANSWER_BYTES {
                        earlier_said += 1;
                        continue;
                    }
                    used += len;
                    kept_said.push(cut);
                }
                kept_said.reverse();
            } else {
                kept_said = self
                    .said
                    .iter()
                    .skip(earlier_said)
                    .map(|text| cut_bytes(text, MAX_SAID_BYTES))
                    .collect();
            }
            if !kept_said.is_empty() {
                fields.insert("said".into(), kept_said.into());
            }
        }
        if earlier_said > 0 {
            omitted.push(format!("{earlier_said} earlier things it said"));
        }
        if !omitted.is_empty() {
            fields.insert("omitted".into(), omitted.into());
        }
        if let Some(tools) = tools {
            fields.insert("tools".into(), tools.into());
        }
        let truncated = if let Some(answer) = answer {
            let answer = answer.trim();
            let overhead = serde_json::to_vec(&fields).map_or(0, |bytes| bytes.len());
            let budget = room
                .saturating_sub(overhead + 140)
                .max(MIN_ANSWER_CHUNK_BYTES);
            insert_answer(&mut fields, answer, 0, budget)
        } else {
            None
        };
        Some(Viewed {
            view: Value::Object(fields),
            truncated,
        })
    }
}

fn insert_answer(
    fields: &mut serde_json::Map<String, Value>,
    rest: &str,
    offset: usize,
    budget: usize,
) -> Option<(usize, usize)> {
    let raw_end = take_json_bytes(rest, budget);
    if raw_end >= rest.len() {
        fields.insert("answer".into(), rest.into());
        return None;
    }
    let split = split_chunk(rest, raw_end);
    let shown = rest.get(..split).unwrap_or(rest).trim_end();
    let remaining = rest.len().saturating_sub(split);
    fields.insert(
        "answer".into(),
        format!(
            "{shown}\n\n[truncated: {} omitted]",
            omitted_label(remaining)
        )
        .into(),
    );
    fields.insert("truncated".into(), true.into());
    fields.insert("remainingBytes".into(), (remaining as u64).into());
    Some((offset + split, remaining))
}

fn build(said: Vec<(u64, bool, Said)>) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for (index, starts, said) in said {
        let said = match said {
            Said::Message(text) if out_of_usage(&text) => Said::Failed(PAUSED_FOR_AGENTS.into()),
            said => said,
        };
        let ended = turns.last().is_none_or(|turn| turn.ended.is_some());
        if starts || ended {
            turns.push(Turn::new(index));
        }
        let Some(turn) = turns.last_mut() else {
            continue;
        };
        match said {
            Said::Asked(text) => {
                turn.asked.push(asked(&text));
                turn.last = Last::Other;
            }
            Said::Message(text) => {
                match turn.said.last_mut().filter(|_| turn.last == Last::Message) {
                    Some(last) => last.push_str(&text),
                    None => turn.said.push(text),
                }
                turn.last = Last::Message;
            }
            Said::Tool { id, title, failed } => {
                match turn.tools.iter_mut().find(|(known, _, _)| *known == id) {
                    Some((_, known_title, known_failed)) => {
                        if title.is_some() {
                            *known_title = title;
                        }
                        *known_failed = failed;
                    }
                    None => turn.tools.push((id, title, failed)),
                }
                turn.last = Last::Other;
            }
            Said::TurnStarted => {}
            Said::TurnEnded { cancelled } => {
                turn.ended = Some(if cancelled { "stopped" } else { "finished" }.into());
            }
            Said::Failed(message) if message == PAUSED_FOR_AGENTS => {
                turn.ended = Some(format!("paused: {message}").into());
            }
            Said::Failed(message) => {
                turn.ended = Some(format!("failed: {}", cut_chars(&message, 400)).into());
            }
        }
    }
    turns
}

/// What was said, as turns, as many as fit, and where reading on resumes (either
/// at the next turn or mid-answer when a long answer was chunked).
fn turns_from(
    said: Vec<(u64, bool, Said)>,
    resume: Option<(u64, usize)>,
) -> (Vec<Value>, Option<Next>) {
    let mut views = Vec::new();
    let mut bytes = 0;
    let mut resume = resume;
    for turn in build(said) {
        if bytes >= MAX_READ_BYTES && !views.is_empty() {
            return (views, Some(Next::Turn(turn.start)));
        }
        let start = turn.start;
        let offset = resume
            .take()
            .filter(|(at, _)| *at == start)
            .map(|(_, offset)| offset);
        let Some(viewed) = turn.view(offset, MAX_READ_BYTES) else {
            continue;
        };
        let size = serde_json::to_vec(&viewed.view).map_or(0, |bytes| bytes.len());
        if bytes + size > MAX_READ_BYTES && !views.is_empty() {
            return (views, Some(Next::Turn(start)));
        }
        bytes += size;
        views.push(viewed.view);
        if let Some((next_offset, omitted)) = viewed.truncated {
            return (
                views,
                Some(Next::Answer {
                    start,
                    offset: next_offset,
                    omitted,
                }),
            );
        }
    }
    (views, None)
}

fn parse_index_offset(raw: &str) -> Option<(u64, Option<usize>)> {
    match raw.split_once('+') {
        Some((index, offset)) => {
            let offset = offset.parse::<usize>().ok().filter(|offset| *offset > 0)?;
            Some((index.parse::<u64>().ok()?, Some(offset)))
        }
        None => Some((raw.parse::<u64>().ok()?, None)),
    }
}

fn format_cursor(prefix: &str, sep: char, next: Option<&Next>, fallback: u64) -> String {
    match next {
        Some(Next::Answer { start, offset, .. }) => format!("{prefix}{sep}{start}+{offset}"),
        Some(Next::Turn(start)) => format!("{prefix}{sep}{start}"),
        None => format!("{prefix}{sep}{fallback}"),
    }
}

fn more_note(rest: Option<&Next>, whole: bool) -> Option<String> {
    match rest {
        Some(Next::Answer { omitted, .. }) => Some(format!(
            "The answer was truncated ({} omitted). Read again with this cursor for the rest.",
            omitted_label(*omitted)
        )),
        Some(Next::Turn(_)) => Some("There is more: read again with this cursor.".to_owned()),
        None if !whole => Some("There is more: read again with this cursor.".to_owned()),
        None => None,
    }
}

/// The chat's last turns, or what it said since `cursor`, from the feed it runs on or the one
/// it kept when it went to sleep or closed, which reads only the tail of its history on disk.
fn read_chat(target: &Listed, feed: &Feed, cursor: Option<&str>) -> Result<Value, String> {
    let name = target
        .name
        .as_deref()
        .map(super::names::at_name)
        .unwrap_or_else(|| target.agent_id.clone());
    let from = cursor
        .map(|cursor| {
            cursor
                .split_once(':')
                .and_then(|(feed, rest)| {
                    let (index, offset) = parse_index_offset(rest)?;
                    Some((feed, index, offset))
                })
                .ok_or("cursor must be one agent_read gave you")
        })
        .transpose()?;
    let mut notes = Vec::new();
    let (index, resume) = match from {
        Some((id, index, offset)) if id == feed.id() => {
            (Some(index), offset.map(|offset| (index, offset)))
        }
        Some(_) => {
            notes.push(format!(
                "{name} started again since that cursor, so this begins at its last turns."
            ));
            (None, None)
        }
        None => (None, None),
    };
    let excerpt = feed.excerpt(index, RECENT_TURNS, EXCERPT_BYTES);
    if excerpt.skipped {
        notes.push("Some of what it said after your cursor is no longer kept.".to_owned());
    }
    let (turns, rest) = turns_from(excerpt.said, resume);
    if turns.is_empty() {
        notes.push(format!("{name} has said nothing since your cursor."));
    }
    if let Some(note) = more_note(rest.as_ref(), true) {
        notes.push(note);
    }
    let mut answer = json!({
        "turns": turns,
        "cursor": format_cursor(&excerpt.feed, ':', rest.as_ref(), excerpt.next),
        "more": rest.is_some(),
    });
    if !notes.is_empty() {
        if let Some(fields) = answer.as_object_mut() {
            fields.insert("note".into(), notes.join(" ").into());
        }
    }
    Ok(answer)
}

fn screen_tail(core: &Core, agent_id: &str, lines: usize) -> Option<String> {
    let session = core.agent_session(agent_id)?;
    let (screen, _) = super::agent::screen_text(&session);
    let mut tail: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .rev()
        .skip_while(|line| line.is_empty())
        .take(lines)
        .collect();
    tail.reverse();
    (!tail.is_empty()).then(|| tail.join("\n"))
}

/// The session file a terminal agent's CLI writes, once the app has said
/// which session it runs, or the one an asleep terminal agent resumes.
fn session_file(core: &Core, agent_id: &str) -> Option<(AgentKind, String, std::path::PathBuf)> {
    let Some(terminal) = core.agent_session(agent_id) else {
        return asleep_session_file(core, agent_id);
    };
    let owner = terminal.owner();
    let cli = AgentKind::from_name(owner.agent_type.as_deref()?)?;
    let session = core.names().session_of(agent_id)?;
    let path = transcript::locate(cli, owner.account_home.as_deref(), &session)?;
    Some((cli, session, path))
}

fn asleep_session_file(
    core: &Core,
    agent_id: &str,
) -> Option<(AgentKind, String, std::path::PathBuf)> {
    let agent =
        (core.workspaces().agent(agent_id)).or_else(|| core.workspaces().closed_agent(agent_id))?;
    let cli = AgentKind::from_name(&agent.provider)?;
    let session = agent.resume_id?;
    let book = core.workspaces().accounts().unwrap_or_default();
    let config_path = (agent.profile_id.as_deref())
        .and_then(|id| book.profile(id))
        .and_then(|profile| profile.config_path.clone());
    let home = account_root(cli, config_path.as_deref(), &BTreeMap::new());
    let path = transcript::locate(cli, home.as_deref(), &session)?;
    Some((cli, session, path))
}

/// The model and effort an agent runs on: the model its last reply came from, else the one it was
/// started on, never its CLI's "default"; the effort its picker holds, else the one it started on.
pub(crate) fn runs_on(core: &Core, agent_id: &str) -> (Option<String>, Option<String>) {
    let picked =
        |value: Option<String>| value.filter(|value| !value.eq_ignore_ascii_case("default"));
    let (transcript, model, effort) = match core.chats().get(agent_id) {
        Some(chat) => {
            let launch = chat.current_now();
            let session = (chat.feed.start().map(|start| start.session_id))
                .or_else(|| launch.resume_id.clone());
            let transcript = AgentKind::from_name(&launch.provider).and_then(|kind| {
                let home = account_root(kind, None, &launch.env);
                Some((kind, transcript::locate(kind, home.as_deref(), &session?)?))
            });
            (transcript, launch.model, launch.effort)
        }
        None => {
            let kept = (core.workspaces().agent(agent_id))
                .or_else(|| core.workspaces().closed_agent(agent_id));
            let transcript = session_file(core, agent_id).map(|(kind, _, path)| (kind, path));
            let (model, effort) = kept.map_or((None, None), |agent| (agent.model, agent.effort));
            (transcript, model, effort)
        }
    };
    let replied = transcript
        .and_then(|(kind, path)| crate::sessions::context::transcript_context(kind, &path))
        .and_then(|context| context.model);
    (replied.or_else(|| picked(model)), picked(effort))
}

/// Whether an agent standing at `status` has ended the turn it was on.
fn at_rest(status: AgentStatus) -> bool {
    !matches!(
        status,
        AgentStatus::Working | AgentStatus::Starting | AgentStatus::Blocked
    )
}

/// What `said` holds with its last turn ended, as Claude never writes where
/// a turn ends.
fn ended(mut said: Vec<(u64, bool, Said)>, at: u64) -> Vec<(u64, bool, Said)> {
    let open = said
        .last()
        .is_some_and(|(_, _, last)| !matches!(last, Said::TurnEnded { .. } | Said::Failed(_)));
    if open {
        said.push((at, false, Said::TurnEnded { cancelled: false }));
    }
    said
}

/// A terminal agent's last turns, or what it said since `cursor`, from its
/// session file, with its screen when there is none or it waits on the
/// person.
fn read_terminal(core: &Core, target: &Listed, cursor: Option<&str>) -> Result<Value, String> {
    let name = target
        .name
        .as_deref()
        .map(super::names::at_name)
        .unwrap_or_else(|| target.agent_id.clone());
    let mut notes = Vec::new();
    let mut answer = match session_file(core, &target.agent_id) {
        Some((cli, session, path)) => {
            let parsed = cursor
                .map(|cursor| {
                    cursor
                        .rsplit_once('@')
                        .and_then(|(session, rest)| {
                            let (at, offset) = parse_index_offset(rest)?;
                            Some((session, at, offset))
                        })
                        .ok_or("cursor must be one agent_read gave you")
                })
                .transpose()?;
            let (from, mut resume) = match parsed {
                Some((from, at, offset)) if from == session => {
                    (Some(at), offset.map(|offset| (at, offset)))
                }
                Some(_) => {
                    notes.push(format!(
                        "{name} started a new session since that cursor, so this begins at its last turns."
                    ));
                    (None, None)
                }
                None => (None, None),
            };
            let excerpt = transcript::read(cli, &path, from, RECENT_TURNS)
                .map_err(|error| format!("Sikemux could not read {name}'s session: {error}"))?;
            if excerpt.restarted {
                resume = None;
                notes.push(format!(
                    "{name}'s session is not where your cursor was, so this begins at its last turns."
                ));
            }
            let said = if excerpt.whole && at_rest(target.status) {
                ended(excerpt.said, excerpt.next)
            } else {
                excerpt.said
            };
            let (turns, rest) = turns_from(said, resume);
            if turns.is_empty() {
                notes.push(format!("{name} has said nothing since your cursor."));
            }
            if let Some(note) = more_note(rest.as_ref(), excerpt.whole) {
                notes.push(note);
            }
            json!({
                "turns": turns,
                "cursor": format_cursor(&session, '@', rest.as_ref(), excerpt.next),
                "more": rest.is_some() || !excerpt.whole,
            })
        }
        None => {
            notes.push(format!(
                "{name} runs in a terminal and Sikemux cannot read its session, so this is what its screen shows now, and there is no cursor."
            ));
            json!({ "turns": [] })
        }
    };
    let file = answer.get("cursor").is_some();
    if !file || target.status == AgentStatus::Blocked {
        if let (Some(screen), Some(fields)) = (
            screen_tail(core, &target.agent_id, SCREEN_LINES),
            answer.as_object_mut(),
        ) {
            fields.insert("screen".into(), screen.into());
        }
        if file {
            notes.push(format!(
                "{name} waits on the person; screen is what it shows them."
            ));
        }
    }
    if !notes.is_empty() {
        if let Some(fields) = answer.as_object_mut() {
            fields.insert("note".into(), notes.join(" ").into());
        }
    }
    Ok(answer)
}

/// What a terminal agent wrote last, if its last turn ended on words, from
/// its session file, or else the end of its screen.
pub(crate) fn terminal_answer(core: &Core, agent_id: &str) -> TerminalTurn {
    let Some((cli, _, path)) = session_file(core, agent_id) else {
        return TerminalTurn {
            start: None,
            answer: screen_tail(core, agent_id, REPORT_SCREEN_LINES).map(TerminalAnswer::Screen),
        };
    };
    let turn = transcript::read(cli, &path, None, 1)
        .ok()
        .and_then(|excerpt| build(excerpt.said).pop());
    TerminalTurn {
        start: turn.as_ref().map(|turn| turn.start),
        answer: turn
            .filter(|turn| turn.last == Last::Message)
            .and_then(|mut turn| turn.said.pop())
            .map(TerminalAnswer::Said),
    }
}

/// Whether `agent_id` has a readable CLI session transcript on disk.
pub(crate) fn has_transcript(core: &Core, agent_id: &str) -> bool {
    session_file(core, agent_id).is_some()
}

/// What a terminal agent wrote in its newest `count` turns, oldest first,
/// each cut to its last `longest` characters to match `Feed::answers`.
pub(crate) fn terminal_answers(
    core: &Core,
    agent_id: &str,
    count: usize,
    longest: usize,
) -> Option<Vec<String>> {
    let (cli, _, path) = session_file(core, agent_id)?;
    let excerpt = transcript::read(cli, &path, None, count).ok()?;
    let mut answers: Vec<String> = build(ended(excerpt.said, excerpt.next))
        .into_iter()
        .filter_map(|turn| {
            let joined = turn.said.join("\n\n");
            let text = joined.trim();
            if text.is_empty() {
                return None;
            }
            let skip = text.chars().count().saturating_sub(longest);
            let kept: String = text.chars().skip(skip).collect();
            Some(if skip > 0 {
                format!("[cut] {kept}")
            } else {
                kept
            })
        })
        .collect();
    let first = answers.len().saturating_sub(count);
    answers.drain(..first);
    Some(answers)
}

/// What an agent at rest said last, when its last turn ended on words: from the feed its chat
/// kept, else its CLI's session file. Reads only their tails and wakes nothing.
pub(crate) fn last_answer(core: &Core, agent_id: &str) -> Option<String> {
    if let Some(feed) = core.chats().heard(agent_id) {
        return feed.last_answer();
    }
    let (cli, path) = match core.chats().shelf.get(agent_id) {
        Some(shelved) => (
            AgentKind::from_name(&shelved.provider)?,
            shelved.transcript?,
        ),
        None => {
            let (cli, _, path) = session_file(core, agent_id)?;
            (cli, path)
        }
    };
    let excerpt = transcript::read(cli, &path, None, 1).ok()?;
    let mut turn = build(ended(excerpt.said, excerpt.next)).pop()?;
    (turn.last == Last::Message)
        .then(|| turn.said.pop())
        .flatten()
}

pub(crate) struct TerminalTurn {
    /// Where the turn starts in the session file, when there is one.
    pub start: Option<u64>,
    pub answer: Option<TerminalAnswer>,
}

pub(crate) enum TerminalAnswer {
    Said(String),
    Screen(String),
}

pub(crate) async fn read(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let caller = request
        .agent_id
        .clone()
        .ok_or("agent_read needs the calling agent's id")?;
    let agent = agent_param(&request.params)?;
    let cursor = cursor_param(&request.params)?.map(str::to_owned);
    let question = super::ask_aside::question_param(&request.params)?;
    if question.is_some() && cursor.is_some() {
        return Err("Pass a question or a cursor, not both".into());
    }
    if let Some(question) = &question {
        let project = super::caller::project_of(core, request);
        if let Some(task) = super::peer::teammate_task(core, &project, &agent) {
            let name = core.names().name(&caller).unwrap_or_else(|| caller.clone());
            let cli = super::agent_now::AgentNowCore::agent_now(&**core, &caller).cli();
            return super::peer::ask(core, &project, &task, question.clone(), name, cli).await;
        }
    }
    super::hosts::peers::find(core, &agent).await;
    let target = {
        let (core, request, caller) = (core.clone(), request.clone(), caller.clone());
        blocking(move || {
            let project = super::caller::project_of(&core, &request);
            let target =
                match roster::resolve(&core, &roster::listed(&core), &agent, &project, &caller) {
                    Ok(target) => target,
                    Err(why) => closed_worker(&core, &caller, &agent).ok_or(why)?,
                };
            if target.agent_id == caller {
                return Err("That agent is you".into());
            }
            Ok(target)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    if target.reach == Reach::Remote {
        if question.is_some() {
            return Err("An agent on another host can't be asked a question yet. Call agent_read without one to read its turns.".into());
        }
        return super::hosts::route::read(core, &caller, &target.agent_id, cursor).await;
    }
    take_up_kept(core, &target).await;
    let core = core.clone();
    blocking(move || match question {
        Some(question) => {
            super::ask_aside::answer(&core, &caller, &target, &question).map_err(Into::into)
        }
        None => read_target(&core, &target, cursor.as_deref()).map_err(Into::into),
    })
    .await
    .map_err(|error| error.to_string())
}

/// A worker `lead` started that has closed, read where it stopped rather than reopened.
fn closed_worker(core: &Core, lead: &str, wanted: &str) -> Option<Listed> {
    let wanted = wanted.trim().trim_start_matches('@');
    let (agent_id, ran) = core.names().closed_worker(lead, wanted)?;
    Some(Listed {
        agent_id,
        name: Some(wanted.to_owned()),
        project: ran.project,
        cli: ran.provider,
        title: ran.title,
        status: AgentStatus::Stopped,
        parent: Some(lead.to_owned()),
        active_at: None,
        rank: 0,
        reach: Reach::Stopped,
    })
}

/// What an agent of a team on a linked host reads of `agent_id`, an agent here.
pub(crate) async fn read_here(
    core: &Arc<Core>,
    agent_id: String,
    cursor: Option<String>,
) -> Result<Value, String> {
    let target = {
        let core = core.clone();
        blocking(move || {
            roster::listed(&core)
                .into_iter()
                .find(|agent| agent.agent_id == agent_id && agent.reach != Reach::Remote)
                .ok_or_else(|| "that agent is not on this host".into())
        })
        .await
        .map_err(|error| error.to_string())?
    };
    take_up_kept(core, &target).await;
    let core = core.clone();
    blocking(move || read_target(&core, &target, cursor.as_deref()).map_err(Into::into))
        .await
        .map_err(|error| error.to_string())
}

/// Takes up the history an earlier core kept of an asleep or closed chat, so it reads
/// without waking anything.
async fn take_up_kept(core: &Arc<Core>, target: &Listed) {
    if matches!(target.reach, Reach::Asleep | Reach::Stopped) {
        super::chat::unshelve(core, &target.agent_id, None).await;
    }
}

pub(crate) fn read_target(
    core: &Core,
    target: &Listed,
    cursor: Option<&str>,
) -> Result<Value, String> {
    let name = target
        .name
        .as_deref()
        .map(super::names::at_name)
        .unwrap_or_else(|| target.agent_id.clone());
    let mut answer = match core.chats().readable(&target.agent_id) {
        Ok(feed) => read_chat(target, &feed, cursor)?,
        Err(_)
            if target.reach == Reach::Terminal
                || session_file(core, &target.agent_id).is_some() =>
        {
            read_terminal(core, target, cursor)?
        }
        Err(_) => json!({
            "turns": [],
            "note": format!("{name} kept no record Sikemux can read."),
        }),
    };
    if let Some(fields) = answer.as_object_mut() {
        fields.insert("name".into(), name.into());
        fields.insert("agentId".into(), target.agent_id.clone().into());
        fields.insert("cli".into(), target.cli.clone().into());
        fields.insert(
            "status".into(),
            roster::status_word(crate::server::roster::standing(target, core)).into(),
        );
        let (model, effort) = runs_on(core, &target.agent_id);
        if let Some(model) = model {
            fields.insert("model".into(), model.into());
        }
        if let Some(effort) = effort {
            fields.insert("effort".into(), effort.into());
        }
    }
    Ok(answer)
}

/// An agent a wait is on. `status` is `None` once it is gone.
#[derive(Clone)]
pub(crate) struct Waited {
    pub(crate) agent_id: String,
    pub(crate) name: String,
    pub(crate) project: String,
    pub(crate) status: Option<AgentStatus>,
}

fn agents_param(params: &Value) -> Result<Vec<String>, String> {
    let wrong = || format!("agents must list 1 to {MAX_AGENTS} agent names or ids");
    let Some(Value::Array(agents)) = params.get("agents") else {
        return Err(wrong());
    };
    if agents.is_empty() || agents.len() > MAX_AGENTS {
        return Err(wrong());
    }
    agents
        .iter()
        .map(|agent| match agent {
            Value::String(agent) if !agent.trim().is_empty() && agent.len() <= 200 => {
                Ok(agent.clone())
            }
            _ => Err(wrong()),
        })
        .collect()
}

/// The agents `wanted` names, as they stand now. An agent id Sikemux named
/// once but no longer runs is an agent that is gone.
pub(crate) fn waited(
    core: &Core,
    request: &HarnessRequest,
    wanted: &[String],
    caller: &str,
) -> Result<Vec<Waited>, String> {
    let project = super::caller::project_of(core, request);
    let agents = roster::listed(core);
    let mut found: Vec<Waited> = Vec::new();
    for agent in wanted {
        let waited = match roster::resolve(core, &agents, agent, &project, caller) {
            Ok(target) => Waited {
                status: Some(crate::server::roster::standing(&target, core)),
                name: target.name.unwrap_or_else(|| target.agent_id.clone()),
                project: target.project,
                agent_id: target.agent_id,
            },
            Err(error) => {
                let named = core
                    .names()
                    .get(agent.trim())
                    .filter(|named| named.agent_id != caller)
                    .ok_or(error)?;
                Waited {
                    agent_id: named.agent_id,
                    name: named.name,
                    project: named.project,
                    status: None,
                }
            }
        };
        if waited.agent_id == caller {
            return Err("You cannot wait on yourself".into());
        }
        if !found.iter().any(|known| known.agent_id == waited.agent_id) {
            found.push(waited);
        }
    }
    if (found.first())
        .is_some_and(|first| found.iter().any(|waited| waited.project != first.project))
    {
        return Err(
            "agent_wait waits on agents of one project at a time; wait on each project's apart"
                .into(),
        );
    }
    Ok(found)
}

/// The agents a wait is on as they stand now, found again by id, as a name
/// no longer finds an agent that has gone.
fn refreshed(core: &Core, waited: &[Waited]) -> Vec<Waited> {
    let agents = roster::listed(core);
    waited
        .iter()
        .map(
            |known| match agents.iter().find(|agent| agent.agent_id == known.agent_id) {
                Some(agent) => Waited {
                    name: agent.name.clone().unwrap_or_else(|| known.name.clone()),
                    status: Some(crate::server::roster::standing(agent, core)),
                    ..known.clone()
                },
                None => Waited {
                    status: None,
                    ..known.clone()
                },
            },
        )
        .collect()
}

fn standing(waited: &Waited) -> Value {
    json!({
        "name": super::names::at_name(&waited.name),
        "agentId": waited.agent_id,
        "status": waited.status.map_or("gone", roster::status_word),
    })
}

pub(crate) async fn wait(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let caller = request
        .agent_id
        .clone()
        .ok_or("agent_wait needs the calling agent's id")?;
    let params = &request.params;
    let wanted = agents_param(params)?;
    let until = Until::of(params)?;
    let after = cursor_param(params)?
        .map(|cursor| {
            cursor
                .parse::<u64>()
                .map_err(|_| "cursor must be one agent_wait gave you".to_owned())
        })
        .transpose()?;
    let timeout = match params.get("timeoutMs") {
        None | Some(Value::Null) => MAX_WAIT_MS,
        Some(value) => value
            .as_u64()
            .filter(|ms| *ms <= MAX_WAIT_MS)
            .ok_or(format!(
                "timeoutMs must be an integer between 0 and {MAX_WAIT_MS}"
            ))?,
    };
    let harness = core.harness();
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let first = {
        let (core, request) = (core.clone(), request.clone());
        blocking(move || waited(&core, &request, &wanted, &caller).map_err(Into::into))
            .await
            .map_err(|error| error.to_string())?
    };
    let project = (first.first())
        .map(|waited| waited.project.clone())
        .ok_or("agent_wait needs an agent to wait on")?;
    let ids: HashSet<String> = first.iter().map(|waited| waited.agent_id.clone()).collect();
    let after = match after {
        Some(after) => after,
        None => {
            let cursor = harness.journal_cursor(&project);
            let already: Vec<Value> = first
                .iter()
                .filter(|waited| until.already(waited.status))
                .map(|waited| {
                    json!({
                        "agent": super::names::at_name(&waited.name),
                        "agentId": waited.agent_id,
                        "what": format!("is {} now", waited.status.map_or("gone", roster::status_word)),
                    })
                })
                .collect();
            if !already.is_empty() {
                return Ok(answer(&first, already, cursor, false, false));
            }
            cursor
        }
    };
    loop {
        let mut changed = harness.subscribe();
        let page = harness.journal_since(&project, after).map_err(|_| {
            "cursor must be one agent_wait gave you for these agents' project".to_owned()
        })?;
        let heard: Vec<Value> = page
            .events
            .iter()
            .filter_map(|entry| {
                let agent_id = entry.agent_id.as_deref().filter(|id| ids.contains(*id))?;
                let happened =
                    Happened::of(&entry.kind).filter(|happened| until.heard(*happened))?;
                let name = first
                    .iter()
                    .find(|waited| waited.agent_id == agent_id)
                    .map_or(agent_id, |waited| waited.name.as_str());
                Some(json!({
                    "agent": super::names::at_name(name),
                    "agentId": agent_id,
                    "what": happened.words(),
                    "cursor": entry.seq.to_string(),
                }))
            })
            .collect();
        let timed_out = Instant::now() >= deadline;
        if !heard.is_empty() || page.truncated || timed_out {
            let now = {
                let (core, first) = (core.clone(), first.clone());
                blocking(move || Ok(refreshed(&core, &first)))
                    .await
                    .map_err(|error| error.to_string())?
            };
            return Ok(answer(
                &now,
                heard,
                page.cursor,
                page.truncated,
                timed_out && page.events.is_empty(),
            ));
        }
        tokio::select! {
            _ = changed.changed() => {}
            _ = tokio::time::sleep_until(deadline) => {}
        }
    }
}

fn answer(
    agents: &[Waited],
    heard: Vec<Value>,
    cursor: u64,
    truncated: bool,
    timed_out: bool,
) -> Value {
    let note = if heard.is_empty() {
        "Nothing you wait for has happened yet. Wait again with this cursor, or carry on: an agent you started reports to you by itself."
    } else {
        "agent_read shows what an agent said. Wait again with this cursor for what comes next."
    };
    let mut answer = json!({
        "events": heard,
        "agents": agents.iter().map(standing).collect::<Vec<_>>(),
        "cursor": cursor.to_string(),
        "timedOut": timed_out && heard.is_empty(),
        "note": note,
    });
    if truncated {
        if let Some(fields) = answer.as_object_mut() {
            fields.insert("truncated".into(), true.into());
        }
    }
    answer
}

#[cfg(test)]
mod tests {
    use sikemux_fold::ChatEventKind;
    use sikemux_wire::protocol::PermissionMode;

    use super::*;

    fn listed(id: &str, name: &str, cli: &str, reach: Reach) -> Listed {
        Listed {
            agent_id: id.to_owned(),
            name: Some(name.to_owned()),
            project: "/repo".to_owned(),
            cli: cli.to_owned(),
            title: Some("Worker".to_owned()),
            status: AgentStatus::Idle,
            parent: Some("chief".to_owned()),
            active_at: None,
            rank: 0,
            reach,
        }
    }

    fn emit_turn(feed: &Arc<Feed>, prompt: &str, answer: &str) {
        feed.emit(ChatEventKind::Prompt, json!({ "text": prompt }));
        feed.emit(ChatEventKind::TurnStarted, json!({}));
        feed.emit(
            ChatEventKind::SessionUpdate,
            json!({
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": answer }
                }
            }),
        );
        feed.emit(
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "end_turn" }),
        );
    }

    #[test]
    fn cut_bytes_surfaces_omitted_size_instead_of_silent_ellipsis() {
        assert_eq!(cut_bytes("short text", 100), "short text");
        let under_kb = cut_bytes(&"a".repeat(200), 50);
        assert!(
            under_kb.ends_with("[truncated: 150 bytes omitted]"),
            "{under_kb}"
        );
        let over_kb = cut_bytes(&"x".repeat(5 * 1024), 1024);
        assert!(over_kb.ends_with("[truncated: 4 KB omitted]"), "{over_kb}");
        let utf8 = cut_bytes(&"é".repeat(100), 11);
        assert!(utf8.contains("[truncated:"), "{utf8}");
    }

    #[tokio::test]
    async fn long_chat_answer_reports_truncation_and_continues_to_completion_via_cursor() {
        let feed = Feed::new("agent-asuna".into(), PermissionMode::WorkspaceWrite, None);
        let mut sections = Vec::new();
        for i in 0..45 {
            sections.push(format!(
                "## Section {i:02}\nFindings for item {i:02}: \"quoted\" value & unicode — café.\n{}",
                "detail ".repeat(65)
            ));
        }
        let full_answer = sections.join("\n\n");
        assert!(full_answer.len() > 20 * 1024);

        emit_turn(&feed, "Audit the Android simulation tooling", &full_answer);
        let target = listed("agent-asuna", "Asuna", "antigravity", Reach::Chat);

        let mut cursor: Option<String> = None;
        let mut chunks = Vec::new();
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(pages <= 10, "continuation loop did not terminate");
            let page = read_chat(&target, &feed, cursor.as_deref()).unwrap();
            let serialized = serde_json::to_vec(&page).unwrap().len();
            assert!(
                serialized <= 10 * 1024,
                "page {pages} exceeded MCP 10 KB budget ({serialized} bytes)"
            );
            let turn = &page["turns"][0];
            let answer = turn["answer"].as_str().unwrap();
            let more = page["more"].as_bool().unwrap();
            if pages == 1 {
                assert!(more, "first page must signal more");
                assert_eq!(turn["truncated"], json!(true));
                assert!(turn["remainingBytes"].as_u64().unwrap() > 0);
                assert!(answer.contains("[truncated:"), "{answer}");
                assert!(page["note"].as_str().unwrap().contains("truncated"));
                assert!(page["cursor"].as_str().unwrap().contains('+'));
                assert!(turn.get("asked").is_some());
            } else {
                assert_eq!(turn["continued"], json!(true));
                assert!(turn.get("asked").is_none());
            }
            let body = answer
                .split_once("\n\n[truncated:")
                .map_or(answer, |(head, _)| head);
            chunks.push(body.to_owned());
            cursor = page["cursor"].as_str().map(str::to_owned);
            if !more {
                assert!(turn.get("truncated").is_none());
                assert!(!page["cursor"].as_str().unwrap().contains('+'));
                break;
            }
        }
        assert!(pages >= 3);
        let rebuilt = chunks.join("\n");
        for i in 0..45 {
            assert!(
                rebuilt.contains(&format!("## Section {i:02}")),
                "missing section {i:02} after cursor continuation"
            );
            assert!(
                rebuilt.contains(&format!("Findings for item {i:02}")),
                "missing findings {i:02} after cursor continuation"
            );
        }

        // Calling once more with the final cursor reports nothing new.
        let done = read_chat(&target, &feed, cursor.as_deref()).unwrap();
        assert_eq!(done["turns"].as_array().unwrap().len(), 0);
        assert_eq!(done["more"], json!(false));
    }

    #[tokio::test]
    async fn continuation_cursor_advances_to_subsequent_turn_and_handles_restarts() {
        let feed = Feed::new("agent-asuna".into(), PermissionMode::WorkspaceWrite, None);
        let long_first = (0..25)
            .map(|i| format!("Line {i:02}: {}", "word ".repeat(85)))
            .collect::<Vec<_>>()
            .join("\n");
        let second = "Second turn final conclusion.";
        emit_turn(&feed, "First prompt", &long_first);
        emit_turn(&feed, "Second prompt", second);
        let target = listed("agent-asuna", "Asuna", "codex", Reach::Chat);

        let mut cursor: Option<String> = None;
        let mut saw_second = false;
        for _ in 0..10 {
            let page = read_chat(&target, &feed, cursor.as_deref()).unwrap();
            for turn in page["turns"].as_array().unwrap() {
                if turn["answer"].as_str() == Some(second) {
                    saw_second = true;
                }
            }
            cursor = page["cursor"].as_str().map(str::to_owned);
            if !page["more"].as_bool().unwrap() {
                break;
            }
        }
        assert!(saw_second, "never reached turn after truncated answer");

        // A cursor from an earlier feed resets cleanly even with +offset.
        let restarted = read_chat(&target, &feed, Some("old-feed:0+4096")).unwrap();
        assert!(restarted["note"]
            .as_str()
            .unwrap()
            .contains("started again"));
        assert!(!restarted["turns"].as_array().unwrap().is_empty());

        // Malformed +offset cursors are rejected.
        assert!(read_chat(&target, &feed, Some(&format!("{}:0+0", feed.id()))).is_err());
        assert!(read_chat(&target, &feed, Some(&format!("{}:0+abc", feed.id()))).is_err());
    }
}
