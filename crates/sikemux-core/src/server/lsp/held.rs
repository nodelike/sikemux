//! The documents clients hold open in language servers, each by a handle,
//! with the text the server has. A client that goes away leaves its documents
//! open a while so it can claim them back, and a hand-over carries them all
//! to the new core, which opens them again.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::super::connection::ClientId;
use super::types::{LspPos, LspTextChange};

/// A document as a language server knows it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct DocKey {
    pub project: String,
    pub language: String,
    pub path: String,
}

struct Hold {
    key: DocKey,
    /// None while the client that held it is away.
    client: Option<ClientId>,
}

struct Text {
    language_id: String,
    text: String,
}

#[derive(Default)]
struct Held {
    holds: HashMap<String, Hold>,
    texts: HashMap<DocKey, Text>,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(Default::default)
}

/// A document carried through a hand-over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Carried {
    pub handle: String,
    #[serde(flatten)]
    pub key: DocKey,
    pub language_id: String,
    pub text: String,
}

/// Notes that `client` opened `key` with `text`, answering the handle it
/// claims the document back by.
pub(crate) fn hold(client: ClientId, key: DocKey, language_id: String, text: String) -> String {
    let handle = crate::cli::auth::new_nonce();
    let mut held = held().locked();
    held.texts.insert(key.clone(), Text { language_id, text });
    held.holds.insert(
        handle.clone(),
        Hold {
            key,
            client: Some(client),
        },
    );
    handle
}

/// The whole text the server now has for `key`.
pub(crate) fn set_text(key: &DocKey, text: String) {
    if let Some(kept) = held().locked().texts.get_mut(key) {
        kept.text = text;
    }
}

/// Applies edits the server was sent. One that does not fit the kept text
/// drops it, so the document is not carried with text the server never had.
pub(crate) fn edit(key: &DocKey, changes: &[LspTextChange]) {
    let mut held = held().locked();
    let Some(kept) = held.texts.get_mut(key) else {
        return;
    };
    if !changes.iter().all(|change| apply(&mut kept.text, change)) {
        held.texts.remove(key);
    }
}

fn apply(text: &mut String, change: &LspTextChange) -> bool {
    let Some(range) = &change.range else {
        text.clone_from(&change.text);
        return true;
    };
    match (offset(text, &range.start), offset(text, &range.end)) {
        (Some(start), Some(end)) if start <= end => {
            text.replace_range(start..end, &change.text);
            true
        }
        _ => false,
    }
}

/// The byte a position names, its character counted in UTF-16 units as LSP
/// counts them; a character past the line's end is its end.
fn offset(text: &str, position: &LspPos) -> Option<usize> {
    let mut line_start = 0;
    for _ in 0..position.line {
        line_start += text[line_start..].find('\n')? + 1;
    }
    let line = &text[line_start..];
    let line_end = line.find('\n').unwrap_or(line.len());
    let mut units = 0;
    for (at, character) in line[..line_end].char_indices() {
        if units >= position.character {
            return Some(line_start + at);
        }
        units += character.len_utf16() as u32;
    }
    Some(line_start + line_end)
}

/// Lets go of one of `client`'s holds on `key`, dropping the text once nobody
/// holds it.
pub(crate) fn let_go(client: ClientId, key: &DocKey) {
    let mut held = held().locked();
    let handle = held
        .holds
        .iter()
        .find(|(_, hold)| hold.client == Some(client) && hold.key == *key)
        .map(|(handle, _)| handle.clone());
    if let Some(handle) = handle {
        held.holds.remove(&handle);
    }
    forget_unheld(&mut held, key);
}

fn forget_unheld(held: &mut Held, key: &DocKey) {
    if !held.holds.values().any(|hold| hold.key == *key) {
        held.texts.remove(key);
    }
}

/// Lets go of every document `client` holds, answering one key per hold.
pub(crate) fn leave(client: ClientId) -> Vec<DocKey> {
    take(|hold| hold.client == Some(client))
}

/// Marks `client`'s documents as waiting for it to come back, answering
/// their handles.
pub(crate) fn orphan(client: ClientId) -> Vec<String> {
    let mut held = held().locked();
    held.holds
        .iter_mut()
        .filter(|(_, hold)| hold.client == Some(client))
        .map(|(handle, hold)| {
            hold.client = None;
            handle.clone()
        })
        .collect()
}

/// Lets go of those of `handles` nobody claimed back, answering one key per
/// hold.
pub(crate) fn expire(handles: &[String]) -> Vec<DocKey> {
    let mut held = held().locked();
    let expired: Vec<DocKey> = handles
        .iter()
        .filter_map(|handle| match held.holds.get(handle) {
            Some(hold) if hold.client.is_none() => held.holds.remove(handle).map(|hold| hold.key),
            _ => None,
        })
        .collect();
    for key in &expired {
        forget_unheld(&mut held, key);
    }
    expired
}

fn take(which: impl Fn(&Hold) -> bool) -> Vec<DocKey> {
    let mut held = held().locked();
    let handles: Vec<String> = held
        .holds
        .iter()
        .filter(|(_, hold)| which(hold))
        .map(|(handle, _)| handle.clone())
        .collect();
    let keys: Vec<DocKey> = handles
        .iter()
        .filter_map(|handle| held.holds.remove(handle).map(|hold| hold.key))
        .collect();
    for key in &keys {
        forget_unheld(&mut held, key);
    }
    keys
}

/// Gives `client` those of `handles` that wait for a client, answering the
/// ones it now holds.
pub(crate) fn claim(client: ClientId, handles: &[String]) -> Vec<String> {
    let mut held = held().locked();
    handles
        .iter()
        .filter(|handle| match held.holds.get_mut(*handle) {
            Some(hold) if hold.client.is_none() => {
                hold.client = Some(client);
                true
            }
            _ => false,
        })
        .cloned()
        .collect()
}

/// Every held document whose text is known, for a hand-over.
pub(crate) fn carry() -> Vec<Carried> {
    let held = held().locked();
    held.holds
        .iter()
        .filter_map(|(handle, hold)| {
            let kept = held.texts.get(&hold.key)?;
            Some(Carried {
                handle: handle.clone(),
                key: hold.key.clone(),
                language_id: kept.language_id.clone(),
                text: kept.text.clone(),
            })
        })
        .collect()
}

/// Takes documents a hand-over carried, waiting for their clients.
pub(crate) fn adopt(carried: &[Carried]) {
    let mut held = held().locked();
    for document in carried {
        held.texts.insert(
            document.key.clone(),
            Text {
                language_id: document.language_id.clone(),
                text: document.text.clone(),
            },
        );
        held.holds.insert(
            document.handle.clone(),
            Hold {
                key: document.key.clone(),
                client: None,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::lsp::types::LspRange;

    fn change(start: (u32, u32), end: (u32, u32), text: &str) -> LspTextChange {
        LspTextChange {
            range: Some(LspRange {
                start: LspPos {
                    line: start.0,
                    character: start.1,
                },
                end: LspPos {
                    line: end.0,
                    character: end.1,
                },
            }),
            range_length: None,
            text: text.into(),
        }
    }

    #[test]
    fn edits_count_characters_in_utf16_units_as_lsp_does() {
        let mut text = "a🙂b\nsecond".to_owned();
        assert!(apply(&mut text, &change((0, 3), (0, 4), "B")));
        assert_eq!(text, "a🙂B\nsecond");
        assert!(apply(&mut text, &change((1, 0), (1, 6), "2nd")));
        assert_eq!(text, "a🙂B\n2nd");
        assert!(!apply(&mut text, &change((5, 0), (5, 1), "x")));
    }
}
