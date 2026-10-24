//! What the phone's native code hands its app. Everything the app shows of a
//! host crosses as the core's own JSON, read by TypeScript that ships over the
//! air, so a field the core adds needs no store build. Only what the native
//! side itself acts on, like a chat's place in its feed, is typed here.

use sikemux_wire::protocol as core;

/// The last of a chat's events the phone heard, to pick up from after a
/// reconnect.
#[derive(uniffi::Record, Clone)]
pub struct ChatMark {
    pub feed: String,
    pub seq: u64,
}

impl From<core::ChatMark> for ChatMark {
    fn from(mark: core::ChatMark) -> Self {
        Self {
            feed: mark.feed,
            seq: mark.seq,
        }
    }
}

impl From<ChatMark> for core::ChatMark {
    fn from(mark: ChatMark) -> Self {
        Self {
            feed: mark.feed,
            seq: mark.seq,
        }
    }
}

/// A chat crosses as the core's JSON: the phone draws what the core folded.
#[derive(uniffi::Enum)]
pub enum ChatAttachment {
    /// The chat as it stands, the core's `ChatView`. Changes follow,
    /// numbered from `mark.seq + 1`.
    Live {
        view_json: String,
        mark: ChatMark,
    },
    /// What changed since the mark the phone attached with, a list of the
    /// core's `ChatChange`.
    Resumed {
        changes_json: String,
        mark: ChatMark,
    },
    Missing,
}

pub(crate) fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

impl From<core::ChatAttachment> for ChatAttachment {
    fn from(attachment: core::ChatAttachment) -> Self {
        match attachment {
            core::ChatAttachment::Live { view, mark } => Self::Live {
                view_json: json(&view),
                mark: mark.into(),
            },
            core::ChatAttachment::Resumed { changes, mark } => Self::Resumed {
                changes_json: json(&changes),
                mark: mark.into(),
            },
            core::ChatAttachment::Missing => Self::Missing,
        }
    }
}

/// What the host sends without being asked.
#[derive(uniffi::Enum)]
pub enum CoreEvent {
    /// What changed in a chat, a list of the core's `ChatChange` as JSON.
    Chat {
        agent_id: String,
        seq: u64,
        changes_json: String,
    },
    /// Everything the phone shows of one host, as the core's `DeviceView` JSON.
    View { view_json: String },
}

impl CoreEvent {
    pub(crate) fn from_core(event: core::Event) -> Option<Self> {
        Some(match event {
            core::Event::Chat {
                agent_id,
                seq,
                changes,
            } => Self::Chat {
                agent_id,
                seq,
                changes_json: json(&changes),
            },
            core::Event::DeviceView { view } => Self::View {
                view_json: json(&view),
            },
            _ => return None,
        })
    }
}
