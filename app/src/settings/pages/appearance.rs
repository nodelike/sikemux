//! How the app looks: its theme, the size of its text, the panes' grain or
//! picture, the agents' buddies, and the window's opacity and blur.

use crate::ui::Clicks;
use sikemux_calls::call;
use sikemux_calls::faces::FaceStyleChange;
use sikemux_view::prefs::PreferenceChange;
use std::rc::Rc;

use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, uniform_list, AnyElement, App, AppContext, Context, Entity,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, PathPromptOptions, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window,
};
use sikemux_view::prefs::{ChatWidth, PaneColours, PaneCoverage, PanePattern, UiStyle};
use sikemux_view::theme::{ThemeSource, ThemeSummary};

use crate::core::Target;
use crate::icons::Icon;
use crate::reads::Read;
use crate::settings::index::Page::{Buddies, Interface, Theme};
use crate::settings::index::{row, section, Entry};
use crate::settings::rows;
use crate::settings::Page;
use crate::settings::Settings;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS,
    TEXT_2XS_H, TEXT_SM, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const THEME: &[Entry] = &[
    section(
        Theme,
        "Wearing",
        "colours colors palette dark light custom fork editor wallpaper customize",
    ),
    section(
        Theme,
        "Themes",
        "colours colors palette dark light custom ghostty search",
    ),
];

/// What a search finds on this page.
pub(in crate::settings) const INTERFACE: &[Entry] = &[
    section(Interface, "Style", ""),
    row(
        Interface,
        "Style",
        "Style",
        "look sharp soft square rounded corners radius shadows hairlines grid",
    ),
    section(Interface, "Text", ""),
    row(
        Interface,
        "Text",
        "Language",
        "language locale translation español deutsch français 中文 日本語 한국어 русский português",
    ),
    row(
        Interface,
        "Text",
        "Text size",
        "font zoom scale larger smaller accessibility",
    ),
    row(
        Interface,
        "Text",
        "Chat width",
        "column wide full screen messages reading measure line length",
    ),
    section(Interface, "Panes", ""),
    row(
        Interface,
        "Panes",
        "Pane texture",
        "dither dithering shader grain noise background effect",
    ),
    row(
        Interface,
        "Panes",
        "Pane image",
        "picture photo wallpaper background dither dithered custom omarchy follow",
    ),
    row(
        Interface,
        "Panes",
        "Pane strength",
        "dither grain picture opacity intensity texture image",
    ),
    row(
        Interface,
        "Panes",
        "Colours",
        "pane picture dither inks palette theme accent custom gradient map",
    ),
    row(
        Interface,
        "Panes",
        "Inks",
        "pane picture dither colours palette custom gradient map",
    ),
    row(
        Interface,
        "Panes",
        "Pattern",
        "pane picture dither bayer ordered noise grid matrix",
    ),
    row(
        Interface,
        "Panes",
        "Dot size",
        "pane picture dither pixel scale cell",
    ),
    row(
        Interface,
        "Panes",
        "Contrast",
        "pane picture dither tone levels",
    ),
    row(
        Interface,
        "Panes",
        "Brightness",
        "pane picture dither tone exposure midtones",
    ),
    row(
        Interface,
        "Panes",
        "Glow",
        "pane picture dither bloom halation light",
    ),
    row(
        Interface,
        "Panes",
        "Glow size",
        "pane picture dither bloom radius spread",
    ),
    row(
        Interface,
        "Panes",
        "Covers",
        "pane picture dither band top whole full coverage",
    ),
];

/// What a search finds on this page.
pub(in crate::settings) const BUDDIES: &[Entry] = &[
    section(
        Buddies,
        "Buddies",
        "agent icon character avatar mascot face",
    ),
    row(
        Buddies,
        "Buddies",
        "Show buddies",
        "agent icon character avatar mascot harness logo",
    ),
    row(
        Buddies,
        "Buddies",
        "Pack",
        "agent icon character avatar mascot style",
    ),
    row(
        Buddies,
        "Buddies",
        "Shape",
        "agent icon character avatar mixed",
    ),
    row(
        Buddies,
        "Buddies",
        "Animate faces",
        "motion blink animation",
    ),
];

