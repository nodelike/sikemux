//! The colour themes the person picks from, and the one they picked: the
//! Sikemux themes, Ghostty's catalogue with the app's colours worked out from
//! each terminal palette, and the person's own, kept in preferences.

use std::sync::OnceLock;

use base64::Engine;
use sikemux_wire::grid::TerminalColors;
use sikemux_wire::theme::{
    Palette, Theme, ThemeChrome, ThemeEditor, ThemeHighlight, ThemeSource, ThemeSummary,
    ThemeTerminal, UiTokens,
};

use crate::protocol::{Preferences, DEFAULT_THEME_ID};

struct Catalogue {
    sikemux: Vec<Theme>,
    ghostty: Vec<Theme>,
}

fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        let sikemux: Vec<Theme> = serde_json::from_str(include_str!("themes/curated.json"))
            .expect("the Sikemux themes parse");
        let taken: Vec<String> = sikemux
            .iter()
            .map(|theme| slugify(&theme.name).replace('-', ""))
            .collect();
        let packed: Vec<(String, String)> =
            serde_json::from_str(include_str!("themes/ghostty.json"))
                .expect("Ghostty's catalogue parses");
        let ghostty = packed
            .into_iter()
            .filter(|(name, _)| !taken.contains(&slugify(name).replace('-', "")))
            .filter_map(|(name, packed)| ghostty_theme(&name, &unpack(&packed)?, None))
            .collect();
        Catalogue { sikemux, ghostty }
    })
}

/// The theme the person picked, or the default when it is gone.
pub(crate) fn resolve(preferences: &Preferences) -> Theme {
    let catalogue = catalogue();
    let find = |id: &str| {
        preferences
            .custom_themes
            .iter()
            .chain(&catalogue.sikemux)
            .chain(&catalogue.ghostty)
            .find(|theme| theme.id == id)
    };
    find(&preferences.theme_id)
        .or_else(|| find(DEFAULT_THEME_ID))
        .cloned()
        .expect("the default theme is in the catalogue")
}

/// The default theme's palette, for an app to draw with before its core answers.
pub fn default_palette() -> Palette {
    palette(&Preferences::default())
}

/// The picked theme with the colours worked out from it for drawing.
pub(crate) fn palette(preferences: &Preferences) -> Palette {
    let theme = resolve(preferences);
    Palette {
        tokens: tokens(&theme),
        theme,
    }
}

