//! What `agent_read` and `agent_wait` answer: what another agent said, in
//! short, and waiting until agents finish or need the person. What agents do
//! is journaled beside the project's task events, so a wait from a cursor
//! misses nothing that happened between two waits.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::Instant;

use crate::cli::protocol::HarnessRequest;
use crate::protocol::AgentStatus;

use super::chat::feed::Said;
use super::connection::blocking;
use super::messages::{self, Listed, Reach};
use super::transcript::{self, Cli};
use super::Core;

const MAX_WAIT_MS: u64 = 30_000;
const MAX_AGENTS: usize = 8;
/// Turns read when no cursor says where to start.
const RECENT_TURNS: usize = 3;
/// About how much one read answers with.
const MAX_READ_BYTES: usize = 32 * 1024;
const MAX_ASKED_CHARS: usize = 600;
const MAX_SAID_BYTES: usize = 2 * 1024;
const MAX_ANSWER_BYTES: usize = 8 * 1024;
const TOOL_TITLES: usize = 6;
const MAX_TITLE_CHARS: usize = 80;
const SCREEN_LINES: usize = 40;
const REPORT_SCREEN_LINES: usize = 30;

/// Something that happened to an agent.
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

    fn words(self) -> &'static str {
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
enum Until {
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
            Some(Value::String(until)) => match until.as_str() {
                "idle" => Ok(Until::Idle),
                "done" => Ok(Until::Done),
                "needs_you" => Ok(Until::NeedsYou),
                "any" => Ok(Until::Any),
                other => Err(format!(
                    "until must be idle, done, needs_you or any, not {other}"
                )),
            },
            Some(_) => Err("until must be idle, done, needs_you or any".into()),
        }
    }

    fn heard(self, happened: Happened) -> bool {
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
    fn already(self, status: Option<AgentStatus>) -> bool {
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

fn cut_bytes(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} […]", text[..end].trim_end())
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
        let Some(length) = text[start..].find('>') else {
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
        .map(|(name, _)| name.replace("&quot;", "\"").replace("&amp;", "&"))
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

    fn view(mut self) -> Value {
        let finished = self.ended.as_ref().and_then(Value::as_str) == Some("finished");
        let answer = (finished && self.last == Last::Message)
            .then(|| self.said.pop())
            .flatten();
        let tools = self.tools_line();
        let mut turn = json!({
            "state": self.ended.take().unwrap_or_else(|| "working".into()),
        });
        let fields = turn.as_object_mut().expect("a turn is an object");
        if !self.asked.is_empty() {
            fields.insert("asked".into(), self.asked.into());
        }
        if !self.said.is_empty() {
            let said: Vec<String> = self
                .said
                .iter()
                .map(|text| cut_bytes(text, MAX_SAID_BYTES))
                .collect();
            fields.insert("said".into(), said.into());
        }
        if let Some(tools) = tools {
            fields.insert("tools".into(), tools.into());
        }
        if let Some(answer) = answer {
            fields.insert("answer".into(), cut_bytes(&answer, MAX_ANSWER_BYTES).into());
        }
        turn
    }
}

fn build(said: Vec<(u64, bool, Said)>) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for (index, starts, said) in said {
        let ended = turns.last().is_none_or(|turn| turn.ended.is_some());
        if starts || ended {
            turns.push(Turn::new(index));
        }
        let turn = turns.last_mut().expect("a turn was just made");
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
            Said::Failed(message) => {
                turn.ended = Some(format!("failed: {}", cut_chars(&message, 400)).into());
            }
        }
    }
    turns
}

/// What was said, as turns, as many as fit, and where the first that did
/// not fit starts.
fn turns(said: Vec<(u64, bool, Said)>) -> (Vec<Value>, Option<u64>) {
    let mut views = Vec::new();
    let mut bytes = 0;
    for turn in build(said) {
        if bytes >= MAX_READ_BYTES && !views.is_empty() {
            return (views, Some(turn.start));
        }
        let start = turn.start;
        let view = turn.view();
        bytes += serde_json::to_vec(&view).map_or(0, |bytes| bytes.len());
        if bytes > MAX_READ_BYTES && !views.is_empty() {
            return (views, Some(start));
        }
        views.push(view);
    }
    (views, None)
}

fn read_cursor(feed: &str, index: u64) -> String {
    format!("{feed}:{index}")
}

