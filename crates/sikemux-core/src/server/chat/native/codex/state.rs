//! What the chat knows of a Codex thread while it runs: which turn answers the
//! prompt, which items are on screen, and its subagents. Each notification
//! comes back as the effects it has on the chat.

use sikemux_fold::update::{agent_text, notice, text_content, thought};
use sikemux_fold::UpdateKind;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::chat::failure::{Failure, FailureKind};

use super::super::super::reset::{month_day, WallClock, Zone};
use super::super::super::states::{self, Outcome, Subagent};
use super::super::json::text_or_empty as text;
use super::super::session::TurnEnd;
use super::super::turns::{self, Ended, Prompted, Started, Turns};
use super::approvals::{self, Question};
use super::items::{self, Items};
use super::notifications::{self, EndedTurn, Notification};
use super::subagents::{Route, Subagents};

/// What one notification does to the chat.
pub(super) enum Effect {
    /// A session update, for the session with this id.
    Update(String, Value),
    End(u64, TurnEnd),
    WorkStarted,
    WorkEnded(&'static str),
    /// Ask Codex to stop the turn with this id.
    Interrupt(String),
}

/// How to answer one of Codex's requests.
pub(super) enum Reply {
    Ask(Question),
    /// Codex asks something with request_user_input, which goes the way of
    /// Sikemux's own questions.
    PutQuestion,
    Answer(Value),
    Refuse,
}

/// Codex's request_user_input answered with what the agent is told in place
/// of the answer, which comes later as a message.
pub(super) fn user_input_told(params: &Value, told: &str) -> Value {
    let answers: serde_json::Map<String, Value> = params["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|question| question["id"].as_str())
        .map(|id| (id.to_owned(), json!({ "answers": [told] })))
        .collect();
    json!({ "answers": answers })
}

#[derive(Serialize, Deserialize)]
pub(super) struct Session {
    pub thread_id: String,
    pub turns: Turns,
    /// The last turn that ended outside a prompt, should a prompt be told it.
    last_ended: Option<EndedTurn>,
    errors: HashMap<String, Value>,
    /// How many times Codex has retried each turn's request.
    retries: HashMap<String, u64>,
    items: Items,
    edits: HashMap<String, Value>,
    #[serde(with = "super::super::json::pairs")]
    tool_calls: HashMap<(String, String), Vec<String>>,
    pub subagents: Subagents,
    pub token_usage: Option<Value>,
    /// The newest turn of the thread the chat has been told of.
    pub latest_turn: Option<String>,
    /// The chat's own tool servers that have not reported starting yet.
    pub starting_servers: HashSet<String>,
    questions: u64,
}

/// What Codex's error info says a failed turn needs.
fn failure_kind(info: &Value) -> FailureKind {
    let status = info
        .as_object()
        .and_then(|info| info.values().next())
        .and_then(|details| details["httpStatusCode"].as_u64());
    if info == "unauthorized" || status == Some(401) {
        return FailureKind::SignIn;
    }
    if status == Some(429) {
        return FailureKind::Limit;
    }
    match info.as_str() {
        Some("usageLimitExceeded" | "rateLimitExceeded") => FailureKind::Limit,
        Some(_) => FailureKind::Other,
        None if cut_off(info) => FailureKind::Network,
        None => FailureKind::Other,
    }
}

/// Whether Codex lost its connection to the model provider.
fn cut_off(info: &Value) -> bool {
    info.as_object().is_some_and(|info| {
        [
            "httpConnectionFailed",
            "responseStreamConnectionFailed",
            "responseStreamDisconnected",
            "responseTooManyFailedAttempts",
        ]
        .iter()
        .any(|key| info.contains_key(*key))
    })
}

/// The failure a turn ended with, from Codex's error for it.
pub(super) fn failure(error: &Value) -> Failure {
    let kind = failure_kind(&error["codexErrorInfo"]);
    let title = error["message"]
        .as_str()
        .filter(|message| !message.is_empty())
        .unwrap_or("Turn failed");
    if kind == FailureKind::Network {
        return Failure::lost_connection(title);
    }
    Failure {
        kind,
        title: title.to_owned(),
        details: None,
        resets_at: (kind == FailureKind::Limit)
            .then(|| resets_at(title))
            .flatten(),
    }
}

/// When a usage limit resets, from the "try again at 2:46 AM" or "try again
/// at Oct 12th, 2026 2:46 AM" Codex ends its message with, in this host's time.
fn resets_at(message: &str) -> Option<u64> {
    let stamp = message
        .rsplit_once("try again at ")?
        .1
        .trim_end_matches('.');
    let (date, time) = match stamp.split_once(", ") {
        Some((day, rest)) => {
            let (year, time) = rest.split_once(' ')?;
            let (month, day) = month_day(day)?;
            (Some((Some(year.parse().ok()?), month, day)), time)
        }
        None => (None, stamp),
    };
    let (clock, half) = time.split_once(' ')?;
    let pm = match half {
        "AM" => false,
        "PM" => true,
        _ => return None,
    };
    let (hour, minute) = clock.split_once(':')?;
    let (hour, minute): (i32, i32) = (hour.parse().ok()?, minute.parse().ok()?);
    if !(1..=12).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    WallClock {
        zone: Zone::Host,
        date,
        hour: hour % 12 + if pm { 12 } else { 0 },
        minute,
    }
    .unix_ms()
}

impl Session {
    pub fn new(thread_id: String) -> Self {
        Self {
            subagents: Subagents::new(thread_id.clone()),
            thread_id,
            turns: Turns::default(),
            last_ended: None,
            errors: HashMap::new(),
            retries: HashMap::new(),
            items: Items::default(),
            edits: HashMap::new(),
            tool_calls: HashMap::new(),
            token_usage: None,
            latest_turn: None,
            starting_servers: HashSet::new(),
            questions: 0,
        }
    }

    /// Codex answered turn `number` with its id for it.
    pub fn started(&mut self, number: u64, id: &str) -> Vec<Effect> {
        let Some(cancelled) = self.turns.started(number, id) else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        if cancelled {
            effects.push(Effect::Interrupt(id.to_owned()));
        }
        if let Some(ended) = self.last_ended.take().filter(|turn| turn.id == id) {
            if let Ended::Prompted(turn) = self.turns.completed(id) {
                effects.extend(self.finish_prompted(turn, &ended));
            }
        }
        effects
    }

    pub fn cancel(&mut self) -> Vec<Effect> {
        self.turns
            .cancel()
            .map(Effect::Interrupt)
            .into_iter()
            .collect()
    }

    /// Codex took the interrupt of turn `id`. The turn is over for the chat
    /// now, and what Codex still says of it is dropped.
    pub fn interrupted(&mut self, id: &str) -> Vec<Effect> {
        match self.turns.interrupted(id) {
            Ended::Prompted(turn) => {
                let ended = EndedTurn {
                    id: id.to_owned(),
                    status: "interrupted".to_owned(),
                    error: None,
                };
                self.finish_prompted(turn, &ended)
            }
            Ended::Unprompted => {
                let mut effects = self.finish_subagents(Subagent::Cancelled);
                effects.push(Effect::WorkEnded("cancelled"));
                effects
            }
            Ended::Neither => Vec::new(),
        }
    }

    fn finish_subagents(&mut self, state: Subagent) -> Vec<Effect> {
        let mut out = Vec::new();
        self.subagents.finish_outstanding(state, &mut out);
        out.into_iter()
            .map(|(session, update)| Effect::Update(session, update))
            .collect()
    }

    fn payload(&self, stop_reason: &str) -> Value {
        let mut payload = json!({ "stopReason": stop_reason });
        if let Some(usage) = self.token_usage.as_ref().and_then(items::turn_usage) {
            payload["usage"] = usage;
        }
        payload
    }

    fn finish_prompted(&mut self, mut turn: Prompted, ended: &EndedTurn) -> Vec<Effect> {
        let error = self.errors.remove(&ended.id);
        let mut effects: Vec<Effect> = self
            .turns
            .title(&mut turn)
            .map(|title| Effect::Update(self.thread_id.clone(), title))
            .into_iter()
            .collect();
        let outcome = states::outcome(&ended.status).unwrap_or(Outcome::Failed);
        let (subagents, end) = turns::prompted_end(outcome, self.payload("end_turn"), || {
            let error = error
                .or_else(|| ended.error.clone().filter(|error| !error.is_null()))
                .unwrap_or_else(|| json!({ "message": "Turn failed", "codexErrorInfo": null }));
            failure(&error)
        });
        if let Some(state) = subagents {
            effects.extend(self.finish_subagents(state));
        }
        effects.push(Effect::End(turn.number, end));
        effects
    }

    fn turn_completed(&mut self, ended: &EndedTurn) -> Vec<Effect> {
        let id = ended.id.as_str();
        self.retries.remove(id);
        let unnamed = self.turns.unnamed();
        match self.turns.completed(id) {
            Ended::Unprompted => {
                self.errors.remove(id);
                let outcome = states::outcome(&ended.status).unwrap_or(Outcome::Failed);
                let (subagents, reason) = turns::unprompted_end(outcome);
                let mut effects = subagents
                    .map(|state| self.finish_subagents(state))
                    .unwrap_or_default();
                if unnamed {
                    self.last_ended = Some(ended.clone());
                }
                effects.push(Effect::WorkEnded(reason));
                effects
            }
            Ended::Prompted(turn) => self.finish_prompted(turn, ended),
            Ended::Neither => Vec::new(),
        }
    }

    /// The effects of one notification from Codex.
    pub fn handle(&mut self, method: &str, params: &Value) -> Vec<Effect> {
        let mut out = Vec::new();
        let mut replay = Vec::new();
        let route = self.subagents.route(method, params, &mut out, &mut replay);
        let mut effects: Vec<Effect> = out
            .into_iter()
            .map(|(session, update)| Effect::Update(session, update))
            .collect();
        match route {
            Route::Taken => {
                for (method, params) in replay {
                    effects.extend(self.handle(&method, &params));
                }
            }
            Route::Session(session) => {
                let root = params["threadId"]
                    .as_str()
                    .is_none_or(|thread| thread == self.thread_id);
                let stale = params["turnId"]
                    .as_str()
                    .is_some_and(|turn| self.turns.is_stale(turn));
                match notifications::read(method, params) {
                    Some(Notification::Unreadable { method, error }) => {
                        if root {
                            effects.extend(self.unreadable(method, error));
                        }
                    }
                    Some(notification) => {
                        if root {
                            effects.extend(self.root_notification(&notification));
                        }
                        if !(root && stale) {
                            effects.extend(
                                self.updates(&notification)
                                    .into_iter()
                                    .map(|update| Effect::Update(session.clone(), update)),
                            );
                        }
                    }
                    None => {}
                }
            }
        }
        effects
    }

    fn root_notification(&mut self, notification: &Notification) -> Vec<Effect> {
        match notification {
            Notification::TurnStarted(started) => {
                let id = started.turn.id;
                self.latest_turn = Some(id.to_owned());
                match self.turns.began(id) {
                    Started::Prompted { cancelled: true } => vec![Effect::Interrupt(id.to_owned())],
                    Started::Unprompted => vec![Effect::WorkStarted],
                    Started::Prompted { .. } | Started::Neither => Vec::new(),
                }
            }
            Notification::TurnCompleted(completed) => self.turn_completed(&completed.turn),
            Notification::Error(notice) if notice.will_retry => {
                let turn = notice.turn_id.unwrap_or_default().to_owned();
                if self.turns.is_stale(&turn) {
                    return Vec::new();
                }
                let attempt = self.retries.entry(turn.clone()).or_default();
                *attempt += 1;
                let category = match failure_kind(&notice.error["codexErrorInfo"]) {
                    FailureKind::Network => "connection",
                    _ => "service",
                };
                let detail = notice.error["message"]
                    .as_str()
                    .unwrap_or("Codex is retrying");
                let update = crate::server::chat::failure::retrying(
                    &format!("{turn}:retry"),
                    category,
                    *attempt,
                    detail,
                );
                vec![Effect::Update(self.thread_id.clone(), update)]
            }
            Notification::Error(notice) => {
                if let Some(turn) = notice.turn_id {
                    self.errors.insert(turn.to_owned(), notice.error.clone());
                }
                Vec::new()
            }
            Notification::TokenUsage(usage) => {
                self.token_usage = Some(usage.token_usage.clone());
                Vec::new()
            }
            Notification::ThreadNamed(_) => {
                self.turns.titled = true;
                Vec::new()
            }
            Notification::McpServerStatus(server) => {
                let name = server.name;
                if !matches!(server.status, "ready" | "failed" | "cancelled")
                    || !self.starting_servers.remove(name)
                {
                    return Vec::new();
                }
                let said = match server.status {
                    "failed" => format!(
                        "[codex-acp forwarded startup error] MCP server `{name}` failed to start: {}",
                        server.error.unwrap_or("unknown MCP startup error")
                    ),
                    "cancelled" => {
                        format!("[codex-acp forwarded startup error] MCP server `{name}` startup was cancelled.")
                    }
                    _ => return Vec::new(),
                };
                vec![Effect::Update(
                    self.thread_id.clone(),
                    json!({
                        "sessionUpdate": UpdateKind::ToolCall,
                        "toolCallId": format!("mcp_startup.{name}"),
                        "kind": "other",
                        "title": format!("mcp__{name}__startup"),
                        "status": "failed",
                        "content": [text_content(&said)],
                    }),
                )]
            }
            _ => Vec::new(),
        }
    }

    /// A turn whose end Codex sent unreadably ends for the chat as failed,
    /// rather than leaving it waiting on an end it cannot read.
    fn unreadable(&mut self, method: &str, error: String) -> Vec<Effect> {
        if method != "turn/completed" {
            return Vec::new();
        }
        let Some(mut turn) = self.turns.give_up() else {
            return Vec::new();
        };
        let mut effects: Vec<Effect> = self
            .turns
            .title(&mut turn)
            .map(|title| Effect::Update(self.thread_id.clone(), title))
            .into_iter()
            .collect();
        effects.extend(self.finish_subagents(Subagent::Failed));
        let failure = Failure {
            kind: FailureKind::Other,
            title: "Codex ended the turn in a way Sikemux could not read".to_owned(),
            details: Some(error),
            resets_at: None,
        };
        effects.push(Effect::End(
            turn.number,
            TurnEnd::Failed {
                failure,
                response: Some(self.payload("end_turn")),
            },
        ));
        effects
    }

    /// The session updates a notification makes, in whichever session it is for.
    fn updates(&mut self, notification: &Notification) -> Vec<Value> {
        match notification {
            Notification::MessageDelta(delta) => {
                vec![self.items.message_delta(delta.item_id, delta.delta)]
            }
            Notification::ReasoningDelta(delta) => {
                vec![self.items.thought_delta(delta.item_id, delta.delta)]
            }
            Notification::ReasoningPartAdded(part) => {
                vec![self.items.thought_delta(part.item_id, "\n\n")]
            }
            Notification::ItemStarted(started) => {
                let head = &started.item;
                match head.kind {
                    "fileChange" => {
                        self.edits.insert(head.id.to_owned(), started.whole.clone());
                    }
                    "mcpToolCall" => {
                        let key = (
                            started.thread_id.unwrap_or_default().to_owned(),
                            head.server.unwrap_or_default().to_owned(),
                        );
                        self.tool_calls
                            .entry(key)
                            .or_default()
                            .push(head.id.to_owned());
                    }
                    _ => {}
                }
                self.items.started(started.whole)
            }
            Notification::ItemCompleted(completed) => {
                let head = &completed.item;
                self.edits.remove(head.id);
                let key = (
                    completed.thread_id.unwrap_or_default().to_owned(),
                    head.server.unwrap_or_default().to_owned(),
                );
                if let Some(calls) = self.tool_calls.get_mut(&key) {
                    calls.retain(|call| call != head.id);
                    if calls.is_empty() {
                        self.tool_calls.remove(&key);
                    }
                }
                self.items.completed(completed.whole)
            }
            Notification::TurnCompleted(completed) => {
                let thread = completed.thread_id.unwrap_or_default();
                self.tool_calls.retain(|(owner, _), _| owner != thread);
                Vec::new()
            }
            Notification::PlanUpdated(updated) => vec![items::plan(&updated.plan)],
            Notification::TokenUsage(usage) => {
                items::usage(&usage.token_usage).into_iter().collect()
            }
            Notification::ThreadNamed(named) => vec![json!({
                "sessionUpdate": UpdateKind::SessionInfoUpdate,
                "title": named.thread_name,
            })],
            Notification::ModelRerouted(rerouted) => vec![thought(
                &format!(
                    "Model rerouted from {} to {} ({}).\n\n",
                    rerouted.from_model, rerouted.to_model, rerouted.reason
                ),
                None,
            )],
            Notification::Compacted => vec![agent_text(
                "*Context compacted to fit the model's context window.*\n\n",
                None,
            )],
            Notification::Warning(warning) => vec![notice("warning", warning.message, None)],
            Notification::ConfigWarning(warning) => {
                vec![notice("warning", warning.summary, warning.details)]
            }
            _ => Vec::new(),
        }
    }

    /// How to answer one of Codex's requests.
    pub fn question(&mut self, method: &str, params: &Value) -> Reply {
        let thread = params["threadId"]
            .as_str()
            .unwrap_or(&self.thread_id)
            .to_owned();
        let session = self.subagents.session_of(&thread);
        let stale = params["turnId"]
            .as_str()
            .is_some_and(|turn| self.turns.is_stale(turn));
        match method {
            "item/commandExecution/requestApproval" => match approvals::command(&session, params) {
                Some(question) if !stale => Reply::Ask(question),
                _ => Reply::Answer(json!({ "decision": "cancel" })),
            },
            "item/fileChange/requestApproval" if stale => {
                Reply::Answer(json!({ "decision": "cancel" }))
            }
            "item/fileChange/requestApproval" => {
                let item = self.edits.get(text(params, "itemId"));
                Reply::Ask(approvals::file_change(&session, params, item))
            }
            "item/permissions/requestApproval" if stale => {
                Reply::Answer(approvals::refused_permissions())
            }
            "item/permissions/requestApproval" => {
                Reply::Ask(approvals::permissions(&session, params))
            }
            "item/tool/requestUserInput" if stale => Reply::Answer(json!({ "answers": {} })),
            "item/tool/requestUserInput" => Reply::PutQuestion,
            "mcpServer/elicitation/request" if stale => {
                Reply::Answer(approvals::cancelled_elicitation())
            }
            "mcpServer/elicitation/request" => {
                let server = text(params, "serverName").to_owned();
                let key = (thread, server.clone());
                let correlated = if approvals::is_tool_approval(params)
                    && self
                        .tool_calls
                        .get(&key)
                        .is_some_and(|calls| calls.len() == 1)
                {
                    self.tool_calls
                        .remove(&key)
                        .and_then(|mut calls| calls.pop())
                } else {
                    None
                };
                let questions = &mut self.questions;
                let standalone = || {
                    *questions += 1;
                    format!("elicitation:{session}:{server}:{questions}")
                };
                match approvals::elicitation(&session, params, correlated, standalone) {
                    Some(question) => Reply::Ask(question),
                    None => Reply::Answer(approvals::cancelled_elicitation()),
                }
            }
            _ => Reply::Refuse,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new("thread".into())
    }

    fn notify(session: &mut Session, method: &str, params: Value) -> Vec<Effect> {
        session.handle(method, &params)
    }

    fn updates(effects: &[Effect]) -> Vec<&Value> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Update(_, update) => Some(update),
                _ => None,
            })
            .collect()
    }

