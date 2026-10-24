//! DESIGN.md's tokens for the theme the core resolves, as GPUI colours.

use std::borrow::Cow;
use std::collections::BTreeMap;

use gpui_kit::component::theme::{Theme as KitTheme, ThemeTokens};
use gpui_kit::{
    px, rems, Anchor, App, BoxShadow, FontFeatures, Global, Hsla, Pixels, Rems, Rgba, SharedString,
};
use sikemux_wire::grid::TerminalColors;
use sikemux_wire::theme::{Palette, ThemeEditor, ThemeHighlight};

pub const UI: &str = "Figtree";

/// How much lighter-than-mid-grey text is thinned, as the power its glyphs' coverage is raised to
/// for white text: the person's pick, nearer WebKit's weight on a dark ground.
const LIGHT_TEXT_CURVE: f32 = 1.5;

/// GPUI Kit's own text size at the default text size. The Kit sets each window's rem to its
/// text size, so the interface's text is measured in it.
pub const REM: f32 = 13.;

/// The person's text size, as a share of the default, which GPUI Kit's text size follows.
#[derive(Clone, Copy)]
struct TextScale(f32);

impl Global for TextScale {}

/// A size for the interface's text, `size` points at the default text size, which grows and
/// shrinks with the person's choice in Appearance.
pub fn ui_text(size: f32) -> Rems {
    rems(size / REM)
}

/// Sizes the interface's text by `scale` from the next frame, in every window.
pub fn set_text_scale(scale: f32, cx: &mut App) {
    if cx
        .try_global::<TextScale>()
        .is_some_and(|kept| kept.0 == scale)
    {
        return;
    }
    cx.set_global(TextScale(scale));
    KitTheme::global_mut(cx).font_size = px(REM * scale);
    cx.refresh_windows();
}

pub const MONO: &str = "JetBrainsMono NFM";
/// The Nerd Font icon glyphs, which mono text falls back to.
pub const MONO_ICONS: &str = "JetBrainsMono NFM Icons";
pub const KBD: &str = ".SystemUIFont";

pub const RADIUS_1: Pixels = px(4.);
pub const RADIUS_2: Pixels = px(5.);
pub const RADIUS_3: Pixels = px(10.);
pub const RADIUS_4: Pixels = px(14.);

/// What a find in a terminal tints its matches with, the current one brighter, in every theme.
const FIND_MATCH: u32 = 0x433866;
const FIND_CURRENT: u32 = 0xa277ff;

#[derive(Clone)]
pub struct Tokens {
    pub surface_sunken: Hsla,
    pub surface: Hsla,
    pub surface_raised: Hsla,
    /// What a chat's composer and its docked strips are filled with.
    pub surface_composer: Hsla,
    pub surface_overlay: Hsla,
    pub surface_hover: Hsla,
    pub surface_chosen: Hsla,
    pub surface_active: Hsla,
    pub surface_selected: Hsla,
    pub surface_tooltip: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub border_selected: Hsla,
    pub line: Hsla,
    pub line_soft: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_disabled: Hsla,
    /// Line numbers and other text that stays out of the way of what it numbers.
    pub ink_faint: Hsla,
    pub dark: bool,
    /// How git sees a file: changed, new to it, added, gone, or moved.
    pub git_modified: Hsla,
    pub git_untracked: Hsla,
    pub git_added: Hsla,
    pub git_deleted: Hsla,
    pub git_renamed: Hsla,
    /// Header actions and other glyphs that recede until hovered.
    pub icon_quiet: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub live: Hsla,
    pub warn: Hsla,
    pub cmd: Hsla,
    /// The mark of a host on the account.
    pub host: Hsla,
    /// An SSH host's mark.
    pub ssh: Hsla,
    pub tree_spine: Hsla,
    pub tree_tick: Hsla,
    /// The dots of the grain behind panes.
    pub shader_dot: Hsla,
    /// What named and indexed colours look like in a terminal; the core answers programs' colour queries with it.
    pub terminal: TerminalColors,
    pub terminal_cursor_text: Hsla,
    pub terminal_selection: Hsla,
    pub terminal_match: Hsla,
    pub terminal_match_current: Hsla,
    /// The code editor's colours and syntax highlighting, as CSS colours [`color`] reads.
    pub editor: ThemeEditor,
    pub highlight: ThemeHighlight,
}

impl Global for Tokens {}

