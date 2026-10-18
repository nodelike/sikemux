//! How devices show the open agents: the group each is in, the word beside
//! it, and where it sits in the list. The app's own copy of the rule is
//! `src/state/agentStatus.ts`.

use std::cmp::Reverse;

use crate::protocol::{AgentGroup, AgentStatus, ChatInfo, ChatState, SessionInfo, SessionKind};

fn group(blocked: bool, working: bool, unread: bool) -> AgentGroup {
    if blocked {
        AgentGroup::Blocked
    } else if working {
        AgentGroup::Working
    } else if unread {
        AgentGroup::Done
    } else {
        AgentGroup::Idle
    }
}

fn status(group: AgentGroup) -> AgentStatus {
    match group {
        AgentGroup::Blocked => AgentStatus::Blocked,
        AgentGroup::Working => AgentStatus::Working,
        AgentGroup::Done => AgentStatus::Done,
        AgentGroup::Idle => AgentStatus::Idle,
    }
}

/// `asking` says the agent waits on the person to answer its question.
pub(crate) fn show_chat(chat: &mut ChatInfo, asking: bool) {
    chat.group = group(
        asking || !chat.pending_permissions.is_empty(),
        chat.running || chat.subagents > 0,
        chat.unread,
    );
    chat.status = match chat.group {
        AgentGroup::Blocked => AgentStatus::Blocked,
        _ if chat.asleep => AgentStatus::Asleep,
        _ if chat.state == ChatState::Stopped => AgentStatus::Stopped,
        _ if chat.state == ChatState::Starting => AgentStatus::Starting,
        shown => status(shown),
    };
}

pub(crate) fn show_session(session: &mut SessionInfo, asking: bool) {
    if session.kind != SessionKind::Terminal || session.agent_id.is_none() {
        return;
    }
    let state = session.agent_state.as_deref();
    let shown = group(
        asking || state == Some("blocked"),
        state == Some("working"),
        session.unread,
    );
    session.group = Some(shown);
    session.status = Some(match shown {
        AgentGroup::Done | AgentGroup::Idle if !session.running || state == Some("stopped") => {
            AgentStatus::Stopped
        }
        shown => status(shown),
    });
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
