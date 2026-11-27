//! The composers outside a chat, each the chat's own, one line tall: the one a call is answered
//! with on its page, which sends the call's answer in place of a message, or with ⌘⏎ a note to
//! its asker that leaves the call open; and the one over an artifact on a desk, which writes such
//! a note about the call linking it, else to the desk's agent about it.

use std::rc::Rc;

use gpui_kit::{AnyElement, App, Context, Entity, IntoElement, SharedString, Window};
use sikemux_calls::call;
use sikemux_calls::chat::PromptChatParams;
use sikemux_view::calls::{AnswerCallParams, CallKind, PersonCall};
use sikemux_view::chat::{ARTIFACT_URI, CALL_URI};
use sikemux_wire::protocol::ChatContext;

use crate::chat::composer::Act;
use crate::chat::ComposerView;
use crate::workbench::Workbench;

const CALL: &str = "call:";
const PAGE: &str = "page:";

/// What a composer outside a chat writes about.
pub(crate) enum About<'a> {
    Call(&'a str),
    /// Artifact `id` on the desk of `agent`.
    Page {
        id: &'a str,
        agent: &'a str,
    },
}

impl<'a> About<'a> {
    /// What the composer named `owner` writes about, when it is none of a chat's.
    pub(crate) fn of(owner: &'a str) -> Option<Self> {
        if let Some(call) = owner.strip_prefix(CALL) {
            return Some(Self::Call(call));
        }
        let (id, agent) = owner.strip_prefix(PAGE)?.split_once(':')?;
        Some(Self::Page { id, agent })
    }
}

/// What names the composer call `id` is answered with.
pub(crate) fn owner(id: &str) -> String {
    format!("{CALL}{id}")
}

/// What names the composer over artifact `id` on the desk of `agent`.
pub(crate) fn page_owner(id: &str, agent: &str) -> String {
    format!("{PAGE}{id}:{agent}")
}

/// The composer named `owner`, with the draft, the files and the menus alone. A step to
/// production is sent by a click of its arrow, never by a key.
pub(crate) fn composer(
    bench: &Workbench,
    owner: &str,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let by_mouse = match About::of(owner)? {
        About::Call(id) => bench.call_at(id)?.1.kind == CallKind::Production,
        About::Page { .. } => false,
    };
    let on_send: Act = {
        let (this, owner) = (cx.weak_entity(), owner.to_owned());
        Rc::new(move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| match About::of(&owner) {
                Some(About::Call(id)) => bench.reply(id, cx),
                _ => bench.write_about_page(&owner, window, cx),
            })
            .ok();
        })
    };
    let on_stop: Act = Rc::new(|_: &mut Window, _: &mut App| {});
    let (composer, offering) =
        crate::chat::drafting(bench, owner, "", String::new(), (on_send, on_stop), cx)?;
    let composer = composer.slim().send_by_mouse(by_mouse);
    Some(crate::chat::mounted(owner, composer, offering, cx).into_any_element())
}

/// What tells the asker of `call` that the message is about it and leaves it open, carried as
/// context so the transcript shows a chip instead of the call id.
fn call_context(call: &PersonCall) -> ChatContext {
    let about = super::prose::plain_name(call).replace(['\r', '\n'], " ");
    let about = if about.trim().is_empty() {
        "Call".to_owned()
    } else {
        about
    };
    let title = match about.char_indices().nth(240) {
        Some((cut, _)) => format!("{}…", about.get(..cut).unwrap_or(&about).trim_end()),
        None => about,
    };
    let text = format!(
        "{title}\nOn your call {} \"{title}\", which stays open.",
        call.id
    )
    .replace("</context", "&lt;/context");
    ChatContext {
        uri: format!("{CALL_URI}{}", call.id),
        text,
        title,
    }
}

