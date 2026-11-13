use serde::{Deserialize, Serialize};
pub use sikemux_calls::lsp::{
    LspChange, LspClaim, LspDiagnostic, LspDiagnosticSeverity, LspDiagnosticsPayload, LspDocument,
    LspDocumentSymbol, LspIncrementalChange, LspLanguage, LspOpen, LspOpened, LspPath, LspPos,
    LspProject, LspRange, LspSave, LspServer, LspTextChange,
};

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
