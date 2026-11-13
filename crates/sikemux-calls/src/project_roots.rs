use serde::{Deserialize, Serialize};
use sikemux_view::prefs::ProjectRoot;

#[derive(Serialize, Deserialize, PartialEq)]
pub struct RootEntry {
    pub name: String,
    pub path: String,
}

/// What is at a path as the person typed it, `~` and all.
#[derive(Serialize, Deserialize)]
pub struct Resolved {
    pub path: String,
    pub kind: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ResolveParams {
    pub path: String,
}

#[derive(Serialize, Deserialize)]
pub struct ScanParams {
    pub roots: Vec<ProjectRoot>,
}
