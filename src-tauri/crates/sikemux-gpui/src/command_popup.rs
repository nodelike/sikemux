//! A custom command run in a terminal floating over the window. Closing it ends the command.

use gpui_kit::{
    actions, div, hsla, px, AnyElement, App, Context, InteractiveElement, IntoElement, KeyBinding,
    MouseButton, ParentElement, SharedString, Styled,
};
use sikemux_wire::protocol::SessionId;

use crate::core::{Ask, TerminalAsk};
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

actions!(command_popup, [ClosePopup]);

const CONTEXT: &str = "CommandPopup";

pub fn install(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", ClosePopup, Some(CONTEXT))]);
}

pub struct Popup {
    title: SharedString,
    session: SessionId,
}

impl Workbench {
    pub fn show_command_popup(
        &mut self,
        title: impl Into<SharedString>,
        session: SessionId,
        cx: &mut Context<Self>,
    ) {
        self.close_command_popup(cx);
        self.command_popup = Some(Popup {
            title: title.into(),
            session,
        });
        cx.notify();
    }

    pub fn close_command_popup(&mut self, cx: &mut Context<Self>) {
        if let Some(popup) = self.command_popup.take() {
            let _ = self
                .asks
                .send(Ask::Terminal(popup.session, TerminalAsk::Kill));
            cx.notify();
        }
    }
}

pub fn overlay(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let popup = bench.command_popup.as_ref()?;
    let t = Tokens::get(cx).clone();
    let this = cx.entity();
    let terminal = crate::terminal::pane(
        bench,
        &this,
        &crate::hosts::Place::This,
        &popup.session.to_string(),
        cx,
    );
    let close = cx.listener(|bench, _: &ClosePopup, _, cx| bench.close_command_popup(cx));
    let card = div()
        .id("command-popup")
        .key_context(format!("{CONTEXT} Modal").as_str())
        .on_action(close)
        .w(px(760.))
        .h(px(480.))
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_4)
        .shadow(t.shadow_float())
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .pl(px(16.))
                .pr(px(8.))
                .py(px(6.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(13.))
                        .text_color(t.text_primary)
                        .child(popup.title.clone()),
                )
                .child(
                    ui::IconButton::new("command-popup-close", Icon::Close, "Close")
                        .small()
                        .on_click(cx.listener(|bench, _, _, cx| bench.close_command_popup(cx))),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .bg(t.surface_sunken)
                .child(terminal),
        );
    Some(
        div()
            .id("command-popup-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.55))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|bench, _, _, cx| bench.close_command_popup(cx)),
            )
            .child(card)
            .into_any_element(),
    )
}