/// The tokens DESIGN.md names, worked out from a theme's colours as the Mac's
/// stylesheet and theme code work them out.
fn tokens(theme: &Theme) -> UiTokens {
    let (c, h) = (&theme.chrome, &theme.highlight);
    let (ink, bg, acc, bg_dim, ink_dim, line) = (
        rgba(&c.ink),
        rgba(&c.bg),
        rgba(&c.acc),
        rgba(&c.bg_dim),
        rgba(&c.ink_dim),
        rgba(&c.line),
    );
    let gray = |percent: f64| mix_oklab(ink, bg, percent / 100.);
    let readable_on = |color: &str, grounds: &[&str]| readable_css(color, grounds, &c.ink, 4.6);
    let grounds = [c.bg.as_str(), c.bg_dim.as_str(), c.bg_raised.as_str()];
    let near = [c.bg.as_str(), c.bg_raised.as_str()];
    let seed = |dark: &str, light: &str| {
        if theme.dark {
            dark.to_owned()
        } else {
            light.to_owned()
        }
    };
    let muted = readable_on(&c.ink_dim, &grounds);
    let live = readable_on(&seed("#78dca1", "#247345"), &near);
    let git_modified = readable_on(&seed("#e2c08d", "#875508"), &grounds);
    let git_deleted = readable_on(&seed("#ed806b", "#aa3824"), &grounds);
    let git_renamed = readable_on(&seed("#94b1ef", "#345bad"), &grounds);
    let cmd = rgba(&h.function);
    let toward_dim = |color: &str, amount: f64| show(mix_oklab(rgba(color), ink_dim, amount));
    UiTokens {
        sunken: show(bg_dim),
        ground: show(bg),
        raised: show(gray(7.)),
        overlay: show(gray(4.)),
        composer: show(gray(5.)),
        surface_composer: show(mix_srgb(gray(7.), bg_dim, 0.55)),
        hover: show(with_alpha(ink, 0.06)),
        chosen: show(with_alpha(ink, 0.09)),
        active: show(with_alpha(ink, 0.10)),
        selected: show(with_alpha(acc, 0.14)),
        tooltip: show(mix_srgb([1., 1., 1., 1.], gray(7.), 0.06 / 1.03)),
        border: show(gray(11.)),
        border_strong: show(gray(22.)),
        border_selected: show(with_alpha(acc, 0.42)),
        line: show(line),
        line_soft: show(rgba(&c.bg_raised)),
        ink: show(ink),
        secondary: show(gray(76.)),
        tertiary: muted.clone(),
        disabled: show(gray(32.)),
        ink_dim: show(ink_dim),
        ink_faint: muted.clone(),
        rest: show(gray(32.)),
        icon_quiet: show(gray(42.)),
        accent: show(acc),
        accent_soft: show(with_alpha(acc, 0.10)),
        on_accent: show(bg_dim),
        warn: readable_on(&seed("#e9ba6c", "#875508"), &near),
        danger: readable_on(&c.danger, &grounds),
        cmd: show(cmd),
        host: show(mix_oklab(acc, ink_dim, 0.8)),
        ssh: show(mix_oklab(cmd, ink_dim, 0.7)),
        tree_spine: show(mix_srgb(acc, line, 0.52)),
        tree_tick: show(with_alpha(acc, 0.72)),
        git_added: readable_on(&seed("#81b88b", "#247345"), &grounds),
        git_untracked: readable_on(&seed("#73c991", "#247345"), &grounds),
        tool_read: toward_dim(&git_renamed, 0.76),
        tool_edit: toward_dim(&git_modified, 0.76),
        tool_delete: toward_dim(&git_deleted, 0.76),
        tool_run: show(mix_oklab(cmd, ink_dim, 0.70)),
        tool_mcp: toward_dim(&live, 0.66),
        shader_dot: if theme.dark {
            show(rgba(&c.bg_raised))
        } else {
            show(mix_srgb(line, rgba(&muted), 0.9))
        },
        git_modified,
        git_deleted,
        git_renamed,
        live,
    }
}

/// [`readable`] for CSS colours; any that is not `#rgb` or `#rrggbb` gives `ink`.
fn readable_css(color: &str, grounds: &[&str], ink: &str, minimum: f64) -> String {
    let parsed = (
        hex(color),
        hex(ink),
        grounds
            .iter()
            .map(|ground| hex(ground))
            .collect::<Option<Vec<_>>>(),
    );
    match parsed {
        (Some(color), Some(ink), Some(grounds)) => css(readable(color, &grounds, ink, minimum)),
        _ => ink.to_owned(),
    }
}

/// Red, green, blue and alpha, each 0 to 1.
type Rgba = [f64; 4];

