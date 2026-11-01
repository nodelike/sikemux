//! How a code host's pull requests and issues read where the app mentions one: their state as
//! a glyph in its colour.

use gpui_kit::Hsla;
use sikemux_plugin_api::codehost::PullState;

use crate::icons::Icon;
use crate::theme::Tokens;

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
