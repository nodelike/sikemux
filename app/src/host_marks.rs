//! How a code host's pull requests and issues read where the app mentions one: the core's
//! answer about them, and their state as a glyph in its colour.

use gpui_kit::{Hsla, SharedString};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_plugin_api::codehost::PullState;

use crate::icons::Icon;
use crate::theme::Tokens;

/// The value inside a code-host answer, or what its failure said.
pub fn answer<T: DeserializeOwned>(value: &Value) -> Result<T, SharedString> {
    if let Some(failed) = value.get("failed") {
        return Err(failed["message"]
            .as_str()
            .unwrap_or("the code host did not answer")
            .to_owned()
            .into());
    }
    serde_json::from_value(value["value"].clone()).map_err(|error| error.to_string().into())
}

/// A pull request's state as a glyph, its colour and its name: green while open, red once
/// closed, purple once merged, grey as a draft.
pub fn pull_look(state: PullState, t: &Tokens) -> (Icon, Hsla, &'static str) {
    match state {
        PullState::Open => (Icon::PullRequest, t.live, "Open"),
        PullState::Draft => (Icon::PullRequest, t.text_tertiary, "Draft"),
        PullState::Closed => (Icon::PullRequest, t.danger, "Closed"),
        PullState::Merged => (Icon::Merge, t.accent, "Merged"),
    }
}

/// An issue's state as a glyph, its colour and its name.
pub fn issue_look(state: &str, reason: Option<&str>, t: &Tokens) -> (Icon, Hsla, &'static str) {
    match (state, reason) {
        ("open", _) => (Icon::Issues, t.live, "Open"),
        (_, Some("not_planned")) => (
            Icon::MarkCancelled,
            t.text_tertiary,
            "Closed as not planned",
        ),
        _ => (Icon::Check, t.accent, "Closed"),
    }
}
