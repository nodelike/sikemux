//! Which agents finished or asked for something while the person was looking
//! elsewhere, and which agents some client has on screen. The app says which
//! agent it shows, and a phone shows the ones it has open while it is in front;
//! the rest are left unread until one shows them. It also keeps when each agent
//! last did something, so lists can put the most recent first and idle agents
//! nobody looks at can be put to sleep.

use crate::server::workspace_state::WorkspaceParts;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::changed::ChangedCore;
use super::connection::ClientId;

#[derive(Default)]
struct View {
    agents: HashSet<String>,
    /// Whether the person is looking, not only whether it is drawn: an app
    /// window in the background still shows its agents but reads none of them.
    looking: bool,
}

#[derive(Default)]
struct State {
    unread: HashSet<String>,
    active_at: HashMap<String, u64>,
    started_at: HashMap<String, u64>,
    views: HashMap<ClientId, View>,
    /// When each agent was last on some client's screen.
    viewed_at: HashMap<String, u64>,
    called_at: HashMap<String, u64>,
    tool_at: HashMap<String, u64>,
}

impl State {
    fn viewed(&self, agent_id: &str) -> bool {
        self.views
            .values()
            .any(|view| view.agents.contains(agent_id))
    }

    fn looked_at(&self, agent_id: &str) -> bool {
        self.views
            .values()
            .any(|view| view.looking && view.agents.contains(agent_id))
    }

    /// Shows `client`'s agents, answering the agents no client had on screen before.
    fn show(&mut self, client: ClientId, view: View) -> Vec<String> {
        let now = epoch_ms();
        let before = self.views.remove(&client).unwrap_or_default();
        let opened = view
            .agents
            .iter()
            .filter(|agent_id| !before.agents.contains(*agent_id) && !self.viewed(agent_id))
            .cloned()
            .collect();
        if view.looking {
            self.unread
                .retain(|agent_id| !view.agents.contains(agent_id));
        }
        if !view.agents.is_empty() {
            self.views.insert(client, view);
        }
        for agent_id in before.agents {
            if !self.viewed(&agent_id) {
                self.viewed_at.insert(agent_id, now);
            }
        }
        opened
    }
}

#[derive(Default)]
pub struct Seen {
    state: Mutex<State>,
}

impl Seen {
    /// The agent went back to work, so what it last finished is old news.
    pub fn working(&self, agent_id: &str) {
        let mut state = self.state.locked();
        state.unread.remove(agent_id);
        state.active_at.insert(agent_id.to_owned(), epoch_ms());
    }

    pub fn started(&self, agent_id: &str) {
        let mut state = self.state.locked();
        let now = epoch_ms();
        state.unread.remove(agent_id);
        state.active_at.insert(agent_id.to_owned(), now);
        state.started_at.insert(agent_id.to_owned(), now);
    }

    /// The agent did something that leaves nothing new to look at.
    pub fn active(&self, agent_id: &str) {
        self.state
            .locked()
            .active_at
            .insert(agent_id.to_owned(), epoch_ms());
    }

    /// The agent finished a turn or is waiting on the person.
    pub fn wants_a_look(&self, agent_id: &str) {
        let mut state = self.state.locked();
        state.active_at.insert(agent_id.to_owned(), epoch_ms());
        if !state.looked_at(agent_id) {
            state.unread.insert(agent_id.to_owned());
        }
    }

    /// The agents `client` has on screen now, replacing what it showed before,
    /// and whether the person is looking at them. Answers the agents it brought
    /// on screen that no client showed.
    pub fn show(
        &self,
        client: ClientId,
        agent_ids: impl IntoIterator<Item = String>,
        looking: bool,
    ) -> Vec<String> {
        let view = View {
            agents: agent_ids.into_iter().collect(),
            looking,
        };
        self.state.locked().show(client, view)
    }

    fn forget_client(&self, client: ClientId) {
        self.state.locked().show(client, View::default());
    }

    pub fn viewed(&self, agent_id: &str) -> bool {
        self.state.locked().viewed(agent_id)
    }

    /// Since when the agent has been neither on screen nor doing anything,
    /// as far as this core has seen.
    pub fn quiet_since(&self, agent_id: &str) -> Option<u64> {
        let state = self.state.locked();
        let viewed = state.viewed_at.get(agent_id).copied();
        let active = state.active_at.get(agent_id).copied();
        viewed.max(active)
    }

