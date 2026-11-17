//! GIFs an agent asks for in its reply as `[gif: what it feels]`. The ask is cut out of the
//! text where it streams, and a GIF event takes its place, so the GIF shows there without the
//! agent making a tool call.

use std::collections::HashMap;

use serde_json::{json, Value};
use sikemux_fold::{ChatEventKind, UpdateKind};

const OPENS: &str = "[gif:";
const LONGEST_ASK: usize = 100;

/// What a reply's text is split into.
#[derive(Debug, PartialEq)]
pub enum Piece {
    Event(ChatEventKind, Value),
    /// A GIF the agent asked for in `session`.
    Ask {
        session: String,
        ask: String,
    },
}

/// The end of a reply chunk that may be the start of an ask, kept by session until the
/// next chunk says whether it is one.
#[derive(Default)]
pub struct Cut {
    held: HashMap<String, Value>,
}

impl Cut {
    pub fn take(&mut self, kind: ChatEventKind, payload: Value) -> Vec<Piece> {
        let Some((session, text)) = reply_text(kind, &payload) else {
            let mut pieces: Vec<Piece> = (self.held.drain())
                .map(|(_, held)| Piece::Event(ChatEventKind::SessionUpdate, held))
                .collect();
            pieces.push(Piece::Event(kind, payload));
            return pieces;
        };
        let earlier = self.held.remove(&session);
        if earlier.is_none() && !text.contains('[') {
            return vec![Piece::Event(kind, payload)];
        }
        let mut text = earlier
            .as_ref()
            .and_then(|held| held.pointer("/update/content/text")?.as_str())
            .unwrap_or_default()
            .to_owned()
            + &text;
        let with = |text: &str| {
            let mut payload = payload.clone();
            if let Some(slot) = payload.pointer_mut("/update/content/text") {
                *slot = json!(text);
            }
            payload
        };
        let mut pieces = Vec::new();
        while let Some((before, ask, after)) = split_ask(&text) {
            if !before.is_empty() {
                pieces.push(Piece::Event(kind, with(before)));
            }
            pieces.push(Piece::Ask {
                session: session.clone(),
                ask: ask.to_owned(),
            });
            text = after.to_owned();
        }
        let kept = unfinished_ask(&text);
        let (shown, kept) = text.split_at(kept);
        if !shown.is_empty() {
            pieces.push(Piece::Event(kind, with(shown)));
        }
        if !kept.is_empty() {
            self.held.insert(session, with(kept));
        }
        pieces
    }
}

/// The session and text of a chunk of the agent's reply.
fn reply_text(kind: ChatEventKind, payload: &Value) -> Option<(String, String)> {
    if kind != ChatEventKind::SessionUpdate
        || UpdateKind::within(payload) != Some(UpdateKind::AgentMessageChunk)
    {
        return None;
    }
    let text = payload.pointer("/update/content/text")?.as_str()?;
    let session = payload.get("sessionId")?.as_str()?;
    Some((session.to_owned(), text.to_owned()))
}

/// The text before the first whole ask, what it asks, and the text after it.
fn split_ask(text: &str) -> Option<(&str, &str, &str)> {
    let mut from = 0;
    while let Some(found) = text.get(from..)?.find(OPENS) {
        let start = from + found;
        let (head, opened) = text.split_at_checked(start)?;
        let inside = opened.get(OPENS.len()..)?;
        let end = inside
            .char_indices()
            .take_while(|(at, ch)| *at <= LONGEST_ASK && *ch != '\n')
            .find(|(_, ch)| *ch == ']')
            .map(|(at, _)| at);
        if let Some(end) = end {
            let ask = inside.get(..end)?.trim();
            if !ask.is_empty() {
                let after = inside.get(end + 1..)?;
                let before = head.strip_suffix(' ').unwrap_or(head);
                return Some((before, ask, after));
            }
        }
        from = start + OPENS.len();
    }
    None
}

/// Where an ask that the next chunk may finish begins, or the text's end when there is none.
fn unfinished_ask(text: &str) -> usize {
    let Some((start, tail)) = text
        .rfind('[')
        .and_then(|start| Some((start, text.get(start..)?)))
    else {
        return text.len();
    };
    let opening = OPENS.starts_with(tail) && tail.len() < OPENS.len();
    let open_ask = tail.starts_with(OPENS)
        && tail.len() <= OPENS.len() + LONGEST_ASK
        && !tail.contains(['\n', ']']);
    if opening || open_ask {
        start
    } else {
        text.len()
    }
}
