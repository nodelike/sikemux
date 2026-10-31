//! Starting work: how the start box pre-picks the way to go about what the
//! person types, which the person can always change before it starts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartMode {
    /// One agent does it, and the project's usual checks say when it is done.
    DoIt,
    /// A lead talks it through with the person first, and it goes on in
    /// slices they approve.
    PlanFirst,
    /// A chat with the person, as today, still kept as a goal.
    WithMe,
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
}

/// The mode pre-picked for what the person typed, and what decided it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Triage {
    pub mode: StartMode,
    /// The fact that called for more than doing it at once, and the path in
    /// the text that showed it; none when nothing did.
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
