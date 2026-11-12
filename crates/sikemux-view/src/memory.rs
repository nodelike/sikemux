//! What the core remembers so the person never says it twice: rules agents
//! follow and knowledge of how things are, kept for a project or a folder
//! above it. Each entry says where it was learned, so it can be checked, and
//! knowledge visibly goes stale. What a goal decided and a goal's own rules
//! are kept on its card by the board.

use serde::{Deserialize, Serialize};

/// Who said something, and in which chat turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Learned {
    /// The agent that said it; none for the person.
    pub agent: Option<String>,
    /// What that agent was called then.
    pub name: Option<String>,
    pub feed: Option<String>,
    pub turn: Option<u64>,
    pub at: u64,
    /// For what the person said in a chat, the agent they said it to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<SaidTo>,
}

/// The agent the person said something to, and what it was called then.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaidTo {
    pub agent: String,
    pub name: Option<String>,
}

/// How agents must work here, such as "shadows, never glows".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRule {
    /// The same for as long as it is kept, and never given to another.
    pub id: u64,
    pub text: String,
    /// It never goes into the repository. Agents read it; only Sikemux's own
    /// window shows it, and other readers get a count of what is hidden.
    pub sensitive: bool,
    pub learned: Learned,
    /// What it said before it was last corrected.
    pub was: Option<String>,
}

/// How things are, such as "the service is called ai-service now".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Knowledge {
    /// The same for as long as it is kept, and never given to another.
    pub id: u64,
    pub text: String,
    /// The paths and services it is about, as whole words. It goes into an
    /// agent's brief when its task or card names one of them, or into every
    /// brief when there are none.
    pub about: Vec<String>,
    /// It never goes into the repository. Agents read it; only Sikemux's own
    /// window shows it, and other readers get a count of what is hidden.
    pub sensitive: bool,
    pub check: Option<MemoryCheck>,
    pub learned: Learned,
    /// What it said before it was last corrected.
    pub was: Option<String>,
    pub checked: Option<LastCheck>,
    /// Why it is no longer known to hold; it stays out of briefs meanwhile.
    pub stale: Option<Stale>,
}

/// What the core can test about knowledge by itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MemoryCheck {
    /// A file or folder exists; a relative path is read from the folder the
    /// knowledge is kept for.
    PathExists { path: String },
    /// Something accepts a connection on the port.
    HostAnswers { host: String, port: u16 },
}

/// The last time the core tested knowledge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastCheck {
    pub at: u64,
    pub outcome: CheckOutcome,
    /// Why it failed, or why it learned nothing.
    pub why: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckOutcome {
    Passed,
    /// The answer says it no longer holds, such as a missing path or a host
    /// that refused the connection.
    Failed,
    /// Nothing answered either way, such as a host whose network is out of
    /// reach while the VPN is off. The knowledge stays as it was.
    Unreached,
}

/// Why knowledge is no longer known to hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Stale {
    /// The core's test failed for `why`, first at `since`. It holds again
    /// once the test passes.
    Failed { since: u64, why: String },
    /// Someone said it no longer holds without knowing what does. It holds
    /// again only once corrected.
    Disputed { why: String, by: Learned },
}

/// A rule as the memory view lists it, with whether it can move into its
/// folder's AGENTS.md.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedRule {
    pub rule: MemoryRule,
    /// Why it cannot move into AGENTS.md; none when it can.
    #[serde(default)]
    pub stays: Option<Stays>,
}

/// Why a rule cannot move into its folder's AGENTS.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stays {
    /// It is sensitive, so it never goes into the repository.
    Sensitive,
    /// Its folder is not the top of a git checkout, so an AGENTS.md there
    /// would not be versioned.
    NoCheckout,
    /// The folder's AGENTS.md links to nothing that can be read, or to a
    /// file outside the checkout.
    LinkedAway,
}

/// What one folder keeps: a project, or a folder above it whose entries every
/// project under it shares.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderMemory {
    pub folder: String,
    pub rules: Vec<ListedRule>,
    pub knowledge: Vec<Knowledge>,
    /// Sensitive entries left out, for a reader other than Sikemux's own window.
    pub hidden: usize,
}

/// A rule the person said again and again in their past sessions in a
/// project, offered to keep.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeededRule {
    /// The same for as long as it is offered.
    pub id: u64,
    pub text: String,
    /// How many sessions it was said in.
    pub sessions: u32,
}

/// Where looking through a project's past sessions for the rules the person
/// keeps repeating stands. It runs once per project unless the person runs
/// it again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Seeding {
    Reading {
        since: u64,
    },
    /// Rules waiting for the person to keep with `keepSeededRules`, which
    /// keeps those it names and drops the rest.
    Offered {
        rules: Vec<SeededRule>,
        at: u64,
    },
    /// It ended, with `kept` of the rules it found kept as rules.
    Ran {
        at: u64,
        kept: u32,
    },
}

/// What applies to a project: its own folder first, then each folder above
/// it, nearest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub folders: Vec<FolderMemory>,
    /// The project's search of its past sessions for rules; none until it runs.
    #[serde(default)]
    pub seeding: Option<Seeding>,
}
