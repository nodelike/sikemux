//! What a client asks of a terminal: input, and finding text on it.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseAction {
    Press,
    Release,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    None,
}

/// What a person does to a terminal. The core turns it into the bytes the
/// program expects under the modes it turned on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalInput {
    /// A key that is not plain text, by its `KeyboardEvent.key` name, or a
    /// character typed with control, option or command held.
    Key {
        key: String,
        #[serde(default)]
        mods: Mods,
    },
    /// Text typed, or composed by an input method.
    Text {
        text: String,
    },
    Paste {
        text: String,
    },
    /// Files dropped on the terminal, pasted as words the person's shell
    /// reads back as those paths.
    Paths {
        paths: Vec<String>,
    },
    Mouse {
        action: MouseAction,
        button: MouseButton,
        col: u16,
        row: u16,
        #[serde(default)]
        mods: Mods,
    },
    /// The wheel turned by `lines`, negative upward, over a cell.
    Wheel {
        col: u16,
        row: u16,
        lines: i32,
        #[serde(default)]
        mods: Mods,
    },
    Focus {
        focused: bool,
    },
}

/// A cell by line number, as clients number lines, and column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GridPoint {
    pub line: u64,
    pub col: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindOptions {
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub whole_word: bool,
}

/// The matches of a search, first to last, and which one is current.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindResult {
    pub epoch: u32,
    /// Index into `matches` of the first match after where the search
    /// started, or before it when searching backwards.
    pub current: Option<usize>,
    /// Each match's first and last cell.
    pub matches: Vec<(GridPoint, GridPoint)>,
    /// There were more matches than `matches` holds.
    pub more: bool,
}