/// What a search finds on this page.
pub(in crate::settings) const WINDOW: &[Entry] = &[
    section(Interface, "Window", ""),
    row(
        Interface,
        "Window",
        "Opacity",
        "transparency translucent see-through",
    ),
    row(
        Interface,
        "Window",
        "Background blur",
        "vibrancy frosted glass",
    ),
];

const THEME_SEARCH: &str = "ThemeSearch";

actions!(
    appearance,
    [PreviousTheme, NextTheme, EarlierTheme, LaterTheme]
);

/// Lets the arrows in the theme search wear the theme above, below, before or after the one worn.
pub fn install(cx: &mut App) {
    let search = Some("ThemeSearch > Input");
    cx.bind_keys([
        KeyBinding::new("up", PreviousTheme, search),
        KeyBinding::new("down", NextTheme, search),
        KeyBinding::new("alt-left", EarlierTheme, search),
        KeyBinding::new("alt-right", LaterTheme, search),
    ]);
}

/// Changes the theme library's search and filters, and draws the pages again.
fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.appearance.as_mut(), change)
}

/// Theme cards across a row of the library.
const COLUMNS: usize = 4;
const CARD_ROW: f32 = 104.;
const THEMES_SHOWN: f32 = 416.;
/// Where each kind of theme comes in the library, and what its filter says.
const SOURCES: [(ThemeSource, &str); 4] = [
    (ThemeSource::Custom, n!("Yours")),
    (ThemeSource::Omarchy, "Omarchy"),
    (ThemeSource::Sikemux, "Sikemux"),
    (ThemeSource::Ghostty, "Ghostty"),
];

