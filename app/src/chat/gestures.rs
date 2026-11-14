//! What an agent's gestures leave in a chat: GIFs where it sent them.

use gpui_kit::{
    div, img, px, AnyElement, App, FontWeight, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, SharedString, Styled, StyledImage,
};
use sikemux_view::chat::GifRef;

use super::Drawn;
use crate::picture::{Picture, Remote};
use crate::theme::{self, Tokens, SPACE_1, TEXT_3XS};
use crate::ui;

/// The longest side a GIF is drawn at, in points.
const GIF_SIDE: f32 = 300.;

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
            .rounded(theme::radius_3())
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
                .rounded(theme::radius_3())
                .bg(looks.bubble)
                .children(shown),
        )
        .child(
            ui::text(t!("Powered by KLIPY"), TEXT_3XS, FontWeight::NORMAL)
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