    fn ended(effects: &[Effect]) -> Option<(u64, &TurnEnd)> {
        effects.iter().find_map(|effect| match effect {
            Effect::End(turn, end) => Some((*turn, end)),
            _ => None,
        })
    }

    fn turn(id: &str, status: &str, error: Value) -> Value {
        json!({ "threadId": "thread", "turn": { "id": id, "items": [], "status": status, "error": error } })
    }

    #[test]
    fn a_turn_the_stream_dropped_ends_as_the_networks_failure() {
        let mut session = session();
        session.turns.titled = true;
        session.turns.begin(2, None, None, None);
        session.started(2, "turn-n");
        let error = json!({ "message": "stream disconnected before completion", "codexErrorInfo": { "responseStreamDisconnected": {} } });
        let effects = notify(
            &mut session,
            "turn/completed",
            turn("turn-n", "failed", error),
        );
        match ended(&effects) {
            Some((2, TurnEnd::Failed { failure, .. })) => {
                assert_eq!(failure.kind, FailureKind::Network);
                assert_eq!(
                    failure.details.as_deref(),
                    Some("stream disconnected before completion")
                );
            }
            _ => panic!("the turn should have failed for want of the network"),
        }
    }

    #[test]
    fn each_retry_codex_makes_is_a_warning_with_its_attempt() {
        let mut session = Session::new("thread".into());
        let retry = json!({
            "error": { "message": "Reconnecting... 1/5", "codexErrorInfo": { "responseStreamDisconnected": {} } },
            "willRetry": true, "threadId": "thread", "turnId": "turn-a",
        });
        notify(&mut session, "error", retry.clone());
        let effects = notify(&mut session, "error", retry);
        let [Effect::Update(thread, update)] = effects.as_slice() else {
            panic!("a retry should tell the chat");
        };
        assert_eq!(thread, "thread");
        let notice = &update["_meta"]["jetbrains"]["air"]["sessionFailure"];
        assert_eq!(notice["severity"], "warning");
        assert_eq!(notice["category"], "connection");
        assert_eq!(notice["attempt"], 2);
        assert_eq!(notice["title"], "Reconnecting... 1/5");
    }

