//! Starting work: how the start box pre-picks the way to go about what the
//! person types, which the person can always change before it starts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartMode {
    /// One agent takes it, and the project's usual checks say when it is done.
    HandOff,
    /// A lead talks it through with the person first, and it goes on in
    /// slices they approve.
    PlanFirst,
    /// A chat with the person, as today, still kept as a goal. What the start
    /// box opens on, and what triage picks when nothing calls for planning.
    #[default]
    Chat,
}

/// Something the text touches that calls for planning first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HardFact {
    Server,
    Migration,
    /// The protocol between the core and its clients, or another wire format.
    Protocol,
    /// What the people using the product see.
    SeenByUsers,
    /// A path the project's sikemux.json lists under `planFirst`.
    Listed,
}

/// The mode pre-picked for what the person typed, and what decided it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Triage {
    pub mode: StartMode,
    /// The fact that called for planning first, and the path in the text that
    /// showed it; none when nothing did.
    #[serde(default)]
    pub because: Option<Because>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Because {
    pub fact: HardFact,
    /// The project's path, as the text names it.
    pub path: String,
}