/// The agent's last turns, or what it said since `cursor`.
fn read_chat(core: &Core, target: &Listed, cursor: Option<&str>) -> Result<Value, String> {
    let chat = core
        .chats
        .get(&target.agent_id)
        .ok_or("that agent is not running")?;
    let name = target.name.as_deref().unwrap_or(&target.agent_id);
    let from = cursor
        .map(|cursor| {
            cursor
                .split_once(':')
                .and_then(|(feed, index)| Some((feed, index.parse::<u64>().ok()?)))
                .ok_or("cursor must be one agent_read gave you")
        })
        .transpose()?;
    let mut notes = Vec::new();
    let index = match from {
        Some((feed, index)) if feed == chat.feed.id() => Some(index),
        Some(_) => {
            notes.push(format!(
                "{name} started again since that cursor, so this begins at its last turns."
            ));
            None
        }
        None => None,
    };
    let excerpt = chat.feed.excerpt(index, RECENT_TURNS, MAX_READ_BYTES * 4);
    if excerpt.skipped {
        notes.push("Some of what it said after your cursor is no longer kept.".to_owned());
    }
    let (turns, rest) = turns(excerpt.said);
    if turns.is_empty() {
        notes.push(format!("{name} has said nothing since your cursor."));
    }
    if rest.is_some() {
        notes.push("There is more: read again with this cursor.".to_owned());
    }
    let mut answer = json!({
        "turns": turns,
        "cursor": read_cursor(&excerpt.feed, rest.unwrap_or(excerpt.next)),
        "more": rest.is_some(),
    });
    if !notes.is_empty() {
        answer["note"] = notes.join(" ").into();
    }
    Ok(answer)
}

