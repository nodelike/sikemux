//! How the app looks: its theme, what shows behind the panes, the agents' faces and the size of
//! its text, with the finer controls folded under Advanced.

use crate::ui::Clicks;
use sikemux_calls::call;
use sikemux_calls::faces::FaceStyleChange;
use sikemux_view::prefs::PreferenceChange;
use std::path::PathBuf;

use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, AppContext, Context, Entity, Hsla, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Window,
};
use sikemux_view::backdrop::BackdropKind;
use sikemux_view::prefs::{
    ChatWidth, Contrast, Motion, PaneBackdrop, PaneColours, PaneCoverage, PanePattern, Preferences,
};
use sikemux_view::theme::{ThemeSource, ThemeSummary};

use super::theme_editor::Picture;
use crate::core::Target;
use crate::icons::Icon;
use crate::reads::Read;
use crate::settings::index::Page::Appearance;
use crate::settings::index::{row, section, Entry};
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H,
    TEXT_SM, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(
        Appearance,
        "Theme",
        "colours colors palette dark light custom fork editor wallpaper picture customize ghostty omarchy search",
    ),
    section(
        Appearance,
        "Backdrop",
        "dither dithering shader grain noise background effect texture gradient stars grid picture none",
    ),
    row(
        Appearance,
        "Backdrop",
        "Pane image",
        "picture photo wallpaper background dither dithered custom omarchy follow",
    ),
    section(
        Appearance,
        "Agent faces",
        "buddies agent icon character avatar mascot face pack style",
    ),
    row(
        Appearance,
        "Agent faces",
        "Show faces",
        "buddies agent icon character avatar mascot harness logo",
    ),
    section(Appearance, "Text", ""),
    row(
        Appearance,
        "Text",
        "Text size",
        "font zoom scale larger smaller accessibility",
    ),
    row(
        Appearance,
        "Text",
        "Chat width",
        "column wide full screen messages reading measure line length",
    ),
    section(Appearance, "Advanced", "reset fine tune"),
    row(
        Appearance,
        "Advanced",
        "Contrast",
        "increase contrast greys borders readable accessibility",
    ),
    row(
        Appearance,
        "Advanced",
        "Motion",
        "reduce motion animation still accessibility",
    ),
    row(
        Appearance,
        "Advanced",
        "Backdrop strength",
        "dither grain picture opacity intensity texture image pane",
    ),
    row(
        Appearance,
        "Advanced",
        "Window see-through",
        "opacity transparency translucent",
    ),
    row(
        Appearance,
        "Advanced",
        "Blur behind",
        "background blur vibrancy frosted glass",
    ),
    row(
        Appearance,
        "Advanced",
        "Face shape",
        "buddies agent icon character avatar mixed mix",
    ),
    row(
        Appearance,
        "Advanced",
        "Animate faces",
        "buddies motion blink animation",
    ),
    section(Appearance, "Picture tuning", "pane picture dither"),
    row(
        Appearance,
        "Picture tuning",
        "Colours",
        "pane picture dither inks palette theme accent custom gradient map",
    ),
    row(
        Appearance,
        "Picture tuning",
        "Inks",
        "pane picture dither colours palette custom gradient map",
    ),
    row(
        Appearance,
        "Picture tuning",
        "Pattern",
        "pane picture dither bayer ordered noise grid matrix",
    ),
    row(
        Appearance,
        "Picture tuning",
        "Dot size",
        "pane picture dither pixel scale cell",
    ),
    row(
        Appearance,
        "Picture tuning",
        "Picture contrast",
        "pane picture dither tone levels",
    ),
    row(
        Appearance,
        "Picture tuning",
        "Brightness",
        "pane picture dither tone exposure midtones",
    ),
    row(
        Appearance,
        "Picture tuning",
        "Glow",
        "pane picture dither bloom halation light",
    ),
    row(
        Appearance,
        "Picture tuning",
        "Glow size",
        "pane picture dither bloom radius spread",
    ),
    row(
        Appearance,
        "Picture tuning",
        "Covers",
        "pane picture dither band top whole full coverage",
    ),
];

/// Whether a search that leads to `section` should open the Advanced fold.
pub fn folded(section: &str) -> bool {
    matches!(section, "Advanced" | "Picture tuning")
}

/// How many themes, backdrops and face packs sit across a row.
const ACROSS: usize = 6;

/// Where each kind of theme comes among the cards, yours first.
const SOURCES: [ThemeSource; 4] = [
    ThemeSource::Custom,
    ThemeSource::Omarchy,
    ThemeSource::Sikemux,
    ThemeSource::Ghostty,
];

/// What the backdrop tiles offer, in their order.
const BACKDROPS: [(PaneBackdrop, &str); 6] = [
    (PaneBackdrop::Theme, n!("The theme's")),
    (PaneBackdrop::Gradient, n!("Gradient")),
    (PaneBackdrop::Stars, n!("Stars")),
    (PaneBackdrop::Grid, n!("Grid")),
    (PaneBackdrop::Picture, n!("Picture")),
    (PaneBackdrop::None, n!("None")),
];

/// What the Backdrop section says is worn: the theme's own kind by the theme's name, or the choice.
fn backdrop_about(choice: PaneBackdrop, theme: &str, kind: BackdropKind) -> SharedString {
    let kind = match kind {
        BackdropKind::Grain => t!("grain"),
        BackdropKind::Gradient => t!("gradient"),
        BackdropKind::Stars => t!("stars"),
        BackdropKind::Grid => t!("grid"),
    };
    match choice {
        PaneBackdrop::Theme => return t!("{theme}'s {kind}.", theme = theme, kind = kind).into(),
        PaneBackdrop::Grain => t!("The grain, as every theme draws it unless it names another."),
        PaneBackdrop::Gradient => t!("Soft washes of the theme's accent and command colours."),
        PaneBackdrop::Stars => t!("A field of stars in the theme's ink and accent."),
        PaneBackdrop::Grid => t!("Even dots in the theme's border colour."),
        PaneBackdrop::Picture => t!("Your own picture, dithered."),
        PaneBackdrop::None => t!("Nothing behind the panes."),
    }
    .into()
}

