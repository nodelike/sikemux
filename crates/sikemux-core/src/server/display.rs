//! How devices show the open agents: the group each is in, the word beside
//! it, and where it sits in the list.

use std::cmp::Reverse;

use crate::protocol::{
    AgentGroup, AgentState, AgentStatus, ChatInfo, ChatState, SessionInfo, SessionKind,
};

/// The one word every client shows for an agent in `group`.
pub(crate) fn group_label(group: AgentGroup) -> &'static str {
    match group {
        AgentGroup::Blocked => "Needs input",
        AgentGroup::Working => "Working",
        AgentGroup::Done => "Done",
        AgentGroup::Idle => "Idle",
    }
}

/// The word for an agent's status, where a list says more than its group.
pub(crate) fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Asleep => "Sleeping",
        AgentStatus::Stopped => "Stopped",
        AgentStatus::Starting => "Starting…",
        status => group_label(status.group()),
    }
}

/// `asking` says the agent waits on the person to answer its question.
pub(crate) fn show_chat(chat: &mut ChatInfo, asking: bool) {
    chat.status = if asking || !chat.pending_permissions.is_empty() {
        AgentStatus::Blocked
    } else if chat.running || chat.subagents > 0 {
        AgentStatus::Working
    } else if chat.unread {
        AgentStatus::Done
    } else {
        match chat.state {
            ChatState::Asleep => AgentStatus::Asleep,
            ChatState::Stopped => AgentStatus::Stopped,
            ChatState::Starting => AgentStatus::Starting,
            ChatState::Ready => AgentStatus::Idle,
        }
    };
    chat.group = chat.status.group();
}

pub(crate) fn show_session(session: &mut SessionInfo, asking: bool) {
    if session.kind != SessionKind::Terminal || session.agent_id.is_none() {
        return;
    }
    let state = session.agent_state;
    let status = if asking || state == Some(AgentState::Blocked) {
        AgentStatus::Blocked
    } else if state == Some(AgentState::Working) {
        AgentStatus::Working
    } else if session.unread {
        AgentStatus::Done
    } else if !session.running || state == Some(AgentState::Stopped) {
        AgentStatus::Stopped
    } else {
        AgentStatus::Idle
    };
    session.status = Some(status);
    session.group = Some(status.group());
}

/// Numbers the open agents in the order they want the person. Within a
/// group of busy agents each keeps its place while it works, the newest
/// begun first; the rest go most recently active first.
pub(crate) fn rank(
    chats: &mut [ChatInfo],
    sessions: &mut [SessionInfo],
    started_at: impl Fn(&str) -> Option<u64>,
) {
    enum Slot {
        Chat(usize),
        Session(usize),
    }
    let key = |agent_id: &str, group: AgentGroup, active_at: Option<u64>| {
        let busy = matches!(group, AgentGroup::Blocked | AgentGroup::Working);
        let at = if busy {
            started_at(agent_id).or(active_at)
        } else {
            active_at
        };
        (group, Reverse(at.unwrap_or(0)), agent_id.to_owned())
    };
    let mut listed: Vec<_> = chats
        .iter()
        .enumerate()
        .map(|(index, chat)| {
            (
                key(&chat.agent_id, chat.group, chat.active_at),
                Slot::Chat(index),
            )
        })
        .collect();
    for (index, session) in sessions.iter().enumerate() {
        let (Some(agent_id), Some(group)) = (&session.agent_id, session.group) else {
            continue;
        };
        if !session.running || chats.iter().any(|chat| &chat.agent_id == agent_id) {
            continue;
        }
        listed.push((
            key(agent_id, group, session.active_at),
            Slot::Session(index),
        ));
    }
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    for (place, (_, slot)) in listed.into_iter().enumerate() {
        let place = u32::try_from(place).unwrap_or(u32::MAX);
        match slot {
            Slot::Chat(index) => chats[index].attention_rank = place,
            Slot::Session(index) => sessions[index].attention_rank = Some(place),
        }
    }
}
