//! What Claude Code writes in place of an answer when a turn cannot run: a
//! message from its `<synthetic>` model asking for a sign-in or saying the
//! account is out of usage.

use serde_json::Value;

use super::super::json::text;

/// The model Claude Code names on a message it wrote itself.
pub(super) const SYNTHETIC_MODEL: &str = "<synthetic>";

/// Text Claude Code shows instead of an answer when the account is out of
/// usage, taken from the Agent SDK.
const USAGE_LIMIT_PREFIXES: [&str; 12] = [
    "You've hit your",
    "You've reached your",
    "You're out of usage credits",
    "Your org is out of usage · add funds to continue",
    "Your org is out of usage · contact your admin",
    "Your seat type doesn't include usage credits",
    "Your seat type doesn't include usage",
    "Your usage allocation has been disabled by your admin",
    "Your group's usage limit is set to $0",
    "Fable 5 requires usage credits",
    "You're out of extra usage",
    "Your seat type doesn't include extra usage",
];

/// Whether Claude Code wrote `message`, the inner message of a record, in
/// place of an answer.
pub(super) fn is_synthetic(message: &Value) -> bool {
    text(message, "model") == Some(SYNTHETIC_MODEL)
}

/// Whether what Claude said asks the person to sign in.
pub(super) fn asks_sign_in(said: &str) -> bool {
    said.contains("Please run /login")
}

/// Whether what Claude said is that the account is out of usage.
pub(super) fn out_of_usage(said: &str) -> bool {
    let said = said.trim_start();
    USAGE_LIMIT_PREFIXES
        .iter()
        .any(|prefix| said.starts_with(prefix))
}
