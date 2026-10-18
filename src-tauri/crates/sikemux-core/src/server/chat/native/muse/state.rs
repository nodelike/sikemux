//! What the chat knows of a Muse session while it runs: which turn answers
//! the prompt, which items are on screen, and its subagents. Each
//! notification comes back as the effects it has on the chat.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::acp::account;

use super::super::super::states::Subagent;
use super::super::json::{text, text_or_empty};
use super::super::session::TurnEnd;
use super::items::{agent_text, Follow, Items};

/// What one notification does to the chat.
pub(super) enum Effect {
    /// A session update, for the session with this id.
    Update(String, Value),
    End(u64, TurnEnd),
    WorkStarted,
    WorkEnded(&'static str),
    Follow(Follow),
}

/// The turn this side prompted.
#[derive(serde::Serialize, serde::Deserialize)]
struct Turn {
    number: u64,
    id: String,
    cancelled: bool,
    /// Shown as the chat's title when Muse has named the session nothing.
    title: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Session {
    pub session_id: String,
    pub cwd: String,
    turn: Option<Turn>,
    /// A turn Muse began on its own, such as after a background task ended.
    unprompted: Option<String>,
    /// Turns this side ended already, whose late notifications are dropped.
    stale: HashSet<String>,
    /// The turn each of the app's message ids began.
    pub message_turns: HashMap<String, String>,
    /// The session's turns in order, for going back to before one.
    pub turns: Vec<String>,
    items: Items,
    /// The latest running token totals.
    pub totals: Option<Value>,
    /// How full the context window is.
    pub context: Option<Value>,
    pub titled: bool,
    /// The approval stages already put to the person.
    asked: HashSet<(String, String)>,
}

/// The category and fixes a failed turn is shown with, by its cause.
fn failure_policy(error: &Value) -> (&'static str, &'static [&'static str]) {
    let retryable = error["retryable"] == true;
    if !error["quota"].is_null() {
        return ("limit", &[]);
    }
    match text_or_empty(error, "kind") {
        "authRequired" => ("access", &["login"]),
        "stepLimit" => ("limit", &["new_session"]),
        "configError" | "launchError" | "mcpLinkError" => ("request", &[]),
        _ if retryable => ("service", &["retry"]),
        _ => ("service", &[]),
    }
}

/// The failure a turn ended with, in the AIR shape the chat reads.
pub(super) fn failure_meta(turn_id: &str, error: &Value) -> Value {
    let (category, actions) = failure_policy(error);
    let title = text(error, "message").unwrap_or("Turn failed");
    json!({ "jetbrains": { "air": { "version": 1, "sessionFailure": {
        "id": format!("{turn_id}:error"),
        "revision": 1,
        "category": category,
        "severity": "error",
        "title": title,
        "actions": actions,
    } } } })
}

/// The tokens a turn used, as the chat reads a prompt's answer.
fn turn_usage(usage: &Value) -> Option<Value> {
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    usage.is_object().then(|| {
        json!({
            "totalTokens": count("inputTokens") + count("outputTokens"),
            "inputTokens": count("inputTokens").saturating_sub(count("cachedTokens")),
            "cachedReadTokens": count("cachedTokens"),
            "outputTokens": count("outputTokens"),
            "thoughtTokens": count("reasoningTokens"),
        })
    })
}

fn notice(title: &str) -> Value {
    json!({ "sessionUpdate": "notice", "severity": "warning", "title": title })
}

/// Muse's to-do list as the chat's plan.
fn plan(params: &Value) -> Value {
    let entries: Vec<Value> = params["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|todo| {
            let status = match text_or_empty(todo, "status") {
                "inProgress" => "in_progress",
                "completed" | "cancelled" => "completed",
                _ => "pending",
            };
            json!({ "content": todo["text"], "status": status, "priority": "medium" })
        })
        .collect();
    json!({ "sessionUpdate": "plan", "entries": entries })
}

impl Session {
    pub fn new(session_id: String, cwd: String) -> Self {
        Self {
            session_id,
            cwd,
            turn: None,
            unprompted: None,
            stale: HashSet::new(),
            message_turns: HashMap::new(),
            turns: Vec::new(),
            items: Items::default(),
            totals: None,
            context: None,
            titled: false,
            asked: HashSet::new(),
        }
    }

    /// Turn `number` is being sent under `id`, the command id Muse names a
    /// fresh turn by.
    pub fn begin(
        &mut self,
        number: u64,
        id: &str,
        message_id: Option<String>,
        title: Option<String>,
    ) {
        self.turn = Some(Turn {
            number,
            id: id.to_owned(),
            cancelled: false,
            title,
        });
        self.adopt(id, message_id.as_deref());
    }

    fn adopt(&mut self, id: &str, message_id: Option<&str>) {
        if let Some(message_id) = message_id {
            self.message_turns
                .insert(message_id.to_owned(), id.to_owned());
        }
        if !self.turns.iter().any(|known| known == id) {
            self.turns.push(id.to_owned());
        }
    }

