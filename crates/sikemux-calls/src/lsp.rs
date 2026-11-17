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
pub struct LspTextChange {
    pub range: Option<LspRange>,
    #[serde(rename = "rangeLength")]
    pub range_length: Option<u32>,
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LspDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnostic {
    pub range: LspRange,
    pub severity: Option<LspDiagnosticSeverity>,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsPayload {
    pub project: String,
    pub language: String,
    pub path: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: u32,
    pub range: LspRange,
    pub selection_range: LspRange,
    pub children: Vec<LspDocumentSymbol>,
}

#[derive(Serialize, Deserialize)]
pub struct LspServer {
    pub project: String,
    pub language: String,
}

/// A file open with a language server, named as the core keeps it.
#[derive(Serialize, Deserialize, Clone)]
pub struct LspDocument {
    pub project: String,
    pub language: String,
    pub path: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspOpen {
    #[serde(flatten)]
    pub document: LspDocument,
    pub content: String,
    pub language_id: Option<String>,
}

/// What a window knows its hold on an open file by, to claim it back after reconnecting.
#[derive(Serialize, Deserialize)]
pub struct LspOpened {
    pub handle: String,
}

#[derive(Serialize, Deserialize)]
pub struct LspChange {
    #[serde(flatten)]
    pub document: LspDocument,
    pub content: String,
    pub version: u32,
}

#[derive(Serialize, Deserialize)]
pub struct LspIncrementalChange {
    #[serde(flatten)]
    pub document: LspDocument,
    pub changes: Vec<LspTextChange>,
    pub version: u32,
}

#[derive(Serialize, Deserialize)]
pub struct LspSave {
    #[serde(flatten)]
    pub document: LspDocument,
    pub content: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct LspLanguage {
    pub language: String,
}

#[derive(Serialize, Deserialize)]
pub struct LspProject {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
pub struct LspPath {
    pub path: String,
}

#[derive(Serialize, Deserialize)]
pub struct LspClaim {
    pub handles: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum LspKind {
    Definition,
    Declaration,
    TypeDefinition,
    Implementation,
    References,
}

#[derive(Serialize, Deserialize)]
pub struct LspLocations {
    #[serde(flatten)]
    pub document: LspDocument,
    pub line: u32,
    pub character: u32,
    pub kind: LspKind,
}