    #[test]
    fn a_prompted_turn_ends_when_codex_completes_it() {
        let mut session = session();
        session
            .turns
            .begin(1, None, Some("msg-1".into()), Some("Fix it".into()));
        assert!(session.started(1, "turn-a").is_empty());
        assert_eq!(session.turns.message_turns["msg-1"], "turn-a");
        assert!(notify(
            &mut session,
            "turn/started",
            json!({ "threadId": "thread", "turn": { "id": "turn-a" } })
        )
        .is_empty());
        notify(
            &mut session,
            "thread/tokenUsage/updated",
            json!({
                "threadId": "thread", "turnId": "turn-a",
                "tokenUsage": { "last": { "totalTokens": 10, "inputTokens": 8, "cachedInputTokens": 2, "outputTokens": 2 },
                                "total": {}, "modelContextWindow": 1000 },
            }),
        );
        let effects = notify(
            &mut session,
            "turn/completed",
            turn("turn-a", "completed", Value::Null),
        );
        assert_eq!(
            updates(&effects)[0],
            &json!({ "sessionUpdate": "session_info_update", "title": "Fix it" })
        );
        match ended(&effects) {
            Some((1, TurnEnd::Completed(payload))) => {
                assert_eq!(payload["stopReason"], "end_turn");
                assert_eq!(payload["usage"]["inputTokens"], 6);
            }
            _ => panic!("the turn should have completed"),
        }
        assert_eq!(session.turns.active(), None);
    }