/// What the page keeps: the sliders and inks, each setting its preference as it moves, and the
/// theme being edited or read from a picture.
pub struct Fields {
    strength: Entity<SliderState>,
    dot_size: Entity<SliderState>,
    contrast: Entity<SliderState>,
    brightness: Entity<SliderState>,
    glow: Entity<SliderState>,
    glow_size: Entity<SliderState>,
    inks: Vec<Entity<ColorPickerState>>,
    opacity: Entity<SliderState>,
    blur: Entity<SliderState>,
    pub editor: Option<super::theme_editor::Editor>,
    /// Whether a picture is being read into a theme.
    pub reading: bool,
    _watching: Vec<Subscription>,
}

/// A slider's value, rounded as the settings keep it.
fn hundredths(value: f32) -> f64 {
    (value as f64 * 100.).round() / 100.
}

fn slider(
    change: fn(f32) -> PreferenceChange,
    range: (f32, f32, f32),
    value: f32,
    cx: &mut Context<Workbench>,
) -> (Entity<SliderState>, Subscription) {
    let (min, max, step) = range;
    let state = cx.new(|_| {
        SliderState::new()
            .min(min)
            .max(max)
            .step(step)
            .default_value(value)
    });
    let moved = cx.subscribe(&state, move |bench, _, event: &SliderEvent, _| {
        let SliderEvent::Change(value) = event else {
            return;
        };
        crate::settings::set(bench, change(value.start()));
    });
    (state, moved)
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    Target::here(&bench.asks).watch_plain(Read::Themes);
    let preferences = bench.preferences();
    let (strength, moved_strength) = slider(
        |value| PreferenceChange::PaneStrength(hundredths(value)),
        (0., 2., 0.05),
        preferences.pane_strength as f32,
        cx,
    );
    let (dot_size, moved_dot_size) = slider(
        |value| PreferenceChange::PaneDotSize(hundredths(value)),
        (1., 6., 0.01),
        preferences.pane_dot_size as f32,
        cx,
    );
    let (contrast, moved_contrast) = slider(
        |value| PreferenceChange::PaneContrast(hundredths(value)),
        (-1., 1., 0.05),
        preferences.pane_contrast as f32,
        cx,
    );
    let (brightness, moved_brightness) = slider(
        |value| PreferenceChange::PaneBrightness(hundredths(value)),
        (-1., 1., 0.05),
        preferences.pane_brightness as f32,
        cx,
    );
    let (glow, moved_glow) = slider(
        |value| PreferenceChange::PaneGlow(hundredths(value)),
        (0., 1., 0.05),
        preferences.pane_glow as f32,
        cx,
    );
    let (glow_size, moved_glow_size) = slider(
        |value| PreferenceChange::PaneGlowSize(hundredths(value)),
        (0., 1., 0.05),
        preferences.pane_glow_size as f32,
        cx,
    );
    let mut watching = vec![
        moved_strength,
        moved_dot_size,
        moved_contrast,
        moved_brightness,
        moved_glow,
        moved_glow_size,
    ];
    let inks = preferences
        .pane_inks
        .iter()
        .enumerate()
        .map(|(at, ink)| {
            let picker =
                cx.new(|cx| ColorPickerState::new(window, cx).default_value(theme::color(ink)));
            watching.push(cx.subscribe_in(
                &picker,
                window,
                move |bench, _, event: &ColorPickerEvent, _, _| {
                    let ColorPickerEvent::Change(Some(color)) = event else {
                        return;
                    };
                    let mut inks = bench.preferences().pane_inks.clone();
                    if let Some(ink) = inks.get_mut(at) {
                        *ink = super::theme_editor::hex(*color);
                    }
                    crate::settings::set(bench, PreferenceChange::PaneInks(inks));
                },
            ));
            picker
        })
        .collect();
    let (opacity, moved_opacity) = slider(
        |value| PreferenceChange::WindowOpacity(hundredths(value)),
        (0., 1., 0.01),
        preferences.window_opacity as f32,
        cx,
    );
    let (blur, moved_blur) = slider(
        |value| PreferenceChange::WindowBlur(value.round().into()),
        (0., 60., 1.),
        preferences.window_blur.min(60.) as f32,
        cx,
    );
    let fields = Fields {
        strength,
        dot_size,
        contrast,
        brightness,
        glow,
        glow_size,
        inks,
        opacity,
        blur,
        editor: None,
        reading: false,
        _watching: {
            watching.extend([moved_opacity, moved_blur]);
            watching
        },
    };
    super::change_pages(bench, cx, |pages, _| pages.appearance = Some(fields));
}

