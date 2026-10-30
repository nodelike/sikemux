//! The agents' built-in tools and the guide they read, as this build declares
//! them. The core serves them to agents' tool servers, so a tool server that
//! outlived an update still offers what the running Sikemux handles.

use std::sync::LazyLock;

use serde_json::{json, Value};

pub const TOOLS: &str = include_str!("../../../../browser/tools.json");
pub const GUIDE: &str = include_str!("../../../../browser/SIKEMUX_GUIDE.md");

static SERVED: LazyLock<Result<Value, String>> = LazyLock::new(|| {
    let manifest: Value = serde_json::from_str(TOOLS).map_err(|error| error.to_string())?;
    Ok(json!({ "manifest": manifest, "guide": GUIDE }))
});

/// What `tools.manifest` answers.
pub fn served() -> Result<Value, String> {
    SERVED.clone()
}
