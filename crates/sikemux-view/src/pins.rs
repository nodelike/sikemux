//! Pins: a note the person leaves on one spot of the product in a desk's page. A pin stays on
//! the page until the agent it went to answers it, and opens again with another note.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub id: String,
    pub project: String,
    /// The agent the pin went to, whose desk it was placed on.
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// The page's address and title when the pin was placed.
    pub url: String,
    pub title: String,
    /// The size the page was laid out at.
    pub width: u32,
    pub height: u32,
    /// What was pinned, in words: "Export button".
    pub label: String,
    /// Where the element is, for the page to find it again.
    pub selector: String,
    /// A picture of the spot when it was placed, and once answered, after; paths on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// The commit the page ran from, when its checkout said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// What the person and the agent said on it, oldest first.
    pub notes: Vec<PinNote>,
    pub state: PinState,
    /// It was answered once and the person wrote again.
    #[serde(default)]
    pub reopened: bool,
    /// When it came to stand as it does: sent, answered or closed.
    pub since: u64,
    /// The agent is at work on it now. Worked out when the view is sent, never kept.
    #[serde(default)]
    pub working: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinNote {
    pub by: PinBy,
    pub words: String,
    pub at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PinBy {
    Person,
    Agent,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PinState {
    /// Sent to its agent and not yet answered.
    Open,
    /// The agent's turn after the pin ended, with what it said.
    Answered,
    /// The person said it looks right.
    Closed,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// A pin the person places: the spot on the page, and their note.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacePinParams {
    pub agent_id: String,
    pub url: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub label: String,
    pub selector: String,
    /// The element's own markup, cut short, for the agent alone.
    #[serde(default)]
    pub html: String,
    /// A picture of the spot, a JPEG on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacedPin {
    pub id: String,
}

/// Another note on a pin, which opens it again and goes to the same agent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinNoteParams {
    pub id: String,
    pub words: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinParams {
    pub id: String,
}

/// The picture of an answered pin's spot, taken once the answering turn ended.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinAfterParams {
    pub id: String,
    pub after: String,
}
