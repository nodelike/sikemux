//! How a search of chats is ranked: where the query matched first, by @name, then by title,
//! then by what was said, then whether the chat waits on the person, works, sits open or
//! closed. Every client draws the order the core sends.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sikemux_calls::workspace::{RecentChats, RecentChatsParams};
use sikemux_view::device::{FoundBy, FoundChat, RecentInfo};
use sikemux_view::inbox::{AgentShown, Inbox};
use sikemux_wire::protocol::AgentGroup;

use super::device_view::DeviceViewCore;
use super::{inbox, workspace, Core, CoreResult};
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;

/// How many closed chats found by their titles go ahead of the open agents found by what
/// they said; the rest follow those agents.
const CLOSED_TITLES_FIRST: usize = 5;

/// What a search ranks, as the device view last showed it: the open and asleep agents in the
/// rail's order, and the saved chats the core holds.
#[derive(Default)]
pub(crate) struct Searched {
    agents: Vec<Searchable>,
    chats: Vec<RecentInfo>,
}

struct Searchable {
    agent_id: String,
    chat: Option<String>,
    name: Option<String>,
    title: Option<String>,
    project: Option<String>,
    tier: Tier,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    NeedsYou,
    Moving,
    Open,
    Closed,
}

impl Searched {
    pub(crate) fn of(
        inbox: &Inbox,
        recent: &[RecentInfo],
        holders: HashMap<String, String>,
    ) -> Self {
        let chat_of: HashMap<&str, &str> = (holders.iter())
            .map(|(chat, agent)| (agent.as_str(), chat.as_str()))
            .collect();
        let agents = (inbox.agents.iter().chain(&inbox.kept))
            .map(|agent| Searchable {
                agent_id: agent.agent_id.clone(),
                chat: chat_of
                    .get(agent.agent_id.as_str())
                    .map(|chat| (*chat).to_owned()),
                name: agent.name.as_deref().map(str::to_lowercase),
                title: agent.titled.then(|| agent.title.to_lowercase()),
                project: agent.project.clone(),
                tier: tier(agent),
            })
            .collect();
        let chats = (recent.iter())
            .filter(|chat| chat.agent_id.is_none())
            .cloned()
            .collect();
        Searched { agents, chats }
    }
}

fn tier(agent: &AgentShown) -> Tier {
    match agent.group {
        AgentGroup::Blocked => Tier::NeedsYou,
        AgentGroup::Working | AgentGroup::Done => Tier::Moving,
        AgentGroup::Idle if agent.unread => Tier::Moving,
        AgentGroup::Idle => Tier::Open,
    }
}

/// What was typed: its words in lower case, and whether a leading `@` asks for names only.
struct Query {
    words: Vec<String>,
    names_only: bool,
}

impl Query {
    fn parse(typed: Option<&str>) -> Option<Self> {
        let typed = typed?.trim().to_lowercase();
        let (names_only, typed) = match typed.strip_prefix('@') {
            Some(name) => (true, name.trim_start().to_owned()),
            None => (false, typed),
        };
        let words: Vec<String> = typed.split_whitespace().map(str::to_owned).collect();
        (!words.is_empty()).then_some(Query { words, names_only })
    }

    /// How closely `name` matches: 0 when it is the query, 1 when the query starts it.
    fn name_match(&self, name: &str) -> Option<u8> {
        let [word] = self.words.as_slice() else {
            return None;
        };
        match name.strip_prefix(word.as_str()) {
            Some("") => Some(0),
            Some(_) => Some(1),
            None => None,
        }
    }

    /// How closely `title` matches: 0 when every word starts one of its words, 1 when every
    /// word is somewhere in it.
    fn title_match(&self, title: &str) -> Option<u8> {
        let mut inside = false;
        for word in &self.words {
            let mut starts = false;
            let mut found = false;
            for (at, _) in title.match_indices(word.as_str()) {
                found = true;
                let before = title.get(..at).and_then(|head| head.chars().next_back());
                if before.is_none_or(|before| !before.is_alphanumeric()) {
                    starts = true;
                    break;
                }
            }
            if !found {
                return None;
            }
            inside |= !starts;
        }
        Some(u8::from(inside))
    }
}

