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
use crate::theme::{Tokens, SPACE_1H, SPACE_3H, TEXT_SM};
use crate::ui;
use crate::workbench::Workbench;

/// How the window stands with its core: which version answered, and why it does not now.
#[derive(Default)]
pub struct Connection {
    pub(crate) version: String,
    pub(crate) lost: Option<String>,
    /// The core stopped on purpose, so it waits for the person to start it again.
    pub(crate) stopped: bool,
    /// The core holding the socket stopped answering, by its process id, and whether the
    /// person asked to restart it.
    pub(crate) stuck: Option<(u32, bool)>,
    /// Opening has taken long enough to show the mark.
    pub(crate) boot_shown: bool,
    /// Opening has taken long enough to say what the window waits for.
    pub(crate) slow_start: bool,
}

impl Connection {
    /// The core at `version` answered. Says whether it is the first to answer this window.
    pub fn connected(&mut self, version: String) -> bool {
        let first = self.version.is_empty();
        self.version = version;
        self.lost = None;
        self.stopped = false;
        self.stuck = None;
        first
    }

    pub fn lose(&mut self, reason: String) {
        self.lost = Some(reason);
    }

    pub fn stop(&mut self) {
        self.lost = None;
        self.stopped = true;
    }

    pub fn hang(&mut self, pid: u32) {
        self.lost = None;
        self.stuck = Some((pid, false));
    }
}

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
                bench.connection.boot_shown = true;
                cx.notify();
            });
            cx.background_executor()
                .timer(NAME_THE_WAIT_AFTER - SHOW_MARK_AFTER)
                .await;
            let _ = this.update(cx, |bench, cx| {
                bench.connection.slow_start = true;
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
        .gap(SPACE_3H)
        .child(ui::icon(Icon::Logo, 44.).text_color(t.accent))
        .when_some(words, |column, words| {
            column.child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_SM))
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
    let words = bench
        .connection
        .slow_start
        .then_some(if bench.connection.version.is_empty() {
            "Waiting for Sikemux to start"
        } else {
            "Opening your workspace"
        });
    let why = bench
        .connection
        .lost
        .clone()
        .filter(|_| bench.connection.slow_start);
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
                    .flex_col()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(crate::theme::ui_text(TEXT_SM))
                            .text_color(t.text_secondary)
                            .child(words),
                    )
                    .when_some(why, |column, why| {
                        column.child(
                            ui::meta(why, cx)
                                .flex_none()
                                .w(px(420.))
                                .text_center()
                                .whitespace_normal(),
                        )
                    }),
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
            .when(bench.connection.boot_shown, |cover| cover.child(mark))
            .into_any_element(),
    )
}

/// Covers the workspace while the connection to the core is down.
pub fn reconnecting(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let reason = bench
        .connection
        .lost
        .clone()
        .filter(|_| bench.view.is_some())?;
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
    if !bench.connection.stopped || bench.view.is_none() {
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

/// Covers the window while a core that stopped answering holds the socket, with the one way
/// out: ending it so another starts.
pub fn stuck(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let (pid, restarting) = bench.connection.stuck?;
    let t = Tokens::get(cx).clone();
    Some(
        div()
            .id("stuck")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(18.))
            .bg(t.surface_sunken)
            .occlude()
            .child(mark_with(
                Some("Sikemux is not answering".to_owned()),
                Some(format!(
                    "Its core (pid {pid}) is still running but stopped responding. Restarting it ends its terminals; chats pick up where they were."
                )),
                cx,
            ))
            .child(
                ui::Button::new(
                    "restart-core",
                    if restarting { "Restarting…" } else { "Restart Sikemux" },
                    ui::Variant::Primary,
                )
                .disabled(restarting)
                .on_click(cx.listener(|bench, _, _, cx| {
                    if let Some((_, restarting)) = &mut bench.connection.stuck {
                        *restarting = true;
                    }
                    let _ = bench.asks.send(crate::core::Ask::StartCore);
                    cx.notify();
                })),
            )
            .into_any_element(),
    )
}