    /// Muse could not take turn `number`.
    pub fn abandon(&mut self, number: u64) {
        if self.turn.as_ref().is_some_and(|turn| turn.number == number) {
            if let Some(turn) = self.turn.take() {
                self.turns.retain(|known| *known != turn.id);
                self.message_turns.retain(|_, known| *known != turn.id);
            }
        }
    }

    /// Muse put turn `number` into the turn with this id, when that is not
    /// the one it was sent as. Answers whether the person already stopped it.
    pub fn started(&mut self, number: u64, id: &str) -> bool {
        let Some(turn) = self.turn.as_mut().filter(|turn| turn.number == number) else {
            return false;
        };
        let sent = std::mem::replace(&mut turn.id, id.to_owned());
        let cancelled = turn.cancelled;
        if sent != id {
            let message_id = self
                .message_turns
                .iter()
                .find(|(_, turn)| **turn == sent)
                .map(|(message, _)| message.clone());
            self.turns.retain(|known| *known != sent);
            self.adopt(id, message_id.as_deref());
        }
        if self.unprompted.as_deref() == Some(id) {
            self.unprompted = None;
        }
        cancelled
    }

    /// The turn running now, prompted or not.
    pub fn active_turn(&self) -> Option<String> {
        self.turn
            .as_ref()
            .map(|turn| turn.id.clone())
            .or_else(|| self.unprompted.clone())
    }

    /// The person asked to stop. Answers the turn to interrupt.
    pub fn cancel(&mut self) -> Option<String> {
        if let Some(turn) = &mut self.turn {
            turn.cancelled = true;
            return Some(turn.id.clone());
        }
        self.unprompted.clone()
    }

    /// Muse took the interrupt of turn `id`. The turn is over for the chat
    /// now, and what Muse still says of it is dropped.
    pub fn interrupted(&mut self, id: &str) -> Vec<Effect> {
        let ours = self.turn.as_ref().is_some_and(|turn| turn.id == id);
        if !ours && self.unprompted.as_deref() != Some(id) {
            return Vec::new();
        }
        self.stale.insert(id.to_owned());
        let mut effects = self.finish_subagents(Subagent::Cancelled);
        if ours {
            let turn = self.turn.take();
            effects.extend(turn.map(|turn| {
                Effect::End(
                    turn.number,
                    TurnEnd::Completed(json!({ "stopReason": "cancelled" })),
                )
            }));
        } else {
            self.unprompted = None;
            effects.push(Effect::WorkEnded("cancelled"));
        }
        effects
    }

    /// The conversation goes on in `session_id`, a copy of this one up to
    /// before turn `turn_id`.
    pub fn moved(&mut self, session_id: String, turn_id: &str) {
        self.session_id = session_id;
        if let Some(at) = self.turns.iter().position(|turn| turn == turn_id) {
            let dropped: HashSet<String> = self.turns.drain(at..).collect();
            self.message_turns.retain(|_, turn| !dropped.contains(turn));
        }
    }

    /// Whether this stage of an approval is new to the person.
    pub fn ask(&mut self, key: (String, String)) -> bool {
        self.asked.insert(key)
    }

    /// The chat session an approval belongs in: the subagent's that asked,
    /// when the chat has a card for it.
    pub fn session_for(&self, params: &Value) -> String {
        params["subagentOrigin"]["childSessionId"]
            .as_str()
            .filter(|child| self.items.is_child(child))
            .unwrap_or(&self.session_id)
            .to_owned()
    }

    fn finish_subagents(&mut self, state: Subagent) -> Vec<Effect> {
        self.items
            .finish_outstanding(state)
            .into_iter()
            .map(|(session, update)| Effect::Update(session, update))
            .collect()
    }

    fn turn_started(&mut self, id: &str) -> Vec<Effect> {
        if self.stale.contains(id) {
            return Vec::new();
        }
        if !self.turns.iter().any(|known| known == id) {
            self.turns.push(id.to_owned());
        }
        if self.turn.is_some() || self.unprompted.is_some() {
            return Vec::new();
        }
        self.unprompted = Some(id.to_owned());
        vec![Effect::WorkStarted]
    }

