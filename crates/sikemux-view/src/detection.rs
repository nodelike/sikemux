//! Agent detection manifest reports that clients deserialize from diagnostics calls.

use serde::{Deserialize, Serialize};

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
pub struct ManifestSummary {
    pub agent: String,
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