/// The last lines of a terminal agent's screen.
fn screen_tail(core: &Core, agent_id: &str, lines: usize) -> Option<String> {
    let session = core.agent_session(agent_id)?;
    let (screen, _) = super::agent::screen_text(&session)?;
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
/// which session it runs.
fn session_file(core: &Core, agent_id: &str) -> Option<(Cli, String, std::path::PathBuf)> {
    let terminal = core.agent_session(agent_id)?;
    let owner = terminal.owner();
    let cli = Cli::of(owner.agent_type.as_deref()?)?;
    let session = core.names.session_of(agent_id)?;
    let path = transcript::locate(cli, owner.account_home.as_deref(), &session)?;
    Some((cli, session, path))
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
    let name = target.name.as_deref().unwrap_or(&target.agent_id);
    let mut notes = Vec::new();
    let mut answer = match session_file(core, &target.agent_id) {
        Some((cli, session, path)) => {
            let from = match cursor.map(|cursor| {
                cursor
                    .rsplit_once('@')
                    .and_then(|(session, at)| Some((session, at.parse::<u64>().ok()?)))
                    .ok_or("cursor must be one agent_read gave you")
            }) {
                Some(Ok((from, at))) if from == session => Some(at),
                Some(Ok(_)) => {
                    notes.push(format!(
                        "{name} started a new session since that cursor, so this begins at its last turns."
                    ));
                    None
                }
                Some(Err(error)) => return Err(error.into()),
                None => None,
            };
            let excerpt = transcript::read(cli, &path, from, RECENT_TURNS)
                .map_err(|error| format!("Sikemux could not read {name}'s session: {error}"))?;
            if excerpt.restarted {
                notes.push(format!(
                    "{name}'s session is not where your cursor was, so this begins at its last turns."
                ));
            }
            let said = if excerpt.whole && at_rest(target.status) {
                ended(excerpt.said, excerpt.next)
            } else {
                excerpt.said
            };
            let (turns, rest) = turns(said);
            if turns.is_empty() {
                notes.push(format!("{name} has said nothing since your cursor."));
            }
            if rest.is_some() || !excerpt.whole {
                notes.push("There is more: read again with this cursor.".to_owned());
            }
            json!({
                "turns": turns,
                "cursor": format!("{session}@{}", rest.unwrap_or(excerpt.next)),
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
        if let Some(screen) = screen_tail(core, &target.agent_id, SCREEN_LINES) {
            answer["screen"] = screen.into();
        }
        if file {
            notes.push(format!(
                "{name} waits on the person; screen is what it shows them."
            ));
        }
    }
    if !notes.is_empty() {
        answer["note"] = notes.join(" ").into();
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
    let target = {
        let (core, request, caller) = (core.clone(), request.clone(), caller.clone());
        blocking(move || {
            let project = messages::caller_project(&core, &request);
            let target =
                messages::resolve(&core, &messages::listed(&core), &agent, &project, &caller)?;
            if target.agent_id == caller {
                return Err("That agent is you".into());
            }
            let name = target
                .name
                .clone()
                .unwrap_or_else(|| target.agent_id.clone());
            messages::blind_between(&core, &caller, &target.agent_id, &name)?;
            Ok(target)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    if target.reach == Reach::Remote {
        return super::hosts::route::read(core, &caller, &target.agent_id, cursor).await;
    }
    let core = core.clone();
    blocking(move || read_target(&core, &target, cursor.as_deref()).map_err(Into::into))
        .await
        .map_err(|error| error.to_string())
}

/// What an agent of a team on a linked host reads of `agent_id`, an agent here.
pub(crate) async fn read_here(
    core: &Arc<Core>,
    agent_id: String,
    cursor: Option<String>,
) -> Result<Value, String> {
    let core = core.clone();
    blocking(move || {
        let target = messages::listed(&core)
            .into_iter()
            .find(|agent| agent.agent_id == agent_id && agent.reach != Reach::Remote)
            .ok_or("that agent is not on this host")?;
        if core.names.is_blind(&target.agent_id) {
            return Err(
                "that agent reviews a card blind for Auto, so no agent may reach it".into(),
            );
        }
        read_target(&core, &target, cursor.as_deref()).map_err(Into::into)
    })
    .await
    .map_err(|error| error.to_string())
}

fn read_target(core: &Core, target: &Listed, cursor: Option<&str>) -> Result<Value, String> {
    let name = target
        .name
        .clone()
        .unwrap_or_else(|| target.agent_id.clone());
    let mut answer = if core.chats.get(&target.agent_id).is_some() {
        read_chat(core, target, cursor)?
    } else {
        match target.reach {
            Reach::Terminal => read_terminal(core, target, cursor)?,
            Reach::Asleep => json!({
                "turns": [],
                "note": format!("{name} is asleep, so Sikemux has nothing of it to read until it runs again. A message with agent_send wakes it."),
            }),
            Reach::Chat | Reach::Stopped | Reach::Remote => json!({
                "turns": [],
                "note": format!("{name} has stopped, so Sikemux has nothing of it to read."),
            }),
        }
    };
    let fields = answer.as_object_mut().expect("an answer is an object");
    fields.insert("name".into(), name.into());
    fields.insert("agentId".into(), target.agent_id.clone().into());
    fields.insert("cli".into(), target.cli.clone().into());
    fields.insert(
        "status".into(),
        messages::status_word(target.standing(core)).into(),
    );
    Ok(answer)
}

/// An agent a wait is on. `status` is `None` once it is gone.
#[derive(Clone)]
struct Waited {
    agent_id: String,
    name: String,
    project: String,
    status: Option<AgentStatus>,
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
fn waited(
    core: &Core,
    request: &HarnessRequest,
    wanted: &[String],
    caller: &str,
) -> Result<Vec<Waited>, String> {
    let project = messages::caller_project(core, request);
    let agents = messages::listed(core);
    let mut found: Vec<Waited> = Vec::new();
    for agent in wanted {
        let waited = match messages::resolve(core, &agents, agent, &project, caller) {
            Ok(target) => Waited {
                status: Some(target.standing(core)),
                name: target.name.unwrap_or_else(|| target.agent_id.clone()),
                project: target.project,
                agent_id: target.agent_id,
            },
            Err(error) => {
                let named = core
                    .names
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
        messages::blind_between(core, caller, &waited.agent_id, &waited.name)?;
        if !found.iter().any(|known| known.agent_id == waited.agent_id) {
            found.push(waited);
        }
    }
    if found
        .iter()
        .any(|waited| waited.project != found[0].project)
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
    let agents = messages::listed(core);
    waited
        .iter()
        .map(
            |known| match agents.iter().find(|agent| agent.agent_id == known.agent_id) {
                Some(agent) => Waited {
                    name: agent.name.clone().unwrap_or_else(|| known.name.clone()),
                    status: Some(agent.standing(core)),
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
        "name": waited.name,
        "agentId": waited.agent_id,
        "status": waited.status.map_or("gone", messages::status_word),
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
    let harness = &core.harness;
    let _slot = harness.wait_slot()?;
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let first = {
        let (core, request) = (core.clone(), request.clone());
        blocking(move || waited(&core, &request, &wanted, &caller).map_err(Into::into))
            .await
            .map_err(|error| error.to_string())?
    };
    let project = first[0].project.clone();
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
                        "agent": waited.name,
                        "agentId": waited.agent_id,
                        "what": format!("is {} now", waited.status.map_or("gone", messages::status_word)),
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
                    "agent": name,
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
        answer["truncated"] = true.into();
    }
    answer
}
