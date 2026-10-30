//! A changed file's card head, as the Git pane, a pull request and a chat's change all draw
//! it: a chevron that folds the diff under it, the file's icon, its folder quiet before its
//! name, then whatever the caller adds at the end.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, App, Div, InteractiveElement, ParentElement, Pixels, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window,
};

use crate::icons::Icon;
use crate::motion;
use crate::theme::{self, Tokens, RADIUS_2, SPACE_2, TEXT_SM};

const FILE_HEAD_HEIGHT: Pixels = px(32.);

/// What clicking a file head's name does: opens the file.
pub type Opens = Box<dyn Fn(&mut Window, &mut App)>;

/// The head of the card for `path`, open or folded. A click on the name calls `open_file`,
/// and anywhere else on the head is left to the caller, which folds the card.
pub fn file_head(
    id: impl Into<SharedString>,
    path: &str,
    open: bool,
    open_file: Option<Opens>,
    cx: &App,
) -> Stateful<Div> {
    let t = Tokens::get(cx);
    let id = id.into();
    let (dir, base) = match path.trim_end_matches('/').rsplit_once('/') {
        Some((dir, base)) => (Some(dir.to_owned()), base.to_owned()),
        None => (None, path.to_owned()),
    };
    let name = div()
        .id(SharedString::from(format!("{id}-name")))
        .min_w_0()
        .flex()
        .truncate()
        .text_size(theme::ui_text(TEXT_SM))
        .when_some(dir, |name, dir| {
            name.child(
                div()
                    .flex_none()
                    .text_color(t.text_tertiary)
                    .child(format!("{dir}/")),
            )
        })
        .child(div().text_color(t.text_primary).child(base.clone()))
        .when_some(open_file, |name, open_file| {
            name.hover(|name| name.underline())
                .tooltip(super::tooltip("Open in editor"))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    open_file(window, cx);
                })
        });
    div()
        .id(id.clone())
        .h(FILE_HEAD_HEIGHT)
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(px(7.))
        .border_1()
        .border_color(t.border)
        .map(|head| {
            if open {
                head.rounded_t(RADIUS_2).border_b_0()
            } else {
                head.rounded(RADIUS_2)
            }
        })
        .bg(t.surface)
        .font_family(theme::UI)
        .cursor_pointer()
        .child(motion::turn(
            format!("{id}:chevron"),
            motion::fold_angle(open),
            motion::DETAIL_FOLD,
            super::icon(Icon::Chevron, 11.).text_color(t.text_tertiary),
        ))
        .child(crate::file_icons::glyph(&base, 14., cx))
        .child(name)
        .child(div().flex_1())
}
