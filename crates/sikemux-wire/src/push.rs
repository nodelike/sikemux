//! What a host's sealed notification holds once the phone opens it. The phone's
//! notification code reads it in Swift and Kotlin as well as TypeScript, all
//! generated from this file.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationKind {
    /// A chat agent asks to do something; the card may carry Allow and Reject.
    Permission,
    /// A terminal agent waits for the person to type.
    Input,
    /// A turn that ran a while has ended.
    Finished,
    /// An agent reported an error or stopped unexpectedly.
    Problem,
    /// A device asks to pair with the host; tapping opens where it is let in.
    Pairing,
    /// Removes the card with the same `collapseId`; shows nothing.
    Clear,
}

/// The Android channel a card goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationChannel {
    NeedsYou,
    Finished,
    Problems,
}

/// The iOS category, which decides the card's buttons. Only `permission`
/// has any: Reject and Allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationCategory {
    Permission,
    NeedsYou,
    Finished,
    Problem,
}

impl NotificationKind {
    pub fn channel(self) -> Option<NotificationChannel> {
        match self {
            Self::Permission | Self::Input | Self::Pairing => Some(NotificationChannel::NeedsYou),
            Self::Finished => Some(NotificationChannel::Finished),
            Self::Problem => Some(NotificationChannel::Problems),
            Self::Clear => None,
        }
    }
}

/// What the phone reads once it opens a sealed notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub v: u8,
    pub kind: NotificationKind,
    /// `None` for `clear`.
    pub channel: Option<NotificationChannel>,
    /// `None` for `clear`. `permission` only when both answers are offered.
    pub category: Option<NotificationCategory>,
    /// The same id the push carried, for removing this card later.
    pub collapse_id: String,
    /// Groups one agent's cards: `<hostKey>/<agentId>`.
    pub thread: String,
    pub host_key: String,
    pub host_name: String,
    pub agent_id: String,
    pub provider: String,
    pub chat_title: Option<String>,
    pub title: String,
    pub body: String,
    /// One line to show in monospace, such as the command an agent wants to run.
    pub detail: Option<String>,
    /// Where tapping the card goes.
    pub url: String,
    /// The permission request to answer with `answer_permission`.
    pub request_id: Option<String>,
    pub allow_option_id: Option<String>,
    pub reject_option_id: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub expires_at: u64,
}
