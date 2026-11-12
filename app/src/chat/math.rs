//! A block of math in an agent's reply, from `$$…$$` or a ```math fence, typeset once the
//! agent has finished writing it, with its LaTeX a click away.

use std::sync::{Arc, LazyLock};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, AnyElement, App, Asset, ElementId, Hsla, ImageSource, InteractiveElement,
    IntoElement, ParentElement, Rgba, SharedString, StatefulInteractiveElement, Styled, Window,
};
use gpui_transcript::Fence;
use latex_rust::{Color, Dim, MathFont, SvgOptions};
use resvg::usvg;

use super::mermaid::{self, Drawing};
use crate::theme::{self, Tokens, SPACE_1, SPACE_2};

/// The size of display math's letters, in points, at the chat's own text size.
const EM_POINTS: f32 = 12.;

/// A formula's LaTeX, the ink it is drawn in, and the scale it is drawn at.
#[derive(Clone, Hash, PartialEq, Eq)]
struct Wanted {
    tex: SharedString,
    ink: [u8; 3],
    /// The chat's text size, in hundredths.
    size: u32,
    /// The screen's scale, in hundredths.
    scale: u32,
}

pub(super) fn block(
    fence: Fence,
    running: bool,
    size: f32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let block = mermaid::kept(&fence, sikemux_markdown::MATH, running, window, cx);
    let t = Tokens::get(cx).clone();
    let drawn = block.read(cx).settled.clone().and_then(|tex| {
        let wanted = Wanted {
            tex,
            ink: rgb(theme::over(t.text_primary, t.surface)),
            size: (size * 100.).round() as u32,
            scale: (window.scale_factor() * 100.).round() as u32,
        };
        window.use_asset::<Typeset>(&wanted, cx)
    });
    let drawing = match drawn {
        Some(Ok(drawing)) if !block.read(cx).source => drawing,
        Some(Err(fault)) if !running => {
            return source(fence, Some(fault), &t);
        }
        _ => return source(fence, None, &t),
    };
    let id = ElementId::NamedChild(Arc::new(fence.id), "math".into());
    div()
        .id(id.clone())
        .group("math")
        .relative()
        .py(SPACE_2)
        .overflow_x_scroll()
        .child(
            div()
                .flex()
                .justify_center()
                .min_w(drawing.size.width)
                .child(
                    img(ImageSource::Render(drawing.image))
                        .w(drawing.size.width)
                        .h(drawing.size.height),
                ),
        )
        .child(
            div()
                .absolute()
                .top(SPACE_1)
                .right_0()
                .invisible()
                .group_hover("math", |shown| shown.visible())
                .child(fence.copy),
        )
        .on_click(move |event, _, cx| {
            if event.click_count() == 2 {
                block.update(cx, |block, cx| {
                    block.source = true;
                    cx.notify();
                });
            }
        })
        .tooltip(crate::ui::tooltip(t!("Double-click to see the LaTeX")))
        .into_any_element()
}

/// The block's LaTeX in a code card, and why it could not be typeset when it could not.
fn source(fence: Fence, fault: Option<SharedString>, t: &Tokens) -> AnyElement {
    let card = crate::reading::code_card(t);
    div()
        .id(ElementId::NamedChild(
            Arc::new(fence.id),
            "math-card".into(),
        ))
        .rounded(card.radius)
        .bg(card.ground)
        .overflow_hidden()
        .child(
            super::schema::head(fence.icon, fence.lang, t)
                .when_some(fault, |title, fault| {
                    title.child(
                        div()
                            .id("math-fault")
                            .mr(SPACE_1)
                            .child(t!("Can't typeset"))
                            .tooltip(crate::ui::tooltip(fault)),
                    )
                })
                .child(fence.copy),
        )
        .child(fence.lines)
        .into_any_element()
}

fn rgb(colour: Hsla) -> [u8; 3] {
    let colour = Rgba::from(colour);
    let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    [byte(colour.r), byte(colour.g), byte(colour.b)]
}

/// The math font every formula is set in, read once.
static FONT: LazyLock<Result<MathFont, String>> =
    LazyLock::new(|| MathFont::stix_two_math().map_err(|fault| fault.to_string()));

/// GPUI's loader for a formula, which typesets it off the main thread once for each text,
/// ink and scale.
enum Typeset {}

impl Asset for Typeset {
    type Source = Wanted;
    type Output = Result<Drawing, SharedString>;

    #[allow(clippy::manual_async_fn)]
    fn load(
        wanted: Wanted,
        _: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move { typeset(&wanted).map_err(SharedString::from) }
    }
}

fn typeset(wanted: &Wanted) -> Result<Drawing, String> {
    let font = FONT.as_ref()?;
    let [r, g, b] = wanted.ink;
    let options = SvgOptions {
        font_size_pt: Dim::ratio((EM_POINTS * wanted.size as f32).round() as i64, 100),
        color: Color::rgb(r, g, b),
        display: true,
    };
    let svg =
        latex_rust::latex_to_svg(&wanted.tex, font, &options).map_err(|fault| fault.to_string())?;
    let tree =
        usvg::Tree::from_str(&svg, &usvg::Options::default()).map_err(|fault| fault.to_string())?;
    mermaid::picture(&tree, wanted.scale as f32 / 100.)
}
