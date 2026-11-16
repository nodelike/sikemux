//! A theme's backdrop checked and resolved: numbers in range, inks parsed, and its strength
//! held to where text over it still reads.

use sikemux_view::backdrop::{BackdropKind, GridShape, ThemeBackdrop, WornBackdrop};
use sikemux_view::theme::UiTokens;

use super::{contrast, css, hex, mix, rgba, written};

/// The most strength any backdrop takes.
pub const MOST_STRENGTH: f64 = 2.;
/// The side of the patch, in points, that stands for one lowercase letter.
const LETTER: f64 = 8.;
/// How solid a waiting pane's edge is at its brightest, where text beside it allows.
const GLOW_MOST: f64 = 0.8;

/// `backdrop` with every number in range and every ink a colour or a token's name, with what
/// was changed to fit; or why it cannot be a backdrop.
pub fn within(
    mut backdrop: ThemeBackdrop,
    tokens: &UiTokens,
) -> Result<(ThemeBackdrop, Vec<String>), String> {
    let mut notes = Vec::new();
    let mut fit = |name: &str, value: &mut f64, low: f64, high: f64| {
        let fitted = (value.clamp(low, high) * 100.).round() / 100.;
        if fitted != *value {
            notes.push(format!(
                "backdrop.{name} {value} is outside {low} to {high}, used {fitted}"
            ));
            *value = fitted;
        }
    };
    let b = &mut backdrop;
    fit("strength", &mut b.strength, 0., MOST_STRENGTH);
    fit("speed", &mut b.speed, 0., 2.);
    fit("scale", &mut b.scale, 0.5, 3.);
    fit("softness", &mut b.softness, 0., 1.);
    fit("dither", &mut b.dither, 0., 1.);
    fit("density", &mut b.density, 0., 1.);
    fit("twinkle", &mut b.twinkle, 0., 1.);
    fit("drift", &mut b.drift, 0., 1.);
    fit("cell", &mut b.cell, 8., 96.);
    fit("weight", &mut b.weight, 0.5, 3.);
    if !(1..=3).contains(&b.depth) {
        let depth = b.depth.clamp(1, 3);
        notes.push(format!(
            "backdrop.depth {} is outside 1 to 3, used {depth}",
            b.depth
        ));
        b.depth = depth;
    }
    if b.seed > 65535 {
        notes.push(format!(
            "backdrop.seed {} is over 65535, used 65535",
            b.seed
        ));
        b.seed = 65535;
    }
    if b.inks.len() > 3 {
        notes.push(format!(
            "backdrop.inks has {}, used the first three",
            b.inks.len()
        ));
        b.inks.truncate(3);
    }
    let named = names(tokens);
    for ink in &mut b.inks {
        if named.iter().any(|name| name == ink) {
            continue;
        }
        *ink = written(ink).ok_or_else(|| {
            format!("backdrop.inks has {ink}, which is neither a colour nor a token such as accent")
        })?;
    }
    notes.extend(strays(&backdrop));
    Ok((backdrop, notes))
}

/// The numbers given that the backdrop's kind does not take.
fn strays(backdrop: &ThemeBackdrop) -> Vec<String> {
    let plain = ThemeBackdrop::default();
    let given = [
        ("softness", backdrop.softness != plain.softness),
        ("dither", backdrop.dither != plain.dither),
        ("density", backdrop.density != plain.density),
        ("twinkle", backdrop.twinkle != plain.twinkle),
        ("depth", backdrop.depth != plain.depth),
        ("drift", backdrop.drift != plain.drift),
        ("shape", backdrop.shape != plain.shape),
        ("cell", backdrop.cell != plain.cell),
        ("weight", backdrop.weight != plain.weight),
    ];
    let takes: &[&str] = match backdrop.kind {
        BackdropKind::Grain => &[],
        BackdropKind::Gradient => &["softness", "dither"],
        BackdropKind::Stars => &["density", "twinkle", "depth", "drift"],
        BackdropKind::Grid => &["shape", "cell", "weight", "drift"],
    };
    let kind = serde_json::to_value(backdrop.kind)
        .ok()
        .and_then(|kind| kind.as_str().map(str::to_owned))
        .unwrap_or_default();
    given
        .into_iter()
        .filter(|(name, given)| *given && !takes.contains(name))
        .map(|(name, _)| format!("backdrop.{name} does nothing for {kind}"))
        .collect()
}

/// `backdrop` with its strength lowered until text over it reads, and what was lowered.
pub fn readable(mut backdrop: ThemeBackdrop, tokens: &UiTokens) -> (ThemeBackdrop, Vec<String>) {
    let most = most(&backdrop, &inks(&backdrop, tokens), tokens);
    if backdrop.strength <= most {
        return (backdrop, Vec::new());
    }
    let note = format!(
        "backdrop.strength {} lowered to {most} so text over it reads",
        backdrop.strength
    );
    backdrop.strength = most;
    (backdrop, vec![note])
}

/// The backdrop as clients draw it under the theme's `tokens`.
pub fn worn(backdrop: &ThemeBackdrop, tokens: &UiTokens) -> WornBackdrop {
    let inks = inks(backdrop, tokens);
    let warn = hex(&tokens.warn).map(css).unwrap_or_default();
    WornBackdrop {
        most: most(backdrop, &inks, tokens),
        glow: glow(&warn, tokens),
        look: backdrop.clone(),
        inks,
        warn,
    }
}

