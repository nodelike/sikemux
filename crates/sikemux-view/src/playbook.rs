//! What a project's Playbook lists: what the person decided once that every later agent goes
//! by, and what runs there with nobody asking. Each line says who said it and where.

use serde::{Deserialize, Serialize};

use crate::chat::TriggerWhen;
use crate::memory::Learned;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playbook {
    /// The rules kept from the person's own words, the project's and the person's own.
    pub rules: Vec<Entry>,
    /// The rules agents noted while working, which never widen what agents may do, and the
    /// knowledge kept of how things are.
    pub notes: Vec<Entry>,
    pub landing: Landing,
    pub automation: Vec<RunsAlone>,
    /// Where agents' actions land outside this machine, and which of them wait for the person.
    #[serde(default)]
    pub places: crate::places::Places,
}

/// A rule or knowledge Sikemux keeps, as the Playbook lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: u64,
    pub text: String,
    pub learned: Learned,
    /// Knowledge of how things are, rather than a rule.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub knowledge: bool,
    /// Knowledge no longer known to hold, which stays out of briefs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sensitive: bool,
    #[serde(default, skip_serializing_if = "crate::memory::Store::is_project")]
    pub store: crate::memory::Store,
    /// Who last moved it between the person's store and the project's, and when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<Learned>,
    /// The person who said it, when someone other than whoever is signed in here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub someone: Option<String>,
    /// In the org's store, written by another of the org's members, past or present.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub theirs: bool,
    /// In the org's store, written by someone no longer in the org.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub left: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<crate::memory::MoveOffer>,
    /// The name of the org the project is shared with: whose store it is, or, for the project's
    /// own entries, who does not get it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Moved to the org's store, it waits there for an owner or admin to keep it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waiting: bool,
    /// The owner or admin who dropped it from the org's waiting list, by first name; empty when
    /// not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped: Option<String>,
    /// Why the org's vault would not take it, so it stays only on this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// A move to the org's store waits for an owner or admin.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asks: bool,
    /// The project is shared with `org`, but its vault is not set up on this host yet, so
    /// nothing kept here reaches the org.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shut: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Landing {
    pub lands: Lands,
    /// What counts as landed here, and how the latest changes reached the branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub means: Option<crate::turn::LandedMeans>,
}

/// Who puts work on the project's branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lands {
    /// Agents land their own work once its checks pass.
    #[default]
    Agents,
    /// Nothing counts as landed until the person lands it.
    Person,
}

/// How agents start on the board's cards nobody holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Auto {
    /// How many work at once.
    pub slots: u32,
    /// Whole dollars of spending past which it starts nothing new.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling: Option<u32>,
    /// The agent that works the cards.
    pub builder: AutoAgent,
    /// The agent that reviews each change before it lands; none for any agent on another CLI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<AutoAgent>,
}

/// One of Auto's agents: the app's launcher it starts with, who pays and its model; none of
/// each for the first that works.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoAgent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Something that runs in the project with nobody asking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RunsAlone {
    /// Agents start on the board's cards nobody holds.
    Auto(Box<Auto>),
    /// A trigger an agent set up, in its own words.
    Trigger {
        id: u64,
        title: String,
        when: TriggerWhen,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_at: Option<u64>,
        /// The agent that set it up, in which chat turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        set: Option<Box<Learned>>,
        /// When the person paused it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paused: Option<u64>,
        /// Its webhook has been called by GitHub.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        from_github: bool,
    },
}