impl Workbench {
    /// The composer named `owner`, made the first time it draws, its empty draft saying `hint`.
    pub(crate) fn reply_view(
        &mut self,
        owner: &str,
        hint: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ComposerView>> {
        if !self.chats.replies.contains_key(owner) {
            let made = self.new_composing(owner, 1, window, cx);
            self.chats.replies.insert(owner.to_owned(), made);
        }
        let composing = self.chats.replies.get_mut(owner)?;
        if composing.hint != hint {
            composing.hint = hint.clone();
            composing
                .draft
                .update(cx, |draft, cx| draft.set_placeholder(hint, window, cx));
        }
        Some(composing.view.clone())
    }

    /// The agent the composer named `owner` writes to: a chat's own, a call's asker, or the one
    /// whose desk its artifact is on.
    pub(crate) fn written_to<'a>(&'a self, owner: &'a str) -> &'a str {
        match About::of(owner) {
            Some(About::Call(id)) => self
                .call_at(id)
                .map_or(owner, |(_, call)| call.asker.agent_id.as_str()),
            Some(About::Page { agent, .. }) => agent,
            None => owner,
        }
    }

    /// Whether call `id`'s composer holds words.
    pub(crate) fn noted(&self, id: &str, cx: &App) -> bool {
        (self.composing(&owner(id)))
            .is_some_and(|composing| !composing.draft.read(cx).value().trim().is_empty())
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
        let words = composing.content(cx).text().trim().to_owned();
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
        let words = composing.content(cx).text().trim().to_owned();
        let (paths, mut context) = composing.outgoing(cx);
        if words.is_empty() && paths.is_empty() {
            return;
        }
        context.push(call_context(call));
        let asker = call.asker.clone();
        let params = PromptChatParams {
            paths,
            context,
            ..PromptChatParams::new(asker.agent_id, words)
        };
        let to = crate::core::Target::new(&self.asks, place);
        self.send_aside(&owner, params, to, asker.name, window, cx);
    }

    /// Sends what the composer named `owner` holds, its artifact attached: as a note on the call
    /// linking the artifact, which stays open, else to the agent whose desk the artifact is on.
    pub(crate) fn write_about_page(
        &mut self,
        owner: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(About::Page { id, agent }), Some(composing)) =
            (About::of(owner), self.chats.replies.get(owner))
        else {
            return;
        };
        let words = composing.content(cx).text().trim().to_owned();
        let (paths, mut context) = composing.outgoing(cx);
        if words.is_empty() && paths.is_empty() {
            return;
        }
        let title = self.page_title(agent, id, cx).unwrap_or_default();
        context.push(ChatContext {
            uri: format!("{ARTIFACT_URI}{id}"),
            text: format!("Written over the artifact \"{title}\" (page id {id})."),
            title,
        });
        let (to, name) = match self.asked_about(id, agent, cx) {
            Some(call) => {
                context.push(call_context(&call));
                (call.asker.agent_id, call.asker.name)
            }
            None => {
                let name = self.agent_by_id(agent).and_then(|row| row.name);
                (agent.to_owned(), name.unwrap_or_default())
            }
        };
        let params = PromptChatParams {
            paths,
            context,
            ..PromptChatParams::new(to, words)
        };
        let at = self.chat_target(owner);
        self.send_aside(owner, params, at, name, window, cx);
    }

    /// Sends `params` to the agent called `name` without opening its chat, and empties the
    /// composer named `owner`, which takes its words back if they do not go.
    fn send_aside(
        &mut self,
        owner: &str,
        params: PromptChatParams,
        to: crate::core::Target,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(held) = (self.composing(owner)).map(|composing| composing.held(cx)) else {
            return;
        };
        let owner = owner.to_owned();
        let sent = to.request_typed::<call::PromptChat>(params);
        self.emptied(&owner, window, cx);
        cx.spawn_in(window, async move |bench, cx| {
            let answer = sent.await;
            let _ = bench.update_in(cx, |bench, window, cx| {
                let toast = match answer {
                    Ok(()) => crate::toast::info(t!("Sent to @{name}", name = name)),
                    Err(error) => {
                        let composing = bench.composing_mut(&owner);
                        if let Some(composing) = composing.filter(|it| it.is_empty(cx)) {
                            composing.put(held, window, cx);
                        }
                        crate::toast::error(error)
                    }
                };
                bench.notify(toast, cx);
            });
        })
        .detach();
    }
}
