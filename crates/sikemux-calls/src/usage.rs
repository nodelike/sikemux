use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;

/// A step a new install takes on its way to using Sikemux. Each is sent once per install.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "camelCase")]
pub enum Step {
    /// The first-run setup opened.
    SetupStarted,
    /// An agent here is installed and signed in.
    AgentReady { agent: String },
    /// The setup finished; `skipped` when it opened the app by itself, an agent being ready.
    SetupFinished { skipped: bool },
    /// The person sent a chat its first message.
    FirstMessage,
    /// The app was used on a UTC day after the one its setup started on.
    SecondDay,
    /// The app asked for a GitHub star.
    StarAsked,
    /// The person opened GitHub from that ask.
    StarOpened,
}

impl Step {
    pub fn event(&self) -> &'static str {
        match self {
            Step::SetupStarted => "setup_started",
            Step::AgentReady { .. } => "agent_ready",
            Step::SetupFinished { .. } => "setup_finished",
            Step::FirstMessage => "first_message",
            Step::SecondDay => "second_day",
            Step::StarAsked => "star_asked",
            Step::StarOpened => "star_opened",
        }
    }

    pub fn bit(&self) -> u8 {
        1 << match self {
            Step::SetupStarted => 0,
            Step::AgentReady { .. } => 1,
            Step::SetupFinished { .. } => 2,
            Step::FirstMessage => 3,
            Step::SecondDay => 4,
            Step::StarAsked => 5,
            Step::StarOpened => 6,
        }
    }

    pub fn properties(&self) -> Value {
        match self {
            Step::AgentReady { agent } => json!({ "agent": agent }),
            Step::SetupFinished { skipped } => json!({ "skipped": skipped }),
            _ => json!({}),
        }
    }
}
