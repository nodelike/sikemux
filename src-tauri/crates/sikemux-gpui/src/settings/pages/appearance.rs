//! How the app looks: its theme, the size of its text, the panes' grain or
//! picture, the agents' buddies, and the window's opacity and blur.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, uniform_list, AnyElement, AppContext, Context, Entity, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window,
};
use serde_json::{json, Value};
use sikemux_wire::theme::{ThemeSource, ThemeSummary};

use crate::core::{Ask, Watch};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

pub const SECTIONS: &[&str] = &["Theme", "Interface", "Buddies", "Window"];
const THEME_ROW: f32 = 30.;
const THEMES_SHOWN: f32 = 340.;

/// What the page keeps: the theme search, the theme list's filter, and the
/// sliders, each setting its preference as it moves.
pub struct Fields {
    search: Entity<InputState>,
    query: SharedString,
    tone: Tone,
    strength: Entity<SliderState>,
    opacity: Entity<SliderState>,
    blur: Entity<SliderState>,
    /// The theme being edited, when one is.
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

fn slider(
    key: &'static str,
    range: (f32, f32, f32),
    value: f32,
    round: bool,
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
        let value = value.start();
        let value: Value = match round {
            true => (value.round() as i64).into(),
            false => ((value as f64 * 100.).round() / 100.).into(),
        };
        crate::settings::set(bench, key, value);
    });
    (state, moved)
}

fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let _ = bench.asks.send(Ask::Watch(
        "themes",
        Watch::new("themes", json!({}), &["preferences"]),
    ));
    let preferences = bench.preferences();
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search themes"));
    let typing = cx.subscribe(&search, |bench, search, event: &InputEvent, cx| {
        if let (InputEvent::Change, Some(fields)) = (
            event,
            bench
                .settings
                .as_mut()
                .and_then(|settings| settings.pages.appearance.as_mut()),
        ) {
            fields.query = search.read(cx).value();
            bench.parts.redraw(&bench.parts.stage, cx);
        }
    });
    let (strength, moved_strength) = slider(
        "paneStrength",
        (0., 2., 0.05),
        preferences.pane_strength as f32,
        false,
        cx,
    );
    let (opacity, moved_opacity) = slider(
        "windowOpacity",
        (0., 1., 0.01),
        preferences.window_opacity as f32,
        false,
        cx,
    );
    let (blur, moved_blur) = slider(
        "windowBlur",
        (0., 60., 1.),
        preferences.window_blur.min(60.) as f32,
        true,
        cx,
    );
    if let Some(settings) = bench.settings.as_mut() {
        settings.pages.appearance = Some(Fields {
            search,
            query: SharedString::default(),
            tone: Tone::All,
            strength,
            opacity,
            blur,
            editor: None,
            reading: None,
            _watching: vec![typing, moved_strength, moved_opacity, moved_blur],
        });
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

/// One line of the theme list: a group's heading, or a theme.
enum Line {
    Group(&'static str, usize),
    Theme(ThemeSummary),
}

fn lines(themes: &[ThemeSummary], query: &str, tone: Tone) -> Vec<Line> {
    let query = query.trim().to_lowercase();
    let wanted = |theme: &&ThemeSummary| {
        (query.is_empty() || theme.name.to_lowercase().contains(&query))
            && match tone {
                Tone::All => true,
                Tone::Dark => theme.dark,
                Tone::Light => !theme.dark,
            }
    };
    let groups = [
        (ThemeSource::Custom, "Your themes"),
        (ThemeSource::Sikemux, "Sikemux"),
        (ThemeSource::Ghostty, "Ghostty"),
    ];
    let mut lines = Vec::new();
    for (source, label) in groups {
        let shown: Vec<&ThemeSummary> = themes
            .iter()
            .filter(|theme| theme.source == source)
            .filter(wanted)
            .collect();
        if shown.is_empty() {
            continue;
        }
        lines.push(Line::Group(label, shown.len()));
        lines.extend(shown.into_iter().cloned().map(Line::Theme));
    }
    lines
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.appearance else {
        cx.defer_in(window, make);
        return Vec::new();
    };
    let t = Tokens::get(cx).clone();
    let preferences = bench.preferences();
    let themes: Vec<ThemeSummary> = bench
        .read("themes")
        .and_then(|themes| serde_json::from_value(themes.clone()).ok())
        .unwrap_or_default();
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
        ui::Segmented::new(
            "theme-tone",
            vec![
                (Icon::Stack, SharedString::from("All")),
                (Icon::Eye, SharedString::from("Dark")),
                (Icon::Star, SharedString::from("Light")),
            ],
            chosen,
        )
        .on_select(move |chosen, _, cx| {
            let tone = [Tone::All, Tone::Dark, Tone::Light][*chosen];
            this.update(cx, |bench, cx| {
                if let Some(fields) = bench
                    .settings
                    .as_mut()
                    .and_then(|settings| settings.pages.appearance.as_mut())
                {
                    fields.tone = tone;
                }
                bench.parts.redraw(&bench.parts.stage, cx);
            })
        })
    };
    let listed = Rc::new(lines(&themes, &fields.query, fields.tone));
    let picked = preferences.theme_id.clone();
    let list = {
        let (listed, this, t) = (listed.clone(), this.clone(), t.clone());
        uniform_list("theme-list", listed.len(), move |range, _, _| {
            range
                .map(|at| match &listed[at] {
                    Line::Group(label, count) => div()
                        .id(("theme-group", at))
                        .flex()
                        .items_end()
                        .gap(px(6.))
                        .h(px(THEME_ROW))
                        .px(px(12.))
                        .pb(px(4.))
                        .text_size(crate::theme::ui_text(10.5))
                        .text_color(t.text_tertiary)
                        .child(*label)
                        .child(count.to_string()),
                    Line::Theme(summary) => {
                        let chosen = summary.id == picked;
                        let (pick, id) = (this.clone(), summary.id.clone());
                        div()
                            .id(("theme", at))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .h(px(THEME_ROW))
                            .px(px(12.))
                            .cursor_pointer()
                            .when(chosen, |row| row.bg(t.surface_chosen))
                            .when(!chosen, |row| row.hover(|style| style.bg(t.surface_hover)))
                            .on_click(move |_, _, cx| {
                                let id = id.clone();
                                pick.update(cx, |bench, _| {
                                    crate::settings::set(bench, "themeId", id)
                                })
                            })
                            .child(
                                div()
                                    .size(px(18.))
                                    .rounded(theme::RADIUS_2)
                                    .border_1()
                                    .border_color(theme::color(&summary.line))
                                    .bg(theme::color(&summary.bg))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .size(px(6.))
                                            .rounded_full()
                                            .bg(theme::color(&summary.acc)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::theme::ui_text(12.5))
                                    .text_color(t.text_primary)
                                    .child(summary.name.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(2.))
                                    .children(summary.swatches.iter().map(|swatch| {
                                        div().size(px(8.)).rounded(px(2.)).bg(theme::color(swatch))
                                    })),
                            )
                            .child({
                                let (this, id, custom) = (
                                    this.clone(),
                                    summary.id.clone(),
                                    summary.source == ThemeSource::Custom,
                                );
                                ui::IconButton::new(
                                    SharedString::from(format!("theme-edit-{}", summary.id)),
                                    Icon::Pencil,
                                    if custom {
                                        "Edit theme"
                                    } else {
                                        "Customize a copy"
                                    },
                                )
                                .small()
                                .on_click(move |_, window, cx| {
                                    let id = id.clone();
                                    this.update(cx, |bench, cx| match custom {
                                        true => super::theme_editor::edit(bench, &id, window, cx),
                                        false => {
                                            super::theme_editor::customize(bench, id, window, cx)
                                        }
                                    })
                                })
                            })
                            .when(summary.source == ThemeSource::Custom, |row| {
                                let (this, id) = (this.clone(), summary.id.clone());
                                row.child(
                                    ui::IconButton::new(
                                        SharedString::from(format!("theme-delete-{}", summary.id)),
                                        Icon::Trash,
                                        "Delete theme",
                                    )
                                    .small()
                                    .on_click(
                                        move |_, _, cx| {
                                            let id = id.clone();
                                            this.update(cx, |bench, _| {
                                                super::theme_editor::delete(bench, &id)
                                            })
                                        },
                                    ),
                                )
                            })
                            .when(chosen, |row| {
                                row.child(ui::icon(Icon::Check, 12.).text_color(t.accent))
                            })
                    }
                })
                .collect()
        })
        .h(px(THEMES_SHOWN))
    };
    let worn = themes
        .iter()
        .find(|summary| summary.id == preferences.theme_id);
    let has_pane_image = preferences.pane_image.is_some();
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
                    "Reading…"
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
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(10.))
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(10.5))
                            .text_color(t.text_tertiary)
                            .child("Wearing"),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::theme::ui_text(13.))
                            .text_color(t.text_primary)
                            .child(worn.name.clone()),
                    ),
            )
            .child(from(Picture::Wallpaper, "From wallpaper"))
            .when(has_pane_image, |row| {
                row.child(from(Picture::PaneImage, "From pane image"))
            })
            .child(
                ui::Button::new(
                    "theme-change",
                    if custom { "Edit" } else { "Customize" },
                    ui::Variant::Outline,
                )
                .icon(Icon::Pencil)
                .on_click(move |_, window, cx| {
                    let id = id.clone();
                    change.update(cx, |bench, cx| match custom {
                        true => super::theme_editor::edit(bench, &id, window, cx),
                        false => super::theme_editor::customize(bench, id, window, cx),
                    })
                }),
            )
    });
    let editing = fields
        .editor
        .as_ref()
        .map(|editor| super::theme_editor::view(editor, this, cx));
    let theme_body = div()
        .children(current)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .p(px(10.))
                .border_b_1()
                .border_color(t.line_soft)
                .child(
                    div()
                        .flex_1()
                        .child(ui::Field::new(&fields.search).icon(Icon::Search)),
                )
                .child(tone),
        )
        .child(list)
        .children(editing);

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
                crate::settings::set(bench, "uiTextScale", scale)
            })
        })
    };
    let image = {
        let (choose, remove) = (this.clone(), this.clone());
        div()
            .flex()
            .gap(px(8.))
            .when(preferences.pane_image.is_some(), |buttons| {
                buttons.child(
                    ui::Button::new("pane-image-remove", "Remove", ui::Variant::Outline).on_click(
                        move |_, _, cx| {
                            remove.update(cx, |bench, _| {
                                crate::settings::set(bench, "paneImage", Value::Null)
                            })
                        },
                    ),
                )
            })
            .child(
                ui::Button::new(
                    "pane-image-choose",
                    if preferences.pane_image.is_some() {
                        "Change…"
                    } else {
                        "Choose…"
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
                                        crate::settings::set(bench, "paneImage", path)
                                    });
                                }
                            }
                        })
                        .detach();
                    })
                }),
            )
    };
    let knob = |state: &Entity<SliderState>, shown: String| {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .w(px(220.))
            .child(div().flex_1().child(Slider::new(state)))
            .child(
                div()
                    .w(px(44.))
                    .text_right()
                    .text_size(crate::theme::ui_text(12.))
                    .text_color(t.text_secondary)
                    .child(shown),
            )
    };
    let interface = div()
        .child(rows::row(
            "Text size",
            Some("Scales labels and controls without loosening the layout."),
            text_size,
            cx,
        ))
        .child(rows::row(
            "Pane texture",
            Some("The dithered grain behind each pane."),
            rows::switch("pane-texture", "paneShader", preferences.pane_shader, this),
            cx,
        ))
        .child(rows::row(
            "Pane image",
            Some("A picture shared by every pane, dithered in place of the grain."),
            image,
            cx,
        ))
        .child(rows::row(
            "Pane strength",
            Some("How strongly the texture or picture shows. 1.00 is the default."),
            knob(
                &fields.strength,
                format!("{:.2}", preferences.pane_strength),
            ),
            cx,
        ));

    let style = bench
        .view
        .as_ref()
        .map(|view| (*view.face_style).clone())
        .unwrap_or_default();
    let face_switch = |id: &'static str, field: &'static str, on: bool| {
        let this = this.clone();
        gpui_kit::component::switch::Switch::new(id)
            .checked(on)
            .on_change(move |on: &bool, _, cx| {
                let mut change = serde_json::Map::new();
                change.insert(field.to_owned(), (*on).into());
                let change = Value::Object(change);
                this.update(cx, |bench, _| {
                    let _ = bench.asks.send(Ask::Call {
                        method: "setFaceStyle",
                        params: change,
                    });
                })
            })
    };
    let packs = crate::faces::packs();
    let chosen_pack = packs.iter().find(|pack| pack.id == style.pack);
    let choose = |pack: String, shape: String| {
        let this = this.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut gpui_kit::App| {
            let change = serde_json::json!({ "pack": pack, "shape": shape });
            this.update(cx, |bench, _| {
                let _ = bench.asks.send(Ask::Call {
                    method: "setFaceStyle",
                    params: change,
                });
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
            .gap(px(6.))
            .p(px(8.))
            .rounded(theme::RADIUS_3)
            .border_1()
            .cursor_pointer()
            .border_color(if on { t.border_selected } else { t.border })
            .when(on, |card| card.bg(t.surface_chosen))
            .when(!on, |card| card.hover(|style| style.bg(t.surface_hover)))
            .child(div().flex().gap(px(2.)).children(faces))
            .child(
                div()
                    .text_size(crate::theme::ui_text(11.))
                    .text_color(t.text_secondary)
                    .child(name),
            )
            .on_click(click)
    };
    let pack_cards = div()
        .flex()
        .flex_wrap()
        .gap(px(8.))
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
                "Every pack".into(),
                mixed,
                style.pack == "all",
                choose("all".into(), "mixed".into()),
            )
        });
    let mixing = style.shape == "mixed" || chosen_pack.is_none();
    let shape_chips = div()
        .flex()
        .flex_wrap()
        .gap(px(6.))
        .child(
            div()
                .id("buddy-shape-mix")
                .h(px(36.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(theme::RADIUS_2)
                .border_1()
                .cursor_pointer()
                .text_size(crate::theme::ui_text(12.))
                .text_color(t.text_secondary)
                .border_color(if mixing { t.border_selected } else { t.border })
                .when(mixing, |chip| chip.bg(t.surface_chosen))
                .child("Mix")
                .on_click(choose(style.pack.clone(), "mixed".into())),
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
                    .rounded(theme::RADIUS_2)
                    .border_1()
                    .cursor_pointer()
                    .border_color(if on { t.border_selected } else { t.border })
                    .when(on, |chip| chip.bg(t.surface_chosen))
                    .when(!on, |chip| chip.hover(|style| style.bg(t.surface_hover)))
                    .tooltip(ui::tooltip(shape.name.clone()))
                    .child(face(pack, shape, at as u32, 26.))
                    .on_click(choose(pack.id.clone(), shape.id.clone()))
            })
        }));
    let stacked = |label: &'static str, about: &'static str, body: AnyElement| {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(14.))
            .py(px(10.))
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                div()
                    .text_size(crate::theme::ui_text(13.))
                    .text_color(t.text_primary)
                    .child(label),
            )
            .child(
                div()
                    .text_size(crate::theme::ui_text(11.5))
                    .text_color(t.text_tertiary)
                    .child(about),
            )
            .child(body)
    };
    let buddies = div()
        .child(rows::row(
            "Show buddies",
            Some("Agents get a character in place of their harness logo."),
            face_switch("buddies-on", "on", style.on),
            cx,
        ))
        .child(stacked(
            "Pack",
            "What new agents look like. Right-click an agent to give it its own.",
            pack_cards.into_any_element(),
        ))
        .child(stacked(
            "Shape",
            if chosen_pack.is_some() {
                "Mix gives each agent its own shape from the pack."
            } else {
                "Every pack mixes shapes too."
            },
            shape_chips.into_any_element(),
        ))
        .child(rows::row(
            "Animate faces",
            Some("Faces move with what their agents are doing."),
            face_switch("buddies-motion", "motion", style.motion),
            cx,
        ));

    let window_section = div()
        .child(rows::row(
            "Opacity",
            Some("Solid at 1.00, translucent below it."),
            knob(
                &fields.opacity,
                format!("{:.2}", preferences.window_opacity),
            ),
            cx,
        ))
        .child(rows::row(
            "Background blur",
            Some("0 is crisp; 20–40px gives a soft frosted effect."),
            knob(
                &fields.blur,
                format!("{}px", preferences.window_blur.round()),
            ),
            cx,
        ));

    let mut sections = vec![
        rows::section(
            "Theme",
            Some(format!("{} built-in · {custom} custom", themes.len() - custom).into()),
            Some("Applies instantly to chrome, editor and terminal.".into()),
            lit("Theme"),
            theme_body,
            cx,
        ),
        rows::section("Interface", None, None, lit("Interface"), interface, cx),
        rows::section("Buddies", None, None, lit("Buddies"), buddies, cx),
    ];
    if cfg!(target_os = "macos") {
        sections.push(rows::section(
            "Window",
            None,
            None,
            lit("Window"),
            window_section,
            cx,
        ));
    }
    sections
}
