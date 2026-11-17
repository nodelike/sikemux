//! Work between people: a project shared with an org, and what its teammates see of it. Across
//! people only a member's name, picture and membership id travel.

use serde::{Deserialize, Serialize};

/// Someone in an org, as their teammates see them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Teammate {
    /// Their membership id in the org.
    pub member: String,
    pub name: String,
    /// The address of their account's picture, when they have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// Whether this is the person themselves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub you: bool,
}

/// Who a project's board is shared with, and who it could be.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSharing {
    pub project: String,
    /// The org its board is shared with; none while only its owner sees it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<SharedWith>,
    /// The orgs it could be shared with: those the account is in whose Team plan is paid.
    pub orgs: Vec<OrgChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWith {
    pub org: String,
    pub name: String,
    /// Whether teammates read it now. Off while the org's Team plan is unpaid: the share is kept
    /// and nothing is served until it is paid again.
    pub on: bool,
    /// Everyone in the org, the person included.
    pub people: Vec<Teammate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgChoice {
    pub org: String,
    pub name: String,
    /// How many people are in it, the person included.
    pub people: u32,
}
