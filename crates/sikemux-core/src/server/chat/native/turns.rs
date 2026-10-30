//! The turns of an agent that names them itself, as Codex and Muse do: the
//! one this side prompted, one the agent began on its own, the ones this
//! side ended already, and the turn each of the app's messages began.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::UpdateKind;

use crate::server::chat::failure::Failure;

use super::super::states::{Outcome, Subagent};
use super::session::TurnEnd;

/// The turn this side prompted.
#[derive(Serialize, Deserialize)]
pub(super) struct Prompted {
    pub number: u64,
    /// The agent's id for it, once known.
    id: Option<String>,
    cancelled: bool,
    message_id: Option<String>,
    /// Shown as the chat's title when the agent has named the session nothing.
    title: Option<String>,
}

/// What became of a turn the agent said ended.
pub(super) enum Ended {
    Prompted(Prompted),
    Unprompted,
    /// Not a turn the chat follows, or one it ended already.
    Neither,
}

/// What a turn the agent said it started is to the chat.
pub(super) enum Started {
    /// The prompted turn, which the person stopped before its id was known
    /// when `cancelled`.
    Prompted {
        cancelled: bool,
    },
    Unprompted,
    Neither,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Turns {
    prompted: Option<Prompted>,
    unprompted: Option<String>,
    /// Turns this side ended already, whose late notifications are dropped.
    stale: HashSet<String>,
    /// The turn each of the app's message ids began.
    pub message_turns: HashMap<String, String>,
    /// The session's turns in order, for going back to before one.
    order: Vec<String>,
    pub titled: bool,
}

impl Turns {
    /// Turn `number` is being sent, under `id` when this side names it.
    pub fn begin(
        &mut self,
        number: u64,
        id: Option<&str>,
        message_id: Option<String>,
        title: Option<String>,
    ) {
        self.prompted = Some(Prompted {
            number,
            id: None,
            cancelled: false,
            message_id,
            title,
        });
        if let Some(id) = id {
            self.adopt(id);
        }
    }

    /// The agent could not take turn `number`.
    pub fn abandon(&mut self, number: u64) {
        if self
            .prompted
            .as_ref()
            .is_none_or(|turn| turn.number != number)
        {
            return;
        }
        if let Some(id) = self.prompted.take().and_then(|turn| turn.id) {
            self.order.retain(|known| *known != id);
            self.message_turns.retain(|_, known| *known != id);
        }
    }

    /// The agent answered turn `number` with its id for it. Answers whether
    /// the person already stopped it, or None when it is not the turn running.
    pub fn started(&mut self, number: u64, id: &str) -> Option<bool> {
        let cancelled = self
            .prompted
            .as_ref()
            .filter(|turn| turn.number == number)?
            .cancelled;
        self.adopt(id);
        Some(cancelled)
    }

    /// The prompted turn is known by `id` from now on.
    fn adopt(&mut self, id: &str) {
        let Some(turn) = self.prompted.as_mut() else {
            return;
        };
        if let Some(sent) = turn.id.replace(id.to_owned()).filter(|sent| sent != id) {
            self.order.retain(|known| *known != sent);
        }
        if let Some(message_id) = &turn.message_id {
            self.message_turns.insert(message_id.clone(), id.to_owned());
        }
        if !self.order.iter().any(|known| known == id) {
            self.order.push(id.to_owned());
        }
        if self.unprompted.as_deref() == Some(id) {
            self.unprompted = None;
        }
    }

    /// The agent said turn `id` began.
    pub fn began(&mut self, id: &str) -> Started {
        if self.stale.contains(id) {
            return Started::Neither;
        }
        match &self.prompted {
            Some(turn) if turn.id.is_none() => {
                let cancelled = turn.cancelled;
                self.adopt(id);
                Started::Prompted { cancelled }
            }
            Some(_) => {
                if !self.order.iter().any(|known| known == id) {
                    self.order.push(id.to_owned());
                }
                Started::Neither
            }
            None => {
                if !self.order.iter().any(|known| known == id) {
                    self.order.push(id.to_owned());
                }
                self.unprompted = Some(id.to_owned());
                Started::Unprompted
            }
        }
    }

    /// Whether this side prompted a turn the agent has not named yet.
    pub fn unnamed(&self) -> bool {
        self.prompted.as_ref().is_some_and(|turn| turn.id.is_none())
    }

    /// The turn running now, prompted or not.
    pub fn active(&self) -> Option<String> {
        self.prompted
            .as_ref()
            .and_then(|turn| turn.id.clone())
            .or_else(|| self.unprompted.clone())
    }

