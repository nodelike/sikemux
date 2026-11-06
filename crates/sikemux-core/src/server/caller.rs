//! Who called an agent tool, and what its params say, for every tool alike.

use serde_json::Value;

use crate::cli::protocol::HarnessRequest;
use std::path::Path;

use super::Core;

pub(crate) fn agent_of(request: &HarnessRequest, tool: &str) -> Result<String, String> {
    request
        .agent_id
        .clone()
        .ok_or_else(|| format!("{tool} needs the calling agent's id"))
}

pub(crate) fn project_of(core: &Core, request: &HarnessRequest) -> String {
    request
        .agent_id
        .as_deref()
        .and_then(|agent_id| core.names.get(agent_id))
        .map(|named| named.project)
        .unwrap_or_else(|| core.workspaces.project_of(Path::new(&request.project)))
}

/// The param `key`, trimmed by the caller's choice; none when it is left out.
pub(crate) fn text_param<'a>(
    params: &'a Value,
    key: &str,
    max: usize,
) -> Result<Option<&'a str>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() && text.len() <= max => Ok(Some(text)),
        Some(Value::String(text)) if !text.trim().is_empty() => Err(format!(
            "{key} is {} bytes, more than the {max} it may hold",
            text.len()
        )),
        Some(_) => Err(format!("{key} must be nonempty text")),
    }
}