/// A CSS colour as themes write them: `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`,
/// `rgba()` or `transparent`; anything else reads as opaque black.
fn rgba(color: &str) -> Rgba {
    let color = color.trim();
    if color == "transparent" {
        return [0.; 4];
    }
    if let Some([r, g, b]) = hex(color) {
        let alpha = color
            .strip_prefix('#')
            .filter(|digits| digits.len() == 8)
            .and_then(|digits| u8::from_str_radix(&digits[6..], 16).ok())
            .map_or(1., |alpha| f64::from(alpha) / 255.);
        return [
            f64::from(r) / 255.,
            f64::from(g) / 255.,
            f64::from(b) / 255.,
            alpha,
        ];
    }
    let inside = color
        .strip_prefix("rgba(")
        .or_else(|| color.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'));
    let parts: Vec<f64> = inside
        .into_iter()
        .flat_map(|rest| rest.split(','))
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    match parts.as_slice() {
        [r, g, b, rest @ ..] => [
            r / 255.,
            g / 255.,
            b / 255.,
            rest.first().copied().unwrap_or(1.),
        ],
        _ => [0., 0., 0., 1.],
    }
}

/// How a phone draws a colour: `#rrggbb`, or `rgba(r, g, b, a)` when see-through.
fn show([r, g, b, a]: Rgba) -> String {
    let byte = |channel: f64| (channel.clamp(0., 1.) * 255.).round() as u8;
    let alpha = byte(a);
    if alpha == 255 {
        return css([byte(r), byte(g), byte(b)]);
    }
    let alpha = (f64::from(alpha) / 255. * 100.).round() / 100.;
    format!("rgba({}, {}, {}, {alpha})", byte(r), byte(g), byte(b))
}

fn with_alpha([r, g, b, _]: Rgba, alpha: f64) -> Rgba {
    [r, g, b, alpha]
}

/// CSS `color-mix(in srgb, a amount, b)`.
fn mix_srgb(a: Rgba, b: Rgba, amount: f64) -> Rgba {
    std::array::from_fn(|index| a[index] * amount + b[index] * (1. - amount))
}

/// CSS `color-mix(in oklab, a amount, b)`.
fn mix_oklab(a: Rgba, b: Rgba, amount: f64) -> Rgba {
    let (from, to) = (oklab(a), oklab(b));
    let mixed: [f64; 3] =
        std::array::from_fn(|index| from[index] * amount + to[index] * (1. - amount));
    let [r, g, bl] = from_oklab(mixed);
    [r, g, bl, a[3] * amount + b[3] * (1. - amount)]
}

fn oklab([r, g, b, _]: Rgba) -> [f64; 3] {
    let linear = |c: f64| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    ]
}

fn from_oklab([l, a, b]: [f64; 3]) -> [f64; 3] {
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    let gamma = |c: f64| {
        let c = c.clamp(0., 1.);
        if c <= 0.003_130_8 {
            12.92 * c
        } else {
            1.055 * c.powf(1. / 2.4) - 0.055
        }
    };
    [
        gamma(4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_),
        gamma(-1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_),
        gamma(-0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701_0 * s_),
    ]
}

