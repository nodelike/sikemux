use serde::{Deserialize, Serialize};
use sikemux_wire::protocol::ChatContext;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerParams {
    pub id: String,
    #[serde(default)]
    pub answer: Option<String>,
    /// Files attached to `answer`, by where the host keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// What was sent with `answer`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ChatContext>,
    #[serde(default)]
    pub choice: Option<usize>,
    /// The choices picked on a question that takes several.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<usize>>,
    /// The assumption the agent went on is right.
    #[serde(default)]
    pub fine: bool,
    /// The person has none of what a question for a secret asks for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub none: bool,
}

#[derive(Serialize, Deserialize)]
pub struct AnswersParams {
    pub answers: Vec<AnswerParams>,
}