/// The saved chats `asked` names, and with a query, every open agent and saved chat it finds
/// in the order to show them. A quick answer looks only at names and the titles of the chats
/// the core holds; a page after the first adds only the saved chats on it.
pub async fn recent_chats(core: &Core, asked: RecentChatsParams) -> CoreResult<RecentChats> {
    let Some(query) = Query::parse(asked.query.as_deref()) else {
        return workspace::recent_chats(core, asked).await;
    };
    let first = asked.cursor.is_none();
    let project = asked.project.clone();
    let mut answer = match asked.quick || query.names_only {
        true => RecentChats::default(),
        false => workspace::recent_chats(core, asked).await?,
    };
    let searched = searched(core);
    let in_scope = |of: Option<&String>| project.is_none() || of == project.as_ref();
    let agents: Vec<&Searchable> = (searched.agents.iter())
        .filter(|agent| first && in_scope(agent.project.as_ref()))
        .collect();
    let mut seen = HashSet::new();
    let closed: Vec<&RecentInfo> = (searched.chats.iter())
        .filter(|chat| first && in_scope(Some(&chat.project)))
        .chain(answer.chats.iter().filter(|chat| chat.agent_id.is_none()))
        .filter(|chat| seen.insert(chat.id.as_str()))
        .collect();
    let found = ranked(&query, &agents, &closed, &answer.said);
    let held = answer.chats.iter().filter(|chat| chat.agent_id.is_some());
    let chats = (found.iter())
        .filter(|found| found.agent_id.is_none())
        .filter_map(|found| {
            closed
                .iter()
                .find(|chat| Some(&chat.id) == found.chat.as_ref())
        })
        .copied()
        .chain(held)
        .cloned()
        .collect();
    answer.chats = chats;
    answer.found = found;
    Ok(answer)
}

/// Where the query matched, how closely, the tier, unpinned, then rail place or age.
type Rank = (FoundBy, u8, Tier, bool, u64);

/// The open agents and closed chats `query` finds, in the order to show them.
fn ranked(
    query: &Query,
    agents: &[&Searchable],
    closed: &[&RecentInfo],
    said: &HashMap<String, sikemux_agents::sessions::said::FoundLine>,
) -> Vec<FoundChat> {
    let said_in = |chat: Option<&String>| {
        !query.names_only && chat.is_some_and(|chat| said.contains_key(chat))
    };
    let mut hits: Vec<(Rank, FoundChat)> = Vec::new();
    for (place, agent) in agents.iter().enumerate() {
        let name = (agent.name.as_deref()).and_then(|name| query.name_match(name));
        let title = (agent.title.as_deref())
            .filter(|_| !query.names_only)
            .and_then(|title| query.title_match(title));
        let (by, closeness) = match (name, title) {
            (Some(closeness), _) => (FoundBy::Name, closeness),
            (None, Some(closeness)) => (FoundBy::Title, closeness),
            (None, None) if said_in(agent.chat.as_ref()) => (FoundBy::Said, 0),
            (None, None) => continue,
        };
        let found = FoundChat {
            agent_id: Some(agent.agent_id.clone()),
            chat: agent.chat.clone(),
            by,
        };
        hits.push(((by, closeness, agent.tier, false, place as u64), found));
    }
    for chat in closed {
        let title = (!query.names_only)
            .then(|| query.title_match(&chat.title.to_lowercase()))
            .flatten();
        let (by, closeness) = match title {
            Some(closeness) => (FoundBy::Title, closeness),
            None if said_in(Some(&chat.id)) => (FoundBy::Said, 0),
            None => continue,
        };
        let found = FoundChat {
            agent_id: None,
            chat: Some(chat.id.clone()),
            by,
        };
        let newest_first = u64::MAX - chat.active_at;
        hits.push((
            (by, closeness, Tier::Closed, !chat.pinned, newest_first),
            found,
        ));
    }
    hits.sort_by_key(|(key, _)| *key);
    let mut shown = Vec::with_capacity(hits.len());
    let mut held_back = Vec::new();
    let mut closed_titles = 0;
    for (_, found) in hits {
        let closed_title = found.by == FoundBy::Title && found.agent_id.is_none();
        closed_titles += usize::from(closed_title);
        if found.by == FoundBy::Said && found.agent_id.is_none() {
            shown.append(&mut held_back);
        }
        match closed_title && closed_titles > CLOSED_TITLES_FIRST {
            true => held_back.push(found),
            false => shown.push(found),
        }
    }
    shown.append(&mut held_back);
    shown
}

/// What a search ranks, as the device view last showed it, or as it stands now when no view
/// was built yet.
fn searched(core: &Core) -> Arc<Searched> {
    if let Some(searched) = core.device_view().searched() {
        return searched;
    }
    let (chats, sessions) = core.ranked_infos();
    let held = core.triggers().held();
    let recent = core.workspaces().recent(&chats, &held);
    let inbox = inbox::view(core, &chats, &sessions);
    Arc::new(Searched::of(&inbox, &recent, core.workspaces().holders()))
}
