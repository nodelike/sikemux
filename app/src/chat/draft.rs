//! A chat's draft as the core keeps it under `chat:<agent>`, so it outlives a restart and
//! follows the person to their other apps: its text, the chips set in it, and its files.
//! A draft from elsewhere replaces this one only while nothing was typed here since the two
//! last agreed, so the person's own words are never overwritten.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::base::input::{InlineToken, InputContent};
use gpui_kit::{App, Context, Window};
use serde_json::{json, Value};

use super::{Composing, Sent};
use crate::workbench::Workbench;

/// How long typing rests before the draft is kept.
const PAUSE: Duration = Duration::from_millis(300);

/// The key the core keeps `agent`'s draft under.
pub fn key(agent: &str) -> String {
    format!("chat:{agent}")
}

/// The draft as `setDraft` takes it and `drafts` answers it.
fn kept(composing: &Composing, cx: &App) -> Value {
    let content = composing.draft.read(cx).content();
    let tokens: Vec<Value> = content
        .tokens()
        .iter()
        .map(|span| {
            let token = span.token();
            json!({
                "start": span.range().start,
                "end": span.range().end,
                "id": token.id(),
                "label": token.label(),
            })
        })
        .collect();
    let mut extra = serde_json::Map::new();
    if !tokens.is_empty() {
        extra.insert("tokens".into(), tokens.into());
    }
    if !composing.attachments.is_empty() {
        extra.insert("attachments".into(), json!(composing.attachments));
    }
    if !composing.context.is_empty() {
        extra.insert("context".into(), json!(composing.context));
    }
    let held: Vec<&str> = content
        .tokens()
        .iter()
        .map(|span| span.token().id().as_ref())
        .collect();
    let tracked: serde_json::Map<String, Value> = composing
        .tracked_context
        .iter()
        .filter(|(id, _)| held.contains(&id.as_str()))
        .map(|(id, context)| (id.clone(), json!(context)))
        .collect();
    if !tracked.is_empty() {
        extra.insert("tracked".into(), tracked.into());
    }
    let mut draft = json!({ "text": content.text() });
    if !extra.is_empty() {
        draft["extra"] = extra.into();
    }
    draft
}

/// A kept draft as the composer holds one; a chip that no longer matches its text is left
/// out rather than refusing the whole draft.
fn restored(draft: &Value) -> Sent {
    let text = draft["text"].as_str().unwrap_or_default();
    let extra = &draft["extra"];
    let mut content = InputContent::new(text.to_owned());
    for token in extra["tokens"].as_array().into_iter().flatten() {
        let (Some(start), Some(end), Some(id)) = (
            token["start"].as_u64(),
            token["end"].as_u64(),
            token["id"].as_str(),
        ) else {
            continue;
        };
        let range = start as usize..end as usize;
        let Some(written) = text.get(range.clone()) else {
            continue;
        };
        let mut chip = InlineToken::new(id.to_owned(), written.to_owned());
        if let Some(label) = token["label"].as_str() {
            chip = chip.with_label(label.to_owned());
        }
        if let Ok(with) = content.clone().with_token(range, chip) {
            content = with;
        }
    }
    let attachments = extra["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|path| path.as_str().map(PathBuf::from))
        .collect();
    let context = serde_json::from_value(extra["context"].clone()).unwrap_or_default();
    let tracked_context = serde_json::from_value(extra["tracked"].clone()).unwrap_or_default();
    Sent {
        content,
        attachments,
        context,
        tracked_context,
    }
}

/// What `drafts` holds for `agent`, shaped as `kept` shapes it.
fn held(drafts: &Value, agent: &str) -> Value {
    let draft = &drafts[key(agent).as_str()];
    let mut held = json!({ "text": draft["text"].as_str().unwrap_or_default() });
    if draft["extra"]
        .as_object()
        .is_some_and(|extra| !extra.is_empty())
    {
        held["extra"] = draft["extra"].clone();
    }
    held
}

/// The core's revision of `agent`'s draft in `drafts`.
fn rev(drafts: &Value, agent: &str) -> u64 {
    drafts[key(agent).as_str()]["rev"]
        .as_u64()
        .unwrap_or_default()
}

/// Whether the chat runs on another host, whose core keeps its draft.
pub fn is_elsewhere(chat: &super::Chat) -> bool {
    *chat.target.place() != crate::hosts::Place::This
}

/// The read the drafts of chats here, or of chats on the host in front, arrive under.
impl Workbench {
    /// Keeps `agent`'s draft once the person pauses.
    /// The person began writing, so a new chat's agent starts while they write. A host whose
    /// core predates this answers that it does not know the method, which nobody needs to hear.
    pub fn began_writing(&self, agent: &str) {
        drop(
            self.chat_target(agent)
                .request("composing", json!({ "agentId": agent })),
        );
    }

    pub fn typed_draft(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        else {
            return;
        };
        let agent = agent.to_owned();
        composing._keeping = cx.spawn(async move |bench, cx| {
            cx.background_executor().timer(PAUSE).await;
            bench
                .update(cx, |bench, cx| bench.keep_draft(&agent, cx))
                .ok();
        });
    }

    pub fn keep_draft(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        else {
            return;
        };
        let draft = kept(composing, cx);
        if draft == composing.synced {
            return;
        }
        composing.synced = draft.clone();
        let mut params =
            json!({ "key": key(agent), "text": draft["text"], "rev": composing.read_rev });
        if let Some(extra) = draft.get("extra") {
            params["extra"] = extra.clone();
        }
        self.chat_target(agent).call("setDraft", params);
    }

    /// Takes the drafts the core keeps into every open composer that has not moved on.
    pub fn drafts(&mut self, drafts: &Value, elsewhere: bool, cx: &mut Context<Self>) {
        for (agent, chat) in &mut self.chats.open {
            if is_elsewhere(chat) != elsewhere {
                continue;
            }
            let Some(composing) = chat.composer.as_mut() else {
                continue;
            };
            let draft = held(drafts, agent);
            if draft == composing.synced {
                composing.read_rev = rev(drafts, agent);
                continue;
            }
            let shown = kept(composing, cx);
            if shown == composing.synced {
                composing.arrived = Some((restored(&draft), shown, rev(drafts, agent)));
                cx.notify();
            }
            composing.synced = draft;
        }
    }
}

impl Composing {
    /// Puts a draft that came from elsewhere in place, unless the person typed since.
    pub fn take_arrived(&mut self, window: &mut Window, cx: &mut App) {
        let Some((arrived, was, rev)) = self.arrived.take() else {
            return;
        };
        if kept(self, cx) == was {
            self.put(arrived, window, cx);
            self.read_rev = rev;
        }
    }

    /// Starts from the draft the core keeps for `agent`, when the composer is made.
    pub fn start_from(&mut self, drafts: &Value, agent: &str, window: &mut Window, cx: &mut App) {
        let draft = held(drafts, agent);
        self.put(restored(&draft), window, cx);
        self.synced = draft;
        self.read_rev = rev(drafts, agent);
    }

    fn put(&mut self, draft: Sent, window: &mut Window, cx: &mut App) {
        self.draft
            .update(cx, |input, cx| input.set_value(draft.content, window, cx));
        self.attachments = draft.attachments;
        self.context = draft.context;
        self.tracked_context = draft.tracked_context;
    }
}
