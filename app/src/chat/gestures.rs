//! What an agent's gestures leave in a chat: emoji on the message it answered,
//! and GIFs where it sent them.

use gpui_kit::{
    div, img, px, AnyElement, App, FontWeight, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, SharedString, Styled, StyledImage,
};
use sikemux_wire::chat::{ChatMessage, GifRef};

use super::Drawn;
use crate::picture::{Picture, Remote};
use crate::theme::{self, Tokens, SPACE_1, SPACE_2H, TEXT_3XS, TEXT_XS};
use crate::ui;

/// The longest side a GIF is drawn at, in points.
const GIF_SIDE: f32 = 300.;
/// How far the emoji hang over the bottom of the bubble.
const OVERHANG: f32 = 9.;
/// An emoji draws a little taller than its font size, so its chip leaves room around it.
const CHIP_HEIGHT: f32 = 22.;

/// The emoji the agent reacted with, hanging off the bottom of the message it answered.
pub fn reactions(message: &ChatMessage, looks: &Drawn) -> Option<AnyElement> {
    if message.reactions.is_empty() {
        return None;
    }
    Some(
        div()
            .mt(px(-OVERHANG))
            .mr(SPACE_2H)
            .flex()
            .gap(SPACE_1)
            .children(message.reactions.iter().map(|emoji| {
                div()
                    .h(px(CHIP_HEIGHT))
                    .min_w(px(CHIP_HEIGHT + 6.))
                    .px(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(looks.line)
                    .bg(looks.bubble)
                    .child(
                        ui::text(emoji.clone(), TEXT_XS, FontWeight::NORMAL).line_height(px(12.)),
                    )
            }))
            .into_any_element(),
    )
}

/// A GIF the agent sent, where it sent it, centred across the reply and credited as KLIPY asks.
pub fn gif(id: &str, gif: &GifRef, looks: &Drawn, cx: &App) -> AnyElement {
    let (width, height) = fit(gif.width, gif.height);
    let asks = looks
        .bench
        .upgrade()
        .map(|bench| bench.read(cx).asks.clone());
    let shown = asks.map(|asks| {
        let picture = Picture::Remote(Remote::new(gif.url.clone(), asks));
        // Only an image with an id keeps the frame it is on, so without one a GIF holds still.
        img(picture.source())
            .id(SharedString::from(format!("gif:{id}")))
            .size_full()
            .rounded(theme::RADIUS_3)
            .object_fit(ObjectFit::Cover)
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(SPACE_1)
        .child(
            div()
                .w(px(width))
                .h(px(height))
                .rounded(theme::RADIUS_3)
                .bg(looks.bubble)
                .children(shown),
        )
        .child(
            ui::text("Powered by KLIPY", TEXT_3XS, FontWeight::NORMAL)
                .text_color(Tokens::get(cx).text_tertiary),
        )
        .into_any_element()
}

/// A GIF's size at no more than `GIF_SIDE` along its longer side, keeping its shape.
fn fit(width: u32, height: u32) -> (f32, f32) {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (GIF_SIDE / width.max(height)).min(1.);
    (width * scale, height * scale)
}