/// The tokens by name, as a device view carries them to phones.
pub(crate) fn by_name(tokens: &UiTokens) -> std::collections::BTreeMap<String, String> {
    serde_json::to_value(tokens)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// The palette an unsaved theme would draw with, for a theme editor's preview.
pub(crate) fn palette_of(theme: Theme) -> Palette {
    Palette {
        tokens: tokens(&theme),
        theme,
    }
}

/// A theme worked out from a terminal palette as Ghostty's are: background,
/// foreground, cursor, cursor text, selection (or empty) and the 16 ANSI
/// colours, each `#rrggbb`, with the accent when the caller picked one.
pub(crate) fn from_colours(name: &str, colours: &[String], accent: Option<&str>) -> Option<Theme> {
    let colours: Vec<Option<[u8; 3]>> = colours.iter().map(|colour| hex(colour)).collect();
    (colours.len() == 21).then_some(())?;
    ghostty_theme(name, &colours, accent.and_then(hex))
}

/// Every theme to pick from: the person's own first, then Sikemux's, then Ghostty's.
pub(crate) fn summaries(preferences: &Preferences) -> Vec<ThemeSummary> {
    let catalogue = catalogue();
    let sources = [
        (ThemeSource::Custom, preferences.custom_themes.as_slice()),
        (ThemeSource::Sikemux, catalogue.sikemux.as_slice()),
        (ThemeSource::Ghostty, catalogue.ghostty.as_slice()),
    ];
    sources
        .into_iter()
        .flat_map(|(source, themes)| themes.iter().map(move |theme| summary(theme, source)))
        .collect()
}

fn summary(theme: &Theme, source: ThemeSource) -> ThemeSummary {
    let t = &theme.terminal;
    ThemeSummary {
        id: theme.id.clone(),
        name: theme.name.clone(),
        dark: theme.dark,
        source,
        bg: theme.chrome.bg.clone(),
        line: theme.chrome.line.clone(),
        acc: theme.chrome.acc.clone(),
        swatches: [&t.red, &t.green, &t.yellow, &t.blue, &t.magenta, &t.cyan]
            .into_iter()
            .cloned()
            .collect(),
    }
}

/// The colours the core answers programs' colour queries with.
pub(crate) fn terminal_colors(theme: &Theme) -> TerminalColors {
    let t = &theme.terminal;
    let fallback = TerminalColors::default();
    let ansi = [
        &t.black,
        &t.red,
        &t.green,
        &t.yellow,
        &t.blue,
        &t.magenta,
        &t.cyan,
        &t.white,
        &t.bright_black,
        &t.bright_red,
        &t.bright_green,
        &t.bright_yellow,
        &t.bright_blue,
        &t.bright_magenta,
        &t.bright_cyan,
        &t.bright_white,
    ];
    TerminalColors {
        foreground: hex(&t.foreground).unwrap_or(fallback.foreground),
        background: hex(&t.background).unwrap_or(fallback.background),
        cursor: hex(&t.cursor),
        ansi: std::array::from_fn(|index| hex(ansi[index]).unwrap_or(fallback.ansi[index])),
    }
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`, ignoring the alpha.
fn hex(color: &str) -> Option<[u8; 3]> {
    let digits = color.trim().strip_prefix('#')?;
    let digits: String = match digits.len() {
        3 => digits.chars().flat_map(|digit| [digit, digit]).collect(),
        6 | 8 => digits[..6].to_owned(),
        _ => return None,
    };
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for character in name.to_lowercase().chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_owned()
}

/// The catalogue keeps each theme's colours as base64 bytes, three to a
/// colour; a theme with no selection colour has 20 colours instead of 21.
fn unpack(packed: &str) -> Option<Vec<Option<[u8; 3]>>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(packed)
        .ok()?;
    let mut colours: Vec<Option<[u8; 3]>> = bytes
        .as_chunks::<3>()
        .0
        .iter()
        .map(|rgb| Some(*rgb))
        .collect();
    if colours.len() == 20 {
        colours.insert(4, None);
    }
    (colours.len() == 21).then_some(colours)
}

fn css([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn mix(from: [u8; 3], to: [u8; 3], amount: f64) -> [u8; 3] {
    std::array::from_fn(|index| {
        let (a, b) = (f64::from(from[index]), f64::from(to[index]));
        (a + (b - a) * amount).round() as u8
    })
}

fn alpha([r, g, b]: [u8; 3], amount: f64) -> String {
    format!("rgba({r},{g},{b},{amount})")
}

fn luminance(rgb: [u8; 3]) -> f64 {
    let linear = |channel: u8| {
        let c = f64::from(channel) / 255.;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}

fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (first, second) = (luminance(a), luminance(b));
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

/// `color` moved toward `ink` just far enough to read at `minimum` contrast on every background.
fn readable(color: [u8; 3], backgrounds: &[[u8; 3]], ink: [u8; 3], minimum: f64) -> [u8; 3] {
    let toward = |step: u8| -> [u8; 3] {
        std::array::from_fn(|index| {
            let (a, b) = (f64::from(color[index]), f64::from(ink[index]));
            (a + (b - a) * f64::from(step) / 100.).round() as u8
        })
    };
    (0..=100)
        .map(toward)
        .find(|candidate| {
            backgrounds
                .iter()
                .all(|background| contrast(*candidate, *background) >= minimum)
        })
        .unwrap_or(ink)
}

fn saturation([r, g, b]: [u8; 3]) -> f64 {
    let (r, g, b) = (
        f64::from(r) / 255.,
        f64::from(g) / 255.,
        f64::from(b) / 255.,
    );
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max == min {
        return 0.;
    }
    (max - min) / (1. - (max + min - 1.).abs())
}

/// The app's, editor's and syntax colours worked out from a terminal palette,
/// the way the Mac app worked them out for Ghostty's themes.
fn ghostty_theme(
    name: &str,
    colours: &[Option<[u8; 3]>],
    accent: Option<[u8; 3]>,
) -> Option<Theme> {
    let (bg, raw_fg, cursor, cursor_text) = (colours[0]?, colours[1]?, colours[2]?, colours[3]?);
    let raw_selection = colours[4];
    let ansi: Vec<[u8; 3]> = colours[5..].iter().copied().collect::<Option<_>>()?;
    let (white, black) = ([0xff; 3], [0; 3]);
    let dark = contrast(white, bg) >= contrast(black, bg);
    let bg_dim = mix(bg, black, if dark { 0.22 } else { 0.035 });
    let bg_raised = mix(bg, raw_fg, if dark { 0.06 } else { 0.045 });
    let fg = readable(
        raw_fg,
        &[bg, bg_dim, bg_raised],
        if dark { white } else { black },
        5.,
    );
    let acc = accent.unwrap_or_else(|| {
        [
            ansi[4], ansi[12], ansi[5], ansi[13], ansi[6], ansi[14], ansi[3], ansi[11],
        ]
        .into_iter()
        .find(|&candidate| saturation(candidate) >= 0.35 && contrast(candidate, bg) >= 3.)
        .unwrap_or(fg)
    });
    let line = mix(bg, fg, if dark { 0.11 } else { 0.13 });
    let selection = raw_selection
        .filter(|&selection| contrast(selection, fg) >= 2.)
        .unwrap_or_else(|| mix(bg, acc, 0.28));
    let ink_dim = mix(fg, bg, 0.38);
    let tone = |color: [u8; 3]| css(readable(color, &[bg], fg, 3.));
    let terminal = |index: usize| css(ansi[index]);
    Some(Theme {
        id: format!("ghostty-{}", slugify(name)),
        name: name.to_owned(),
        dark,
        chrome: ThemeChrome {
            bg: css(bg),
            bg_dim: css(bg_dim),
            bg_raised: css(bg_raised),
            ink: css(fg),
            ink_dim: css(ink_dim),
            ink_muted: css(mix(fg, bg, 0.5)),
            acc: css(acc),
            acc_line: alpha(acc, 0.3),
            acc_dim: alpha(acc, 0.1),
            line: css(line),
            hl: alpha(acc, 0.055),
            danger: terminal(1),
        },
        editor: ThemeEditor {
            fg: css(fg),
            bg: "transparent".to_owned(),
            caret: css(cursor),
            selection: css(selection),
            active_line: alpha(acc, 0.055),
            gutter: css(mix(fg, bg, 0.62)),
            gutter_active: css(acc),
            indent: css(line),
            indent_active: css(mix(bg, fg, 0.2)),
        },
        highlight: ThemeHighlight {
            keyword: tone(ansi[5]),
            string: tone(ansi[2]),
            comment: tone(ansi[8]),
            number: tone(ansi[3]),
            function: tone(ansi[4]),
            kind: tone(ansi[11]),
            variable: css(fg),
            property: tone(ansi[6]),
            tag: tone(ansi[1]),
            operator: css(ink_dim),
            link: tone(ansi[6]),
            invalid: tone(ansi[9]),
            meta: css(ink_dim),
            heading: css(acc),
        },
        terminal: ThemeTerminal {
            background: css(bg),
            foreground: css(raw_fg),
            cursor: css(cursor),
            cursor_accent: css(cursor_text),
            selection_background: css(selection),
            black: terminal(0),
            red: terminal(1),
            green: terminal(2),
            yellow: terminal(3),
            blue: terminal(4),
            magenta: terminal(5),
            cyan: terminal(6),
            white: terminal(7),
            bright_black: terminal(8),
            bright_red: terminal(9),
            bright_green: terminal(10),
            bright_yellow: terminal(11),
            bright_blue: terminal(12),
            bright_magenta: terminal(13),
            bright_cyan: terminal(14),
            bright_white: terminal(15),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::default_palette;

    /// The shared chat page draws with this copy, having no core to ask.
    const COPY: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../server/app/src/chat/themes/defaultPalette.json"
    );

    #[test]
    fn the_default_palette_copy_is_up_to_date() {
        let palette = default_palette();
        if std::env::var_os("SIKEMUX_WRITE_DEFAULT_PALETTE").is_some() {
            let pretty = serde_json::to_string_pretty(&palette).expect("a palette serializes");
            std::fs::write(COPY, pretty + "\n").expect("the copy is writable");
        }
        let copy: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(COPY).expect("the copy exists"))
                .expect("the copy parses");
        assert!(
            copy == serde_json::to_value(&palette).expect("a palette serializes"),
            "server/app/src/chat/themes/defaultPalette.json is out of date: in src-tauri, run \
             SIKEMUX_WRITE_DEFAULT_PALETTE=1 cargo test -p sikemux-core --lib default_palette_copy"
        );
    }
}