    #[test]
    fn a_usage_limit_says_when_it_resets() {
        let now = sikemux_process::clock::epoch_ms();
        let said = |at: &str| {
            resets_at(&format!("You've hit your usage limit. Upgrade to Plus to continue using Codex (https://chatgpt.com/explore/plus), or try again at {at}."))
        };
        let later = said("Oct 12th, 2027 2:46 AM").expect("a day ahead");
        assert!(later > now + 300 * 86_400_000);
        let today = said("11:59 PM").expect("a clock");
        assert!(today > now && today <= now + 86_400_000);
        assert_eq!(said("Oct 12th, 2025 2:46 AM"), None);
        assert_eq!(
            resets_at("You've hit your usage limit, or try again later."),
            None
        );
    }

    #[test]
    fn a_usage_limit_fails_the_turn_for_another_account() {
        let mut session = session();
        session.turns.titled = true;
        session.turns.begin(2, None, None, None);
        session.started(2, "turn-b");
        let error = json!({
            "message": "You've hit your usage limit. Upgrade to Plus to continue using Codex (https://chatgpt.com/explore/plus), or try again at Oct 12th, 2026 2:46 AM.",
            "codexErrorInfo": "usageLimitExceeded", "additionalDetails": null, "misalignment": null,
        });
        notify(
            &mut session,
            "thread/status/changed",
            json!({ "threadId": "thread", "status": { "type": "systemError" } }),
        );
        notify(
            &mut session,
            "error",
            json!({ "error": error, "willRetry": false, "threadId": "thread", "turnId": "turn-b" }),
        );
        let effects = notify(
            &mut session,
            "turn/completed",
            turn("turn-b", "failed", error.clone()),
        );
        match ended(&effects) {
            Some((2, TurnEnd::Failed { failure, response })) => {
                assert_eq!(failure.kind, FailureKind::Limit);
                assert!(failure.title.starts_with("You've hit your usage limit"));
                let response = response.as_ref().unwrap();
                assert_eq!(response["stopReason"], "end_turn");
            }
            _ => panic!("the turn should have failed"),
        }
    }

