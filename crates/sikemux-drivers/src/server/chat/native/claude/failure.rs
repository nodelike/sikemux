//! What Claude Code writes in place of an answer when a turn cannot run: a
//! message from its `<synthetic>` model asking for a sign-in or saying the
//! account is out of usage.

use serde_json::Value;

use super::super::json::text;
pub(super) use sikemux_agents::acp::failure::out_of_usage;

/// The model Claude Code names on a message it wrote itself.
pub(super) const SYNTHETIC_MODEL: &str = "<synthetic>";

/// Whether Claude Code wrote `message`, the inner message of a record, in
/// place of an answer.
pub(super) fn is_synthetic(message: &Value) -> bool {
    text(message, "model") == Some(SYNTHETIC_MODEL)
}

pub(super) fn asks_sign_in(said: &str) -> bool {
    said.contains("Please run /login")
}