/// How the pane picture is dithered, each control in a row.
fn picture_rows(
    preferences: &Preferences,
    fields: &Fields,
    this: &Entity<Workbench>,
    knob: &dyn Fn(&Entity<SliderState>) -> gpui_kit::Div,
    cx: &App,
) -> impl IntoElement {
    let words = |id: &'static str,
                 labels: &[&'static str],
                 changes: Vec<PreferenceChange>,
                 chosen: usize| {
        let this = this.clone();
        ui::Segmented::words(
            id,
            labels
                .iter()
                .map(|label| (SharedString::from(*label), None))
                .collect(),
            chosen,
        )
        .on_select(move |at, _, cx| {
            let Some(change) = changes.get(*at).cloned() else {
                return;
            };
            this.update(cx, |bench, _| crate::settings::set(bench, change))
        })
    };
    let colours = words(
        "pane-colours",
        &[t!("Theme"), t!("Picture"), t!("Custom")],
        vec![
            PreferenceChange::PaneColours(PaneColours::Theme),
            PreferenceChange::PaneColours(PaneColours::Picture),
            PreferenceChange::PaneColours(PaneColours::Custom),
        ],
        match preferences.pane_colours {
            PaneColours::Theme => 0,
            PaneColours::Picture => 1,
            PaneColours::Custom => 2,
        },
    );
    let pattern = words(
        "pane-pattern",
        &["2×2", "4×4", "8×8", t!("Noise")],
        vec![
            PreferenceChange::PanePattern(PanePattern::Bayer2),
            PreferenceChange::PanePattern(PanePattern::Bayer4),
            PreferenceChange::PanePattern(PanePattern::Bayer8),
            PreferenceChange::PanePattern(PanePattern::Noise),
        ],
        match preferences.pane_pattern {
            PanePattern::Bayer2 => 0,
            PanePattern::Bayer4 => 1,
            PanePattern::Bayer8 => 2,
            PanePattern::Noise => 3,
        },
    );
    let coverage = words(
        "pane-coverage",
        &[t!("Top"), t!("Whole pane")],
        vec![
            PreferenceChange::PaneCoverage(PaneCoverage::Band),
            PreferenceChange::PaneCoverage(PaneCoverage::Full),
        ],
        match preferences.pane_coverage {
            PaneCoverage::Band => 0,
            PaneCoverage::Full => 1,
        },
    );
    div()
        .child(rows::row(
            t!("Colours"),
            Some(t!(
                "Theme dithers in the accent and text colour, Picture keeps its own."
            )),
            colours,
            cx,
        ))
        .when(preferences.pane_colours == PaneColours::Custom, |rows_of| {
            rows_of.child(rows::row(
                t!("Inks"),
                Some(t!("Dark, middle and light, in that order.")),
                div()
                    .flex()
                    .gap(SPACE_2)
                    .children(fields.inks.iter().map(ColorPicker::new)),
                cx,
            ))
        })
        .child(rows::row(t!("Pattern"), None, pattern, cx))
        .child(rows::row(t!("Dot size"), None, knob(&fields.dot_size), cx))
        .child(rows::row(
            t!("Picture contrast"),
            None,
            knob(&fields.contrast),
            cx,
        ))
        .child(rows::row(
            t!("Brightness"),
            None,
            knob(&fields.brightness),
            cx,
        ))
        .child(rows::row(
            t!("Glow"),
            Some(t!("Light around the picture's bright parts.")),
            knob(&fields.glow),
            cx,
        ))
        .child(rows::row(
            t!("Glow size"),
            None,
            knob(&fields.glow_size),
            cx,
        ))
        .child(rows::row(t!("Covers"), None, coverage, cx))
}

/// Whether any control under Advanced differs from its default.
fn advanced_changed(preferences: &Preferences, defaults: &Preferences) -> bool {
    preferences.contrast != defaults.contrast
        || preferences.motion != defaults.motion
        || preferences.pane_strength != defaults.pane_strength
        || preferences.window_opacity != defaults.window_opacity
        || preferences.window_blur != defaults.window_blur
        || preferences.pane_colours != defaults.pane_colours
        || preferences.pane_inks != defaults.pane_inks
        || preferences.pane_pattern != defaults.pane_pattern
        || preferences.pane_dot_size != defaults.pane_dot_size
        || preferences.pane_contrast != defaults.pane_contrast
        || preferences.pane_brightness != defaults.pane_brightness
        || preferences.pane_glow != defaults.pane_glow
        || preferences.pane_glow_size != defaults.pane_glow_size
        || preferences.pane_coverage != defaults.pane_coverage
}

/// Puts every control under Advanced back to its default, moving the sliders and inks with it.
fn reset_advanced(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let defaults = Preferences::default();
    for change in [
        PreferenceChange::Contrast(defaults.contrast),
        PreferenceChange::Motion(defaults.motion),
        PreferenceChange::PaneStrength(defaults.pane_strength),
        PreferenceChange::WindowOpacity(defaults.window_opacity),
        PreferenceChange::WindowBlur(defaults.window_blur),
        PreferenceChange::PaneColours(defaults.pane_colours),
        PreferenceChange::PaneInks(defaults.pane_inks.clone()),
        PreferenceChange::PanePattern(defaults.pane_pattern),
        PreferenceChange::PaneDotSize(defaults.pane_dot_size),
        PreferenceChange::PaneContrast(defaults.pane_contrast),
        PreferenceChange::PaneBrightness(defaults.pane_brightness),
        PreferenceChange::PaneGlow(defaults.pane_glow),
        PreferenceChange::PaneGlowSize(defaults.pane_glow_size),
        PreferenceChange::PaneCoverage(defaults.pane_coverage),
    ] {
        crate::settings::set(bench, change);
    }
    let Some(fields) = super::state(bench, cx).and_then(|pages| pages.appearance.as_ref()) else {
        return;
    };
    let sliders = [
        (fields.strength.clone(), defaults.pane_strength as f32),
        (fields.opacity.clone(), defaults.window_opacity as f32),
        (fields.blur.clone(), defaults.window_blur.min(60.) as f32),
        (fields.dot_size.clone(), defaults.pane_dot_size as f32),
        (fields.contrast.clone(), defaults.pane_contrast as f32),
        (fields.brightness.clone(), defaults.pane_brightness as f32),
        (fields.glow.clone(), defaults.pane_glow as f32),
        (fields.glow_size.clone(), defaults.pane_glow_size as f32),
    ];
    let inks = fields.inks.clone();
    for (slider, value) in sliders {
        slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
    }
    for (picker, ink) in inks.iter().zip(&defaults.pane_inks) {
        picker.update(cx, |picker, cx| {
            picker.set_value(theme::color(ink), window, cx)
        });
    }
}

/// Asks for one picture file, and hands its path to `then`.
fn pick_picture(
    window: &mut Window,
    cx: &mut Context<Workbench>,
    then: impl FnOnce(&mut Workbench, PathBuf, &mut Window, &mut Context<Workbench>) + 'static,
) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    cx.spawn_in(window, async move |bench, cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        let _ = bench.update_in(cx, |bench, window, cx| then(bench, path, window, cx));
    })
    .detach();
}

