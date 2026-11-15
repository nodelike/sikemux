//! What stands in each project now, kept by the core for the strip at the top of its chief's
//! chat, for the phones and for the island: the person's open asks, every open agent in one of
//! four groups with one fact and since when, the project's latest release, and the agents closed
//! lately. Clients word and draw it; none works a group or a fact out of its own.

use serde::{Deserialize, Serialize};

use crate::chat::FailureKind;
use crate::turn::{ChecksStand, TurnWaitKind};

/// One project's strip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectNow {
    /// The workspace project, by id.
    pub project: String,
    /// In the order of their groups, and within a group the longest standing first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<NowRow>,
    /// None for a folder that is not a repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<NowRelease>,
    /// Newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<NowClosed>,
    /// What the person asked that is not yet in the app they run, nobody's first, then the
    /// longest standing first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<NowAsk>,
}

/// One message the person sent an agent, from then until its change is in the app they run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowAsk {
    pub id: u64,
    /// The first line of what they wrote.
    pub title: String,
    pub stand: AskStand,
    /// When it came to stand so, in Unix milliseconds.
    pub since: u64,
    /// The images the person sent with it, by path on the core's computer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

/// How far an ask has gone, in the order the card lists them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AskStand {
    /// Every agent on it closed before its work reached the project's branch.
    Nobody,
    Working {
        /// By id.
        agents: Vec<String>,
    },
    /// On the project's branch, and not in the build of the app the person runs.
    OnBranch,
    InYourBuild,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// The groups a strip shows, in the person's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NowGroup {
    WaitingOnYou,
    ReadyToLook,
    Moving,
    /// Nothing set to wake it and nothing for the person to look at: it may be stuck.
    NotSure,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// One open agent on the strip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowRow {
    pub agent: String,
    pub group: NowGroup,
    /// The lead it works for when that is not the project's chief, by id: a client folds it
    /// under that lead's row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    pub fact: NowFact,
    /// When the fact came to stand, in Unix milliseconds.
    pub since: u64,
}

/// The one thing a row says of its agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NowFact {
    /// It waits on the person for `count` things: calls and permissions.
    Asks {
        count: u32,
    },
    Failed {
        why: FailureKind,
    },
    /// Its turn ended with work left open and nothing set to wake it.
    Stopped,
    Working {
        /// The step of its plan it is on, when it said one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<String>,
    },
    /// Its turn ended on something set to wake it.
    Waiting {
        on: TurnWaitKind,
        /// What it waits on, in words: a process, a timer's title, an agent's name.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        what: String,
        /// The agent it waits on, by id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        /// Its place in the host's line, 1 for next.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<u32>,
        /// How long such a wait usually takes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_ms: Option<u64>,
    },
    /// Its turn finished with changes, and how far they have gone.
    Work {
        at: FixAt,
        checks: ChecksStand,
        /// The release tag that has them, once one does, such as `v0.9.3-nightly.22`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release: Option<String>,
    },
    /// Its turn finished and changed nothing.
    Answered,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// How far a finished change has gone, in the order it goes. A fix is fixed for the person only
/// at `Running`, or `Shipped` for a project that deploys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FixAt {
    Uncommitted,
    /// Committed in its own checkout and not on the project's branch.
    Committed,
    /// On the project's branch, as the project lands changes.
    Landed,
    /// In a release tag.
    Released,
    /// In the build of the app the person runs.
    Running,
    Staged,
    Shipped,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// The project's latest release and what its branch has since.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowRelease {
    pub branch: String,
    /// The newest tag on the branch; none before the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// When that tag was made, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagged_at: Option<u64>,
    /// Commits on the branch since the tag, or in all before the first.
    pub since_tag: u32,
    /// How CI stands on the branch's tip; none where none was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiStand>,
    /// A run that publishes a release is going.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub releasing: bool,
    /// The release tag the app the person runs was built at, or its commit when untagged, while
    /// that build is of this project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CiStand {
    Passed,
    Running,
    Failed,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// An agent closed lately, with what reopening it with `reopenAgent` needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowClosed {
    /// What `reopenAgent` takes.
    pub agent: String,
    /// What it was called, since a closed agent's name is not on the workspace's agents.
    pub name: String,
    pub at: u64,
    pub why: ClosedWhy,
    /// How far its work had gone as it closed; none when it changed nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<FixAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClosedWhy {
    /// The core closed it, its work landed.
    Landed,
    /// The core closed it, it changed nothing.
    NothingChanged,
    /// Its lead or another agent closed it.
    ByAgent,
    ByPerson,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}
