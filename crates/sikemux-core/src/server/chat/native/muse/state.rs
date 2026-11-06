//! What the chat knows of a Muse session while it runs: which turn answers
//! the prompt, which items are on screen, and its subagents. Each
//! notification comes back as the effects it has on the chat.

use sikemux_fold::update::{self, agent_text, notice};
use std::collections::HashSet;

use serde_json::{json, Value};

use crate::acp::failure::{Failure, FailureKind};

use super::super::super::states::{self, Outcome, Subagent};
use super::super::json::{text, text_or_empty};
use super::super::rpc_driver::{self, RpcSession};
use super::super::turns::{Ended, Started, Turns};
use super::items::{Follow, Items};
use super::notifications::{Notification, Stage, Todo, Totals, TurnCompleted, TurnUsage, Window};

pub(super) type Effect = rpc_driver::Effect<Follow>;

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
    update::TurnUsage {
        total: usage.input_tokens + usage.output_tokens,
        input: usage.input_tokens,
        cached: usage.cached_tokens,
        output: usage.output_tokens,
        thought: usage.reasoning_tokens,
    }
    .answer()
}

/// Muse's to-do list as the chat's plan.
fn plan(todos: &[Todo]) -> Value {
    update::plan(
        todos
            .iter()
            .map(|todo| update::plan_entry(&todo.text, &todo.status))
            .collect(),
    )
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

    fn turn_completed(&mut self, ended: &TurnCompleted) -> Vec<Effect> {
        let outcome = states::outcome(&ended.terminal).unwrap_or(Outcome::Failed);
        match self.turns.completed(&ended.turn_id) {
            Ended::Unprompted => self.end_unprompted(outcome),
            Ended::Prompted(turn) => {
                let mut payload = self.payload();
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
                    effects.push(Effect::Update(session, update::usage(used, size)));
                }
                self.context = Some(usage.window);
            }
            Notification::NameChanged(named) if root => {
                if let Some(name) = named.name {
                    self.turns.titled = true;
                    effects.push(Effect::Update(session, update::session_title(&name)));
                }
            }
            Notification::TodoListChanged(todos) => {
                effects.push(Effect::Update(session, plan(&todos.items)));
            }
            _ => {}
        }
        effects.extend(follow.into_iter().map(Effect::Own));
        effects
    }

    fn unreadable(&mut self, notification: Notification) -> Vec<Effect> {
        match notification {
            Notification::Unreadable { method, error } if method == "turn/completed" => {
                self.ended_unreadably(error)
            }
            _ => Vec::new(),
        }
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

impl Default for Session {
    fn default() -> Self {
        Self::new(String::new(), String::new())
    }
}

impl RpcSession for Session {
    type Own = Follow;
    const NAME: &'static str = "Muse";

    fn id(&self) -> &str {
        &self.session_id
    }

    fn turns(&mut self) -> &mut Turns {
        &mut self.turns
    }

    fn finish_subagents(&mut self, state: Subagent) -> Vec<Effect> {
        self.items
            .finish_outstanding(state)
            .into_iter()
            .map(|(session, update)| Effect::Update(session, update))
            .collect()
    }
}
