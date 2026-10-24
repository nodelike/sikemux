//! What the window shows while the core is not there: the mark while it opens,
//! and a word on what is happening when that takes a while or the connection
//! drops. The window reconnects by itself, so neither asks anything.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, white, AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    Styled,
};

use crate::icons::Icon;
use crate::theme::Tokens;
use crate::ui;
use crate::workbench::Workbench;

/// An opening that finishes within this shows only the window's ground, so it never flashes.
const SHOW_MARK_AFTER: Duration = Duration::from_millis(400);
/// An opening still going after this says what it waits on.
const NAME_THE_WAIT_AFTER: Duration = Duration::from_millis(3000);

impl Workbench {
    /// Shows the mark once opening has taken [`SHOW_MARK_AFTER`], and says what the window
    /// waits for once it has taken [`NAME_THE_WAIT_AFTER`].
    pub fn wait_patiently(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOW_MARK_AFTER).await;
            let _ = this.update(cx, |bench, cx| {
                bench.boot_shown = true;
                cx.notify();
            });
            cx.background_executor()
                .timer(NAME_THE_WAIT_AFTER - SHOW_MARK_AFTER)
                .await;
            let _ = this.update(cx, |bench, cx| {
                bench.slow_start = true;
                cx.notify();
            });
        })
        .detach();
    }
}

fn mark_with(
    words: Option<String>,
    detail: Option<String>,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(14.))
        .child(ui::icon(Icon::Logo, 44.).text_color(t.accent))
        .when_some(words, |column, words| {
            column.child(
                div()
                    .text_size(crate::theme::ui_text(13.))
                    .text_color(t.text_secondary)
                    .child(words),
            )
        })
        .when_some(detail, |column, detail| {
            column.child(
                ui::meta(detail, cx)
                    .max_w(px(420.))
                    .text_center()
                    .whitespace_normal(),
            )
        })
}

/// Covers the window until the core's first view arrives.
pub fn opening(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    if bench.view.is_some() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let words = bench.slow_start.then_some(if bench.version.is_empty() {
        "Waiting for Sikemux to start"
    } else {
        "Opening your workspace"
    });
    let mark = div()
        .relative()
        .child(ui::icon(Icon::Logo, 34.).text_color(white()).opacity(0.9))
        .when_some(words, |mark, words| {
            mark.child(
                div()
                    .absolute()
                    .top(px(34. + 14.))
                    .left(px(17.))
                    .w(px(0.))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(crate::theme::ui_text(13.))
                            .text_color(t.text_secondary)
                            .child(words),
                    ),
            )
        });
    Some(
        div()
            .id("opening")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.surface_sunken)
            .occlude()
            .when(bench.boot_shown, |cover| cover.child(mark))
            .into_any_element(),
    )
}

/// Covers the workspace while the connection to the core is down.
pub fn reconnecting(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let reason = bench.lost.clone().filter(|_| bench.view.is_some())?;
    let t = Tokens::get(cx).clone();
    Some(
        div()
            .id("reconnecting")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla {
                a: 0.82,
                ..t.surface_sunken
            })
            .occlude()
            .child(mark_with(
                Some("Lost Sikemux, reconnecting".to_owned()),
                Some(reason),
                cx,
            ))
            .into_any_element(),
    )
}

/// Covers the workspace while the core is stopped on purpose, with the one way back: starting it.
pub fn stopped(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    if !bench.stopped || bench.view.is_none() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    Some(
        div()
            .id("stopped")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(18.))
            .bg(Hsla {
                a: 0.82,
                ..t.surface_sunken
            })
            .occlude()
            .child(mark_with(
                Some("Sikemux is stopped".to_owned()),
                Some("Its terminals and agents stopped with it.".to_owned()),
                cx,
            ))
            .child(
                ui::Button::new("start-core", "Start Sikemux", ui::Variant::Primary).on_click(
                    cx.listener(|bench, _, _, _| {
                        let _ = bench.asks.send(crate::core::Ask::StartCore);
                    }),
                ),
            )
            .into_any_element(),
    )
}
