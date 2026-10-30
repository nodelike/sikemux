//! The questions agents put to the person, as the app, the notch and phones show them.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionChoice {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The agent marked it as the one it would pick.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recommended: bool,
}

/// Where a question stands for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuestionState {
    /// It waits on the lead it went to first.
    Lead,
    /// The asker waits on the person's answer.
    Open,
    /// The asker went on with its assumption, which the person may correct.
    Assumed,
    /// The person answered it and the asker has not read the answer yet.
    Answered,
}

/// A question as the app, the notch and phones show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuestion {
    pub id: String,
    pub agent_id: String,
    /// The asker's name, such as Maple.
    pub name: String,
    pub cli: String,
    pub project: String,
    /// The asker's own words.
    pub asked: String,
    /// What the person is asked, which a lead that passed it on may have
    /// reworded.
    pub text: String,
    /// Two or three words naming the question, such as Photo access.
    pub header: Option<String>,
    pub choices: Vec<QuestionChoice>,
    /// The person may pick more than one of `choices`.
    pub multi: bool,
    /// The asker waits for the answer; otherwise it goes on with `assumption`.
    pub blocked: bool,
    pub assumption: Option<String>,
    /// The answer is a secret, such as a password: typed into a masked field,
    /// never the composer, and never shown again.
    pub secret: bool,
    pub at: u64,
    pub state: QuestionState,
    /// Why it reached the person rather than the lead it went to first, such
    /// as "Maple didn't know".
    pub via_note: Option<String>,
    /// What the field a secret is typed into says beside it.
    pub secret_note: Option<String>,
    /// The person's answer, while its state is answered; none for a secret.
    pub answer: Option<String>,
    /// The ticket of the board card the asker is working, such as PROJ-12.
    pub ticket: Option<String>,
}
