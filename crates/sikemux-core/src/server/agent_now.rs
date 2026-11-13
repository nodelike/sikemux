//! Whether an agent is at work, at rest or waiting on something, asked one way however it runs:
//! a chat here, a terminal here, or on a linked host.

use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
use std::sync::Arc;

use sikemux_wire::protocol::{AgentState, AgentStatus};

use super::agent::AgentActivity;
use super::chat::Chat;
use super::Core;

pub(crate) struct AgentNow<'a> {
    core: &'a Core,
    id: &'a str,
}

/// Reading what one agent is doing now.
pub(crate) trait AgentNowCore {
    fn agent_now<'a>(&'a self, id: &'a str) -> AgentNow<'a>;
}

impl AgentNowCore for Core {
    fn agent_now<'a>(&'a self, id: &'a str) -> AgentNow<'a> {
        AgentNow { core: self, id }
    }
}

impl AgentNow<'_> {
    fn chat(&self) -> Option<Arc<Chat>> {
        self.core.chats().get(self.id)
    }

    fn in_terminal<T>(&self, read: impl FnOnce(&AgentActivity) -> T) -> Option<T> {
        let session = self.core.agent_session(self.id)?;
        session.agent.as_ref().map(read)
    }

    /// The CLI it runs under, when it runs here.
    pub(crate) fn cli(&self) -> Option<String> {
        match self.chat() {
            Some(chat) => Some(chat.provider().to_owned()),
            None => self.core.agent_session(self.id)?.info().agent_type,
        }
    }

    /// It runs now, here or on a linked host.
    pub(crate) fn running(&self) -> bool {
        self.chat().is_some()
            || self.core.agent_session(self.id).is_some()
            || self.core.hosts().team.get(self.id).is_some()
    }

    /// It keeps its name and its cards: it runs here, the workspace holds it, or it runs on a
    /// linked host this one knows.
    pub(crate) fn open(&self) -> bool {
        self.chat().is_some()
            || self.core.agent_session(self.id).is_some()
            || self.core.workspaces().agent(self.id).is_some()
            || self.core.names().is_pinned(self.id)
    }

    /// In the middle of a turn, as far as its chat or a terminal the core can read says. Waiting
    /// on the person in the middle of one counts.
    pub(crate) fn mid_turn(&self) -> bool {
        match self.chat() {
            Some(chat) => chat.turn_running(),
            None => self.in_terminal(AgentActivity::busy).unwrap_or(false),
        }
    }

    /// Not known to be at rest: a terminal the core cannot read counts, and an agent on a linked
    /// host says for itself.
    pub(crate) fn maybe_mid_turn(&self) -> bool {
        if let Some(chat) = self.chat() {
            return chat.turn_running();
        }
        if let Some(foreign) = self.core.hosts().team.get(self.id) {
            return foreign.status == AgentStatus::Working;
        }
        self.in_terminal(|agent| !agent.idle()).unwrap_or(false)
    }

    /// At rest and ready for more.
    pub(crate) fn resting(&self) -> bool {
        match self.chat() {
            Some(chat) => chat.is_ready() && !chat.turn_running(),
            None => self.in_terminal(AgentActivity::idle).unwrap_or(false),
        }
    }

    /// What its chat is doing, for one with a chat here.
    pub(crate) fn chat_state(&self) -> Option<AgentState> {
        self.chat().map(|chat| chat.state_now())
    }

    /// A question it asked and a message on its way each bring it back by themselves.
    pub(crate) fn waits_on_something(&self) -> bool {
        self.core.harness().questions.waits_on_answer(self.id)
            || self.core.harness().mailbox.has_waiting(self.id)
    }

    /// Whether an agent it started, or one that agent started, other than `but`, is still at work
    /// or has messages on the way to it.
    pub(crate) fn workers_busy(&self, but: &str) -> bool {
        self.core.running_agents().iter().any(|agent| {
            agent != but
                && self.core.names().leads(self.id, agent)
                && (self.core.agent_now(agent).mid_turn()
                    || self.core.harness().mailbox.has_waiting(agent))
        })
    }

    /// Something of its is moving: its turn, a subagent, mail on the way or a worker.
    pub(crate) fn at_work(&self) -> bool {
        self.mid_turn()
            || self
                .chat()
                .is_some_and(|chat| chat.feed.running_subagents() > 0)
            || self.core.harness().mailbox.has_waiting(self.id)
            || self.workers_busy(self.id)
    }
}
