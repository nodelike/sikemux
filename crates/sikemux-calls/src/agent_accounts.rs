use crate::agent_updates::Updating;
use serde::{Deserialize, Serialize};
use sikemux_agents::agent_accounts::Book;
use sikemux_agents::agent_accounts::Profile;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub provider: String,
    pub config_path: Option<String>,
}

/// Which account's sign-in to look at afresh; every account's without a provider.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Forget {
    pub provider: Option<String>,
    pub config_path: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAccountsParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveChatParams {
    pub agent_id: String,
    pub account_id: String,
}

/// The account a chat runs on, those it may move to, and why there are none when there are none,
/// as a client offers them.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatAccounts {
    pub account: Option<NamedAccount>,
    pub others: Vec<NamedAccount>,
    pub none_because: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NamedAccount {
    pub id: String,
    pub label: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Authenticate {
    pub agent_id: String,
    pub method_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct Removing {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct Choosing {
    pub provider: String,
    pub id: String,
}

/// A chat a pick moved, and the account it was on.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MovedChat {
    pub agent_id: String,
    pub from: String,
}

/// A chat a pick left where it was, and why.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StayedChat {
    pub agent_id: String,
    pub why: String,
}

/// The accounts after a pick, what a client hands back to undo it, and the chats that stayed.
#[derive(Serialize, Deserialize)]
pub struct Picked {
    pub book: Book,
    pub undo: Unpicking,
    #[serde(default)]
    pub stayed: Vec<StayedChat>,
}

/// A pick to take back: the account picked, the one chosen before it, and the chats it moved.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unpicking {
    pub provider: String,
    pub picked: String,
    pub back_to: String,
    pub moved: Vec<MovedChat>,
}

/// Whether new chats go to the account whose week ends soonest, for every agent.
#[derive(Serialize, Deserialize)]
pub struct Smart {
    pub on: bool,
}

/// Whether new chats in the project at `project` may start on the account `account_id`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InProject {
    pub account_id: String,
    pub project: String,
    pub allowed: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Saving {
    pub profile: Profile,
}

/// The agent whose pin to take away.
#[derive(Serialize, Deserialize)]
pub struct Adding {
    pub provider: String,
    pub name: String,
}

/// The account whose limit reset to spend, by the launcher its usage row names.
#[derive(Serialize, Deserialize)]
pub struct UsingReset {
    pub launcher: String,
    /// The reset to spend, by its kind's id; none spends the one the agent would.
    #[serde(default)]
    pub kind: Option<String>,
}

/// What spending a reset did, and the resets the account holds after it.
#[derive(Serialize, Deserialize)]
pub struct UsedReset {
    pub outcome: sikemux_agents::agent_accounts::usage::ResetOutcome,
    pub left: Option<u32>,
}

/// How much of its plan an account has used, as the core last read it.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    /// The launcher that starts agents on the account.
    pub launcher: String,
    pub provider: String,
    /// The account as the agent picker names it.
    pub label: String,
    pub signed_in: bool,
    /// Its plan and rolling windows as its CLI last reported them, however long ago.
    pub usage: Option<sikemux_agents::agent_accounts::usage::Usage>,
    /// When that reading was taken, in Unix milliseconds.
    pub read_at: Option<u64>,
    /// Why the latest lookup found no reading.
    pub problem: Option<String>,
    /// Whether smart accounts would start the agent's next new chat on it.
    #[serde(default)]
    pub next: bool,
}

pub const READING_STALE_AFTER_MS: u64 = 15 * 60 * 1000;

impl AccountUsage {
    /// Whether its numbers are old news at `now_ms`: the last lookup failed, or the reading
    /// is well past the next one due.
    pub fn is_stale(&self, now_ms: u64) -> bool {
        self.problem.is_some()
            || self
                .read_at
                .is_some_and(|read| now_ms.saturating_sub(read) >= READING_STALE_AFTER_MS)
    }
}

/// An agent Sikemux can run, as this host has it on the account the person chose for it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    pub command: String,
    pub available: bool,
    pub error: Option<String>,
    /// The programs passed over on the way to the one found.
    pub warning: Option<String>,
    pub profile_id: Option<String>,
    pub config_path: Option<String>,
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
    pub status: sikemux_agents::agent_accounts::status::AgentStatus,
    /// The release of its CLI installed here, when the CLI names one.
    #[serde(default)]
    pub version: Option<String>,
    /// The releases its chats run now, newest first.
    #[serde(default)]
    pub running: Vec<String>,
    /// The newest release out.
    #[serde(default)]
    pub latest: Option<String>,
    /// How an update the person asked for goes.
    #[serde(default)]
    pub updating: Option<Updating>,
    /// The models updating to the newest release lets it run, by name.
    #[serde(default)]
    pub unlocks: Vec<String>,
}
