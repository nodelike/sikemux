//! Which agents finished or asked for something while the person was looking
//! elsewhere. The app says which agents it shows, and a phone shows the ones it
//! has open while it is in front; the rest are left unread until one shows them. It also keeps when each agent last did something, so
//! lists can put the most recent first.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use super::connection::ClientId;
use super::remote::unix_ms;

#[derive(Default)]
struct State {
    unread: HashSet<String>,
    active_at: HashMap<String, u64>,
    started_at: HashMap<String, u64>,
    on_screen: HashSet<String>,
    /// The app connection that said what is on screen; nothing is once it goes.
    shown_by: Option<ClientId>,
    on_phones: HashSet<String>,
    /// Agents that finished or asked while a phone showed them, not yet told to the app.
    seen_on_phones: Vec<String>,
}

impl State {
    fn shown(&self, agent_id: &str) -> bool {
        self.on_screen.contains(agent_id) || self.on_phones.contains(agent_id)
    }

    fn read_shown(&mut self) {
        let State {
            unread,
            on_screen,
            on_phones,
            ..
        } = self;
        unread.retain(|agent_id| !on_screen.contains(agent_id) && !on_phones.contains(agent_id));
    }
}

#[derive(Default)]
pub(crate) struct Seen {
    state: Mutex<State>,
}

impl Seen {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The agent went back to work, so what it last finished is old news.
    pub(crate) fn working(&self, agent_id: &str) {
        let mut state = self.lock();
        state.unread.remove(agent_id);
        state.active_at.insert(agent_id.to_owned(), unix_ms());
    }

    /// The agent began a turn of work.
    pub(crate) fn started(&self, agent_id: &str) {
        let mut state = self.lock();
        let now = unix_ms();
        state.unread.remove(agent_id);
        state.active_at.insert(agent_id.to_owned(), now);
        state.started_at.insert(agent_id.to_owned(), now);
    }

    /// The agent did something that leaves nothing new to look at.
    pub(crate) fn active(&self, agent_id: &str) {
        self.lock().active_at.insert(agent_id.to_owned(), unix_ms());
    }

    /// The agent finished a turn or is waiting on the person.
    pub(crate) fn wants_a_look(&self, agent_id: &str) {
        let mut state = self.lock();
        state.active_at.insert(agent_id.to_owned(), unix_ms());
        if !state.shown(agent_id) {
            state.unread.insert(agent_id.to_owned());
        } else if state.on_phones.contains(agent_id) {
            state.seen_on_phones.push(agent_id.to_owned());
        }
    }

    pub(crate) fn on_screen(&self, client: ClientId, agent_ids: Vec<String>) {
        let mut state = self.lock();
        state.on_screen = agent_ids.into_iter().collect();
        state.shown_by = Some(client);
        state.read_shown();
    }

    /// The agents some phone shows now. Answers with those a phone saw since
    /// last asked: ones it just opened that were unread, and ones that finished
    /// or asked while it showed them.
    pub(crate) fn on_phones(&self, agent_ids: HashSet<String>) -> Vec<String> {
        let mut state = self.lock();
        let mut seen = std::mem::take(&mut state.seen_on_phones);
        seen.extend(
            state
                .unread
                .iter()
                .filter(|agent_id| agent_ids.contains(*agent_id))
                .cloned(),
        );
        state.on_phones = agent_ids;
        state.read_shown();
        seen.sort();
        seen.dedup();
        seen
    }

    pub(crate) fn client_gone(&self, client: ClientId) {
        let mut state = self.lock();
        if state.shown_by == Some(client) {
            state.on_screen.clear();
            state.shown_by = None;
        }
    }

    pub(crate) fn unread(&self, agent_id: &str) -> bool {
        self.lock().unread.contains(agent_id)
    }

    pub(crate) fn active_at(&self, agent_id: &str) -> Option<u64> {
        self.lock().active_at.get(agent_id).copied()
    }

    /// When the agent began the turn it is on or last ran.
    pub(crate) fn started_at(&self, agent_id: &str) -> Option<u64> {
        self.lock().started_at.get(agent_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_off_screen_stays_unread_until_it_is_shown_or_works_again() {
        let seen = Seen::default();
        seen.wants_a_look("a");
        assert!(seen.unread("a"));
        seen.working("a");
        assert!(!seen.unread("a"));
        seen.wants_a_look("a");
        seen.on_screen(1, vec!["a".into()]);
        assert!(!seen.unread("a"));
    }

    #[test]
    fn an_agent_is_active_when_it_opens_works_or_wants_a_look() {
        let seen = Seen::default();
        assert_eq!(seen.active_at("a"), None);
        seen.active("a");
        let opened = seen.active_at("a").unwrap();
        seen.working("a");
        let worked = seen.active_at("a").unwrap();
        seen.wants_a_look("a");
        let looked = seen.active_at("a").unwrap();
        assert!(opened <= worked && worked <= looked);
        assert_eq!(seen.active_at("b"), None);
    }

    #[test]
    fn an_agent_on_screen_is_never_left_unread() {
        let seen = Seen::default();
        seen.on_screen(1, vec!["a".into()]);
        seen.wants_a_look("a");
        assert!(!seen.unread("a"));
        seen.on_screen(1, Vec::new());
        seen.wants_a_look("a");
        assert!(seen.unread("a"));
    }

    #[test]
    fn nothing_is_on_screen_once_the_app_that_showed_it_goes() {
        let seen = Seen::default();
        seen.on_screen(1, vec!["a".into()]);
        seen.client_gone(2);
        seen.wants_a_look("a");
        assert!(!seen.unread("a"));
        seen.client_gone(1);
        seen.wants_a_look("a");
        assert!(seen.unread("a"));
    }

    #[test]
    fn an_agent_open_on_a_phone_is_read_and_stays_read_while_shown() {
        let seen = Seen::default();
        seen.wants_a_look("a");
        assert_eq!(
            seen.on_phones(HashSet::from(["a".to_owned()])),
            vec!["a".to_owned()]
        );
        assert!(!seen.unread("a"));
        assert!(seen.on_phones(HashSet::from(["a".to_owned()])).is_empty());
        seen.wants_a_look("a");
        assert!(!seen.unread("a"));
        assert_eq!(
            seen.on_phones(HashSet::from(["a".to_owned()])),
            vec!["a".to_owned()]
        );
        seen.on_phones(HashSet::new());
        seen.wants_a_look("a");
        assert!(seen.unread("a"));
    }
}
