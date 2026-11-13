//! Rules kept from what the person said in a chat that the person removed again, from the
//! line's Undo or the Playbook. The chat's line says so, also in a chat that was asleep or a core
//! that started since, and its agent hears it with its next turn.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sikemux_process::sync::Locked;
use sikemux_view::chat::KeptRule;
use sikemux_view::memory::MemoryRule;
use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};

use crate::server::chat::feed::Feed;
use crate::server::chat::Chat;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::Core;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Removed {
    agent: String,
    project: String,
    id: u64,
    text: String,
    /// The feed its line was last put on.
    feed: Option<String>,
}

impl Removed {
    pub(super) fn of(agent: &str, project: &str, rule: &MemoryRule) -> Self {
        Self {
            agent: agent.to_owned(),
            project: project.to_owned(),
            id: rule.id,
            text: rule.text.clone(),
            feed: None,
        }
    }
}

/// Puts the line of each rule removed from `agent`'s chat on the feed it is heard on now.
pub(super) fn show(core: &Core, agent: &str) {
    if let Some(feed) = core.chats().heard(agent) {
        show_on(core, agent, &feed);
    }
}

/// `agent`'s chat is ready and its saved history shown: a line it has not had on this feed goes
/// at its end.
pub(crate) fn chat_ready(core: &Core, chat: &Chat) {
    if *chat.saved_reading.borrow() {
        return;
    }
    show_on(core, chat.agent_id(), &chat.feed);
}

fn show_on(core: &Core, agent: &str, feed: &Arc<Feed>) {
    let id = feed.id().to_owned();
    let lines: Vec<KeptRule> = {
        let mut state = core.memory().state.locked();
        let mut lines = Vec::new();
        let mut changed = Vec::new();
        for (folder, held) in state.held.iter_mut() {
            for removed in (held.removed.iter_mut())
                .filter(|removed| removed.agent == agent && removed.feed.as_ref() != Some(&id))
            {
                removed.feed = Some(id.clone());
                lines.push(KeptRule {
                    id: removed.id,
                    project: removed.project.clone(),
                    undone: true,
                });
                changed.push(folder.clone());
            }
        }
        changed.dedup();
        for folder in changed {
            core.memory().save(&mut state, &folder);
        }
        lines
    };
    let session = (feed.start()).map_or_else(|| agent.to_owned(), |start| start.session_id);
    for rule in lines {
        feed.emit(
            sikemux_fold::ChatEventKind::SessionUpdate,
            json!({ "sessionId": session, "update": sikemux_fold::update::rule_notice(rule) }),
        );
    }
}

/// What `agent` hears with its next turn of the rules the person removed from its chat, which
/// are then done with.
pub(crate) fn take_note(core: &Core, agent: &str) -> Option<ChatContext> {
    let gone: Vec<Removed> = {
        let mut state = core.memory().state.locked();
        let mut gone = Vec::new();
        let mut changed = Vec::new();
        for (folder, held) in state.held.iter_mut() {
            let before = gone.len();
            held.removed.retain(|removed| match removed.agent == agent {
                true => {
                    gone.push(removed.clone());
                    false
                }
                false => true,
            });
            if gone.len() > before {
                changed.push(folder.clone());
            }
        }
        for folder in changed {
            core.memory().save(&mut state, &folder);
        }
        gone
    };
    if gone.is_empty() {
        return None;
    }
    let text = (gone.iter())
        .map(|removed| {
            format!(
                "The person removed rule #{}, which you kept from their words: \"{}\". Don't follow it.",
                removed.id, removed.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(ChatContext {
        uri: format!("{FOR_AGENT_URI}rules-removed"),
        title: "Rules the person removed".into(),
        text,
    })
}