    fn turn_completed(&mut self, params: &Value) -> Vec<Effect> {
        let id = text_or_empty(params, "turnId");
        if self.stale.remove(id) {
            return Vec::new();
        }
        let terminal = text_or_empty(params, "terminal");
        if self.unprompted.as_deref() == Some(id) {
            self.unprompted = None;
            let mut effects = Vec::new();
            if terminal != "completed" {
                effects.extend(self.finish_subagents(if terminal == "cancelled" {
                    Subagent::Cancelled
                } else {
                    Subagent::Failed
                }));
            }
            effects.push(Effect::WorkEnded(if terminal == "cancelled" {
                "cancelled"
            } else {
                "end_turn"
            }));
            return effects;
        }
        if self.turn.as_ref().is_none_or(|turn| turn.id != id) {
            return Vec::new();
        }
        let Some(turn) = self.turn.take() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        if !self.titled {
            if let Some(title) = turn.title {
                self.titled = true;
                effects.push(Effect::Update(
                    self.session_id.clone(),
                    json!({ "sessionUpdate": "session_info_update", "title": title }),
                ));
            }
        }
        let mut payload = json!({ "stopReason": "end_turn" });
        if let Some(usage) = turn_usage(&params["usage"]) {
            payload["usage"] = usage;
        }
        match terminal {
            "completed" => {
                effects.push(Effect::End(turn.number, TurnEnd::Completed(payload)));
            }
            "cancelled" => {
                effects.extend(self.finish_subagents(Subagent::Cancelled));
                payload["stopReason"] = json!("cancelled");
                effects.push(Effect::End(turn.number, TurnEnd::Completed(payload)));
            }
            _ => {
                effects.extend(self.finish_subagents(Subagent::Failed));
                let error = Some(params["error"].clone())
                    .filter(|error| error.is_object())
                    .unwrap_or_else(|| {
                        json!({
                            "kind": "hostDeath",
                            "message": text(params, "reason").unwrap_or("Turn failed"),
                            "retryable": true,
                        })
                    });
                let meta = failure_meta(id, &error);
                payload["_meta"] = meta.clone();
                if let Some(failure) = account::failure(&meta) {
                    effects.push(Effect::End(
                        turn.number,
                        TurnEnd::Failed {
                            failure,
                            response: Some(payload),
                        },
                    ));
                }
            }
        }
        effects
    }

    /// The effects of one notification from Muse.
    pub fn handle(&mut self, method: &str, params: &Value) -> Vec<Effect> {
        let session = text_or_empty(params, "sessionId").to_owned();
        let root = session == self.session_id;
        if !root && !self.items.is_child(&session) {
            return Vec::new();
        }
        let stale = params["turnId"]
            .as_str()
            .or_else(|| params["item"]["turnId"].as_str())
            .is_some_and(|turn| root && self.stale.contains(turn));
        let mut effects = Vec::new();
        let mut follow = Vec::new();
        let cwd = self.cwd.clone();
        match method {
            "item/started" | "item/updated" if !stale => {
                let item = &params["item"];
                if item["kind"] != "userMessage" {
                    effects.extend(
                        self.items
                            .opened(&session, item, &cwd, &mut follow)
                            .into_iter()
                            .map(|(session, update)| Effect::Update(session, update)),
                    );
                }
            }
            "item/completed" if !stale => {
                let item = &params["item"];
                if item["kind"] != "userMessage" {
                    effects.extend(
                        self.items
                            .completed(&session, item, &cwd, &mut follow)
                            .into_iter()
                            .map(|(session, update)| Effect::Update(session, update)),
                    );
                }
            }
            "item/delta" if !stale => {
                if let Some(update) = self.items.delta(&session, params) {
                    effects.push(Effect::Update(session.clone(), update));
                }
            }
            "turn/started" if root => {
                effects.extend(self.turn_started(text_or_empty(params, "turnId")))
            }
            "turn/completed" if root => effects.extend(self.turn_completed(params)),
            "turn/retryScheduled" if root && !stale => {
                effects.push(Effect::Update(
                    session.clone(),
                    notice(&format!(
                        "Retrying (attempt {} of {}): {}",
                        params["nextAttempt"],
                        params["maxAttempts"],
                        text_or_empty(params, "reason")
                    )),
                ));
            }
            "session/modelRouteUnserved" if root => {
                effects.push(Effect::Update(
                    session.clone(),
                    notice("Muse could not reach the chosen model and used another"),
                ));
            }
            "session/tokenUsage" if root => {
                self.totals = Some(params["cumulative"].clone());
            }
            "session/contextUsage" if root => {
                self.context = Some(params.clone());
                if let (Some(used), Some(size)) = (
                    params["usedTokens"].as_u64(),
                    params["windowTokens"].as_u64(),
                ) {
                    effects.push(Effect::Update(
                        session.clone(),
                        json!({ "sessionUpdate": "usage_update", "used": used, "size": size }),
                    ));
                }
            }
            "session/nameChanged" if root => {
                if let Some(name) = text(params, "name") {
                    self.titled = true;
                    effects.push(Effect::Update(
                        session.clone(),
                        json!({ "sessionUpdate": "session_info_update", "title": name }),
                    ));
                }
            }
            "session/todoListChanged" => {
                effects.push(Effect::Update(session.clone(), plan(params)));
            }
            _ => {}
        }
        effects.extend(follow.into_iter().map(Effect::Follow));
        effects
    }

    /// Tells a subagent's card its conversation, read after it ended.
    pub fn told_subagent(&mut self, child: &str, items: &[Value]) -> Vec<Effect> {
        let cwd = self.cwd.clone();
        let mut effects = Vec::new();
        let mut follow = Vec::new();
        for item in items {
            if item["kind"] == "userMessage" {
                continue;
            }
            effects.extend(
                self.items
                    .completed(child, item, &cwd, &mut follow)
                    .into_iter()
                    .map(|(session, update)| Effect::Update(session, update)),
            );
        }
        effects
    }

    pub fn subagent_said(&mut self, child: &str, result: &str) -> Vec<Effect> {
        self.items.child_spoke(child);
        vec![Effect::Update(child.to_owned(), agent_text(result, None))]
    }
}
