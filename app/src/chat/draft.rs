//! A chat's draft as the core keeps it under `chat:<agent>`, so it outlives a restart and
//! follows the person to their other apps: its text, the chips set in it, and its files.
//! A draft from elsewhere replaces this one only while nothing was typed here since the two
//! last agreed, so the person's own words are never overwritten.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input::{InlineToken, InputContent};
use gpui_kit::{App, Context, Focusable, Window};
use sikemux_calls::call;
use sikemux_calls::drafts::SetDraftParams;
use sikemux_calls::params::AgentParams;
use sikemux_view::chat::{chat_draft_key, DraftExtra, DraftToken, QUOTE_MAX, REPLY_URI};
use sikemux_wire::protocol::ChatContext;

use super::{Composing, Sent};
use crate::core::Target;
use crate::reads::Drafts;
use crate::workbench::Workbench;

/// How long typing rests before the draft is kept.
const PAUSE: Duration = Duration::from_millis(300);

/// The draft as the composer last agreed it with the core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    text: String,
    extra: DraftExtra,
}

/// The draft as `setDraft` takes it and `drafts` answers it.
fn kept(composing: &Composing, cx: &App) -> Kept {
    let content = composing.draft.read(cx).content();
    let tokens: Vec<DraftToken> = content
        .tokens()
        .iter()
        .map(|span| {
            let token = span.token();
            DraftToken {
                start: span.range().start,
                end: span.range().end,
                id: token.id().to_string(),
                label: Some(token.label().to_string()),
            }
        })
        .collect();
    let tracked = composing
        .tracked_context
        .iter()
        .filter(|(id, _)| tokens.iter().any(|token| &token.id == *id))
        .map(|(id, context)| (id.clone(), context.clone()))
        .collect();
    Kept {
        text: content.text().to_string(),
        extra: DraftExtra {
            tokens,
            attachments: composing
                .attachments
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            context: composing.context.clone(),
            tracked,
            rest: Default::default(),
        },
    }
}

/// A kept draft as the composer holds one; a chip that no longer matches its text is left
/// out rather than refusing the whole draft.
fn restored(draft: &Kept) -> Sent {
    let mut content = InputContent::new(draft.text.clone());
    for token in &draft.extra.tokens {
        let range = token.start..token.end;
        let Some(written) = draft.text.get(range.clone()) else {
            continue;
        };
        let mut chip = InlineToken::new(token.id.clone(), written.to_owned());
        if let Some(label) = &token.label {
            chip = chip.with_label(label.clone());
        }
        if let Ok(with) = content.clone().with_token(range, chip) {
            content = with;
        }
    }
    Sent {
        content,
        attachments: draft.extra.attachments.iter().map(PathBuf::from).collect(),
        context: draft.extra.context.clone(),
        tracked_context: draft
            .extra
            .tracked
            .iter()
            .map(|(id, context)| (id.clone(), context.clone()))
            .collect(),
    }
}

/// What `drafts` holds for `agent`, shaped as `kept` shapes it.
fn held(drafts: &Drafts, agent: &str) -> Kept {
    let draft = drafts.get(&chat_draft_key(agent));
    Kept {
        text: draft.map_or_else(String::new, |draft| draft.text.clone()),
        extra: draft
            .and_then(|draft| draft.extra.clone())
            .unwrap_or_default(),
    }
}

/// The core's revision of `agent`'s draft in `drafts`.
fn rev(drafts: &Drafts, agent: &str) -> u64 {
    drafts
        .get(&chat_draft_key(agent))
        .map_or(0, |draft| draft.rev)
}

/// Saves of a draft the core has not answered yet. A read of the drafts that crossed one on the
/// way is older than the save, and would bring back the words it replaced.
#[derive(Clone, Default)]
pub struct Saving(Rc<Cell<u32>>);

impl Saving {
    pub fn on_its_way(&self) -> bool {
        self.0.get() > 0
    }

    pub fn save(&self, target: &Target, params: SetDraftParams, cx: &App) {
        self.0.set(self.0.get() + 1);
        let saved = target.request_typed::<call::SetDraft>(params);
        let saving = self.0.clone();
        cx.foreground_executor()
            .spawn(async move {
                if let Err(error) = saved.await {
                    eprintln!("sikemux: the core did not keep a draft: {error}");
                }
                saving.set(saving.get() - 1);
            })
            .detach();
    }
}