    #[test]
    fn failures_are_told_apart_by_their_cause() {
        let kind = |info: Value| failure(&json!({ "message": "m", "codexErrorInfo": info })).kind;
        assert_eq!(kind(json!("unauthorized")), FailureKind::SignIn);
        assert_eq!(
            kind(json!({ "responseStreamDisconnected": { "httpStatusCode": 401 } })),
            FailureKind::SignIn
        );
        assert_eq!(
            kind(json!({ "httpConnectionFailed": { "httpStatusCode": 429 } })),
            FailureKind::Limit
        );
        assert_eq!(kind(json!("rateLimitExceeded")), FailureKind::Limit);
        assert_eq!(kind(json!("contextWindowExceeded")), FailureKind::Other);
        assert_eq!(kind(json!("other")), FailureKind::Other);
        assert_eq!(
            kind(json!({ "responseStreamDisconnected": { "httpStatusCode": null } })),
            FailureKind::Network
        );
        assert_eq!(kind(Value::Null), FailureKind::Other);
    }

    #[test]
    fn a_failed_turn_without_an_error_notification_uses_the_turns_own() {
        let mut session = session();
        session.turns.titled = true;
        session.turns.begin(3, None, None, None);
        session.started(3, "turn-c");
        let effects = notify(
            &mut session,
            "turn/completed",
            turn(
                "turn-c",
                "failed",
                json!({ "message": "stream disconnected", "codexErrorInfo": "other" }),
            ),
        );
        match ended(&effects) {
            Some((3, TurnEnd::Failed { failure, .. })) => {
                assert_eq!(failure.kind, FailureKind::Other);
                assert_eq!(failure.title, "stream disconnected");
            }
            _ => panic!("the turn should have failed"),
        }
    }

