use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LspPos {
    pub line: u32,
    pub character: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LspRange {
    pub start: LspPos,
    pub end: LspPos,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LspTextChange {
    pub range: Option<LspRange>,
    #[serde(rename = "rangeLength")]
    pub range_length: Option<u32>,
    pub text: String,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LspDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnostic {
    pub range: LspRange,
    pub severity: Option<LspDiagnosticSeverity>,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsPayload {
    pub project: String,
    pub language: String,
    pub path: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: u32,
    pub range: LspRange,
    pub selection_range: LspRange,
    pub children: Vec<LspDocumentSymbol>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum LspKind {
    Definition,
    Declaration,
    TypeDefinition,
    Implementation,
    References,
}