/// The themes on the cards: the one worn, then the rest as the library orders them.
fn carded(themes: &[ThemeSummary], worn: &str) -> Vec<ThemeSummary> {
    let rest = SOURCES.iter().flat_map(|source| {
        themes
            .iter()
            .filter(move |theme| theme.source == *source && theme.id != worn)
    });
    themes
        .iter()
        .filter(|theme| theme.id == worn)
        .chain(rest)
        .take(ACROSS)
        .cloned()
        .collect()
}

/// A choice among a row of pictures: lit while chosen, shaded under the pointer.
fn tile(id: impl Into<SharedString>, chosen: bool, t: &Tokens) -> Stateful<gpui_kit::Div> {
    div()
        .id(id.into())
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .p(SPACE_1)
        .rounded(px(8.))
        .cursor_pointer()
        .when(chosen, |tile| tile.bg(t.surface_chosen))
        .when(!chosen, |tile| {
            tile.hover(|style| style.bg(t.surface_hover))
        })
}

/// A tile's name under its picture, with a tick while it is chosen.
fn tile_name(name: impl Into<SharedString>, chosen: bool, t: &Tokens) -> gpui_kit::Div {
    div()
        .h(px(22.))
        .flex()
        .items_center()
        .gap(SPACE_1)
        .pl(SPACE_0)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .text_color(if chosen {
                    t.text_primary
                } else {
                    t.text_secondary
                })
                .child(name.into()),
        )
        .when(chosen, |name| {
            name.child(ui::icon(Icon::Check, 12.).text_color(t.accent))
        })
}

/// Places, as parts of a tile's width and height, for the dots a backdrop's picture scatters.
const SCATTER: [(f32, f32); 9] = [
    (0.1, 0.22),
    (0.24, 0.7),
    (0.36, 0.34),
    (0.5, 0.8),
    (0.58, 0.18),
    (0.7, 0.55),
    (0.84, 0.3),
    (0.9, 0.78),
    (0.16, 0.48),
];

/// A small picture of how a backdrop looks, drawn from the theme's colours.
fn backdrop_swatch(choice: PaneBackdrop, kind: BackdropKind, t: &Tokens, cx: &App) -> AnyElement {
    let frame = div()
        .relative()
        .h(px(46.))
        .overflow_hidden()
        .rounded(px(6.))
        .border(crate::theme::border())
        .border_color(t.border)
        .bg(t.surface);
    let dot = |(x, y): (f32, f32), size: f32, color: Hsla| {
        div()
            .absolute()
            .left(relative(x))
            .top(relative(y))
            .size(px(size))
            .rounded(px(size))
            .bg(color)
    };
    let kind = match choice {
        PaneBackdrop::Theme => Some(kind),
        PaneBackdrop::Grain => Some(BackdropKind::Grain),
        PaneBackdrop::Gradient => Some(BackdropKind::Gradient),
        PaneBackdrop::Stars => Some(BackdropKind::Stars),
        PaneBackdrop::Grid => Some(BackdropKind::Grid),
        PaneBackdrop::Picture | PaneBackdrop::None => None,
    };
    match (choice, kind) {
        (PaneBackdrop::Picture, _) => match crate::ui::grain::picture_preview(cx) {
            Some(preview) => frame.child(preview.size_full()),
            None => frame
                .flex()
                .items_center()
                .justify_center()
                .child(ui::icon(Icon::Image, 14.).text_color(t.text_tertiary)),
        },
        (_, Some(BackdropKind::Gradient)) => frame.bg(gpui_kit::linear_gradient(
            135.,
            gpui_kit::linear_color_stop(t.accent.opacity(0.5), 0.),
            gpui_kit::linear_color_stop(t.surface, 1.),
        )),
        (_, Some(BackdropKind::Stars)) => {
            frame.children(SCATTER.iter().enumerate().map(|(at, place)| match at % 4 {
                0 => dot(*place, 3., t.accent),
                _ => dot(*place, 2., t.text_secondary),
            }))
        }
        (_, Some(BackdropKind::Grid)) => frame.children((0..7).flat_map(|column| {
            (0..3).map(move |line| {
                dot(
                    (0.08 + column as f32 * 0.14, 0.2 + line as f32 * 0.28),
                    2.,
                    t.border_strong,
                )
            })
        })),
        (_, Some(BackdropKind::Grain)) => frame.children((0..36).map(|at| {
            let x = (at * 37 % 97) as f32 / 97.;
            let y = (at * 53 % 89) as f32 / 89.;
            dot((x, y), 1., t.text_tertiary.opacity(0.6))
        })),
        _ => frame,
    }
    .into_any_element()
}

/// Six to a row, with empty places after the last so every tile keeps one width.
fn in_rows(tiles: Vec<AnyElement>) -> impl IntoElement {
    let mut tiles = tiles.into_iter().peekable();
    let mut lines = Vec::new();
    while tiles.peek().is_some() {
        let line: Vec<AnyElement> = tiles.by_ref().take(ACROSS).collect();
        let gaps = ACROSS - line.len();
        lines.push(
            div()
                .flex()
                .gap(SPACE_1H)
                .children(line)
                .children((0..gaps).map(|_| div().flex_1())),
        );
    }
    div()
        .px(SPACE_2)
        .pt(SPACE_2)
        .pb(SPACE_2)
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .children(lines)
}