/// What the page keeps: the theme search, the theme list's filter, and the
/// sliders, each setting its preference as it moves.
pub struct Fields {
    search: Entity<InputState>,
    query: SharedString,
    tone: Tone,
    source: Option<ThemeSource>,
    strength: Entity<SliderState>,
    dot_size: Entity<SliderState>,
    contrast: Entity<SliderState>,
    brightness: Entity<SliderState>,
    glow: Entity<SliderState>,
    glow_size: Entity<SliderState>,
    inks: Vec<Entity<ColorPickerState>>,
    opacity: Entity<SliderState>,
    blur: Entity<SliderState>,
    language: rows::Choice,
    pub editor: Option<super::theme_editor::Editor>,
    /// The picture a theme is being drawn from, while it is read.
    pub reading: Option<super::theme_editor::Picture>,
    _watching: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    All,
    Dark,
    Light,
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
    let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search themes")));
    let typing = cx.subscribe(&search, |bench, search, event: &InputEvent, cx| {
        if let InputEvent::Change = event {
            let query = search.read(cx).value();
            change_fields(bench, cx, |fields, _| fields.query = query);
        }
    });
    let (strength, moved_strength) = slider(
        |value| PreferenceChange::PaneStrength(hundredths(value)),
        (0., 2., 0.05),
        preferences.pane_strength as f32,
        cx,
    );
    let (dot_size, moved_dot_size) = slider(
        |value| PreferenceChange::PaneDotSize(value.round() as u32),
        (1., 6., 1.),
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
        typing,
        moved_strength,
        moved_dot_size,
        moved_contrast,
        moved_brightness,
        moved_glow,
        moved_glow_size,
    ];
    let inks = (0..preferences.pane_inks.len())
        .map(|at| {
            let picker = cx.new(|cx| {
                ColorPickerState::new(window, cx)
                    .default_value(theme::color(&preferences.pane_inks[at]))
            });
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
    let languages = crate::i18n::LANGUAGES;
    let labels = [t!("Same as the system"), "English"]
        .into_iter()
        .chain(languages.iter().map(|language| language.name))
        .collect::<Vec<_>>();
    let chosen = match preferences.language.as_deref() {
        None => 0,
        Some(code) => (languages.iter())
            .position(|language| language.code == code)
            .map_or(1, |at| at + 2),
    };
    let language = rows::Choice::new(
        labels,
        chosen,
        move |bench, at, _| {
            let code = match at {
                0 => None,
                1 => Some("en".to_owned()),
                at => Some(languages[at - 2].code.to_owned()),
            };
            crate::settings::set(bench, PreferenceChange::Language(code))
        },
        window,
        cx,
    );
    let fields = Fields {
        language,
        search,
        query: SharedString::default(),
        tone: Tone::All,
        source: None,
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
        reading: None,
        _watching: {
            watching.extend([moved_opacity, moved_blur]);
            watching
        },
    };
    super::change_pages(bench, cx, |pages, _| pages.appearance = Some(fields));
}

/// How the pane picture is dithered: a preview drawn as the panes draw it, then each control.
fn picture_rows(
    preferences: &sikemux_view::prefs::Preferences,
    fields: &Fields,
    this: &Entity<Workbench>,
    knob: &dyn Fn(&Entity<SliderState>, String) -> gpui_kit::Div,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx);
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
            let change = changes[*at].clone();
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
    let signed = |value: f64| format!("{:+.2}", value);
    div()
        .child(
            div().px(SPACE_4).py(SPACE_3).child(
                div()
                    .relative()
                    .h(px(180.))
                    .rounded(crate::theme::radius_3())
                    .border_1()
                    .border_color(t.border)
                    .overflow_hidden()
                    .children(
                        crate::ui::grain::picture_preview(cx).map(|preview| preview.size_full()),
                    ),
            ),
        )
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
        .child(rows::row(
            t!("Dot size"),
            None,
            knob(&fields.dot_size, format!("{}pt", preferences.pane_dot_size)),
            cx,
        ))
        .child(rows::row(
            t!("Contrast"),
            None,
            knob(&fields.contrast, signed(preferences.pane_contrast)),
            cx,
        ))
        .child(rows::row(
            t!("Brightness"),
            None,
            knob(&fields.brightness, signed(preferences.pane_brightness)),
            cx,
        ))
        .child(rows::row(
            t!("Glow"),
            Some(t!("Light around the picture's bright parts.")),
            knob(&fields.glow, format!("{:.2}", preferences.pane_glow)),
            cx,
        ))
        .child(rows::row(
            t!("Glow size"),
            None,
            knob(
                &fields.glow_size,
                format!("{:.2}", preferences.pane_glow_size),
            ),
            cx,
        ))
        .child(rows::row(t!("Covers"), None, coverage, cx))
}

/// Wears the theme `by` places from the one worn in the library as filtered, else its first or last.
fn step(bench: &Workbench, by: isize, cx: &App) {
    let Some(fields) = super::state(bench, cx).and_then(|pages| pages.appearance.as_ref()) else {
        return;
    };
    let themes = &bench.reads.themes;
    let listed: Vec<String> = shown(themes, fields)
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    let Some(last) = listed.len().checked_sub(1) else {
        return;
    };
    let worn = bench.preferences().theme_id.clone();
    let next = match listed.iter().position(|id| *id == worn) {
        Some(at) => at.saturating_add_signed(by).min(last),
        None if by > 0 => 0,
        None => last,
    };
    crate::settings::set(bench, PreferenceChange::ThemeId(listed[next].clone()));
}

/// The themes the library shows as searched and filtered: yours, then Sikemux's, then Ghostty's.
fn shown(themes: &[ThemeSummary], fields: &Fields) -> Vec<ThemeSummary> {
    let query = fields.query.trim().to_lowercase();
    let wanted = |theme: &&ThemeSummary| {
        (query.is_empty() || theme.name.to_lowercase().contains(&query))
            && fields.source.is_none_or(|source| theme.source == source)
            && match fields.tone {
                Tone::All => true,
                Tone::Dark => theme.dark,
                Tone::Light => !theme.dark,
            }
    };
    SOURCES
        .iter()
        .flat_map(|(source, _)| {
            themes
                .iter()
                .filter(move |theme| theme.source == *source)
                .filter(wanted)
                .cloned()
        })
        .collect()
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
            .rounded(crate::theme::corner(px(2.)))
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
        .rounded(crate::theme::corner(px(6.)))
        .border_1()
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
                            .border_r_1()
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
    page: Page,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.appearance else {
        return Vec::new();
    };
    let t = Tokens::get(cx).clone();
    let preferences = bench.preferences();
    let themes = &bench.reads.themes;
    let custom = themes
        .iter()
        .filter(|theme| theme.source == ThemeSource::Custom)
        .count();

    let tone = {
        let this = this.clone();
        let chosen = match fields.tone {
            Tone::All => 0,
            Tone::Dark => 1,
            Tone::Light => 2,
        };
        ui::Segmented::words(
            "theme-tone",
            vec![
                (t!("All").into(), None),
                (t!("Dark").into(), None),
                (t!("Light").into(), None),
            ],
            chosen,
        )
        .on_select(move |chosen, _, cx| {
            let tone = [Tone::All, Tone::Dark, Tone::Light][*chosen];
            this.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| fields.tone = tone);
            })
        })
    };
    let offered: Vec<(Option<ThemeSource>, SharedString, Option<u64>)> = std::iter::once((
        None,
        SharedString::from(t!("All")),
        Some(themes.len() as u64),
    ))
    .chain(SOURCES.iter().filter_map(|(source, label)| {
        let count = themes
            .iter()
            .filter(|theme| theme.source == *source)
            .count();
        (count > 0).then(|| {
            (
                Some(*source),
                SharedString::from(crate::i18n::tr(label)),
                Some(count as u64),
            )
        })
    }))
    .collect();
    let sources = {
        let this = this.clone();
        let picked: Vec<Option<ThemeSource>> =
            offered.iter().map(|(source, _, _)| *source).collect();
        let chosen = picked
            .iter()
            .position(|source| *source == fields.source)
            .unwrap_or(0);
        ui::Segmented::words(
            "theme-source",
            offered
                .iter()
                .map(|(_, label, count)| (label.clone(), *count))
                .collect(),
            chosen,
        )
        .on_select(move |chosen, _, cx| {
            let source = picked.get(*chosen).copied().flatten();
            this.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| fields.source = source);
            })
        })
    };
    let listed = shown(themes, fields);
    let found = listed.len();
    let rows_of: Rc<Vec<Vec<ThemeSummary>>> =
        Rc::new(listed.chunks(COLUMNS).map(<[_]>::to_vec).collect());
    let picked = preferences.theme_id.clone();
    let shown_rows = rows_of.len();
    let list = {
        let (this, t) = (this.clone(), t.clone());
        uniform_list("theme-grid", shown_rows, move |range, _, _| {
            range
                .map(|at| {
                    let cards = rows_of[at].iter().map(|summary| {
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
                                this.update(cx, |bench, _| super::theme_editor::delete(bench, &id))
                            })
                        });
                        div()
                            .id(SharedString::from(format!("theme-{}", summary.id)))
                            .group(group.clone())
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(SPACE_1)
                            .p(SPACE_1)
                            .rounded(crate::theme::corner(px(8.)))
                            .cursor_pointer()
                            .when(chosen, |card| card.bg(t.surface_chosen))
                            .when(!chosen, |card| {
                                card.hover(|style| style.bg(t.surface_hover))
                            })
                            .clicks(move |_, _, cx| {
                                let id = id.clone();
                                pick.update(cx, |bench, _| {
                                    crate::settings::set(bench, PreferenceChange::ThemeId(id))
                                })
                            })
                            .child(preview(summary, 58., false))
                            .child(
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
                                            .child(summary.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .opacity(0.)
                                            .group_hover(group, |actions| actions.opacity(1.))
                                            .child(edit)
                                            .children(delete),
                                    )
                                    .when(chosen, |name| {
                                        name.child(ui::icon(Icon::Check, 12.).text_color(t.accent))
                                    }),
                            )
                    });
                    let gaps = COLUMNS - rows_of[at].len();
                    div()
                        .id(("theme-row", at))
                        .w_full()
                        .h(px(CARD_ROW))
                        .flex()
                        .gap(SPACE_1H)
                        .children(cards)
                        .children((0..gaps).map(|_| div().flex_1()))
                })
                .collect()
        })
        .h(px(THEMES_SHOWN.min(CARD_ROW * shown_rows.max(1) as f32)))
    };
    let worn = themes
        .iter()
        .find(|summary| summary.id == preferences.theme_id);
    let has_pane_image = preferences.pane_image.is_some();
    let on_omarchy = themes
        .iter()
        .any(|summary| summary.source == ThemeSource::Omarchy);
    let following_omarchy = on_omarchy && preferences.pane_image_follows_omarchy;
    let current = worn.map(|worn| {
        use super::theme_editor::Picture;
        let custom = worn.source == ThemeSource::Custom;
        let (change, id) = (this.clone(), worn.id.clone());
        let from = |picture: Picture, label: &'static str| {
            let this = this.clone();
            ui::Button::new(
                match picture {
                    Picture::Wallpaper => "theme-from-wallpaper",
                    Picture::PaneImage => "theme-from-pane-image",
                },
                if fields.reading == Some(picture) {
                    t!("Reading…")
                } else {
                    label
                },
                ui::Variant::Outline,
            )
            .icon(Icon::Image)
            .disabled(fields.reading.is_some())
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| {
                    super::theme_editor::from_picture(bench, picture, window, cx)
                })
            })
        };
        let kind = SOURCES
            .iter()
            .find(|(source, _)| *source == worn.source)
            .map_or("", |(_, label)| crate::i18n::tr(label));
        div()
            .flex()
            .items_center()
            .gap(px(18.))
            .p(SPACE_4)
            .child(
                div()
                    .w(px(200.))
                    .flex_none()
                    .child(preview(worn, 112., true)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(ui::caps(t!("Wearing"), cx))
                    .child(
                        ui::text(worn.name.clone(), 17., gpui_kit::FontWeight::SEMIBOLD)
                            .truncate()
                            .text_color(t.text_primary),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_XS))
                            .text_color(t.text_tertiary)
                            .child(format!(
                                "{} · {kind}",
                                if worn.dark { t!("Dark") } else { t!("Light") }
                            )),
                    )
                    .child(
                        div()
                            .pt(SPACE_2H)
                            .flex()
                            .flex_wrap()
                            .gap(SPACE_1H)
                            .child(
                                ui::Button::new(
                                    "theme-change",
                                    if custom { t!("Edit") } else { t!("Customize") },
                                    ui::Variant::Outline,
                                )
                                .icon(Icon::Pencil)
                                .on_click(move |_, window, cx| {
                                    let id = id.clone();
                                    change.update(cx, |bench, cx| match custom {
                                        true => super::theme_editor::edit(bench, &id, window, cx),
                                        false => {
                                            super::theme_editor::customize(bench, id, window, cx)
                                        }
                                    })
                                }),
                            )
                            .when(super::theme_editor::wallpaper_offered(), |row| {
                                row.child(from(Picture::Wallpaper, t!("From wallpaper")))
                            })
                            .when(has_pane_image, |row| {
                                row.child(from(Picture::PaneImage, t!("From pane image")))
                            }),
                    ),
            )
    });
    let editing = fields
        .editor
        .as_ref()
        .map(|editor| super::theme_editor::view(editor, this, cx));
    let library = div()
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .px(SPACE_3)
                .pt(SPACE_3)
                .pb(SPACE_2)
                .child(
                    div()
                        .flex_1()
                        .key_context(THEME_SEARCH)
                        .on_action(
                            window.listener_for(this, |bench, _: &PreviousTheme, _, cx| {
                                step(bench, -(COLUMNS as isize), cx)
                            }),
                        )
                        .on_action(window.listener_for(this, |bench, _: &NextTheme, _, cx| {
                            step(bench, COLUMNS as isize, cx)
                        }))
                        .on_action(window.listener_for(this, |bench, _: &EarlierTheme, _, cx| {
                            step(bench, -1, cx)
                        }))
                        .on_action(
                            window.listener_for(this, |bench, _: &LaterTheme, _, cx| {
                                step(bench, 1, cx)
                            }),
                        )
                        .child(ui::Field::new(&fields.search).icon(Icon::Search)),
                )
                .child(tone),
        )
        .child(div().px(SPACE_3).pb(SPACE_2H).flex().child(sources))
        .child(
            div()
                .px(SPACE_2)
                .pb(SPACE_2)
                .child(list)
                .when(found == 0, |grid| {
                    grid.child(rows::note(t!("No themes match."), cx))
                }),
        );

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
            let scale = [1., 1.1, 1.25][*chosen];
            this.update(cx, |bench, _| {
                crate::settings::set(bench, PreferenceChange::UiTextScale(scale))
            })
        })
    };
    let ui_style = {
        let this = this.clone();
        let styles = [UiStyle::Soft, UiStyle::Sharp];
        ui::Segmented::new(
            "ui-style",
            vec![
                (Icon::Window, SharedString::from(t!("Soft"))),
                (Icon::Table, SharedString::from(t!("Sharp"))),
            ],
            styles
                .iter()
                .position(|style| *style == preferences.ui_style)
                .unwrap_or(0),
        )
        .on_select(move |chosen, _, cx| {
            let style = styles[*chosen];
            this.update(cx, |bench, _| {
                crate::settings::set(bench, PreferenceChange::UiStyle(style))
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
            let width = widths[*chosen];
            this.update(cx, |bench, _| {
                crate::settings::set(bench, PreferenceChange::ChatWidth(width))
            })
        })
    };
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
                            let picked = cx.prompt_for_paths(PathPromptOptions {
                                files: true,
                                directories: false,
                                multiple: false,
                                prompt: None,
                            });
                            cx.spawn_in(window, async move |bench, cx| {
                                if let Ok(Ok(Some(paths))) = picked.await {
                                    if let Some(path) = paths.first() {
                                        let path = path.to_string_lossy().into_owned();
                                        let _ = bench.update(cx, |bench, _| {
                                            crate::settings::set(
                                                bench,
                                                PreferenceChange::PaneImage(Some(path)),
                                            )
                                        });
                                    }
                                }
                            })
                            .detach();
                        })
                    }),
                )
            })
    };
    let knob = |state: &Entity<SliderState>, shown: String| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .w(px(220.))
            .child(div().flex_1().child(Slider::new(state)))
            .child(
                div()
                    .w(px(44.))
                    .text_right()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_secondary)
                    .child(shown),
            )
    };
    let style_rows = div().child(rows::row(
        t!("Style"),
        Some(t!("Soft rounds panels and floats them apart. Sharp squares every corner and sets the rails and stage flush, parted by hairlines.")),
        ui_style,
        cx,
    ));
    let text = div()
        .child(rows::row(
            t!("Language"),
            Some(t!("The language Sikemux's own words are in. What agents write stays as they wrote it.")),
            fields.language.view(),
            cx,
        ))
        .child(rows::row(
            t!("Text size"),
            Some(t!("Scales labels and controls without loosening the layout.")),
            text_size,
            cx,
        ))
        .child(rows::row(
            t!("Chat width"),
            Some(t!("How wide a chat's messages and composer grow on a wide window.")),
            chat_width,
            cx,
        ));
    let interface = div()
        .child(rows::row(
            t!("Pane texture"),
            Some(t!("The dithered grain behind each pane.")),
            rows::switch(
                "pane-texture",
                PreferenceChange::PaneShader,
                preferences.pane_shader,
                this,
            ),
            cx,
        ))
        .child(rows::row(
            t!("Pane image"),
            Some(if following_omarchy {
                t!("Omarchy's background, dithered in place of the grain.")
            } else {
                t!("A picture shared by every pane, dithered in place of the grain.")
            }),
            image,
            cx,
        ))
        .when(on_omarchy, |rows_of| {
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
        })
        .child(rows::row(
            t!("Pane strength"),
            Some(t!(
                "How strongly the texture or picture shows. 1.00 is the default."
            )),
            knob(
                &fields.strength,
                format!("{:.2}", preferences.pane_strength),
            ),
            cx,
        ))
        .when(preferences.pane_shader && has_pane_image, |rows_of| {
            rows_of.child(picture_rows(preferences, fields, this, &knob, cx))
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
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut gpui_kit::App| {
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
    let card = |id: String, name: String, faces: Vec<AnyElement>, on: bool, click| {
        div()
            .id(SharedString::from(id))
            .w(px(108.))
            .flex()
            .flex_col()
            .items_center()
            .gap(SPACE_1H)
            .p(SPACE_2)
            .rounded(theme::radius_3())
            .border_1()
            .cursor_pointer()
            .border_color(if on { t.border_strong } else { t.border })
            .when(on, |card| card.bg(t.surface_chosen))
            .when(!on, |card| card.hover(|style| style.bg(t.surface_hover)))
            .child(div().flex().gap(SPACE_0).children(faces))
            .child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_secondary)
                    .child(name),
            )
            .clicks(click)
    };
    let pack_cards = div()
        .flex()
        .flex_wrap()
        .gap(SPACE_2)
        .children(packs.iter().map(|pack| {
            let step = (pack.shapes.len() / 3).max(1);
            let faces = (0..3)
                .filter_map(|at| {
                    pack.shapes
                        .get((at * step) % pack.shapes.len().max(1))
                        .map(|shape| face(pack, shape, at as u32 * 2, 24.))
                })
                .collect();
            card(
                format!("buddy-pack-{}", pack.id),
                pack.name.clone(),
                faces,
                pack.id == style.pack,
                choose(pack.id.clone(), "mixed".into()),
            )
        }))
        .child({
            let mixed = [1usize, 7, 8]
                .iter()
                .enumerate()
                .filter_map(|(at, index)| {
                    let pack = packs.get(*index)?;
                    Some(face(pack, pack.shapes.first()?, at as u32 + 1, 24.))
                })
                .collect();
            card(
                "buddy-pack-all".into(),
                t!("Every pack").into(),
                mixed,
                style.pack == "all",
                choose("all".into(), "mixed".into()),
            )
        });
    let mixing = style.shape == "mixed" || chosen_pack.is_none();
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
                .border_1()
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
                div()
                    .id(SharedString::from(format!("buddy-shape-{}", shape.id)))
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::radius_2())
                    .border_1()
                    .border_color(if on { t.border_strong } else { t.border })
                    .when(on, |chip| chip.bg(t.surface_chosen))
                    .when(!on, |chip| chip.hover(|style| style.bg(t.surface_hover)))
                    .tooltip(ui::tooltip(shape.name.clone()))
                    .child(face(pack, shape, at as u32, 26.))
                    .clicks(choose(pack.id.clone(), shape.id.clone()))
            })
        }));
    let stacked = |label: &'static str, about: &'static str, body: AnyElement| {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .px(SPACE_4)
            .py(SPACE_2H)
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_SM))
                    .text_color(t.text_primary)
                    .child(label),
            )
            .child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                    .text_color(t.text_tertiary)
                    .child(about),
            )
            .child(body)
    };
    let buddies = div()
        .child(rows::row(
            t!("Show buddies"),
            Some(t!("Agents get a character in place of their harness logo.")),
            face_switch(
                "buddies-on",
                |on| FaceStyleChange {
                    on: Some(on),
                    ..FaceStyleChange::default()
                },
                style.on,
            ),
            cx,
        ))
        .child(stacked(
            t!("Pack"),
            t!("What new agents look like. Right-click an agent to give it its own."),
            pack_cards.into_any_element(),
        ))
        .child(stacked(
            t!("Shape"),
            if chosen_pack.is_some() {
                t!("Mix gives each agent its own shape from the pack.")
            } else {
                t!("Every pack mixes shapes too.")
            },
            shape_chips.into_any_element(),
        ))
        .child(rows::row(
            t!("Animate faces"),
            Some(t!("Faces move with what their agents are doing.")),
            face_switch(
                "buddies-motion",
                |motion| FaceStyleChange {
                    motion: Some(motion),
                    ..FaceStyleChange::default()
                },
                style.motion,
            ),
            cx,
        ));

    let window_section = div()
        .child(rows::row(
            t!("Opacity"),
            Some(t!("Solid at 1.00, translucent below it.")),
            knob(
                &fields.opacity,
                format!("{:.2}", preferences.window_opacity),
            ),
            cx,
        ))
        .child(rows::row(
            t!("Background blur"),
            Some(t!("0 is crisp; 20–40px gives a soft frosted effect.")),
            knob(
                &fields.blur,
                format!("{}px", preferences.window_blur.round()),
            ),
            cx,
        ));

    match page {
        Page::Theme => {
            let mut sections = Vec::new();
            sections.extend(current.map(|current| rows::card(lit("Wearing"), current, cx)));
            sections.extend(editing);
            sections.push(rows::section(
                n!("Themes"),
                Some(
                    t!(
                        "{built_in} built-in · {custom} yours",
                        built_in = themes.len() - custom,
                        custom = custom
                    )
                    .into(),
                ),
                None,
                lit("Themes"),
                library,
                cx,
            ));
            sections
        }
        Page::Interface => {
            let mut sections = vec![
                rows::section(n!("Style"), None, None, lit("Style"), style_rows, cx),
                rows::section(n!("Text"), None, None, lit("Text"), text, cx),
                rows::section(n!("Panes"), None, None, lit("Panes"), interface, cx),
            ];
            if crate::settings::index::here(Page::Interface, "Window") {
                sections.push(rows::section(
                    n!("Window"),
                    None,
                    None,
                    lit("Window"),
                    window_section,
                    cx,
                ));
            }
            sections
        }
        _ => vec![rows::section(
            n!("Buddies"),
            None,
            None,
            lit("Buddies"),
            buddies,
            cx,
        )],
    }
}
