//! How a core keeps itself current, as clients read and show it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::prefs::UpdateChannel;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateState {
    #[default]
    Idle,
    /// Downloaded and in place, waiting for the chats in `blockers` to finish
    /// what they are doing.
    Waiting,
    Downloading,
    Installing,
    /// In place; the windows start again into it.
    Ready,
    Failed,
}

/// The newest release on the person's channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRelease {
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
    pub credits: Option<ReleaseCredits>,
}

/// The last check, kept because a failed one has nowhere else to show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// Unix milliseconds.
    pub at: u64,
    pub channel: UpdateChannel,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// This build updates itself: a release running inside its app.
    pub updates: bool,
    pub current: String,
    pub checked: Option<UpdateCheck>,
    pub available: Option<UpdateRelease>,
    pub state: UpdateState,
    pub error: Option<String>,
    pub blockers: Vec<UpdateBlocker>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    /// The person chose to stop the chats rather than wait, which the window
    /// that starts again tells the next core.
    pub forced: bool,
    /// What to run in a terminal to install `available`, for an app that its
    /// installer updates rather than itself.
    pub install_command: Option<String>,
    /// Whether this host puts updates in place by itself, which only the
    /// person turns off; none on anything but a host's core.
    pub automatic: Option<bool>,
    /// The update under way, which `outcome` names once it has one.
    pub id: Option<String>,
    /// How this host's last update ended, kept on disk by the one that put it in place.
    pub outcome: Option<UpdateOutcome>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateOutcomeState {
    /// In place, waiting for the new core to say it started well.
    Pending,
    Done,
    /// The new core did not start well, so the one before it is back.
    RolledBack,
    Failed,
}

/// The written result of one update, so a failure that happens while nobody
/// listens is still there to show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcome {
    pub id: String,
    pub state: UpdateOutcomeState,
    pub from: String,
    pub to: Option<String>,
    /// Unix milliseconds.
    pub at: u64,
    pub reason: Option<String>,
    /// The last lines of what went wrong, for the page to show.
    pub evidence: Option<String>,
}

/// A chat whose agent is at work, which an update now would stop and
/// start again on its session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBlocker {
    pub agent_id: String,
    pub provider: String,
    pub title: Option<String>,
    /// `starting` while the agent is still starting, `busy` while the
    /// chat is in the middle of a request.
    pub reason: String,
}

/// Credits a release carries in its update manifest, so they show without asking GitHub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseCredits {
    pub commits: u32,
    pub compare: String,
    pub contributors: Vec<Contributor>,
    /// `data:` URLs keyed by each contributor's avatar address.
    pub avatars: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contributor {
    pub login: String,
    pub name: String,
    pub commits: u32,
    pub avatar: String,
}
