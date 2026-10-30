//! What the chat knows of a Muse session while it runs: which turn answers
//! the prompt, which items are on screen, and its subagents. Each
//! notification comes back as the effects it has on the chat.

use sikemux_fold::update::{agent_text, notice};
use sikemux_fold::UpdateKind;
use std::collections::HashSet;

use serde_json::{json, Value};

use crate::server::chat::failure::{Failure, FailureKind};

use super::super::super::states::{self, Outcome, Subagent};
use super::super::json::{text, text_or_empty};
use super::super::session::TurnEnd;
use super::super::turns::{self, Ended, Prompted, Started, Turns};
use super::items::{Follow, Items};
use super::notifications::{Notification, Stage, Todo, Totals, TurnCompleted, TurnUsage, Window};

/// What one notification does to the chat.
pub(super) enum Effect {
    /// A session update, for the session with this id.
    Update(String, Value),
    End(u64, TurnEnd),
    WorkStarted,
    WorkEnded(&'static str),
    Follow(Follow),
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Session {
    pub session_id: String,
    pub cwd: String,
    /// Muse begins a turn on its own too, such as after a background task ended.
    pub turns: Turns,
    items: Items,
    /// The latest running token totals.
    pub totals: Option<Totals>,
    /// How full the context window is.
    pub context: Option<Window>,
    /// The approval stages already put to the person.
    asked: HashSet<(String, String)>,
}

/// The failure a turn ended with, from Muse's error for it.
pub(super) fn failure(error: &Value) -> Failure {
    let kind = if !error["quota"].is_null() {
        FailureKind::Limit
    } else if text_or_empty(error, "kind") == "authRequired" {
        FailureKind::SignIn
    } else {
        FailureKind::Other
    };
    Failure {
        kind,
        title: text(error, "message").unwrap_or("Turn failed").to_owned(),
        details: None,
        resets_at: None,
    }
}

/// The tokens a turn used, as the chat reads a prompt's answer.
fn turn_usage(usage: &TurnUsage) -> Value {
    json!({
        "totalTokens": usage.input_tokens + usage.output_tokens,
        "inputTokens": usage.input_tokens.saturating_sub(usage.cached_tokens),
        "cachedReadTokens": usage.cached_tokens,
        "outputTokens": usage.output_tokens,
        "thoughtTokens": usage.reasoning_tokens,
    })
}

/// Muse's to-do list as the chat's plan.
fn plan(todos: &[Todo]) -> Value {
    let entries: Vec<Value> = todos
        .iter()
        .map(|todo| {
            let status = match todo.status.as_str() {
                "inProgress" => "in_progress",
                "completed" | "cancelled" => "completed",
                _ => "pending",
            };
            json!({ "content": todo.text, "status": status, "priority": "medium" })
        })
        .collect();
    json!({ "sessionUpdate": UpdateKind::Plan, "entries": entries })
}

impl Session {
    pub fn new(session_id: String, cwd: String) -> Self {
        Self {
            session_id,
            cwd,
            turns: Turns::default(),
            items: Items::default(),
            totals: None,
            context: None,
            asked: HashSet::new(),
        }
    }

    /// Muse took the interrupt of turn `id`. The turn is over for the chat
    /// now, and what Muse still says of it is dropped.
    pub fn interrupted(&mut self, id: &str) -> Vec<Effect> {
        match self.turns.interrupted(id) {
            Ended::Prompted(turn) => {
                let mut effects = self.finish_subagents(Subagent::Cancelled);
                effects.push(Effect::End(
                    turn.number,
                    TurnEnd::Completed(json!({ "stopReason": "cancelled" })),
                ));
                effects
            }
            Ended::Unprompted => {
                let mut effects = self.finish_subagents(Subagent::Cancelled);
                effects.push(Effect::WorkEnded("cancelled"));
                effects
            }
            Ended::Neither => Vec::new(),
        }
    }

    /// The conversation goes on in `session_id`, a copy of this one up to
    /// before turn `turn_id`.
    pub fn moved(&mut self, session_id: String, turn_id: &str) {
        self.session_id = session_id;
        self.turns.rewound(turn_id);
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

    fn turn_completed(&mut self, ended: &TurnCompleted) -> Vec<Effect> {
        let outcome = states::outcome(&ended.terminal).unwrap_or(Outcome::Failed);
        match self.turns.completed(&ended.turn_id) {
            Ended::Unprompted => {
                let (subagents, reason) = turns::unprompted_end(outcome);
                let mut effects = subagents
                    .map(|state| self.finish_subagents(state))
                    .unwrap_or_default();
                effects.push(Effect::WorkEnded(reason));
                effects
            }
            Ended::Prompted(turn) => {
                let mut payload = json!({ "stopReason": "end_turn" });
                if let Some(usage) = &ended.usage {
                    payload["usage"] = turn_usage(usage);
                }
                self.end_prompted(turn, outcome, payload, || {
                    let error = ended
                        .error
                        .clone()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| {
                            json!({
                                "kind": "hostDeath",
                                "message": ended.reason.as_deref().unwrap_or("Turn failed"),
                                "retryable": true,
                            })
                        });
                    failure(&error)
                })
            }
            Ended::Neither => Vec::new(),
        }
    }

    fn end_prompted(
        &mut self,
        mut turn: Prompted,
        outcome: Outcome,
        payload: Value,
        failure: impl FnOnce() -> Failure,
    ) -> Vec<Effect> {
        let mut effects: Vec<Effect> = self
            .turns
            .title(&mut turn)
            .map(|title| Effect::Update(self.session_id.clone(), title))
            .into_iter()
            .collect();
        let (subagents, end) = turns::prompted_end(outcome, payload, failure);
        if let Some(state) = subagents {
            effects.extend(self.finish_subagents(state));
        }
        effects.push(Effect::End(turn.number, end));
        effects
    }

    /// The effects of one notification from Muse.
    pub fn handle(&mut self, notification: Notification) -> Vec<Effect> {
        let Some(session) = notification.session_id().map(str::to_owned) else {
            return self.unreadable(notification);
        };
        let root = session == self.session_id;
        if !root && !self.items.is_child(&session) {
            return Vec::new();
        }
        let stale = |turn: Option<&str>| turn.is_some_and(|turn| root && self.turns.is_stale(turn));
        let mut effects = Vec::new();
        let mut follow = Vec::new();
        let cwd = self.cwd.clone();
        match notification {
            Notification::Item { stage, params } if !stale(params.turn_id()) => {
                if params.item["kind"] != "userMessage" {
                    let shown = match stage {
                        Stage::Opened => {
                            self.items.opened(&session, &params.item, &cwd, &mut follow)
                        }
                        Stage::Completed => {
                            self.items
                                .completed(&session, &params.item, &cwd, &mut follow)
                        }
                    };
                    effects.extend(
                        shown
                            .into_iter()
                            .map(|(session, update)| Effect::Update(session, update)),
                    );
                }
            }
            Notification::ItemDelta(delta) if !stale(delta.turn_id.as_deref()) => {
                if let Some(update) = self.items.delta(&session, &delta) {
                    effects.push(Effect::Update(session, update));
                }
            }
            Notification::TurnStarted(started) if root => {
                if let Started::Unprompted = self.turns.began(&started.turn_id) {
                    effects.push(Effect::WorkStarted);
                }
            }
            Notification::TurnCompleted(ended) if root => {
                effects.extend(self.turn_completed(&ended))
            }
            Notification::RetryScheduled(retry) if root && !stale(retry.turn_id.as_deref()) => {
                let said = format!(
                    "Retrying (attempt {} of {}): {}",
                    retry.next_attempt, retry.max_attempts, retry.reason
                );
                effects.push(Effect::Update(session, notice("warning", &said, None)));
            }
            Notification::ModelRouteUnserved(_) if root => {
                effects.push(Effect::Update(
                    session,
                    notice(
                        "warning",
                        "Muse could not reach the chosen model and used another",
                        None,
                    ),
                ));
            }
            Notification::TokenUsage(usage) if root => self.totals = Some(usage.cumulative),
            Notification::ContextUsage(usage) if root => {
                if let (Some(used), Some(size)) =
                    (usage.window.used_tokens, usage.window.window_tokens)
                {
                    effects.push(Effect::Update(
                        session,
                        json!({ "sessionUpdate": UpdateKind::UsageUpdate, "used": used, "size": size }),
                    ));
                }
                self.context = Some(usage.window);
            }
            Notification::NameChanged(named) if root => {
                if let Some(name) = named.name {
                    self.turns.titled = true;
                    effects.push(Effect::Update(
                        session,
                        json!({ "sessionUpdate": UpdateKind::SessionInfoUpdate, "title": name }),
                    ));
                }
            }
            Notification::TodoListChanged(todos) => {
                effects.push(Effect::Update(session, plan(&todos.items)));
            }
            _ => {}
        }
        effects.extend(follow.into_iter().map(Effect::Follow));
        effects
    }

    /// A turn whose end Muse sent unreadably ends for the chat as failed,
    /// rather than leaving it waiting on an end it cannot read.
    fn unreadable(&mut self, notification: Notification) -> Vec<Effect> {
        let Notification::Unreadable { method, error } = notification else {
            return Vec::new();
        };
        if method != "turn/completed" {
            return Vec::new();
        }
        let Some(turn) = self.turns.give_up() else {
            return Vec::new();
        };
        let payload = json!({ "stopReason": "end_turn" });
        self.end_prompted(turn, Outcome::Failed, payload, || Failure {
            kind: FailureKind::Other,
            title: "Muse ended the turn in a way Sikemux could not read".to_owned(),
            details: Some(error),
            resets_at: None,
        })
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
