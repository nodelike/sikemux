use std::fmt;

use serde::{Deserialize, Serialize};
use sikemux_wire::protocol::AgentState;

use super::agent_kind::AgentKind;

impl AgentKind {
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.label() == label)
    }
}

impl Serialize for AgentKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for AgentKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let label = String::deserialize(deserializer)?;
        Self::from_label(&label)
            .ok_or_else(|| serde::de::Error::custom(format!("no agent is called {label}")))
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentDetectionState {
    Unknown,
    Idle,
    Working,
    Blocked,
}

impl From<AgentDetectionState> for AgentState {
    fn from(state: AgentDetectionState) -> Self {
        match state {
            AgentDetectionState::Unknown => AgentState::Unknown,
            AgentDetectionState::Idle => AgentState::Idle,
            AgentDetectionState::Working => AgentState::Working,
            AgentDetectionState::Blocked => AgentState::Blocked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DetectionConfidence {
    Authoritative,
    Strong,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestSourceKind {
    Bundled,
    LocalOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSourceInfo {
    pub kind: ManifestSourceKind,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatcherEvidence {
    pub pattern: String,
    pub matched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleEvidence {
    pub contains: Vec<MatcherEvidence>,
    pub regex: Vec<MatcherEvidence>,
    pub line_regex: Vec<MatcherEvidence>,
    pub all_gate_count: usize,
    pub all_gate_matches: usize,
    pub any_gate_count: usize,
    pub any_gate_matches: usize,
    pub not_gate_count: usize,
    pub not_gate_matches: usize,
    pub region_bytes: usize,
    pub region_preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedRule {
    pub id: String,
    pub priority: i32,
    pub region: String,
    pub state: AgentDetectionState,
    pub matched: bool,
    pub evidence: RuleEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionExplain {
    pub agent: AgentKind,
    pub state: AgentDetectionState,
    pub source: ManifestSourceInfo,
    pub confidence: DetectionConfidence,
    pub matched_rule: Option<String>,
    pub screen_detection_skipped: bool,
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
    pub skip_state_update: bool,
    pub fallback_reason: Option<String>,
    pub evaluated_rules: Vec<EvaluatedRule>,
    pub warning: Option<String>,
    pub manifest_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSummary {
    pub agent: AgentKind,
    pub version: String,
    pub source: ManifestSourceInfo,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestReloadReport {
    pub manifests: Vec<ManifestSummary>,
    pub warnings: Vec<String>,
}