    /// The person asked to stop. Answers the turn to interrupt, when it is
    /// known yet.
    pub fn cancel(&mut self) -> Option<String> {
        if let Some(turn) = &mut self.prompted {
            turn.cancelled = true;
            return turn.id.clone();
        }
        self.unprompted.clone()
    }

    /// The agent took the interrupt of turn `id`. The turn is over for the
    /// chat now, and what the agent still says of it is dropped.
    pub fn interrupted(&mut self, id: &str) -> Ended {
        if self
            .prompted
            .as_ref()
            .is_some_and(|turn| turn.id.as_deref() == Some(id))
        {
            self.stale.insert(id.to_owned());
            return self.prompted.take().map_or(Ended::Neither, Ended::Prompted);
        }
        if self.unprompted.as_deref() == Some(id) {
            self.stale.insert(id.to_owned());
            self.unprompted = None;
            return Ended::Unprompted;
        }
        Ended::Neither
    }

    /// The agent said turn `id` ended.
    pub fn completed(&mut self, id: &str) -> Ended {
        if self.stale.remove(id) {
            return Ended::Neither;
        }
        if self.unprompted.as_deref() == Some(id) {
            self.unprompted = None;
            return Ended::Unprompted;
        }
        let ours = self
            .prompted
            .as_ref()
            .is_some_and(|turn| turn.id.as_deref().is_none_or(|own| own == id));
        if !ours {
            return Ended::Neither;
        }
        self.adopt(id);
        self.prompted.take().map_or(Ended::Neither, Ended::Prompted)
    }

    /// The prompted turn ends now whatever the agent says of it later.
    pub fn give_up(&mut self) -> Option<Prompted> {
        let turn = self.prompted.take()?;
        if let Some(id) = &turn.id {
            self.stale.insert(id.clone());
        }
        Some(turn)
    }

    /// Whether what the agent says of turn `id` comes too late to show.
    pub fn is_stale(&self, id: &str) -> bool {
        self.stale.contains(id)
    }

    /// The session's turns are `order`, as its history lists them.
    pub fn restore(&mut self, order: Vec<String>) {
        self.order = order;
    }

    /// Where turn `id` stands: the turn before it, or None for the first.
    /// Err when the chat no longer has it.
    pub fn before(&self, id: &str) -> Result<Option<String>, ()> {
        let at = self.order.iter().position(|turn| turn == id).ok_or(())?;
        Ok(at.checked_sub(1).map(|at| self.order[at].clone()))
    }

    /// The conversation went back to before turn `id`, which goes with every
    /// turn after it.
    pub fn rewound(&mut self, id: &str) {
        let dropped: HashSet<String> = match self.order.iter().position(|turn| turn == id) {
            Some(at) => self.order.drain(at..).collect(),
            None => HashSet::from([id.to_owned()]),
        };
        self.message_turns.retain(|_, turn| !dropped.contains(turn));
    }

    /// The update that names the session after `turn`'s prompt, when the
    /// agent has named it nothing.
    pub fn title(&mut self, turn: &mut Prompted) -> Option<Value> {
        let title = turn.title.take().filter(|_| !self.titled)?;
        self.titled = true;
        Some(json!({ "sessionUpdate": UpdateKind::SessionInfoUpdate, "title": title }))
    }
}

/// How a prompted turn that ended as `outcome` ends for the chat, and what
/// its subagents still at work end as. `payload` is the turn's answer.
pub(super) fn prompted_end(
    outcome: Outcome,
    mut payload: Value,
    failure: impl FnOnce() -> Failure,
) -> (Option<Subagent>, TurnEnd) {
    match outcome {
        Outcome::Completed => (None, TurnEnd::Completed(payload)),
        Outcome::Stopped => {
            payload["stopReason"] = json!("cancelled");
            (Some(Subagent::Cancelled), TurnEnd::Completed(payload))
        }
        Outcome::Failed => (
            Some(Subagent::Failed),
            TurnEnd::Failed {
                failure: failure(),
                response: Some(payload),
            },
        ),
    }
}

/// What a turn the agent began on its own ending as `outcome` ends its
/// subagents still at work as, and why the chat's work stopped.
pub(super) fn unprompted_end(outcome: Outcome) -> (Option<Subagent>, &'static str) {
    match outcome {
        Outcome::Completed => (None, "end_turn"),
        Outcome::Stopped => (Some(Subagent::Cancelled), "cancelled"),
        Outcome::Failed => (Some(Subagent::Failed), "end_turn"),
    }
}
