//! Chat agents spoken to over the Agent Client Protocol: what the core sends
//! them and how it reads what they send back. The connections themselves run
//! in the server.

pub mod air;
pub mod native;
pub mod sign_in;

use serde_json::Value;

pub use agent_client_protocol::schema;

use crate::agent_catalog::{self, ChatDriver};
use crate::launch::PermissionMode;

/// The session mode that carries a permission mode. Agents whose modes are not
/// about permissions get none, and Sikemux answers their requests itself.
pub fn permission_mode_id(
    provider: &str,
    mode: PermissionMode,
    setup: &Value,
) -> Result<Option<&'static str>, String> {
    let Some(ChatDriver::Acp { modes, .. }) =
        agent_catalog::agent(provider).and_then(|agent| agent.chat.as_ref())
    else {
        return Err(format!("{provider} does not chat over ACP"));
    };
    let Some(modes) = modes else {
        return Ok(None);
    };
    let expected = match mode {
        PermissionMode::Bypass => modes.bypass,
        PermissionMode::WorkspaceWrite => modes.normal,
    };
    setup
        .pointer("/modes/availableModes")
        .and_then(Value::as_array)
        .and_then(|modes| {
            modes
                .iter()
                .filter_map(|mode| mode.get("id").and_then(Value::as_str))
                .find(|id| *id == expected)
        })
        .map(|_| Some(expected))
        .ok_or_else(|| format!("The {provider} adapter does not offer permission mode {expected}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permissions_use_advertised_provider_modes() {
        let setup = json!({ "modes": { "availableModes": [{ "id": "accept_edits" }, { "id": "dont_ask" }] } });
        assert_eq!(
            permission_mode_id("hermes", PermissionMode::WorkspaceWrite, &setup).unwrap(),
            Some("accept_edits")
        );
        assert_eq!(
            permission_mode_id("hermes", PermissionMode::Bypass, &setup).unwrap(),
            Some("dont_ask")
        );
        assert!(permission_mode_id("hermes", PermissionMode::Bypass, &json!({})).is_err());
    }

    #[test]
    fn agents_without_permission_modes_are_answered_by_the_host() {
        for provider in ["opencode", "omp", "grok"] {
            assert_eq!(
                permission_mode_id(provider, PermissionMode::Bypass, &json!({})).unwrap(),
                None
            );
            assert_eq!(
                permission_mode_id(provider, PermissionMode::WorkspaceWrite, &json!({})).unwrap(),
                None
            );
        }
    }
}