    #[test]
    fn a_stop_interrupts_the_turn_once_codex_names_it() {
        let mut session = session();
        session.turns.titled = true;
        session.turns.begin(4, None, None, None);
        assert!(session.cancel().is_empty());
        let effects = session.started(4, "turn-d");
        assert!(matches!(effects.as_slice(), [Effect::Interrupt(id)] if id == "turn-d"));
        let effects = session.interrupted("turn-d");
        match ended(&effects) {
            Some((4, TurnEnd::Completed(payload))) => {
                assert_eq!(payload["stopReason"], "cancelled")
            }
            _ => panic!("the turn should have been cancelled"),
        }
        let late = notify(
            &mut session,
            "item/agentMessage/delta",
            json!({
                "threadId": "thread", "turnId": "turn-d", "itemId": "m", "delta": "late",
            }),
        );
        assert!(late.is_empty());
        assert!(notify(
            &mut session,
            "turn/completed",
            turn("turn-d", "interrupted", Value::Null)
        )
        .is_empty());
    }

    #[test]
    fn work_codex_starts_itself_is_its_own_turn() {
        let mut session = session();
        let effects = notify(
            &mut session,
            "turn/started",
            json!({ "threadId": "thread", "turn": { "id": "auto" } }),
        );
        assert!(matches!(effects.as_slice(), [Effect::WorkStarted]));
        assert_eq!(session.turns.active().as_deref(), Some("auto"));
        let effects = notify(
            &mut session,
            "turn/completed",
            turn("auto", "completed", Value::Null),
        );
        assert!(matches!(
            effects.as_slice(),
            [Effect::WorkEnded("end_turn")]
        ));
    }

