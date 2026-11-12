//! What a driver and the chat's feed both name.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How far into its agent CLI's own record of a session a chat's history reaches, so a resume
/// replays only what the CLI added since.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reach {
    /// The length of a transcript file.
    Bytes(u64),
    /// The latest turn of a thread.
    Turn(String),
}

/// Reads further back into an agent CLI's own record of a session than the chat holds, a part
/// at a time: the updates it tells, oldest first, until nothing earlier is left.
pub type Earlier = Box<dyn FnMut() -> Result<Option<Vec<(String, Value)>>, String> + Send>;

const MAX_TITLE_CHARS: usize = 256;

/// A title as the chat shows it: one line of bounded length without markdown's marks or the
/// `[Image #n]` tags that stand for attached pictures, or none when blank. Text that is only a
/// code block keeps its characters.
pub fn title(text: &str) -> Option<String> {
    let text = &without_image_tags(text);
    let words = sikemux_markdown::words(text);
    let text = if words.trim().is_empty() {
        text
    } else {
        &words
    };
    let title: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    (!title.is_empty()).then_some(title)
}

pub fn without_image_tags(text: &str) -> String {
    const OPEN: &str = "[Image #";
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(OPEN) {
        let after = &rest[at + OPEN.len()..];
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let tag = digits > 0 && after[digits..].starts_with(']');
        let end = at + OPEN.len() + if tag { digits + 1 } else { 0 };
        kept.push_str(&rest[..if tag { at } else { end }]);
        rest = &rest[end..];
    }
    kept.push_str(rest);
    kept
}