/// How solid a waiting pane's warn edge may be with the dim ink beside it still at 1.5:1, or
/// nine tenths of what the plain ground gives where that is less.
fn glow(warn: &str, tokens: &UiTokens) -> f64 {
    let (Some(warn), Some(ground), Some(dim)) =
        (hex(warn), hex(&tokens.sunken), hex(&tokens.ink_dim))
    else {
        return 0.;
    };
    let needs = 1.5_f64.min(0.9 * contrast(dim, ground));
    highest(GLOW_MOST, |alpha| {
        contrast(dim, mix(ground, warn, alpha)) >= needs
    })
}

/// The backdrop's inks as `#rrggbb`, its kind's own when it names none. The grain with none
/// keeps no ink of its own and is drawn in the theme's grain colour, as it always was.
fn inks(backdrop: &ThemeBackdrop, tokens: &UiTokens) -> Vec<String> {
    let own: &[&str] = match backdrop.kind {
        BackdropKind::Grain => &[],
        BackdropKind::Gradient => &["accent", "cmd"],
        BackdropKind::Stars => &["ink", "accent"],
        BackdropKind::Grid => &["borderStrong"],
    };
    let given: Vec<&str> = match backdrop.inks.is_empty() {
        true => own.to_vec(),
        false => backdrop.inks.iter().map(String::as_str).collect(),
    };
    let named = serde_json::to_value(tokens).unwrap_or_default();
    given
        .into_iter()
        .map(|ink| {
            named
                .get(ink)
                .and_then(|token| token.as_str())
                .unwrap_or(ink)
        })
        .map(|ink| {
            let [r, g, b, _] = rgba(ink);
            css([r, g, b].map(|channel| (channel.clamp(0., 1.) * 255.).round() as u8))
        })
        .collect()
}

fn names(tokens: &UiTokens) -> Vec<String> {
    match serde_json::to_value(tokens) {
        Ok(serde_json::Value::Object(fields)) => fields.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The most strength `backdrop` can take with text over it still reading, in hundredths. Over
/// the brightest letter-sized patch the kind reaches, the ink keeps 4.5:1 and the dim ink 3:1;
/// over its brightest single pixel the dim ink keeps 1.5:1. Where the plain ground already
/// gives less, nine tenths of that is enough. The grain in the theme's own colour is never held.
fn most(backdrop: &ThemeBackdrop, inks: &[String], tokens: &UiTokens) -> f64 {
    let inks: Vec<[u8; 3]> = inks.iter().filter_map(|ink| hex(ink)).collect();
    let (Some(ground), Some(ink), Some(dim)) =
        (hex(&tokens.sunken), hex(&tokens.ink), hex(&tokens.ink_dim))
    else {
        return MOST_STRENGTH;
    };
    if inks.is_empty() {
        return MOST_STRENGTH;
    }
    let need = |text: [u8; 3], wanted: f64| wanted.min(0.9 * contrast(text, ground));
    let (ink_needs, dim_needs, point_needs) = (need(ink, 4.5), need(dim, 3.), need(dim, 1.5));
    let share = patch_share(backdrop);
    let solid = backdrop.kind.solid();
    let holds = |strength: f64| {
        let alpha = (solid * strength).min(1.);
        inks.iter().all(|&over| {
            let patch = mix(ground, over, alpha * share);
            let point = mix(ground, over, alpha);
            contrast(ink, patch) >= ink_needs
                && contrast(dim, patch) >= dim_needs
                && contrast(dim, point) >= point_needs
        })
    };
    highest(MOST_STRENGTH, holds)
}

/// The highest value up to `most`, in hundredths, at which `holds` still does.
fn highest(most: f64, holds: impl Fn(f64) -> bool) -> f64 {
    if holds(most) {
        return most;
    }
    let (mut low, mut high) = (0., most);
    for _ in 0..24 {
        let middle = 0.5 * (low + high);
        match holds(middle) {
            true => low = middle,
            false => high = middle,
        }
    }
    (low * 100.).floor() / 100.
}

/// How much of a letter-sized patch the kind can cover at its brightest, from 0 to 1.
fn patch_share(backdrop: &ThemeBackdrop) -> f64 {
    let area = LETTER * LETTER;
    match backdrop.kind {
        BackdropKind::Grain | BackdropKind::Gradient => 1.,
        BackdropKind::Stars => {
            let radius = 1.1 * backdrop.scale;
            (std::f64::consts::PI * radius * radius / area).min(1.)
        }
        BackdropKind::Grid => {
            let cell = backdrop.cell * backdrop.scale;
            match backdrop.shape {
                GridShape::Lines => {
                    let across = (LETTER / cell).max(1.) * backdrop.weight / LETTER;
                    1. - (1. - across.min(1.)).powi(2)
                }
                GridShape::Dots => {
                    let dots = (LETTER / cell).max(1.).powi(2);
                    let radius = backdrop.weight;
                    (dots * std::f64::consts::PI * radius * radius / area).min(1.)
                }
            }
        }
    }
}