    #[test]
    fn a_turn_ends_while_its_subagents_still_work() {
        let mut session = session();
        session.turns.titled = true;
        session.turns.begin(5, None, None, None);
        session.started(5, "turn-e");
        notify(
            &mut session,
            "item/completed",
            json!({ "threadId": "thread", "turnId": "turn-e", "item": {
            "type": "collabAgentToolCall", "id": "c", "tool": "spawnAgent", "status": "completed",
            "senderThreadId": "thread", "receiverThreadIds": ["kid"], "prompt": "Look",
            "agentsStates": {},
        } }),
        );
        let effects = notify(
            &mut session,
            "turn/completed",
            turn("turn-e", "completed", Value::Null),
        );
        assert!(matches!(ended(&effects), Some((5, TurnEnd::Completed(_)))));
        notify(
            &mut session,
            "item/started",
            json!({ "threadId": "thread", "item": {
            "type": "subAgentActivity", "id": "a", "kind": "started", "agentThreadId": "kid", "agentPath": "/root/kid",
        } }),
        );
        let effects = notify(
            &mut session,
            "turn/completed",
            json!({ "threadId": "kid", "turn": { "id": "k", "status": "completed" } }),
        );
        assert_eq!(
            updates(&effects)[0]["sessionUpdate"],
            "subagent_state_update"
        );
        assert!(ended(&effects).is_none());
    }

