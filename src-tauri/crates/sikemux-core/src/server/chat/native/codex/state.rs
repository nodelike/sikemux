//! What the chat knows of a Codex thread while it runs: which turn answers the
//! prompt, which items are on screen, and its subagents. Each notification
//! comes back as the effects it has on the chat.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::acp::account;

use super::super::session::TurnEnd;
use super::approvals::{self, Question};
use super::items::{self, Items};
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
    /// Turn `n` finished with subagents still at work; it is answered when
    /// they end, or after a while regardless.
    AwaitSubagents(u64),
}

/// How to answer one of Codex's requests.
pub(super) enum Reply {
    Ask(Question),
    Answer(Value),
    Refuse,
}

/// The turn this side prompted.
struct Turn {
    number: u64,
    /// Codex's id for it, once Codex has said.
    id: Option<String>,
    cancelled: bool,
    message_id: Option<String>,
    /// Shown as the chat's title when Codex has named the thread nothing.
    title: Option<String>,
}

pub(super) struct Session {
    pub thread_id: String,
    turn: Option<Turn>,
    /// A turn Codex began on its own.
    unprompted: Option<String>,
    /// The last turn that ended outside a prompt, should a prompt be told it.
    last_ended: Option<Value>,
    waiting: Option<(u64, Value)>,
    /// Turns this side ended already, whose late notifications are dropped.
    stale: HashSet<String>,
    errors: HashMap<String, Value>,
    /// The turn each of the app's message ids began.
    pub message_turns: HashMap<String, String>,
    items: Items,
    edits: HashMap<String, Value>,
    tool_calls: HashMap<(String, String), Vec<String>>,
    pub subagents: Subagents,
    pub token_usage: Option<Value>,
    pub titled: bool,
    /// The chat's own tool servers that have not reported starting yet.
    pub starting_servers: HashSet<String>,
    questions: u64,
    /// Set once Codex's output closed.
    pub gone: bool,
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn notice(title: &str, description: Option<&str>) -> Value {
    let mut update = json!({ "sessionUpdate": "notice", "severity": "warning", "title": title });
    if let Some(description) = description.filter(|description| !description.is_empty()) {
        update["description"] = json!(description);
    }
    update
}

/// The category and fixes Codex's adapter gives a turn error, by its cause.
fn failure_policy(info: &Value) -> (&'static str, &'static [&'static str]) {
    let status = info
        .as_object()
        .and_then(|info| info.values().next())
        .and_then(|details| details["httpStatusCode"].as_u64());
    if info == "unauthorized" || status == Some(401) {
        return ("access", &["login"]);
    }
    if status == Some(429) {
        return ("limit", &["retry"]);
    }
    if let Some(info) = info.as_str() {
        return match info {
            "usageLimitExceeded" => ("limit", &[]),
            "rateLimitExceeded" => ("limit", &["retry"]),
            "contextWindowExceeded" | "sessionBudgetExceeded" => ("limit", &["new_session"]),
            "serverOverloaded" => ("service", &["retry"]),
            "cyberPolicy" | "misalignmentPolicyViolation" | "badRequest" => ("request", &[]),
            "internalServerError" => ("service", &["retry", "new_session"]),
            _ => ("service", &["retry"]),
        };
    }
    let transport = info.as_object().is_some_and(|info| {
        [
            "httpConnectionFailed",
            "responseStreamConnectionFailed",
            "responseStreamDisconnected",
            "responseTooManyFailedAttempts",
        ]
        .iter()
        .any(|key| info.contains_key(*key))
    });
    if transport {
        ("connection", &["retry", "new_session"])
    } else {
        ("service", &["retry"])
    }
}

/// The `_meta` Codex's adapter answers a failed turn with.
pub(super) fn failure_meta(turn_id: &str, error: &Value) -> Value {
    let (category, actions) = failure_policy(&error["codexErrorInfo"]);
    let title = error["message"]
        .as_str()
        .filter(|message| !message.is_empty())
        .unwrap_or("Turn failed");
    json!({ "jetbrains": { "air": { "version": 1, "sessionFailure": {
        "id": format!("{turn_id}:error"),
        "revision": 1,
        "category": category,
        "severity": "error",
        "title": title,
        "actions": actions,
    } } } })
}

impl Session {
    pub fn new(thread_id: String) -> Self {
        Self {
            subagents: Subagents::new(thread_id.clone()),
            thread_id,
            turn: None,
            unprompted: None,
            last_ended: None,
            waiting: None,
            stale: HashSet::new(),
            errors: HashMap::new(),
            message_turns: HashMap::new(),
            items: Items::default(),
            edits: HashMap::new(),
            tool_calls: HashMap::new(),
            token_usage: None,
            titled: false,
            starting_servers: HashSet::new(),
            questions: 0,
            gone: false,
        }
    }

    /// Turn `number` is being sent.
    pub fn begin(&mut self, number: u64, message_id: Option<String>, title: Option<String>) {
        self.turn = Some(Turn {
            number,
            id: None,
            cancelled: false,
            message_id,
            title,
        });
    }

    /// Codex could not take turn `number`.
    pub fn abandon(&mut self, number: u64) {
        if self.turn.as_ref().is_some_and(|turn| turn.number == number) {
            self.turn = None;
        }
    }

    /// Codex answered turn `number` with its id for it.
    pub fn started(&mut self, number: u64, id: &str) -> Vec<Effect> {
        if self.turn.as_ref().is_none_or(|turn| turn.number != number) {
            return Vec::new();
        }
        let mut effects = self.adopt(id);
        if let Some(ended) = self.last_ended.take().filter(|turn| turn["id"] == id) {
            effects.extend(self.finish_prompted(&ended));
        }
        effects
    }

    fn adopt(&mut self, id: &str) -> Vec<Effect> {
        let Some(turn) = self.turn.as_mut().filter(|turn| turn.id.is_none()) else {
            return Vec::new();
        };
        turn.id = Some(id.to_owned());
        if let Some(message_id) = &turn.message_id {
            self.message_turns.insert(message_id.clone(), id.to_owned());
        }
        if self.unprompted.as_deref() == Some(id) {
            self.unprompted = None;
        }
        if turn.cancelled {
            vec![Effect::Interrupt(id.to_owned())]
        } else {
            Vec::new()
        }
    }

    /// The turn running now, prompted or not.
    pub fn active_turn(&self) -> Option<String> {
        self.turn
            .as_ref()
            .and_then(|turn| turn.id.clone())
            .or_else(|| self.unprompted.clone())
    }

    pub fn cancel(&mut self) -> Vec<Effect> {
        if let Some(turn) = &mut self.turn {
            turn.cancelled = true;
            return turn.id.clone().map(Effect::Interrupt).into_iter().collect();
        }
        if let Some((number, mut payload)) = self.waiting.take() {
            let mut effects = self.finish_subagents("cancelled");
            payload["stopReason"] = json!("cancelled");
            effects.push(Effect::End(number, TurnEnd::Completed(payload)));
            return effects;
        }
        self.unprompted
            .clone()
            .map(Effect::Interrupt)
            .into_iter()
            .collect()
    }

    /// Codex took the interrupt of turn `id`. The turn is over for the chat
    /// now, and what Codex still says of it is dropped.
    pub fn interrupted(&mut self, id: &str) -> Vec<Effect> {
        if self
            .turn
            .as_ref()
            .is_some_and(|turn| turn.id.as_deref() == Some(id))
        {
            self.stale.insert(id.to_owned());
            return self.finish_prompted(&json!({ "id": id, "status": "interrupted" }));
        }
        if self.unprompted.as_deref() == Some(id) {
            self.stale.insert(id.to_owned());
            self.unprompted = None;
            let mut effects = self.finish_subagents("cancelled");
            effects.push(Effect::WorkEnded("cancelled"));
            return effects;
        }
        Vec::new()
    }

    /// The wait on turn `number`'s subagents is over.
    pub fn subagents_waited(&mut self, number: u64) -> Vec<Effect> {
        if self.gone
            || self
                .waiting
                .as_ref()
                .is_none_or(|(waiting, _)| *waiting != number)
        {
            return Vec::new();
        }
        let mut effects = self.finish_subagents("failed");
        if let Some((number, payload)) = self.waiting.take() {
            effects.push(Effect::End(number, TurnEnd::Completed(payload)));
        }
        effects
    }

    /// The conversation went back to before turn `turn_id`.
    pub fn rewound(&mut self, turn_id: &str) {
        self.message_turns.retain(|_, turn| turn != turn_id);
    }

    fn finish_subagents(&mut self, state: &str) -> Vec<Effect> {
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

    fn finish_prompted(&mut self, ended: &Value) -> Vec<Effect> {
        let Some(turn) = self.turn.take() else {
            return Vec::new();
        };
        let id = text(ended, "id").to_owned();
        let error = self.errors.remove(&id);
        let mut effects = Vec::new();
        if !self.titled {
            if let Some(title) = turn.title {
                self.titled = true;
                effects.push(Effect::Update(
                    self.thread_id.clone(),
                    json!({ "sessionUpdate": "session_info_update", "title": title }),
                ));
            }
        }
        match text(ended, "status") {
            "completed" => {
                let payload = self.payload("end_turn");
                if self.subagents.outstanding() {
                    self.waiting = Some((turn.number, payload));
                    effects.push(Effect::AwaitSubagents(turn.number));
                } else {
                    effects.push(Effect::End(turn.number, TurnEnd::Completed(payload)));
                }
            }
            "interrupted" => {
                effects.extend(self.finish_subagents("cancelled"));
                effects.push(Effect::End(
                    turn.number,
                    TurnEnd::Completed(self.payload("cancelled")),
                ));
            }
            _ => {
                effects.extend(self.finish_subagents("failed"));
                let error = error
                    .or_else(|| Some(ended["error"].clone()).filter(|error| !error.is_null()))
                    .unwrap_or_else(|| json!({ "message": "Turn failed", "codexErrorInfo": null }));
                let meta = failure_meta(&id, &error);
                let mut response = self.payload("end_turn");
                response["_meta"] = meta.clone();
                if let Some(failure) = account::failure(&meta) {
                    effects.push(Effect::End(
                        turn.number,
                        TurnEnd::Failed {
                            failure,
                            response: Some(response),
                        },
                    ));
                }
            }
        }
        effects
    }

    fn turn_started(&mut self, id: &str) -> Vec<Effect> {
        if self.stale.contains(id) {
            return Vec::new();
        }
        match &self.turn {
            Some(turn) if turn.id.is_none() => self.adopt(id),
            Some(_) => Vec::new(),
            None => {
                self.unprompted = Some(id.to_owned());
                vec![Effect::WorkStarted]
            }
        }
    }

    fn turn_completed(&mut self, ended: &Value) -> Vec<Effect> {
        let id = text(ended, "id");
        if self.stale.remove(id) {
            return Vec::new();
        }
        if self.unprompted.as_deref() == Some(id) {
            self.unprompted = None;
            self.errors.remove(id);
            let status = text(ended, "status");
            let mut effects = Vec::new();
            if status != "completed" {
                let state = if status == "interrupted" {
                    "cancelled"
                } else {
                    "failed"
                };
                effects.extend(self.finish_subagents(state));
            }
            if self.turn.as_ref().is_some_and(|turn| turn.id.is_none()) {
                self.last_ended = Some(ended.clone());
            }
            effects.push(Effect::WorkEnded(if status == "interrupted" {
                "cancelled"
            } else {
                "end_turn"
            }));
            return effects;
        }
        let ours = self
            .turn
            .as_ref()
            .is_some_and(|turn| turn.id.as_deref().is_none_or(|own| own == id));
        if ours {
            let mut effects = self.adopt(id);
            effects.retain(|effect| !matches!(effect, Effect::Interrupt(_)));
            effects.extend(self.finish_prompted(ended));
            return effects;
        }
        Vec::new()
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
                    .is_some_and(|turn| self.stale.contains(turn));
                if root {
                    effects.extend(self.root_notification(method, params));
                }
                if !(root && stale) {
                    effects.extend(
                        self.updates(method, params)
                            .into_iter()
                            .map(|update| Effect::Update(session.clone(), update)),
                    );
                }
            }
        }
        if self.waiting.is_some() && !self.subagents.outstanding() {
            if let Some((number, payload)) = self.waiting.take() {
                effects.push(Effect::End(number, TurnEnd::Completed(payload)));
            }
        }
        effects
    }

    fn root_notification(&mut self, method: &str, params: &Value) -> Vec<Effect> {
        match method {
            "turn/started" => self.turn_started(text(&params["turn"], "id")),
            "turn/completed" => self.turn_completed(&params["turn"]),
            "error" if params["willRetry"] != true => {
                if let Some(turn) = params["turnId"].as_str() {
                    self.errors.insert(turn.to_owned(), params["error"].clone());
                }
                Vec::new()
            }
            "thread/tokenUsage/updated" => {
                self.token_usage = Some(params["tokenUsage"].clone());
                Vec::new()
            }
            "thread/name/updated" => {
                self.titled = true;
                Vec::new()
            }
            "mcpServer/startupStatus/updated" => {
                let name = text(params, "name");
                let status = text(params, "status");
                if !matches!(status, "ready" | "failed" | "cancelled")
                    || !self.starting_servers.remove(name)
                {
                    return Vec::new();
                }
                let said = match status {
                    "failed" => format!(
                        "[codex-acp forwarded startup error] MCP server `{name}` failed to start: {}",
                        params["error"].as_str().unwrap_or("unknown MCP startup error")
                    ),
                    "cancelled" => {
                        format!("[codex-acp forwarded startup error] MCP server `{name}` startup was cancelled.")
                    }
                    _ => return Vec::new(),
                };
                vec![Effect::Update(
                    self.thread_id.clone(),
                    json!({
                        "sessionUpdate": "tool_call",
                        "toolCallId": format!("mcp_startup.{name}"),
                        "kind": "other",
                        "title": format!("mcp__{name}__startup"),
                        "status": "failed",
                        "content": [items::content(json!({ "type": "text", "text": said }))],
                    }),
                )]
            }
            _ => Vec::new(),
        }
    }

    /// The session updates a notification makes, in whichever session it is for.
    fn updates(&mut self, method: &str, params: &Value) -> Vec<Value> {
        let item_id = text(params, "itemId");
        match method {
            "item/agentMessage/delta" => {
                vec![self.items.message_delta(item_id, text(params, "delta"))]
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                vec![self.items.thought_delta(item_id, text(params, "delta"))]
            }
            "item/reasoning/summaryPartAdded" => vec![self.items.thought_delta(item_id, "\n\n")],
            "item/started" => {
                let item = &params["item"];
                let id = text(item, "id").to_owned();
                match text(item, "type") {
                    "fileChange" => {
                        self.edits.insert(id, item.clone());
                    }
                    "mcpToolCall" => {
                        let key = (
                            text(params, "threadId").to_owned(),
                            text(item, "server").to_owned(),
                        );
                        self.tool_calls.entry(key).or_default().push(id);
                    }
                    _ => {}
                }
                self.items.started(item)
            }
            "item/completed" => {
                let item = &params["item"];
                let id = text(item, "id");
                self.edits.remove(id);
                let key = (
                    text(params, "threadId").to_owned(),
                    text(item, "server").to_owned(),
                );
                if let Some(calls) = self.tool_calls.get_mut(&key) {
                    calls.retain(|call| call != id);
                    if calls.is_empty() {
                        self.tool_calls.remove(&key);
                    }
                }
                self.items.completed(item)
            }
            "turn/completed" => {
                let thread = text(params, "threadId");
                self.tool_calls.retain(|(owner, _), _| owner != thread);
                Vec::new()
            }
            "turn/plan/updated" => vec![items::plan(params)],
            "thread/tokenUsage/updated" => items::usage(params).into_iter().collect(),
            "thread/name/updated" => vec![json!({
                "sessionUpdate": "session_info_update",
                "title": params["threadName"],
            })],
            "model/rerouted" => vec![items::thought(
                &format!(
                    "Model rerouted from {} to {} ({}).\n\n",
                    text(params, "fromModel"),
                    text(params, "toModel"),
                    text(params, "reason")
                ),
                None,
            )],
            "thread/compacted" => vec![items::agent_text(
                "*Context compacted to fit the model's context window.*\n\n",
                None,
                None,
            )],
            "warning" => vec![notice(text(params, "message"), None)],
            "configWarning" => vec![notice(text(params, "summary"), params["details"].as_str())],
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
            .is_some_and(|turn| self.stale.contains(turn));
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
            "item/tool/requestUserInput" => Reply::Answer(json!({ "answers": {} })),
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
            "execCommandApproval" => Reply::Ask(approvals::legacy_command(&session, params)),
            "applyPatchApproval" => Reply::Ask(approvals::legacy_patch(&session, params)),
            _ => Reply::Refuse,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::account::FailureKind;

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
    fn a_prompted_turn_ends_when_codex_completes_it() {
        let mut session = session();
        session.begin(1, Some("msg-1".into()), Some("Fix it".into()));
        assert!(session.started(1, "turn-a").is_empty());
        assert_eq!(session.message_turns["msg-1"], "turn-a");
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
        assert_eq!(session.active_turn(), None);
    }

    #[test]
    fn a_usage_limit_fails_the_turn_for_another_account() {
        let mut session = session();
        session.titled = true;
        session.begin(2, None, None);
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
                assert_eq!(
                    response.pointer("/_meta/jetbrains/air/sessionFailure/id"),
                    Some(&json!("turn-b:error"))
                );
            }
            _ => panic!("the turn should have failed"),
        }
    }

    #[test]
    fn failures_are_told_apart_by_their_cause() {
        let kind = |info: Value| {
            account::failure(&failure_meta(
                "t",
                &json!({ "message": "m", "codexErrorInfo": info }),
            ))
            .unwrap()
            .kind
        };
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
            FailureKind::Other
        );
        assert_eq!(kind(Value::Null), FailureKind::Other);
    }

    #[test]
    fn a_failed_turn_without_an_error_notification_uses_the_turns_own() {
        let mut session = session();
        session.titled = true;
        session.begin(3, None, None);
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
        session.titled = true;
        session.begin(4, None, None);
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
        assert_eq!(session.active_turn().as_deref(), Some("auto"));
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
    fn a_turn_ends_only_once_its_subagents_do() {
        let mut session = session();
        session.titled = true;
        session.begin(5, None, None);
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
        assert!(matches!(effects.as_slice(), [Effect::AwaitSubagents(5)]));
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
        assert!(matches!(ended(&effects), Some((5, TurnEnd::Completed(_)))));
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
            session.question("item/tool/requestUserInput", &json!({ "threadId": "thread" })),
            Reply::Answer(answer) if answer == json!({ "answers": {} })
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
        assert_eq!(session.active_turn(), None);
    }
}