/// A small picture of a theme made from its own colours: its ground and edge, its accent,
/// lines of text in its terminal's colours, and those colours along the foot.
fn preview(summary: &ThemeSummary, height: f32, rail: bool) -> gpui_kit::Div {
    let swatch = |at: usize| {
        summary
            .swatches
            .get(at)
            .map_or(theme::color(&summary.acc), |swatch| theme::color(swatch))
    };
    let bar = |color, width: f32| {
        div()
            .h(px(4.))
            .w(gpui_kit::relative(width))
            .rounded(px(2.))
            .bg(color)
    };
    let line = |parts: Vec<(gpui_kit::Hsla, f32)>| {
        div()
            .flex()
            .gap(SPACE_1)
            .children(parts.into_iter().map(|(color, width)| bar(color, width)))
    };
    div()
        .h(px(height))
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(6.))
        .border(crate::theme::border())
        .border_color(theme::color(&summary.line))
        .bg(theme::color(&summary.bg))
        .child(
            div()
                .flex_1()
                .flex()
                .when(rail, |body| {
                    let quiet = theme::color(&summary.line);
                    body.child(
                        div()
                            .w(px(46.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .gap(px(7.))
                            .p(px(9.))
                            .border_r(crate::theme::border())
                            .border_color(quiet)
                            .child(bar(theme::color(&summary.acc), 0.8))
                            .child(bar(quiet, 1.))
                            .child(bar(quiet, 0.7))
                            .child(bar(quiet, 0.85)),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(SPACE_1H)
                        .px(px(9.))
                        .child(line(vec![
                            (theme::color(&summary.acc), 0.22),
                            (swatch(5), 0.38),
                        ]))
                        .child(line(vec![(swatch(4), 0.16), (swatch(1), 0.5)]))
                        .child(line(vec![(swatch(2), 0.3), (swatch(3), 0.22)]))
                        .when(rail, |code| {
                            code.child(line(vec![
                                (swatch(0), 0.12),
                                (swatch(5), 0.3),
                                (swatch(1), 0.2),
                            ]))
                            .child(line(vec![(swatch(3), 0.42)]))
                        }),
                ),
        )
        .child(
            div()
                .h(px(4.))
                .flex()
                .children((0..6).map(|at| div().flex_1().h_full().bg(swatch(at)))),
        )
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    _window: &Window,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.appearance else {
        return Vec::new();
    };
    let t = Tokens::get(cx).clone();
    let preferences = bench.preferences();
    let themes = &bench.reads.themes;
    let picked = preferences.theme_id.clone();
    let worn = themes.iter().find(|summary| summary.id == picked);
    let has_pane_image = preferences.pane_image.is_some();
    let on_omarchy = themes
        .iter()
        .any(|summary| summary.source == ThemeSource::Omarchy);
    let following_omarchy = on_omarchy && preferences.pane_image_follows_omarchy;

    let cards = carded(themes, &picked)
        .into_iter()
        .map(|summary| {
            let chosen = summary.id == picked;
            let custom = summary.source == ThemeSource::Custom;
            let group = SharedString::from(format!("theme-card-{}", summary.id));
            let (pick, id) = (this.clone(), summary.id.clone());
            let edit = {
                let (this, id) = (this.clone(), summary.id.clone());
                ui::IconButton::new(
                    SharedString::from(format!("theme-edit-{}", summary.id)),
                    Icon::Pencil,
                    if custom {
                        t!("Edit theme")
                    } else {
                        t!("Customize a copy")
                    },
                )
                .small()
                .on_click(move |_, window, cx| {
                    let id = id.clone();
                    this.update(cx, |bench, cx| match custom {
                        true => super::theme_editor::edit(bench, &id, window, cx),
                        false => super::theme_editor::customize(bench, id, window, cx),
                    })
                })
            };
            let delete = custom.then(|| {
                let (this, id) = (this.clone(), summary.id.clone());
                ui::IconButton::new(
                    SharedString::from(format!("theme-delete-{}", summary.id)),
                    Icon::Trash,
                    t!("Delete theme"),
                )
                .small()
                .on_click(move |_, _, cx| {
                    let id = id.clone();
                    this.update(cx, |bench, cx| super::theme_editor::delete(bench, &id, cx))
                })
            });
            tile(format!("theme-{}", summary.id), chosen, &t)
                .group(group.clone())
                .clicks(move |_, _, cx| {
                    let id = id.clone();
                    pick.update(cx, |bench, _| {
                        crate::settings::set(bench, PreferenceChange::ThemeId(id))
                    })
                })
                .child(preview(&summary, 58., false))
                .child(
                    tile_name(summary.name.clone(), chosen, &t).child(
                        div()
                            .flex()
                            .opacity(0.)
                            .group_hover(group, |actions| actions.opacity(1.))
                            .child(edit)
                            .children(delete),
                    ),
                )
                .into_any_element()
        })
        .collect();
    let all_themes = {
        let this = this.clone();
        ui::Button::new(
            "theme-all",
            t!("All {count} themes…", count = themes.len()),
            ui::Variant::Outline,
        )
        .icon(Icon::Search)
        .on_click(move |_, window, cx| {
            this.update(cx, |bench, cx| bench.toggle_theme_palette(window, cx))
        })
    };
    let change_theme = worn.map(|worn| {
        let custom = worn.source == ThemeSource::Custom;
        let (this, id) = (this.clone(), worn.id.clone());
        ui::Button::new(
            "theme-change",
            if custom { t!("Edit") } else { t!("Customize") },
            ui::Variant::Outline,
        )
        .icon(Icon::Pencil)
        .on_click(move |_, window, cx| {
            let id = id.clone();
            this.update(cx, |bench, cx| match custom {
                true => super::theme_editor::edit(bench, &id, window, cx),
                false => super::theme_editor::customize(bench, id, window, cx),
            })
        })
    });
    let from_picture = match fields.reading {
        true => ui::Button::new("theme-from-picture", t!("Reading…"), ui::Variant::Outline)
            .icon(Icon::Image)
            .disabled(true)
            .into_any_element(),
        false => {
            let this = this.clone();
            ui::menu_button(
                "theme-from-picture",
                gpui_kit::Anchor::TopLeft,
                ui::Button::new(
                    "theme-from-picture-button",
                    t!("From a picture…"),
                    ui::Variant::Outline,
                )
                .icon(Icon::Image),
                move |mut popup, _, cx| {
                    let read = |picture: Picture| {
                        let this = this.clone();
                        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                            let picture = picture.clone();
                            this.update(cx, |bench, cx| {
                                super::theme_editor::from_picture(bench, picture, window, cx)
                            })
                        }
                    };
                    if super::theme_editor::wallpaper_offered() {
                        popup = popup.item(
                            ui::MenuRow::new(t!("Desktop wallpaper"))
                                .icon(Icon::Image)
                                .item(cx)
                                .on_click(read(Picture::Wallpaper)),
                        );
                    }
                    if has_pane_image {
                        popup = popup.item(
                            ui::MenuRow::new(t!("Pane image"))
                                .icon(Icon::Image)
                                .item(cx)
                                .on_click(read(Picture::PaneImage)),
                        );
                    }
                    let this = this.clone();
                    popup.item(
                        ui::MenuRow::new(t!("Choose a file…"))
                            .icon(Icon::File)
                            .item(cx)
                            .on_click(move |_, window, cx| {
                                this.update(cx, |_, cx| {
                                    pick_picture(window, cx, |bench, path, window, cx| {
                                        super::theme_editor::from_picture(
                                            bench,
                                            Picture::File(path),
                                            window,
                                            cx,
                                        )
                                    })
                                })
                            }),
                    )
                },
            )
            .into_any_element()
        }
    };
    let theme_body = div().child(in_rows(cards)).child(
        div()
            .px(SPACE_4)
            .pb(SPACE_3)
            .flex()
            .flex_wrap()
            .gap(SPACE_1H)
            .child(all_themes)
            .children(change_theme)
            .child(from_picture),
    );
    let editing = fields
        .editor
        .as_ref()
        .map(|editor| super::theme_editor::view(editor, this, cx));

    let choice = preferences.pane_backdrop;
    let picture = choice == PaneBackdrop::Picture;
    let theme_name = worn.map_or_else(String::new, |worn| worn.name.clone());
    let theme_kind = crate::ui::grain::theme_kind(cx);
    let backdrop_tiles = BACKDROPS
        .iter()
        .map(|&(backdrop, label)| {
            let chosen = backdrop == choice;
            let this = this.clone();
            tile(format!("backdrop-{label}"), chosen, &t)
                .clicks(move |_, _, cx| {
                    this.update(cx, |bench, _| {
                        crate::settings::set(bench, PreferenceChange::PaneBackdrop(backdrop))
                    })
                })
                .child(backdrop_swatch(backdrop, theme_kind, &t, cx))
                .child(tile_name(crate::i18n::tr(label), chosen, &t))
                .into_any_element()
        })
        .collect();
    let image = {
        let (choose, remove) = (this.clone(), this.clone());
        div()
            .flex()
            .gap(SPACE_2)
            .when(!following_omarchy && has_pane_image, |buttons| {
                buttons.child(
                    ui::Button::new("pane-image-remove", t!("Remove"), ui::Variant::Outline)
                        .on_click(move |_, _, cx| {
                            remove.update(cx, |bench, _| {
                                crate::settings::set(bench, PreferenceChange::PaneImage(None))
                            })
                        }),
                )
            })
            .when(!following_omarchy, |buttons| {
                buttons.child(
                    ui::Button::new(
                        "pane-image-choose",
                        if has_pane_image {
                            t!("Change…")
                        } else {
                            t!("Choose…")
                        },
                        ui::Variant::Outline,
                    )
                    .on_click(move |_, window, cx| {
                        choose.update(cx, |_, cx| {
                            pick_picture(window, cx, |bench, path, _, _| {
                                let path = path.to_string_lossy().into_owned();
                                crate::settings::set(bench, PreferenceChange::PaneImage(Some(path)))
                            })
                        })
                    }),
                )
            })
    };
    let backdrop = div()
        .child(in_rows(backdrop_tiles))
        .when(picture, |rows_of| {
            rows_of.child(rows::row(
                t!("Pane image"),
                Some(if following_omarchy {
                    t!("Omarchy's background, dithered behind each pane.")
                } else {
                    t!("A picture shared by every pane, dithered behind it.")
                }),
                image,
                cx,
            ))
        })
        .when(picture && on_omarchy, |rows_of| {
            rows_of.child(rows::row(
                t!("Follow Omarchy background"),
                Some(t!(
                    "Panes show the background Omarchy is showing, and change when it does."
                )),
                rows::switch(
                    "pane-image-follows-omarchy",
                    PreferenceChange::PaneImageFollowsOmarchy,
                    preferences.pane_image_follows_omarchy,
                    this,
                ),
                cx,
            ))
        });

    let style = bench
        .this_view()
        .map(|view| (*view.face_style).clone())
        .unwrap_or_default();
    let face_switch = |id: &'static str, change: fn(bool) -> FaceStyleChange, on: bool| {
        let this = this.clone();
        gpui_kit::component::switch::Switch::new(id)
            .checked(on)
            .on_change(move |on: &bool, _, cx| {
                let on = *on;
                this.update(cx, |bench, _| {
                    Target::here(&bench.asks).call_typed::<call::SetFaceStyle>(change(on));
                })
            })
    };
    let packs = crate::faces::packs();
    let chosen_pack = packs.iter().find(|pack| pack.id == style.pack);
    let choose = |pack: String, shape: String| {
        let this = this.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            let change = FaceStyleChange {
                pack: Some(pack.clone()),
                shape: Some(shape.clone()),
                ..FaceStyleChange::default()
            };
            this.update(cx, |bench, _| {
                Target::here(&bench.asks).call_typed::<call::SetFaceStyle>(change);
            })
        }
    };
    let face = |pack: &crate::faces::Pack, shape: &crate::faces::Shape, color: u32, size: f32| {
        crate::faces::resting(pack, shape, color, cx)
            .map(|image| gpui_kit::img(image).size(px(size)).into_any_element())
            .unwrap_or_else(|| div().size(px(size)).into_any_element())
    };
    let pack_tile = |id: String, name: String, faces: Vec<AnyElement>, on: bool, click| {
        tile(id, on, &t)
            .clicks(click)
            .child(
                div()
                    .h(px(46.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(SPACE_0)
                    .rounded(px(6.))
                    .border(crate::theme::border())
                    .border_color(t.border)
                    .children(faces),
            )
            .child(tile_name(name, on, &t))
            .into_any_element()
    };
    let mut pack_tiles: Vec<AnyElement> = packs
        .iter()
        .map(|pack| {
            let step = (pack.shapes.len() / 3).max(1);
            let faces = (0..3)
                .filter_map(|at| {
                    pack.shapes
                        .get((at * step) % pack.shapes.len().max(1))
                        .map(|shape| face(pack, shape, at as u32 * 2, 20.))
                })
                .collect();
            pack_tile(
                format!("buddy-pack-{}", pack.id),
                pack.name.clone(),
                faces,
                pack.id == style.pack,
                choose(pack.id.clone(), "mixed".into()),
            )
        })
        .collect();
    pack_tiles.push({
        let mixed = [1usize, 7, 8]
            .iter()
            .enumerate()
            .filter_map(|(at, index)| {
                let pack = packs.get(*index)?;
                Some(face(pack, pack.shapes.first()?, at as u32 + 1, 20.))
            })
            .collect();
        pack_tile(
            "buddy-pack-all".into(),
            t!("Every pack").into(),
            mixed,
            style.pack == "all",
            choose("all".into(), "mixed".into()),
        )
    });
    let faces = div().child(in_rows(pack_tiles)).child(rows::row(
        t!("Show faces"),
        Some(t!(
            "Agents get a character in place of their harness logo. Right-click an agent to give it its own."
        )),
        face_switch(
            "buddies-on",
            |on| FaceStyleChange {
                on: Some(on),
                ..FaceStyleChange::default()
            },
            style.on,
        ),
        cx,
    ));

    let text_size = {
        let this = this.clone();
        let chosen = match preferences.ui_text_scale {
            scale if scale >= 1.2 => 2,
            scale if scale >= 1.05 => 1,
            _ => 0,
        };
        ui::Segmented::new(
            "text-size",
            vec![
                (Icon::Text, SharedString::from("100%")),
                (Icon::Text, SharedString::from("110%")),
                (Icon::Text, SharedString::from("125%")),
            ],
            chosen,
        )
        .on_select(move |chosen, _, cx| {
            let Some(&scale) = [1., 1.1, 1.25].get(*chosen) else {
                return;
            };
            this.update(cx, |bench, _| {
                crate::settings::set(bench, PreferenceChange::UiTextScale(scale))
            })
        })
    };
    let chat_width = {
        let this = this.clone();
        let widths = [ChatWidth::Comfortable, ChatWidth::Wide, ChatWidth::Full];
        ui::Segmented::new(
            "chat-width",
            vec![
                (Icon::Message, SharedString::from(t!("Comfortable"))),
                (Icon::Message, SharedString::from(t!("Wide"))),
                (Icon::Message, SharedString::from(t!("Full"))),
            ],
            widths
                .iter()
                .position(|width| *width == preferences.chat_width)
                .unwrap_or(0),
        )
        .on_select(move |chosen, _, cx| {
            let Some(&width) = widths.get(*chosen) else {
                return;
            };
            this.update(cx, |bench, _| {
                crate::settings::set(bench, PreferenceChange::ChatWidth(width))
            })
        })
    };
    let text = div()
        .child(rows::row(
            t!("Text size"),
            Some(t!(
                "Scales labels and controls without loosening the layout."
            )),
            text_size,
            cx,
        ))
        .child(rows::row(
            t!("Chat width"),
            Some(t!(
                "How wide a chat's messages and composer grow on a wide window."
            )),
            chat_width,
            cx,
        ));

    let mut sections = vec![rows::section(
        n!("Theme"),
        worn.map(|worn| SharedString::from(worn.name.clone())),
        None,
        lit("Theme"),
        theme_body,
        cx,
    )];
    sections.extend(editing);
    sections.push(rows::section(
        n!("Backdrop"),
        Some(backdrop_about(choice, &theme_name, theme_kind)),
        None,
        lit("Backdrop"),
        backdrop,
        cx,
    ));
    sections.push(rows::section(
        n!("Agent faces"),
        None,
        None,
        lit("Agent faces"),
        faces,
        cx,
    ));
    sections.push(rows::section(n!("Text"), None, None, lit("Text"), text, cx));
    sections.extend(advanced(
        preferences,
        fields,
        pages.advanced,
        &style,
        chosen_pack,
        this,
        lit,
        cx,
    ));
    sections
}

/// The Advanced fold: its heading always, its rows once opened, and the picture's own controls
/// as a section after it while a picture is behind the panes.
#[allow(clippy::too_many_arguments)]
fn advanced(
    preferences: &Preferences,
    fields: &Fields,
    open: bool,
    style: &sikemux_view::device::FaceStyle,
    chosen_pack: Option<&crate::faces::Pack>,
    this: &Entity<Workbench>,
    lit: &dyn Fn(&str) -> bool,
    cx: &App,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx);
    let on_mac = cfg!(target_os = "macos");
    let toggle = {
        let this = this.clone();
        ui::Button::new(
            "appearance-advanced",
            if open { t!("Hide") } else { t!("Show") },
            ui::Variant::Ghost,
        )
        .on_click(move |_, _, cx| {
            this.update(cx, |bench, cx| {
                super::change_pages(bench, cx, |pages, _| pages.advanced = !pages.advanced)
            });
        })
    };
    let reset = open.then(|| {
        let this = this.clone();
        ui::Button::new("appearance-reset", t!("Reset all"), ui::Variant::Ghost)
            .icon(Icon::Refresh)
            .disabled(!advanced_changed(preferences, &Preferences::default()))
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| reset_advanced(bench, window, cx))
            })
    });
    let action = div()
        .flex()
        .gap(SPACE_1)
        .children(reset)
        .child(toggle)
        .into_any_element();
    let summary = if on_mac {
        t!("Contrast, motion, see-through, picture tuning")
    } else {
        t!("Contrast, motion, picture tuning")
    };
    let knob = |state: &Entity<SliderState>| div().w(px(220.)).child(Slider::new(state));
    let segments = |id: &'static str,
                    icon: Icon,
                    labels: [&'static str; 3],
                    chosen: usize,
                    changes: [PreferenceChange; 3]| {
        let this = this.clone();
        ui::Segmented::new(
            id,
            labels
                .iter()
                .map(|label| (icon, SharedString::from(*label)))
                .collect(),
            chosen,
        )
        .on_select(move |chosen, _, cx| {
            let Some(change) = changes.get(*chosen).cloned() else {
                return;
            };
            this.update(cx, |bench, _| crate::settings::set(bench, change))
        })
    };
    let contrast = {
        let this = this.clone();
        let levels = [Contrast::Normal, Contrast::More];
        ui::Segmented::new(
            "contrast",
            vec![
                (Icon::Contrast, SharedString::from(t!("Normal"))),
                (Icon::Contrast, SharedString::from(t!("More"))),
            ],
            usize::from(preferences.contrast == Contrast::More),
        )
        .on_select(move |chosen, _, cx| {
            let Some(&contrast) = levels.get(*chosen) else {
                return;
            };
            this.update(cx, |bench, _| {
                crate::settings::set(bench, PreferenceChange::Contrast(contrast))
            })
        })
    };
    let motions = [Motion::System, Motion::Reduced, Motion::Off];
    let motion = segments(
        "motion",
        Icon::Move,
        [t!("As macOS"), t!("Reduced"), t!("Off")],
        motions
            .iter()
            .position(|motion| *motion == preferences.motion)
            .unwrap_or(0),
        motions.map(PreferenceChange::Motion),
    );
    let mixing = style.shape == "mixed" || chosen_pack.is_none();
    let choose = |pack: String, shape: String| {
        let this = this.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            let change = FaceStyleChange {
                pack: Some(pack.clone()),
                shape: Some(shape.clone()),
                ..FaceStyleChange::default()
            };
            this.update(cx, |bench, _| {
                Target::here(&bench.asks).call_typed::<call::SetFaceStyle>(change);
            })
        }
    };
    let shape_chips = div()
        .flex()
        .flex_wrap()
        .gap(SPACE_1H)
        .child(
            div()
                .id("buddy-shape-mix")
                .h(px(36.))
                .px(SPACE_3)
                .flex()
                .items_center()
                .rounded(theme::radius_2())
                .border(crate::theme::border())
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .border_color(if mixing { t.border_strong } else { t.border })
                .when(mixing, |chip| chip.bg(t.surface_chosen))
                .child(t!("Mix"))
                .clicks(choose(style.pack.clone(), "mixed".into())),
        )
        .children(chosen_pack.into_iter().flat_map(|pack| {
            pack.shapes.iter().enumerate().map(|(at, shape)| {
                let on = shape.id == style.shape;
                let face = crate::faces::resting(pack, shape, at as u32, cx)
                    .map(|image| gpui_kit::img(image).size(px(26.)).into_any_element())
                    .unwrap_or_else(|| div().size(px(26.)).into_any_element());
                div()
                    .id(SharedString::from(format!("buddy-shape-{}", shape.id)))
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::radius_2())
                    .border(crate::theme::border())
                    .border_color(if on { t.border_strong } else { t.border })
                    .when(on, |chip| chip.bg(t.surface_chosen))
                    .when(!on, |chip| chip.hover(|style| style.bg(t.surface_hover)))
                    .tooltip(ui::tooltip(shape.name.clone()))
                    .child(face)
                    .clicks(choose(pack.id.clone(), shape.id.clone()))
            })
        }));
    let shape = div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .px(SPACE_4)
        .py(SPACE_2H)
        .border_b(crate::theme::border())
        .border_color(t.line_soft)
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_SM))
                .text_color(t.text_primary)
                .child(t!("Face shape")),
        )
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(if chosen_pack.is_some() {
                    t!("Mix gives each agent its own shape from the pack.")
                } else {
                    t!("Every pack mixes shapes too.")
                }),
        )
        .child(shape_chips);
    let animate = {
        let this = this.clone();
        gpui_kit::component::switch::Switch::new("buddies-motion")
            .checked(style.motion)
            .on_change(move |on: &bool, _, cx| {
                let change = FaceStyleChange {
                    motion: Some(*on),
                    ..FaceStyleChange::default()
                };
                this.update(cx, |bench, _| {
                    Target::here(&bench.asks).call_typed::<call::SetFaceStyle>(change);
                })
            })
    };
    let body = div().when(open, |rows_of| {
        rows_of
            .child(rows::row(
                t!("Contrast"),
                Some(t!(
                    "Greys nearer the text and borders that stand out, in every theme."
                )),
                contrast,
                cx,
            ))
            .child(rows::row(
                t!("Motion"),
                Some(t!("How much the interface moves, in every theme.")),
                motion,
                cx,
            ))
            .when(preferences.pane_backdrop != PaneBackdrop::None, |rows_of| {
                rows_of.child(rows::row(
                    t!("Backdrop strength"),
                    Some(t!("How strongly the backdrop shows behind the panes.")),
                    knob(&fields.strength),
                    cx,
                ))
            })
            .when(on_mac, |rows_of| {
                rows_of
                    .child(rows::row(
                        t!("Window see-through"),
                        Some(t!("Solid at the far right, translucent below it.")),
                        knob(&fields.opacity),
                        cx,
                    ))
                    .child(rows::row(
                        t!("Blur behind"),
                        Some(t!("How much what shows through the window is blurred.")),
                        knob(&fields.blur),
                        cx,
                    ))
            })
            .child(shape)
            .child(rows::row(
                t!("Animate faces"),
                Some(t!("Faces move with what their agents are doing.")),
                animate,
                cx,
            ))
    });
    let mut sections = vec![rows::section_with(
        n!("Advanced"),
        Some(summary.into()),
        Some(action),
        None,
        lit("Advanced"),
        body,
        cx,
    )];
    let tuned =
        preferences.pane_backdrop == PaneBackdrop::Picture && preferences.pane_image.is_some();
    if open && tuned {
        sections.push(rows::section(
            n!("Picture tuning"),
            None,
            None,
            lit("Picture tuning"),
            picture_rows(preferences, fields, this, &knob, cx),
            cx,
        ));
    }
    sections
}