    #[test]
    fn approvals_find_the_edit_they_are_about() {
        let mut session = session();
        notify(
            &mut session,
            "item/started",
            json!({ "threadId": "thread", "turnId": "t", "item": {
            "type": "fileChange", "id": "edit-1", "status": "inProgress",
            "changes": [{ "path": "/nowhere/a.txt", "kind": { "type": "add" }, "diff": "x" }],
        } }),
        );
        let Reply::Ask(question) = session.question(
            "item/fileChange/requestApproval",
            &json!({ "threadId": "thread", "turnId": "t", "itemId": "edit-1" }),
        ) else {
            panic!("an edit should be asked about");
        };
        assert_eq!(
            question.request["toolCall"]["locations"],
            json!([{ "path": "/nowhere/a.txt" }])
        );
        assert!(matches!(
            session.question(
                "item/tool/requestUserInput",
                &json!({ "threadId": "thread" })
            ),
            Reply::PutQuestion
        ));
        assert!(matches!(
            session.question("item/tool/call", &json!({})),
            Reply::Refuse
        ));
    }

    #[test]
    fn a_tool_server_approval_lands_on_the_call_waiting_for_it() {
        let mut session = session();
        notify(
            &mut session,
            "item/started",
            json!({ "threadId": "thread", "item": {
            "type": "mcpToolCall", "id": "call_1", "server": "s", "tool": "t", "status": "inProgress",
            "arguments": {}, "result": null, "error": null,
        } }),
        );
        let Reply::Ask(question) = session.question(
            "mcpServer/elicitation/request",
            &json!({
                "threadId": "thread", "serverName": "s", "mode": "form", "message": "Run?",
                "requestedSchema": { "type": "object", "properties": {} },
                "_meta": { "codex_approval_kind": "mcp_tool_call" },
            }),
        ) else {
            panic!("a tool approval should be asked about");
        };
        assert_eq!(question.request["toolCall"]["toolCallId"], "call_1");
    }

    #[test]
    fn warnings_become_notices_and_failed_tool_servers_a_failed_call() {
        let mut session = session();
        session.starting_servers.insert("sikemux-tools".into());
        let effects = notify(
            &mut session,
            "warning",
            json!({ "threadId": "thread", "message": "Skills shortened" }),
        );
        assert_eq!(
            updates(&effects)[0],
            &json!({ "sessionUpdate": "notice", "severity": "warning", "title": "Skills shortened" })
        );
        let effects = notify(
            &mut session,
            "mcpServer/startupStatus/updated",
            json!({
                "threadId": "thread", "name": "sikemux-tools", "status": "failed", "error": "spawn failed",
            }),
        );
        let update = updates(&effects)[0];
        assert_eq!(update["title"], "mcp__sikemux-tools__startup");
        assert_eq!(update["status"], "failed");
        assert!(notify(
            &mut session,
            "mcpServer/startupStatus/updated",
            json!({
                "threadId": "thread", "name": "sikemux-tools", "status": "failed", "error": "again",
            })
        )
        .is_empty());
    }

    #[test]
    fn other_threads_do_not_move_the_chats_turns() {
        let mut session = session();
        let effects = notify(
            &mut session,
            "turn/started",
            json!({ "threadId": "elsewhere", "turn": { "id": "x" } }),
        );
        assert!(effects.is_empty());
        assert_eq!(session.turns.active(), None);
    }
}
