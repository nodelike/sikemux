//! Small parts the pane's pages share.

use super::*;

use crate::theme::{SPACE_1H, SPACE_2, TEXT_3XS_H};

pub(super) fn chip(text: impl Into<SharedString>, ink: Hsla, cx: &App) -> AnyElement {
    let _ = cx;
    div()
        .flex_none()
        .px(SPACE_1H)
        .py(px(1.))
        .rounded(RADIUS_1)
        .bg(ink.opacity(0.16))
        .text_size(crate::theme::ui_text(TEXT_3XS_H))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .whitespace_nowrap()
        .child(text.into())
        .into_any_element()
}

pub(super) fn quiet(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    ui::meta(text, cx).py(SPACE_2).into_any_element()
}
