//! A chat showing one of the person's asks alone: only the rows the core filed under it, and
//! what the person writes goes to the agent as a reply on it.

use sikemux_view::chat::ChatMessage;
use sikemux_wire::protocol::{ChatContext, ASK_REPLY_URI};

use super::{Chat, ChatRow};
use crate::core::ChatAsk;

/// The ask a chat shows alone.
#[derive(Clone, PartialEq)]
pub(crate) struct Narrowed {
    pub id: u64,
    /// When the person sent it, before which no row can be its.
    pub at: u64,
    pub title: String,
}

impl Chat {
    /// Shows only `to`'s rows, or every row again, and says what to ask the core for them.
    pub(crate) fn narrow(&mut self, agent: &str, to: Option<Narrowed>) -> Option<ChatAsk> {
        if self.narrowed == to {
            return None;
        }
        self.narrowed = to;
        (self.outside, self.ask_rows) = (false, 0);
        Some(ChatAsk::Latest {
            agent: agent.to_owned(),
            feed: self.mark.as_ref()?.feed.clone(),
        })
    }

    pub(super) fn shows(&self, row: &ChatMessage) -> bool {
        self.narrowed
            .as_ref()
            .is_none_or(|narrowed| row.ask == Some(narrowed.id))
    }

    pub(super) fn of_ask(&mut self, rows: Vec<ChatMessage>) -> Vec<ChatRow> {
        let kept: Vec<ChatRow> = (rows.into_iter())
            .filter(|row| self.shows(row))
            .map(ChatRow::new)
            .collect();
        self.ask_rows += kept.len();
        kept
    }

    /// The page reaches back past when the ask shown alone was sent, so no earlier page holds
    /// any of its rows.
    pub(super) fn before_ask(&self, rows: &[ChatMessage]) -> bool {
        self.narrowed.as_ref().is_some_and(|narrowed| {
            (rows.iter())
                .filter_map(|row| row.sent_at)
                .any(|at| at < narrowed.at)
        })
    }

    /// What tells the agent a message is a reply on the ask shown alone.
    pub(crate) fn reply_context(&self) -> Option<ChatContext> {
        let narrowed = self.narrowed.as_ref()?;
        Some(ChatContext {
            uri: format!("{ASK_REPLY_URI}{}", narrowed.id),
            title: format!("Ask #{}", narrowed.id),
            text: narrowed.title.clone(),
        })
    }

    /// An earlier page to ask for while the ask shown alone has no rows held yet, as an empty
    /// transcript never scrolls to its top to ask for one.
    pub(super) fn more_of_ask(&mut self, agent: &str) -> Option<ChatAsk> {
        (self.narrowed.is_some() && self.ask_rows == 0)
            .then(|| self.earlier(agent))
            .flatten()
    }

    pub(crate) fn narrowed_to(&self) -> Option<u64> {
        self.narrowed.as_ref().map(|narrowed| narrowed.id)
    }
}
