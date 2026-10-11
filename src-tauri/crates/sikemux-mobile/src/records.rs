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

/// A chat's events cross as JSON, `{ kind, payload }` each: the chat screen
/// reads them with the host app's own chat code.
#[derive(uniffi::Enum)]
pub enum ChatAttachment {
    /// Live events follow, numbered from `mark.seq + 1`.
    Live {
        session_id: String,
        capabilities_json: String,
        setup_json: String,
        permission_mode: String,
        running: bool,
        turned: bool,
        replay_json: String,
        mark: ChatMark,
        /// The replay is the chat's end. `chat_history` pages back from here.
        older_before: Option<u64>,
    },
    /// What the phone missed since the mark it attached with.
    Resumed {
        events_json: String,
        mark: ChatMark,
    },
    Missing,
    /// The chat said more than the host keeps; the host app has to open it.
    Restart,
}

/// Earlier events of a chat, as `{ kind, payload }` JSON, oldest first.
#[derive(uniffi::Record)]
pub struct ChatHistory {
    pub events_json: String,
    /// Where the page before this one ends; absent at the chat's start.
    pub older_before: Option<u64>,
}

pub(crate) fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

impl From<core::ChatAttachment> for ChatAttachment {
    fn from(attachment: core::ChatAttachment) -> Self {
        match attachment {
            core::ChatAttachment::Live {
                start,
                permission_mode,
                running,
                turned,
                replay,
                mark,
                older_before,
            } => Self::Live {
                session_id: start.session_id,
                capabilities_json: json(&start.capabilities),
                setup_json: json(&start.setup),
                permission_mode,
                running,
                turned,
                replay_json: json(&replay),
                mark: mark.into(),
                older_before,
            },
            core::ChatAttachment::Resumed { events, mark } => Self::Resumed {
                events_json: json(&events),
                mark: mark.into(),
            },
            core::ChatAttachment::Missing => Self::Missing,
            core::ChatAttachment::Restart => Self::Restart,
        }
    }
}

/// What the host sends without being asked.
#[derive(uniffi::Enum)]
pub enum CoreEvent {
    /// One of a chat's events, as `{ kind, payload }` JSON.
    Chat {
        agent_id: String,
        seq: u64,
        event_json: String,
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
                event,
            } => Self::Chat {
                agent_id,
                seq,
                event_json: json(&event),
            },
            core::Event::DeviceView { view } => Self::View {
                view_json: json(&view),
            },
            _ => return None,
        })
    }
}
