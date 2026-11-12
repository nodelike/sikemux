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
    /// What was pinned, in the page's own words: "Export button".
    pub label: String,
    /// Where the element is, for the page to find it again.
    pub selector: String,
    /// Pictures of the spot when placed and once answered, by id, fetched with `pinPicture`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// No after picture could be taken without the person: the next time they have the page in
    /// front, the app takes it from their own tab.
    #[serde(default)]
    pub after_by_hand: bool,
    /// The page is on a port its agent's checkout serves, so the after picture is taken in a
    /// private view off screen that sees nothing signed in.
    #[serde(default)]
    pub after_private: bool,
    /// The checkout's commit when the pin was placed.
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
    /// When the turn that took its last note began; only that turn's own end answers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    /// The turn that took it was stopped or failed, so nobody is at work on it.
    #[serde(default)]
    pub stopped: bool,
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
    /// The turn that took it ended by itself, with what the agent said.
    Answered,
    /// The person said it looks right, or its agent or project went.
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
    /// A picture of the spot from the person's own tab, a base64 JPEG.
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

/// The picture of an answered pin's spot, a base64 JPEG; none when it could not be taken
/// without the person.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinAfterParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// One of a pin's pictures, by the id the pin names it by.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinPictureParams {
    pub id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinPicture {
    pub mime_type: String,
    pub data: String,
}
