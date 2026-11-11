//! An account's avatar: Boring Avatars' "marble" (MIT, boringdesigners), drawn from its name in
//! one of a few fixed palettes, so the same account looks the same in every theme.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{Image, ImageFormat};

const PALETTES: [[&str; 5]; 4] = [
    ["#00686c", "#32c2b9", "#edecb3", "#fad928", "#ff9915"],
    ["#002c2b", "#ff3d00", "#ffbc11", "#0a837f", "#076461"],
    ["#244242", "#51bd9c", "#a3e3b1", "#ffe8b3", "#ff2121"],
    ["#a8ab9b", "#172a38", "#ec4b5d", "#f48773", "#e0c590"],
];

thread_local! {
    static DRAWN: RefCell<HashMap<(String, usize), Arc<Image>>> = RefCell::default();
}

/// The avatar of the account named `name`, in the palette for its place among its agent's
/// accounts, so accounts listed together never share one.
pub(crate) fn avatar(name: &str, place: usize) -> Arc<Image> {
    let key = (name.to_owned(), place % PALETTES.len());
    DRAWN.with_borrow_mut(|drawn| {
        drawn
            .entry(key)
            .or_insert_with_key(|(name, palette)| {
                let svg = svg(name, &PALETTES[*palette]);
                Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
            })
            .clone()
    })
}

struct Element {
    color: &'static str,
    x: i64,
    y: i64,
    scale: f64,
    rotate: i64,
}

fn svg(name: &str, colors: &[&'static str; 5]) -> String {
    let number = hash(name);
    let parts: Vec<Element> = (0..3)
        .map(|at| {
            let seed = number * (at + 1);
            Element {
                color: colors[((number + at) % colors.len() as i64) as usize],
                x: unit(seed, 8, 1),
                y: unit(seed, 8, 2),
                scale: 1.2 + unit(seed, 4, 0) as f64 / 10.,
                rotate: unit(seed, 360, 1),
            }
        })
        .collect();
    let place = |part: &Element, scale: f64| {
        format!(
            "translate({} {}) rotate({} 40 40) scale({scale})",
            part.x, part.y, part.rotate
        )
    };
    format!(
        concat!(
            r##"<svg viewBox="0 0 80 80" fill="none" xmlns="http://www.w3.org/2000/svg" width="80" height="80">"##,
            r##"<mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="80" height="80"><rect width="80" height="80" rx="160" fill="#FFFFFF"/></mask>"##,
            r##"<g mask="url(#m)"><rect width="80" height="80" fill="{}"/>"##,
            r##"<path filter="url(#f)" d="M32.414 59.35L50.376 70.5H72.5v-71H33.728L26.5 13.381l19.057 27.08L32.414 59.35z" fill="{}" transform="{}"/>"##,
            r##"<path filter="url(#f)" style="mix-blend-mode:overlay" d="M22.216 24L0 46.75l14.108 38.129L78 86l-3.081-59.276-22.378 4.005 12.972 20.186-23.35 27.395L22.215 24z" fill="{}" transform="{}"/></g>"##,
            r##"<defs><filter id="f" filterUnits="userSpaceOnUse" color-interpolation-filters="sRGB"><feFlood flood-opacity="0" result="BackgroundImageFix"/><feBlend in="SourceGraphic" in2="BackgroundImageFix" result="shape"/><feGaussianBlur stdDeviation="7" result="effect1_foregroundBlur"/></filter></defs></svg>"##
        ),
        parts[0].color,
        parts[1].color,
        place(&parts[1], parts[2].scale),
        parts[2].color,
        place(&parts[2], parts[2].scale),
    )
}

/// The name as a number, as the original's JavaScript reckons it over UTF-16, so an account
/// draws the same as it would there.
fn hash(name: &str) -> i64 {
    let mut sum: i32 = 0;
    for unit in name.encode_utf16() {
        sum = sum
            .wrapping_shl(5)
            .wrapping_sub(sum)
            .wrapping_add(i32::from(unit));
    }
    i64::from(sum).abs()
}

fn digit(number: i64, place: u32) -> i64 {
    (number / 10_i64.pow(place)) % 10
}

fn unit(number: i64, range: i64, place: u32) -> i64 {
    let value = number % range;
    match place != 0 && digit(number, place) % 2 == 0 {
        true => -value,
        false => value,
    }
}
