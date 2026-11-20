//! The composer a call is answered with on its page: the chat's own, one line tall, which
//! sends the call's answer in place of a message, or with ⌘⏎ a note to its asker that leaves
//! the call open.

use std::rc::Rc;

use gpui_kit::{AnyElement, App, Context, Entity, IntoElement, SharedString, Window};
use sikemux_calls::call;
use sikemux_calls::chat::PromptChatParams;
use sikemux_view::calls::{AnswerCallParams, CallKind, PersonCall};

use crate::chat::composer::Act;
use crate::chat::ComposerView;
use crate::workbench::Workbench;

const OWNER: &str = "call:";

/// What names the composer call `id` is answered with.
pub(crate) fn owner(id: &str) -> String {
    format!("{OWNER}{id}")
}

/// The call the composer named `owner` answers, when it answers one.
pub(crate) fn call_of(owner: &str) -> Option<&str> {
    owner.strip_prefix(OWNER)
}

/// The composer named `owner`, with the draft, the files and the menus alone. A step to
/// production is sent by a click of its arrow, never by a key.
pub(crate) fn composer(
    bench: &Workbench,
    owner: &str,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let id = call_of(owner)?;
    let (_, call) = bench.call_at(id)?;
    let by_mouse = call.kind == CallKind::Production;
    let on_send: Act = {
        let (this, id) = (cx.weak_entity(), id.to_owned());
        Rc::new(move |_: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| bench.reply(&id, cx)).ok();
        })
    };
    let on_stop: Act = Rc::new(|_: &mut Window, _: &mut App| {});
    let (composer, offering) =
        crate::chat::drafting(bench, owner, "", String::new(), (on_send, on_stop), cx)?;
    let composer = composer.slim().send_by_mouse(by_mouse);
    Some(crate::chat::mounted(owner, composer, offering, cx).into_any_element())
}

impl Workbench {
    /// The composer `call` is answered with, made the first time its page draws, its empty
    /// draft saying `hint`.
    pub(crate) fn reply_view(
        &mut self,
        call: &PersonCall,
        hint: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ComposerView>> {
        let owner = owner(&call.id);
        if !self.chats.replies.contains_key(&owner) {
            let made = self.new_composing(&owner, 1, window, cx);
            self.chats.replies.insert(owner.clone(), made);
        }
        let composing = self.chats.replies.get_mut(&owner)?;
        if composing.hint != hint {
            composing.hint = hint.clone();
            composing
                .draft
                .update(cx, |draft, cx| draft.set_placeholder(hint, window, cx));
        }
        Some(composing.view.clone())
    }

    /// Answers call `id` from its page: the choice made there with what its composer holds as
    /// the note, or with none made those words alone, and the composer's files either way. A
    /// call for a secret has no composer and is answered from its field.
    pub(crate) fn reply(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(composing) = self.chats.replies.get(&owner(id)) else {
            return self.go_with(id, cx);
        };
        let Some((_, call)) = self.call_at(id) else {
            return;
        };
        let words = composing.draft.read(cx).value().trim().to_owned();
        let words = (!words.is_empty()).then_some(words);
        let paths = composing.outgoing(cx).0;
        let option = self.calls.read(cx).chosen(call).into_iter().next();
        let id = id.to_owned();
        let params = match (option, words) {
            (None, None) => {
                if !paths.is_empty() {
                    let note = t!("Pick a choice or write a line to go with the files");
                    self.notify(crate::toast::info(note), cx);
                }
                return;
            }
            (None, words) => AnswerCallParams {
                id,
                words,
                paths,
                ..AnswerCallParams::default()
            },
            (option, note) => AnswerCallParams {
                id,
                option,
                note,
                paths,
                ..AnswerCallParams::default()
            },
        };
        self.send_answer(params, cx);
    }

    /// Sends what call `id`'s composer holds to its asker's chat as a note on the call, and
    /// leaves the call open.
    pub(crate) fn follow_up(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let owner = owner(id);
        let (Some(composing), Some((place, call))) =
            (self.chats.replies.get(&owner), self.call_at(id))
        else {
            return;
        };
        let words = composing.draft.read(cx).value().trim().to_owned();
        let paths = composing.outgoing(cx).0;
        if words.is_empty() && paths.is_empty() {
            return;
        }
        let about = (call.title.as_deref())
            .or_else(|| {
                call.question
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
            })
            .unwrap_or_default();
        let asker = call.asker.clone();
        let params = PromptChatParams {
            paths,
            ..PromptChatParams::new(
                asker.agent_id,
                format!("On your call \"{about}\", which stays open: {words}"),
            )
        };
        let sent =
            crate::core::Target::new(&self.asks, place).request_typed::<call::PromptChat>(params);
        self.emptied(&owner, window, cx);
        cx.spawn(async move |bench, cx| {
            let toast = match sent.await {
                Ok(()) => crate::toast::info(t!("Sent to @{name}", name = asker.name)),
                Err(error) => crate::toast::error(error),
            };
            bench.update(cx, |bench, cx| bench.notify(toast, cx)).ok();
        })
        .detach();
    }
}
