//! YOLO's look: a rainbow through the theme's own colours, as the word in the composer and as
//! the composer's edge.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{
    div, img, linear_color_stop, linear_gradient, Div, Hsla, Image, ImageFormat, Img,
    ParentElement, Pixels, Rgba, Styled,
};

use crate::theme::{self, Tokens};

const LETTERS: &str = "M257 700V406L8 0H141L315 289L489 0H622L373 405V700Z M1104 712Q1000 712 920 666Q840 620 794 538Q749 456 749 350Q749 244 794 162Q840 81 919 34Q998 -12 1101 -12Q1205 -12 1284 34Q1364 81 1408 162Q1453 244 1453 350Q1453 456 1408 538Q1364 620 1286 666Q1207 712 1104 712ZM1104 606Q1172 606 1224 573Q1276 540 1306 482Q1335 425 1335 350Q1335 276 1305 218Q1275 160 1222 128Q1170 95 1101 95Q1033 95 980 128Q928 160 898 218Q867 275 867 350Q867 425 898 482Q928 540 982 573Q1035 606 1104 606Z M1656 700V0H1772V594H2089V700Z M2577 712Q2473 712 2393 666Q2313 620 2268 538Q2222 456 2222 350Q2222 244 2268 162Q2313 81 2392 34Q2471 -12 2574 -12Q2678 -12 2758 34Q2837 81 2882 162Q2926 244 2926 350Q2926 456 2882 538Q2837 620 2758 666Q2680 712 2577 712ZM2577 606Q2645 606 2697 573Q2749 540 2778 482Q2808 425 2808 350Q2808 276 2778 218Q2748 160 2696 128Q2643 95 2574 95Q2506 95 2454 128Q2401 160 2370 218Q2340 275 2340 350Q2340 425 2370 482Q2401 540 2454 573Q2508 606 2577 606Z";
const VIEW: (f32, f32, f32, f32) = (8., -12., 2918., 724.);

/// The rainbow, left to right: the theme's accent, then its code's keyword, number and string
/// colours, each one only where it differs from the one before.
fn stops(t: &Tokens) -> Vec<Hsla> {
    let code = &t.highlight;
    let mut stops: Vec<Hsla> = Vec::new();
    let colors = [t.accent]
        .into_iter()
        .chain([&code.keyword, &code.number, &code.string].map(|css| theme::color(css)));
    for color in colors {
        if stops.last() != Some(&color) {
            stops.push(color);
        }
    }
    stops
}

fn hex(color: Hsla) -> String {
    let Rgba { r, g, b, .. } = color.into();
    let byte = |part: f32| (part.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}

thread_local! {
    static DRAWN: RefCell<HashMap<(u32, String), Arc<Image>>> = RefCell::default();
}

/// The word drawn `tall` pixels high in `stops`, at twice that for a Retina screen, since GPUI
/// draws an SVG image at the size it states.
fn drawn(tall: f32, stops: &[Hsla]) -> Arc<Image> {
    let colors: Vec<String> = stops.iter().map(|stop| hex(*stop)).collect();
    DRAWN.with_borrow_mut(|drawn| {
        drawn
            .entry((tall.to_bits(), colors.join(",")))
            .or_insert_with(|| {
                let (x, y, width, height) = VIEW;
                let last = (colors.len() - 1).max(1) as f32;
                let stops: String = (colors.iter().enumerate())
                    .map(|(at, color)| {
                        let offset = at as f32 / last;
                        format!(r#"<stop offset="{offset}" stop-color="{color}"/>"#)
                    })
                    .collect();
                let (shown_width, shown_height) = (tall * 2. * width / height, tall * 2.);
                let svg = format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" width="{shown_width}" height="{shown_height}" viewBox="{x} {y} {width} {height}"><defs><linearGradient id="g" gradientUnits="userSpaceOnUse" x1="{x}" y1="0" x2="{}" y2="0">{stops}</linearGradient></defs><path fill="url(#g)" d="{LETTERS}"/></svg>"##,
                    x + width
                );
                Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
            })
            .clone()
    })
}

/// The word YOLO in one smooth rainbow of the theme's colours, as tall as letters at `size` stand.
pub fn wordmark(size: f32, t: &Tokens) -> Img {
    let (_, _, width, height) = VIEW;
    let tall = size * height / 1000.;
    img(drawn(tall, &stops(t)))
        .flex_none()
        .h(theme::ui_text(tall))
        .w(theme::ui_text(tall * width / height))
}

/// The rainbow behind a box set in from it, so it shows only as the box's edge, stronger on a
/// light theme: one two-colour gradient between each pair of neighbours, as GPUI's take two stops.
pub fn edge(radius: Pixels, t: &Tokens) -> Div {
    let strength = if t.dark { 0.45 } else { 0.6 };
    let mut colors: Vec<Hsla> = (stops(t).into_iter())
        .map(|color| Hsla {
            a: strength,
            ..color
        })
        .collect();
    if colors.len() == 1 {
        colors.push(colors[0]);
    }
    let last = colors.len() - 2;
    div()
        .absolute()
        .inset_0()
        .flex()
        .children(colors.windows(2).enumerate().map(|(at, pair)| {
            let band = div().flex_1().h_full().bg(linear_gradient(
                90.,
                linear_color_stop(pair[0], 0.),
                linear_color_stop(pair[1], 1.),
            ));
            let band = if at == 0 {
                band.rounded_l(radius)
            } else {
                band
            };
            if at == last {
                band.rounded_r(radius)
            } else {
                band
            }
        }))
}
