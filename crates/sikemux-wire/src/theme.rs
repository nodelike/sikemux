//! A colour theme: the app's colours, the code editor's, syntax highlighting
//! and the terminal's, each a CSS colour. The core resolves the person's
//! choice to one of these; clients draw with it.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub chrome: ThemeChrome,
    pub editor: ThemeEditor,
    pub highlight: ThemeHighlight,
    pub terminal: ThemeTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeChrome {
    pub bg: String,
    pub bg_dim: String,
    pub bg_raised: String,
    pub ink: String,
    pub ink_dim: String,
    pub ink_muted: String,
    pub acc: String,
    pub acc_line: String,
    pub acc_dim: String,
    pub line: String,
    pub hl: String,
    pub danger: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeEditor {
    pub fg: String,
    pub bg: String,
    pub caret: String,
    pub selection: String,
    pub active_line: String,
    pub gutter: String,
    pub gutter_active: String,
    pub indent: String,
    pub indent_active: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeHighlight {
    pub keyword: String,
    pub string: String,
    pub comment: String,
    pub number: String,
    pub function: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub variable: String,
    pub property: String,
    pub tag: String,
    pub operator: String,
    pub link: String,
    pub invalid: String,
    pub meta: String,
    pub heading: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeTerminal {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub cursor_accent: String,
    pub selection_background: String,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_magenta: String,
    pub bright_cyan: String,
    pub bright_white: String,
}

/// A theme as a picker lists it: its name, where it comes from and its colours at a glance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSummary {
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub source: ThemeSource,
    pub bg: String,
    pub line: String,
    pub acc: String,
    /// The terminal's red, green, yellow, blue, magenta and cyan.
    pub swatches: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeSource {
    /// One the person made, which they can edit and delete.
    Custom,
    Sikemux,
    Ghostty,
}

/// The picked theme with the colours every app draws with, worked out from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Palette {
    pub theme: Theme,
    pub tokens: UiTokens,
}

/// DESIGN.md's colours for one theme, each `#rrggbb`, or `rgba(r, g, b, a)`
/// when see-through. Phones read them by these names from the device view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiTokens {
    /// The darkest ground, behind panes.
    pub sunken: String,
    pub ground: String,
    pub raised: String,
    pub overlay: String,
    /// One step above the ground, where a phone's composer sits.
    pub composer: String,
    /// What the Mac's chat composer and its docked strips are filled with.
    pub surface_composer: String,
    pub hover: String,
    pub chosen: String,
    pub active: String,
    pub selected: String,
    pub tooltip: String,
    pub border: String,
    pub border_strong: String,
    pub border_selected: String,
    pub line: String,
    pub line_soft: String,
    pub ink: String,
    pub secondary: String,
    /// Text that recedes: labels, hints, timestamps.
    pub tertiary: String,
    pub disabled: String,
    pub ink_dim: String,
    pub ink_faint: String,
    pub rest: String,
    /// Header actions and other glyphs that recede until hovered.
    pub icon_quiet: String,
    pub accent: String,
    pub accent_soft: String,
    pub on_accent: String,
    /// Running, connected, passing.
    pub live: String,
    /// Waiting on the person, or at risk.
    pub warn: String,
    pub danger: String,
    /// Commands and the names of things that run.
    pub cmd: String,
    /// The mark of a host on the account.
    pub host: String,
    /// An SSH host's mark.
    pub ssh: String,
    pub tree_spine: String,
    pub tree_tick: String,
    pub git_added: String,
    pub git_modified: String,
    pub git_untracked: String,
    pub git_deleted: String,
    pub git_renamed: String,
    /// What each kind of tool call is marked with in a chat.
    pub tool_read: String,
    pub tool_edit: String,
    pub tool_delete: String,
    pub tool_run: String,
    pub tool_mcp: String,
    /// The dots of the pane grain behind a phone's panes.
    pub shader_dot: String,
}