impl Tokens {
    pub fn new(palette: &Palette) -> Self {
        let (k, t) = (&palette.tokens, &palette.theme.terminal);
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
        Self {
            surface_sunken: color(&k.sunken),
            surface: color(&k.ground),
            surface_raised: color(&k.raised),
            surface_composer: color(&k.surface_composer),
            surface_overlay: color(&k.overlay),
            surface_hover: color(&k.hover),
            surface_chosen: color(&k.chosen),
            surface_active: color(&k.active),
            surface_selected: color(&k.selected),
            surface_tooltip: color(&k.tooltip),
            border: color(&k.border),
            border_strong: color(&k.border_strong),
            border_selected: color(&k.border_selected),
            line: color(&k.line),
            line_soft: color(&k.line_soft),
            text_primary: color(&k.ink),
            text_secondary: color(&k.secondary),
            text_tertiary: color(&k.tertiary),
            text_disabled: color(&k.disabled),
            ink_faint: color(&k.ink_faint),
            dark: palette.theme.dark,
            git_modified: color(&k.git_modified),
            git_untracked: color(&k.git_untracked),
            git_added: color(&k.git_added),
            git_deleted: color(&k.git_deleted),
            git_renamed: color(&k.git_renamed),
            icon_quiet: color(&k.icon_quiet),
            accent: color(&k.accent),
            on_accent: color(&k.on_accent),
            danger: color(&k.danger),
            live: color(&k.live),
            warn: color(&k.warn),
            cmd: color(&k.cmd),
            host: color(&k.host),
            ssh: color(&k.ssh),
            tree_spine: color(&k.tree_spine),
            tree_tick: color(&k.tree_tick),
            shader_dot: color(&k.shader_dot),
            terminal: TerminalColors {
                foreground: bytes(parse(&t.foreground)),
                background: bytes(parse(&t.background)),
                cursor: Some(bytes(parse(&t.cursor))),
                ansi: ansi.map(|color| bytes(parse(color))),
            },
            terminal_cursor_text: color(&t.cursor_accent),
            terminal_selection: color(&t.selection_background),
            terminal_match: hex(FIND_MATCH).into(),
            terminal_match_current: hex(FIND_CURRENT).into(),
            editor: palette.theme.editor.clone(),
            highlight: palette.theme.highlight.clone(),
        }
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// DESIGN.md's `--shadow-4`, for anything that floats.
    pub fn shadow_float(&self) -> Vec<BoxShadow> {
        let black = |alpha: f32| Hsla {
            h: 0.,
            s: 0.,
            l: 0.,
            a: alpha,
        };
        vec![
            BoxShadow {
                color: black(0.44),
                offset: gpui_kit::point(px(0.), px(8.)),
                blur_radius: px(16.),
                spread_radius: px(-8.),
                inset: false,
            },
            BoxShadow {
                color: black(0.62),
                offset: gpui_kit::point(px(0.), px(28.)),
                blur_radius: px(64.),
                spread_radius: px(-16.),
                inset: false,
            },
        ]
    }
}

pub fn brand(provider: &str) -> Hsla {
    hex(match provider {
        "claude" => 0xd97757,
        "codex" => 0x7a9dff,
        "hermes" => 0xe0a050,
        "pi" => 0x7dd3fc,
        "opencode" => 0xa78bfa,
        "omp" => 0xf97316,
        "grok" => 0xfcfcfc,
        "muse" => 0x0081fb,
        "sikemux" => 0xa277ff,
        _ => 0x8b8898,
    })
    .into()
}

pub fn tabular() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

/// Loads the bundled fonts and draws with the default theme until the core says which is picked.
pub fn install(cx: &mut App) {
    let fonts = [
        include_bytes!("../../../notch/Fonts/Figtree_400Regular.ttf").as_slice(),
        include_bytes!("../../../notch/Fonts/Figtree_500Medium.ttf").as_slice(),
        include_bytes!("../../../notch/Fonts/Figtree_600SemiBold.ttf").as_slice(),
        include_bytes!("../assets/fonts/Figtree_400Regular_Italic.ttf").as_slice(),
        include_bytes!("../assets/fonts/Figtree_600SemiBold_Italic.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular-Base.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Bold-Base.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Italic-Base.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-BoldItalic-Base.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular-Icons.ttf").as_slice(),
    ];
    cx.text_system()
        .add_fonts(fonts.into_iter().map(Cow::Borrowed).collect())
        .expect("bundled fonts load");
    cx.text_system().set_antialiased_only(true);
    cx.text_system()
        .set_light_text_curve(Some(LIGHT_TEXT_CURVE));

    apply(&sikemux_core::server::themes::default_palette(), cx);
    let toasts = &mut KitTheme::global_mut(cx).notification;
    toasts.placement = Anchor::BottomCenter;
    toasts.width = px(520.);
    toasts.margins.bottom = px(16.);
}

/// Draws with `palette` from the next frame, GPUI Kit's widgets too.
pub fn apply(palette: &Palette, cx: &mut App) {
    let tokens = Tokens::new(palette);
    let scale = cx.try_global::<TextScale>().map_or(1., |scale| scale.0);
    let kit = KitTheme::global_mut(cx);
    kit.font_family = SharedString::new_static(UI);
    kit.mono_font_family = SharedString::new_static(MONO);
    kit.font_size = px(REM * scale);
    kit.mono_font_size = px(11.);
    kit.radius = RADIUS_2;
    kit.radius_lg = RADIUS_4;
    kit.background = tokens.surface;
    kit.foreground = tokens.text_primary;
    kit.muted_foreground = tokens.text_tertiary;
    kit.popover = tokens.surface_overlay;
    kit.popover_foreground = tokens.text_primary;
    kit.border = tokens.border;
    kit.input = tokens.border;
    kit.ring = tokens.border_selected;
    kit.caret = tokens.text_primary;
    kit.selection = Hsla {
        a: 0.30,
        ..tokens.accent
    };
    kit.accent = tokens.surface_hover;
    kit.accent_foreground = tokens.text_primary;
    kit.secondary_hover = tokens.surface_hover;
    kit.secondary_active = tokens.surface_chosen;
    kit.secondary_foreground = tokens.text_primary;
    kit.list_hover = tokens.surface_hover;
    kit.list_active = tokens.surface_chosen;
    kit.primary = tokens.accent;
    kit.primary_foreground = tokens.on_accent;
    kit.danger = tokens.danger;
    kit.info = tokens.accent;
    kit.success = tokens.live;
    kit.warning = tokens.warn;
    kit.scrollbar_thumb = Hsla {
        a: 0.42,
        ..tokens.text_tertiary
    };
    kit.tokens = ThemeTokens::from(&kit.colors);
    cx.set_global(tokens);
}

fn bytes(color: Rgba) -> [u8; 3] {
    [color.r, color.g, color.b].map(|channel| (channel * 255.).round() as u8)
}

/// A CSS colour as themes write them: `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()` or `transparent`.
pub fn color(css: &str) -> Hsla {
    parse(css).into()
}

fn parse(css: &str) -> Rgba {
    let css = css.trim();
    if css == "transparent" {
        return Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.,
        };
    }
    if let Some(digits) = css.strip_prefix('#') {
        let digits: String = if digits.len() == 3 {
            digits.chars().flat_map(|digit| [digit, digit]).collect()
        } else {
            digits.to_owned()
        };
        match (digits.len(), u32::from_str_radix(&digits, 16)) {
            (6, Ok(value)) => return gpui_kit::rgb(value),
            (8, Ok(value)) => return gpui_kit::rgba(value),
            _ => {}
        }
    }
    let channels = css
        .strip_prefix("rgba(")
        .or_else(|| css.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'));
    let parts: Vec<f32> = channels
        .into_iter()
        .flat_map(|rest| rest.split(','))
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    match parts.as_slice() {
        [r, g, b, rest @ ..] => Rgba {
            r: r / 255.,
            g: g / 255.,
            b: b / 255.,
            a: rest.first().copied().unwrap_or(1.),
        },
        _ => Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 1.,
        },
    }
}

