//! Requests every core answers, whatever protocol version it speaks, so an app
//! can ask a core it cannot otherwise talk to to replace itself or stop.
//!
//! The shape of everything in this module is fixed for good: a client sends
//! one [`crate::protocol::FrameKind::Frozen`] frame holding a [`FrozenRequest`] as its first
//! frame, the core answers with one [`crate::protocol::FrameKind::Frozen`] frame holding a
//! [`FrozenReply`], and closes the connection. Add new requests as new `op`
//! values; never change or remove a field.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::protocol::BuildIdentity;

/// The format of the state a core hands to its replacement. A core only
/// replaces itself with a binary that reads its format.
pub const RESUME_FORMAT: u32 = 9;

/// Every hand-over format a core of this build takes over from.
pub const READS_FORMATS: [u32; 6] = [4, 5, 6, 7, 8, RESUME_FORMAT];

/// The argument that makes a core binary print its [`UpgradeInfo`].
pub const UPGRADE_INFO_ARG: &str = "--upgrade-info";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FrozenRequest {
    /// Replace this core with `binary` in the same process, keeping every
    /// session. Without `force`, a core with chats it cannot carry over yet
    /// waits for them however long they take; with it, a chat that still
    /// cannot be carried over after a few seconds is stopped and started
    /// again on its session.
    Upgrade {
        binary: PathBuf,
        #[serde(default)]
        force: bool,
    },
    /// Stop every session and exit.
    StopEverything,
    /// Replace this core with its own binary in the same process, keeping
    /// every session and chat as [`Self::Upgrade`] does, and waiting however
    /// long its chats take to be able to carry their agents over. Answered
    /// before the core restarts, and once more, with `Refused`, only if the
    /// restart failed and the core carried on as it was.
    Restart,
    /// Which chats an update now would stop rather than carry over while
    /// their agents work. Answered with [`FrozenReply::UpdateBlockers`].
    UpdateBlockers,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FrozenReply {
    Accepted,
    Refused {
        message: String,
    },
    /// The core replaces itself later, once the chat turns it is running
    /// end, and goes on serving until then.
    Deferred {
        message: String,
    },
    UpdateBlockers {
        chats: Vec<UpdateBlocker>,
    },
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

/// What `<binary> core --upgrade-info` prints, as one JSON object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeInfo {
    pub version: String,
    pub commit: String,
    pub built_at: u64,
    pub source: String,
    /// The oldest hand-over format the binary takes over from. Cores
    /// that predate `reads_formats` compare their own format with this.
    pub resume_format: u32,
    #[serde(default)]
    pub reads_formats: Vec<u32>,
}

impl UpgradeInfo {
    pub fn of(build: &BuildIdentity) -> Self {
        Self {
            version: build.version.clone(),
            commit: build.commit.clone(),
            built_at: build.built_at,
            source: build.source.clone(),
            resume_format: READS_FORMATS[0],
            reads_formats: READS_FORMATS.to_vec(),
        }
    }

    /// Whether the binary takes over from a core writing `format`.
    pub fn reads(&self, format: u32) -> bool {
        if self.reads_formats.is_empty() {
            self.resume_format == format
        } else {
            self.reads_formats.contains(&format)
        }
    }

    pub fn build(&self) -> BuildIdentity {
        BuildIdentity {
            version: self.version.clone(),
            commit: self.commit.clone(),
            built_at: self.built_at,
            source: self.source.clone(),
        }
    }
}