    pub fn unread(&self, agent_id: &str) -> bool {
        self.state.locked().unread.contains(agent_id)
    }

    pub fn active_at(&self, agent_id: &str) -> Option<u64> {
        self.state.locked().active_at.get(agent_id).copied()
    }

    /// The agent was named with `@` or sent a message by another agent.
    pub fn called(&self, agent_id: &str) {
        self.state
            .locked()
            .called_at
            .insert(agent_id.to_owned(), epoch_ms());
    }

    pub fn called_at(&self, agent_id: &str) -> Option<u64> {
        self.state.locked().called_at.get(agent_id).copied()
    }

    pub fn used_tool(&self, agent_id: &str) {
        self.state
            .locked()
            .tool_at
            .insert(agent_id.to_owned(), epoch_ms());
    }

    pub fn tool_at(&self, agent_id: &str) -> Option<u64> {
        self.state.locked().tool_at.get(agent_id).copied()
    }

    /// When the agent began the turn it is on or last ran.
    pub fn started_at(&self, agent_id: &str) -> Option<u64> {
        self.state.locked().started_at.get(agent_id).copied()
    }
}

impl super::part::Part for Seen {
    fn client_gone(&self, _core: &std::sync::Arc<super::Core>, client: ClientId) {
        self.forget_client(client);
    }
}

/// `client` shows `agent_ids` now, `focused` when the person is looking at them: the Mac's
/// window in front, or a phone's app in front and unlocked. Each client says this the same way.
/// An agent listed stays awake, focused or not; one shown while the person looks counts as read
/// and, for a chief, has the person with it, which lets go of the chief's kept lines shown there.
/// Only a client that `draws_terminals` has a terminal agent's ended terminal started again.
pub fn on_screen(
    core: &std::sync::Arc<super::Core>,
    client: ClientId,
    agent_ids: Vec<String>,
    focused: bool,
    draws_terminals: bool,
) {
    (core.workspace_above().agents_shown)(core, &agent_ids, focused, draws_terminals);
    core.seen().show(client, agent_ids, focused);
    core.changed(super::changed::What::View);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shows(seen: &Seen, client: ClientId, agent_ids: &[&str]) -> Vec<String> {
        seen.show(client, agent_ids.iter().map(|id| (*id).to_owned()), true)
    }

    #[test]
    fn an_agent_off_screen_stays_unread_until_it_is_shown_or_works_again() {
        let seen = Seen::default();
        seen.wants_a_look("a");
        assert!(seen.unread("a"));
        seen.working("a");
        assert!(!seen.unread("a"));
        seen.wants_a_look("a");
        shows(&seen, 1, &["a"]);
        assert!(!seen.unread("a"));
    }

    #[test]
    fn an_agent_is_active_when_it_opens_works_or_wants_a_look() {
        let seen = Seen::default();
        assert_eq!(seen.active_at("a"), None);
        let long_ago = |seen: &Seen| {
            seen.state.locked().active_at.insert("a".to_owned(), 1);
        };
        seen.active("a");
        assert!(seen.active_at("a").is_some_and(|at| at > 1));
        long_ago(&seen);
        seen.working("a");
        assert!(seen.active_at("a").is_some_and(|at| at > 1));
        long_ago(&seen);
        seen.wants_a_look("a");
        assert!(seen.active_at("a").is_some_and(|at| at > 1));
        assert_eq!(seen.active_at("b"), None);
    }

    #[test]
    fn an_agent_on_screen_is_never_left_unread() {
        let seen = Seen::default();
        shows(&seen, 1, &["a"]);
        seen.wants_a_look("a");
        assert!(!seen.unread("a"));
        shows(&seen, 1, &[]);
        seen.wants_a_look("a");
        assert!(seen.unread("a"));
    }

    #[test]
    fn every_client_s_screen_counts_until_that_client_goes() {
        let seen = Seen::default();
        assert_eq!(shows(&seen, 1, &["a"]), vec!["a".to_owned()]);
        assert!(shows(&seen, 2, &["a"]).is_empty());
        seen.forget_client(1);
        seen.wants_a_look("a");
        assert!(!seen.unread("a"));
        seen.forget_client(2);
        seen.wants_a_look("a");
        assert!(seen.unread("a"));
    }
}
