use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerParams {
    pub trigger: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldTriggerParams {
    pub trigger: u64,
    /// True pauses it, false resumes it.
    pub paused: bool,
}

/// A trigger the person sets up themselves.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddTriggerParams {
    /// What its row is called; the first line of `prompt` when none is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// What the chat is told each time it fires.
    pub prompt: String,
    pub when: TriggerOn,
    pub wakes: TriggerWakes,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TriggerOn {
    Every {
        seconds: u64,
    },
    /// Each day at `minute` past midnight on the host's clock, on `weekdays` counted from Sunday
    /// as 0, or on every day when there are none.
    Daily {
        minute: u32,
        weekdays: Vec<u8>,
    },
    /// Once, at `at` in Unix milliseconds.
    Once {
        at: u64,
    },
    /// On each request to an address the core makes for it.
    Webhook,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TriggerWakes {
    /// An open chat, told each time it fires.
    Chat { agent: String },
    /// A new chat in the project, started each time it fires, by its absolute path.
    Project { project: String },
}
