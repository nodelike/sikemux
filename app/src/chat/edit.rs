//! Writing a sent message again: its bubble becomes an editor, and sending it rewinds the
//! chat to that message, which the core asks the agent to take back, and sends it anew.

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::{
    div, relative, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement,
    ParentElement, Styled, Subscription, Window,
};
use gpui_transcript::Change;
use sikemux_wire::chat::ChatMessage;

use super::{text_of, CancelEdit, Drawn};
use crate::core::ChatAsk;
use crate::theme;
use crate::theme::{SPACE_1H, SPACE_2, SPACE_3};
use crate::ui::{Button, Variant};
use crate::workbench::Workbench;

/// The sent message being written again, by its row.
pub struct Editing {
    row: String,
    draft: Entity<TextareaState>,
    _sent: Subscription,
}

impl Editing {
    pub fn is(&self, row: &str) -> bool {
        self.row == row
    }
}

/// What the person wrote in a message, as they wrote it.
pub fn written(message: &ChatMessage) -> String {
    message
        .parts
        .iter()
        .filter_map(text_of)
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl Workbench {
    /// Turns the bubble of sent message `row` into an editor holding `text`.
    pub fn edit_message(
        &mut self,
        agent: &str,
        row: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.open.get(agent) else {
            return;
        };
        let draft = cx.new(|cx| {
            let mut draft = TextareaState::new(window, cx)
                .auto_grow(1, 12)
                .submit_on_enter(true);
            draft.set_value(text, window, cx);
            draft
        });
        draft.read(cx).focus_handle(cx).focus(window, cx);
        let sent = {
            let agent = agent.to_owned();
            cx.subscribe_in(
                &draft,
                window,
                move |bench, _, event: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        bench.send_edit(&agent, cx);
                    }
                },
            )
        };
        let editing = Editing {
            row: row.to_owned(),
            draft,
            _sent: sent,
        };
        chat.reader.update(cx, |reading, cx| {
            reading.editing = Some(editing);
            cx.notify();
        });
    }

    pub fn cancel_edit(&mut self, agent: &str, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.open.get(agent) {
            chat.reader.update(cx, |reading, cx| {
                reading.editing = None;
                cx.notify();
            });
        }
    }

    /// Sends the message being written again in place of the one it was, and everything after it.
    pub fn send_edit(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.open.get(agent) else {
            return;
        };
        let Some(editing) = chat.reader.update(cx, |reading, cx| {
            cx.notify();
            reading.editing.take()
        }) else {
            return;
        };
        let text = editing.draft.read(cx).value().trim().to_owned();
        let rows = chat.transcript.read(cx).rows();
        let Some(message) = rows
            .iter()
            .map(|row| &row.message)
            .find(|message| message.id == editing.row)
        else {
            return;
        };
        let Some(prompt) = message.prompt_id.clone().filter(|_| !text.is_empty()) else {
            return;
        };
        let paths = message.attachments.clone();
        chat.transcript.update(cx, |transcript, cx| {
            transcript.push(Change::DropFrom(editing.row.into()), cx)
        });
        self.ask_chat(ChatAsk::Edit {
            agent: agent.to_owned(),
            message: prompt,
            text,
            paths,
        });
    }
}

/// The bubble of a sent message the person is writing again.
pub fn editor(agent: &str, editing: &Editing, looks: &Drawn) -> gpui_kit::AnyElement {
    let (bench, cancel, send) = (looks.bench.clone(), agent.to_owned(), agent.to_owned());
    let cancelled = {
        let bench = bench.clone();
        move |_: &CancelEdit, _: &mut Window, cx: &mut gpui_kit::App| {
            bench
                .update(cx, |bench, cx| bench.cancel_edit(&cancel, cx))
                .ok();
        }
    };
    let buttons = div()
        .flex()
        .justify_end()
        .gap(SPACE_1H)
        .child(
            Button::new("edit-cancel", "Cancel", Variant::Outline).on_click({
                let (bench, agent) = (bench.clone(), agent.to_owned());
                move |_, _, cx| {
                    bench
                        .update(cx, |bench, cx| bench.cancel_edit(&agent, cx))
                        .ok();
                }
            }),
        )
        .child(
            Button::new("edit-send", "Send", Variant::Primary).on_click(move |_, _, cx| {
                bench
                    .update(cx, |bench, cx| bench.send_edit(&send, cx))
                    .ok();
            }),
        );
    div()
        .key_context("MessageEditor")
        .on_action(cancelled)
        .w(relative(0.8))
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .px(SPACE_3)
        .py(SPACE_2)
        .border_1()
        .border_color(looks.line)
        .rounded(theme::RADIUS_3)
        .bg(looks.bubble)
        .child(
            Textarea::new(&editing.draft)
                .appearance(false)
                .text_size(crate::theme::ui_text(13.5)),
        )
        .child(buttons)
        .into_any_element()
}