fn hex(value: u32) -> Rgba {
    gpui_kit::rgb(value)
}

/// CSS `color-mix(in oklab, a amount, b)` for two opaque colours.
/// `a` mixed into `b` by `amount`, as CSS's `color-mix(in oklab, a amount, b)` does.
pub fn mix(a: Hsla, b: Hsla, amount: f32) -> Hsla {
    mix_oklab(a.into(), b.into(), amount).into()
}

fn mix_oklab(a: Rgba, b: Rgba, amount: f32) -> Rgba {
    let [l1, a1, b1] = oklab(a);
    let [l2, a2, b2] = oklab(b);
    let lerp = |x: f32, y: f32| x * amount + y * (1. - amount);
    from_oklab([lerp(l1, l2), lerp(a1, a2), lerp(b1, b2)])
}

fn oklab(color: Rgba) -> [f32; 3] {
    let linear = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(color.r), linear(color.g), linear(color.b));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3]) -> Rgba {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    let gamma = |c: f32| {
        let c = c.clamp(0., 1.);
        if c <= 0.003_130_8 {
            12.92 * c
        } else {
            1.055 * c.powf(1. / 2.4) - 0.055
        }
    };
    Rgba {
        r: gamma(4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_),
        g: gamma(-1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_),
        b: gamma(-0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_),
        a: 1.,
    }
}

/// The window's colours and fonts as the custom properties pages style against.
/// `ground` is what the page draws behind itself; with none it lets what is under it show.
pub fn page_rule(t: &Tokens, ground: Option<Hsla>) -> String {
    let css = |color: Hsla| {
        let rgba = color.to_rgb();
        format!(
            "rgba({},{},{},{})",
            (rgba.r * 255.).round(),
            (rgba.g * 255.).round(),
            (rgba.b * 255.).round(),
            rgba.a
        )
    };
    let variables = BTreeMap::from([
        (
            "--background".to_owned(),
            ground.map_or_else(|| "transparent".to_owned(), css),
        ),
        ("--foreground".to_owned(), css(t.text_primary)),
        ("--muted-foreground".to_owned(), css(t.text_secondary)),
        ("--accent".to_owned(), css(t.accent)),
        ("--border".to_owned(), css(t.border)),
        ("--font-sans".to_owned(), format!("{UI}, system-ui")),
        ("--font-mono".to_owned(), format!("{MONO}, ui-monospace")),
    ]);
    sikemux_core::pages::theme_rule(t.dark, &variables)
}
