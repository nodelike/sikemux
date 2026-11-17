//! A project's drift audits as clients show them: how often one runs, and what the latest found.

use serde::{Deserialize, Serialize};

use crate::chat::TriggerWhen;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditView {
    /// When audits run; none runs none.
    #[serde(default)]
    pub every: Option<TriggerWhen>,
    #[serde(default)]
    pub next_at: Option<u64>,
    #[serde(default)]
    pub latest: Option<AuditDigest>,
}

/// What one audit found: the problems it filed as cards, and how many found before were fixed since.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditDigest {
    /// The audit's own card, which its findings sit under.
    pub card: u64,
    pub at: u64,
    #[serde(default)]
    pub agent_name: Option<String>,
    /// Its agent is still reading.
    pub running: bool,
    /// How many commits landed since the audit before.
    pub commits: u32,
    /// Since the audit before, or none for the first.
    #[serde(default)]
    pub since: Option<u64>,
    pub findings: Vec<AuditFinding>,
    /// Findings of earlier audits done since the audit before.
    pub fixed: u32,
    /// What its agent said of the codebase as a whole.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditFinding {
    pub card: u64,
    pub title: String,
    pub done: bool,
}
