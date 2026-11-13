use serde::{Deserialize, Serialize};
use sikemux_agents::agent_accounts::Book;

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