/// Takes the drafts the core keeps into every composer, of chats on this host or `elsewhere`,
/// that has not moved on.
pub fn take(
    chats: &mut super::Chats,
    drafts: &Drafts,
    elsewhere: bool,
    cx: &mut Context<Workbench>,
) {
    let open = (chats.open.iter_mut())
        .filter(|(_, chat)| is_elsewhere(chat) == elsewhere)
        .filter_map(|(agent, chat)| Some((agent.as_str(), chat.composer.as_mut()?)));
    for (owner, composing) in open {
        if composing.saving.on_its_way() {
            continue;
        }
        let draft = held(drafts, owner);
        if draft == composing.synced {
            composing.read_rev = rev(drafts, owner);
            continue;
        }
        let shown = kept(composing, cx);
        let untouched = match composing.arrived.take() {
            Some((_, was, _)) => shown == was,
            None => shown == composing.synced,
        };
        if untouched {
            composing.arrived = Some((restored(&draft), shown, rev(drafts, owner)));
            cx.notify();
        }
        composing.synced = draft;
    }
}

/// Whether the chat runs on another host, whose core keeps its draft.
pub fn is_elsewhere(chat: &super::Chat) -> bool {
    *chat.target.place() != crate::hosts::Place::This
}

impl Workbench {
    /// The person began writing, so a new chat's agent starts while they write. A host whose
    /// core predates this answers that it does not know the method, which nobody needs to hear.
    pub fn began_writing(&self, agent: &str) {
        drop(
            self.chat_target(agent)
                .request_typed::<call::Composing>(AgentParams {
                    agent_id: agent.to_owned(),
                }),
        );
    }

    /// Quotes the answer `message` of `agent`'s chat in its draft, in place of any quote there,
    /// and puts the keys in the draft.
    pub fn reply_to(
        &mut self,
        agent: &str,
        message: &str,
        quote: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(composing) = self.composing_mut(agent) else {
            return;
        };
        let quote = match quote.char_indices().nth(QUOTE_MAX) {
            Some((cut, _)) => format!("{}…", quote[..cut].trim_end()),
            None => quote.to_owned(),
        };
        composing
            .context
            .retain(|item| !item.uri.starts_with(REPLY_URI));
        composing.context.insert(
            0,
            ChatContext {
                uri: format!("{REPLY_URI}{message}"),
                title: "Earlier answer".to_owned(),
                text: quote,
            },
        );
        let draft = composing.draft.clone();
        draft.read(cx).focus_handle(cx).focus(window, cx);
        self.typed_draft(agent, cx);
        cx.notify();
    }

    /// Keeps `agent`'s draft once the person pauses.
    pub fn typed_draft(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(composing) = self.composing_mut(agent) else {
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

    /// Keeps `agent`'s draft with the core, without its words while they answer a secret.
    pub fn keep_draft(&mut self, agent: &str, cx: &mut Context<Self>) {
        let secret = self.typed_goes(agent, cx) == crate::calls::Typed::Secret;
        let Some(composing) = self.composing_mut(agent) else {
            return;
        };
        let mut draft = kept(composing, cx);
        if secret {
            draft.text.clear();
            draft.extra.tokens.clear();
            draft.extra.tracked.clear();
        }
        if draft == composing.synced {
            return;
        }
        composing.synced = draft.clone();
        let params = SetDraftParams {
            key: chat_draft_key(agent),
            text: draft.text,
            extra: (!draft.extra.is_empty()).then_some(draft.extra),
            rev: Some(composing.read_rev),
        };
        let saving = composing.saving.clone();
        saving.save(&self.chat_target(agent), params, cx);
    }
}

impl Composing {
    /// Leaves the call for a secret the draft's words answered, now closed: words typed while it
    /// waited go, and a draft left as it was when the call came stays. Either way the undo
    /// history goes, so a secret cut from the draft can't come back. Says whether any words went.
    pub fn left_secret(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let Some(before) = self.secret_since.take() else {
            return false;
        };
        let typed = *self.draft.read(cx).value() != *before;
        (self.draft).update(cx, |draft, cx| {
            let kept = match typed {
                true => InputContent::new(String::new()),
                false => draft.content(),
            };
            draft.set_value(kept, window, cx);
        });
        typed
    }

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
    pub fn start_from(&mut self, drafts: &Drafts, agent: &str, window: &mut Window, cx: &mut App) {
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
