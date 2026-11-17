//! What the core remembers so the person never says it twice: rules agents
//! follow and knowledge of how things are, kept for a project or for the
//! person. Each entry says where it was learned, so it can be checked, and
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
    /// The person whose words these were, by their account's first name, so a store that two
    /// people's agents read can say which; none for an agent, or when no one was signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
}

/// The agent the person said something to, and what it was called then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Who last moved it between the person's store and a project's, when, and from where.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<Moved>,
}

/// The person moved an entry between their own store and a project's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    #[serde(flatten)]
    pub by: Learned,
    /// The key of the store it came from.
    pub from: String,
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
    /// Who last moved it between the person's store and a project's, when, and from where.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<Moved>,
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

/// A rule as the memory view lists it, with whether it can move into the
/// project's AGENTS.md.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedRule {
    pub rule: MemoryRule,
    /// Why it cannot move into AGENTS.md; none when it can.
    #[serde(default)]
    pub stays: Option<Stays>,
}

/// Why a rule cannot move into the project's AGENTS.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stays {
    /// It is sensitive, so it never goes into the repository.
    Sensitive,
    /// It is the person's own, kept for every project rather than this repository.
    Yours,
    /// The project's folder is not the top of a git checkout, so an AGENTS.md there
    /// would not be versioned.
    NoCheckout,
    /// The project's AGENTS.md links to nothing that can be read, or to a
    /// file outside the checkout.
    LinkedAway,
}

/// Whose rules and knowledge these are: the project's, which every agent on its repository reads
/// wherever it is open, or the person's own, which follow them to every project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Store {
    #[default]
    Project,
    Personal,
}

impl Store {
    pub fn is_project(&self) -> bool {
        *self == Store::Project
    }

    /// The store an entry of this one moves to.
    pub fn other(self) -> Store {
        match self {
            Store::Project => Store::Personal,
            Store::Personal => Store::Project,
        }
    }
}

/// The chief's offer to move an entry to the other store, until it is moved or the person says
/// Not now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MoveOffer {
    /// A rule of the person's own names something of this repository, so the project's agents
    /// would follow it too.
    AboutHere,
    /// The person keeps the same rule for `elsewhere`, a project by its name, too, so it would
    /// follow them everywhere as theirs.
    SaidElsewhere { elsewhere: String },
}

/// What one store keeps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreMemory {
    pub store: Store,
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

/// What applies to a project: its own store, then the person's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub stores: Vec<StoreMemory>,
    /// The project's search of its past sessions for rules; none until it runs.
    #[serde(default)]
    pub seeding: Option<Seeding>,
}
