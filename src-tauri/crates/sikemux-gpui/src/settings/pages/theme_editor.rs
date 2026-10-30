//! A theme of the person's own, forked from another or worked out from the desktop
//! wallpaper, with each colour changed in place. The whole window wears the draft while
//! it is edited, and the picked theme again when the edit ends.

use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, Hsla, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Window,
};
use serde_json::{json, Value};
use sikemux_wire::changed::Topic;

use crate::core::{self, Ask, Watch};
use crate::icons::Icon;
use crate::settings::rows;
use crate::settings::Settings;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS, TEXT_2XS_H, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

/// A group's key in the theme, its name, a line on what it colours, and its colours by key and name.
type Group = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// Each group of a theme's colours: its key in the theme, its name, a line on what it
/// colours, and its colours by key and name.
const GROUPS: [Group; 4] = [
    (
        "chrome",
        "Interface",
        "Window chrome, rails, accents.",
        &[
            ("bg", "background"),
            ("bgDim", "background dim"),
            ("bgRaised", "background raised"),
            ("ink", "text"),
            ("inkDim", "text dim"),
            ("inkMuted", "text muted"),
            ("acc", "accent"),
            ("accLine", "accent border"),
            ("accDim", "accent wash"),
            ("line", "border"),
            ("hl", "row highlight"),
            ("danger", "danger"),
        ],
    ),
    (
        "editor",
        "Editor",
        "Code surface, caret, gutters.",
        &[
            ("fg", "foreground"),
            ("bg", "background"),
            ("caret", "caret"),
            ("selection", "selection"),
            ("activeLine", "active line"),
            ("gutter", "gutter"),
            ("gutterActive", "gutter active"),
            ("indent", "indent guide"),
            ("indentActive", "indent active"),
        ],
    ),
    (
        "highlight",
        "Syntax",
        "Token colours in the code editor.",
        &[
            ("keyword", "keyword"),
            ("string", "string"),
            ("comment", "comment"),
            ("number", "number"),
            ("function", "function"),
            ("type", "type"),
            ("variable", "variable"),
            ("property", "property"),
            ("tag", "tag"),
            ("operator", "operator"),
            ("link", "link"),
            ("invalid", "invalid"),
            ("meta", "meta"),
            ("heading", "heading"),
        ],
    ),
    (
        "terminal",
        "Terminal",
        "The terminal's 16 colours, with its cursor and selection.",
        &[
            ("background", "background"),
            ("foreground", "foreground"),
            ("cursor", "cursor"),
            ("cursorAccent", "cursor text"),
            ("selectionBackground", "selection"),
            ("black", "black"),
            ("red", "red"),
            ("green", "green"),
            ("yellow", "yellow"),
            ("blue", "blue"),
            ("magenta", "magenta"),
            ("cyan", "cyan"),
            ("white", "white"),
            ("brightBlack", "bright black"),
            ("brightRed", "bright red"),
            ("brightGreen", "bright green"),
            ("brightYellow", "bright yellow"),
            ("brightBlue", "bright blue"),
            ("brightMagenta", "bright magenta"),
            ("brightCyan", "bright cyan"),
            ("brightWhite", "bright white"),
        ],
    ),
];

/// The theme being edited: the draft, what it started as, and the fields of the group shown.
pub struct Editor {
    draft: Value,
    original: Value,
    /// Saving adds it as a new theme, rather than changing the one it was.
    new: bool,
    base: String,
    group: usize,
    name: Entity<InputState>,
    fields: Vec<Field>,
    _naming: Subscription,
}

/// One colour of the group shown: its text, any CSS colour, and a picker that sets it as hex.
struct Field {
    label: &'static str,
    text: Entity<InputState>,
    picker: Entity<ColorPickerState>,
    _watching: [Subscription; 2],
}

/// The theme being edited, while the editor is open.
fn editor<'a>(bench: &Workbench, cx: &'a App) -> Option<&'a Editor> {
    super::state(bench, cx)?
        .appearance
        .as_ref()?
        .editor
        .as_ref()
}

/// Changes the theme being edited, and draws the pages again.
fn change_editor<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Editor, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_form(
        bench,
        cx,
        |pages| pages.appearance.as_mut()?.editor.as_mut(),
        change,
    )
}

/// Changes what the theme pages keep, and draws them again.
fn change_appearance<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut super::appearance::Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.appearance.as_mut(), change)
}

fn new_id() -> String {
    let now = chrono::Utc::now().timestamp_millis();
    format!("custom-{now:x}")
}

/// `#rrggbb` for a colour the picker chose.
pub(super) fn hex(color: Hsla) -> String {
    let rgba = color.to_rgb();
    let byte = |part: f32| (part.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(rgba.r),
        byte(rgba.g),
        byte(rgba.b)
    )
}

/// Wears `draft` in the whole window, as the editor's live preview.
fn preview(bench: &Workbench, cx: &mut Context<Workbench>) {
    let Some(draft) = editor(bench, cx).map(|editor| editor.draft.clone()) else {
        return;
    };
    let asks = bench.asks.clone();
    cx.spawn(async move |_, cx| {
        let Ok(palette) = core::request(&asks, "themePalette", json!({ "theme": draft })).await
        else {
            return;
        };
        let Ok(palette) = serde_json::from_value(palette) else {
            return;
        };
        cx.update(|cx| {
            theme::apply(&palette, cx);
            crate::syntax::restyle(cx);
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Wears the picked theme again, ending a preview.
pub fn wear_picked(bench: &Workbench) {
    let _ = bench.asks.send(Ask::Watch(
        "theme",
        Watch::new("theme", json!({}), &[Topic::Preferences]),
    ));
}

/// Makes the fields of group `group` for the draft as it is now.
fn fields(
    draft: &Value,
    group: usize,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> Vec<Field> {
    let (key, _, _, colours) = GROUPS[group];
    colours
        .iter()
        .map(|(colour, label)| {
            let value = draft[key][*colour].as_str().unwrap_or_default().to_owned();
            let text = cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_value(value.clone(), window, cx);
                input
            });
            let picker =
                cx.new(|cx| ColorPickerState::new(window, cx).default_value(theme::color(&value)));
            let colour: &'static str = colour;
            let typed = cx.subscribe(&text, move |bench, text, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = text.read(cx).value().trim().to_owned();
                    set(bench, key, colour, value, cx);
                }
            });
            let text_of = text.clone();
            let picked = cx.subscribe_in(
                &picker,
                window,
                move |bench, _, event: &ColorPickerEvent, window, cx| {
                    let ColorPickerEvent::Change(Some(color)) = event else {
                        return;
                    };
                    let value = hex(*color);
                    text_of.update(cx, |text, cx| {
                        if text.value().as_ref() != value {
                            text.set_value(value.clone(), window, cx);
                        }
                    });
                    set(bench, key, colour, value, cx);
                },
            );
            Field {
                label,
                text,
                picker,
                _watching: [typed, picked],
            }
        })
        .collect()
}

/// Sets one colour of the draft and wears it.
fn set(bench: &mut Workbench, group: &str, key: &str, value: String, cx: &mut Context<Workbench>) {
    let unchanged = editor(bench, cx).map(|editor| editor.draft[group][key] == value.as_str());
    if unchanged != Some(false) {
        return;
    }
    change_editor(bench, cx, |editor, _| {
        editor.draft[group][key] = json!(value)
    });
    preview(bench, cx);
}

/// Opens the editor on `draft`, which started as `original`.
fn open(
    bench: &mut Workbench,
    draft: Value,
    original: Value,
    new: bool,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let base = original["name"].as_str().unwrap_or_default().to_owned();
    let name = cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder("theme name");
        input.set_value(
            draft["name"].as_str().unwrap_or_default().to_owned(),
            window,
            cx,
        );
        input
    });
    let naming = cx.subscribe(&name, |bench, name, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            let value = name.read(cx).value().to_string();
            bench.settings.update(cx, |settings, _| {
                let open = settings.open.as_mut();
                let editor = open.and_then(|open| open.pages.appearance.as_mut()?.editor.as_mut());
                if let Some(editor) = editor {
                    editor.draft["name"] = json!(value);
                }
            });
        }
    });
    let shown = fields(&draft, 0, window, cx);
    let editor = Editor {
        draft,
        original,
        new,
        base,
        group: 0,
        name,
        fields: shown,
        _naming: naming,
    };
    if change_appearance(bench, cx, |appearance, _| appearance.editor = Some(editor)).is_some() {
        preview(bench, cx);
    }
}

/// Forks the theme `id` into an editable copy.
pub fn customize(bench: &Workbench, id: String, window: &mut Window, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn_in(window, async move |bench, cx| {
        let Ok(palette) = core::request(&asks, "theme", json!({ "id": id })).await else {
            return;
        };
        let original = palette["theme"].clone();
        let mut draft = original.clone();
        draft["id"] = json!(new_id());
        draft["name"] = json!(format!(
            "{} custom",
            original["name"].as_str().unwrap_or("theme")
        ));
        let _ = bench.update_in(cx, |bench, window, cx| {
            open(bench, draft, original, true, window, cx)
        });
    })
    .detach();
}

/// Edits one of the person's own themes.
pub fn edit(bench: &mut Workbench, id: &str, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(theme) = bench
        .preferences()
        .custom_themes
        .clone()
        .into_iter()
        .find(|theme| theme.id == id)
    else {
        return;
    };
    let theme = serde_json::to_value(theme).unwrap_or_default();
    open(bench, theme.clone(), theme, false, window, cx);
}

/// A picture a theme can be drawn from.
#[derive(Clone, Copy, PartialEq)]
pub enum Picture {
    Wallpaper,
    PaneImage,
}

pub use wallpaper::offered as wallpaper_offered;

/// Works out a theme from the colours of the desktop wallpaper or the pane image, and opens
/// it to edit.
pub fn from_picture(
    bench: &mut Workbench,
    picture: Picture,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let path = match picture {
        Picture::Wallpaper => wallpaper::path(),
        Picture::PaneImage => bench
            .preferences()
            .pane_image
            .clone()
            .map(std::path::PathBuf::from),
    };
    let Some(path) = path else {
        return;
    };
    change_appearance(bench, cx, |appearance, _| {
        appearance.reading = Some(picture)
    });
    let asks = bench.asks.clone();
    cx.spawn_in(window, async move |bench, cx| {
        let read = cx
            .background_executor()
            .spawn(async move { wallpaper::pixels(&path) })
            .await;
        let unread = read.is_none();
        let theme = match read {
            Some((name, pixels)) => {
                let name = match picture {
                    Picture::Wallpaper => format!("{name} wallpaper"),
                    Picture::PaneImage => name,
                };
                let asked = json!({ "name": name, "pixels": pixels });
                core::request(&asks, "themeFromPixels", asked).await.ok()
            }
            None => None,
        };
        let _ = bench.update_in(cx, |bench, window, cx| {
            change_appearance(bench, cx, |appearance, _| appearance.reading = None);
            if unread {
                let note = match picture {
                    Picture::Wallpaper => "This wallpaper's format can't be read.",
                    Picture::PaneImage => "This pane image's format can't be read.",
                };
                bench.notify(crate::toast::error(note), cx);
            }
            if let Some(original) = theme {
                let mut draft = original.clone();
                draft["id"] = json!(new_id());
                open(bench, draft, original, true, window, cx);
            }
        });
    })
    .detach();
}

/// Takes away one of the person's own themes, going back to the default one when it was worn.
pub fn delete(bench: &Workbench, id: &str) {
    let preferences = bench.preferences();
    let kept: Vec<_> = preferences
        .custom_themes
        .clone()
        .into_iter()
        .filter(|theme| theme.id != id)
        .collect();
    crate::settings::set(
        bench,
        "customThemes",
        serde_json::to_value(kept).unwrap_or_default(),
    );
    if preferences.theme_id == id {
        crate::settings::set(bench, "themeId", "aura-noir");
    }
}

fn close(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change_appearance(bench, cx, |appearance, _| appearance.editor = None);
    wear_picked(bench);
}

/// Keeps the draft among the person's themes and wears it.
fn save(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(mut draft) = editor(bench, cx).map(|editor| editor.draft.clone()) else {
        return;
    };
    let named = draft["name"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("custom theme")
        .to_owned();
    draft["name"] = json!(named);
    let id = draft["id"].as_str().unwrap_or_default().to_owned();
    let mut themes: Vec<Value> = bench
        .preferences()
        .custom_themes
        .clone()
        .into_iter()
        .filter_map(|theme| serde_json::to_value(theme).ok())
        .collect();
    match themes.iter_mut().find(|theme| theme["id"] == id.as_str()) {
        Some(theme) => *theme = draft,
        None => themes.push(draft),
    }
    crate::settings::set(bench, "customThemes", Value::Array(themes));
    crate::settings::set(bench, "themeId", id);
    change_appearance(bench, cx, |appearance, _| appearance.editor = None);
}

/// Shows group `group`'s colours.
fn show_group(
    bench: &mut Workbench,
    group: usize,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(draft) = editor(bench, cx).map(|editor| editor.draft.clone()) else {
        return;
    };
    let shown = fields(&draft, group, window, cx);
    change_editor(bench, cx, |editor, _| {
        editor.group = group;
        editor.fields = shown;
    });
}

/// Puts every colour back as the theme it started from had it, keeping its name.
fn reset(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(group) = change_editor(bench, cx, |editor, _| {
        let (id, name) = (editor.draft["id"].clone(), editor.draft["name"].clone());
        editor.draft = editor.original.clone();
        editor.draft["id"] = id;
        editor.draft["name"] = name;
        editor.group
    }) else {
        return;
    };
    show_group(bench, group, window, cx);
    preview(bench, cx);
}

fn toggle_dark(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let flipped = change_editor(bench, cx, |editor, _| {
        let dark = editor.draft["dark"] == true;
        editor.draft["dark"] = json!(!dark);
    });
    if flipped.is_some() {
        preview(bench, cx);
    }
}

/// A few lines of code and the terminal's colours, as the draft draws them.
fn sample(draft: &Value) -> AnyElement {
    let h = &draft["highlight"];
    let paint = |key: &str| theme::color(h[key].as_str().unwrap_or_default());
    let word = |text: &'static str, key: &str| div().text_color(paint(key)).child(text);
    let ansi = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "white",
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightMagenta",
        "brightCyan",
        "brightWhite",
    ];
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .p(SPACE_3)
                .rounded(theme::RADIUS_2)
                .bg(theme::color(
                    draft["editor"]["bg"].as_str().unwrap_or_default(),
                ))
                .text_color(theme::color(
                    draft["editor"]["fg"].as_str().unwrap_or_default(),
                ))
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(
                    div()
                        .italic()
                        .text_color(paint("comment"))
                        .child("// fork a base, tweak, save"),
                )
                .child(
                    div()
                        .flex()
                        .child(word("const ", "keyword"))
                        .child(word("swatch", "variable"))
                        .child(word(" = ", "operator"))
                        .child(word("paint", "function"))
                        .child(word("(", "operator"))
                        .child(word("\"#a277ff\"", "string"))
                        .child(word(", ", "operator"))
                        .child(word("0.3", "number"))
                        .child(word(");", "operator")),
                ),
        )
        .child(
            div()
                .flex()
                .gap(SPACE_0)
                .p(SPACE_1H)
                .rounded(theme::RADIUS_2)
                .bg(theme::color(
                    draft["terminal"]["background"].as_str().unwrap_or_default(),
                ))
                .children(ansi.iter().map(|key| {
                    div().flex_1().h(px(12.)).rounded(px(2.)).bg(theme::color(
                        draft["terminal"][*key].as_str().unwrap_or_default(),
                    ))
                })),
        )
        .into_any_element()
}

fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    variant: ui::Variant,
    this: &Entity<Workbench>,
    run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
) -> ui::Button {
    let this = this.clone();
    let run = std::rc::Rc::new(run);
    ui::Button::new(id, label, variant).on_click(move |_, window, cx| {
        let run = run.clone();
        this.update(cx, |bench, cx| run(bench, window, cx))
    })
}

/// The editor, under the theme list while a theme is being edited.
pub fn view(editor: &Editor, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (_, _, hint, _) = GROUPS[editor.group];
    let dark = editor.draft["dark"] == true;
    let tabs = {
        let this = this.clone();
        ui::Segmented::new(
            "theme-groups",
            vec![
                (Icon::Window, SharedString::from(GROUPS[0].1)),
                (Icon::Editor, SharedString::from(GROUPS[1].1)),
                (Icon::Code, SharedString::from(GROUPS[2].1)),
                (Icon::Terminal, SharedString::from(GROUPS[3].1)),
            ],
            editor.group,
        )
        .on_select(move |group, window, cx| {
            let group = *group;
            this.update(cx, |bench, cx| show_group(bench, group, window, cx))
        })
    };
    let named = div()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .child(ui::Field::new(&editor.name))
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(format!("based on {}", editor.base)),
        );
    let head = div()
        .flex()
        .items_center()
        .justify_end()
        .gap(SPACE_2)
        .child(button(
            "theme-dark",
            if dark { "Dark" } else { "Light" },
            ui::Variant::Outline,
            this,
            |bench, _, cx| toggle_dark(bench, cx),
        ))
        .child(button(
            "theme-reset",
            "Reset",
            ui::Variant::Outline,
            this,
            reset,
        ))
        .child(
            button(
                "theme-cancel",
                "Cancel",
                ui::Variant::Outline,
                this,
                |bench, _, cx| close(bench, cx),
            )
            .icon(Icon::Close),
        )
        .child(
            button(
                "theme-save",
                if editor.new { "Save theme" } else { "Update" },
                ui::Variant::Primary,
                this,
                |bench, _, cx| save(bench, cx),
            )
            .icon(Icon::Check),
        );
    let grid = div()
        .flex()
        .flex_wrap()
        .gap(SPACE_2)
        .children(editor.fields.iter().map(|field| {
            div()
                .w(px(204.))
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ColorPicker::new(&field.picker))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(SPACE_0)
                        .child(
                            div()
                                .text_size(crate::theme::ui_text(TEXT_2XS))
                                .text_color(t.text_tertiary)
                                .child(field.label),
                        )
                        .child(ui::Field::new(&field.text).mono()),
                )
        }));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .p(SPACE_4)
        .border_t_1()
        .border_color(t.line_soft)
        .child(named)
        .child(head)
        .child(sample(&editor.draft))
        .child(tabs)
        .child(div().text_size(crate::theme::ui_text(TEXT_2XS_H)).text_color(t.text_tertiary).child(hint))
        .child(grid)
        .child(rows::note(
            "Any CSS colour works in the text box: use rgba(…) for a translucent wash. The picker sets hex.",
            cx,
        ))
        .into_any_element()
}

/// The desktop wallpaper, read as the core's `themeFromPixels` takes it.
mod wallpaper {
    use std::path::{Path, PathBuf};

    use base64::Engine;

    /// How many pixels across the wallpaper is shrunk to before its colours are read.
    const SIZE: u32 = 160;

    /// Whether this desktop says where its wallpaper is: the Mac does, and on Linux GNOME.
    pub fn offered() -> bool {
        cfg!(target_os = "macos")
            || (cfg!(target_os = "linux")
                && std::env::var("XDG_CURRENT_DESKTOP")
                    .is_ok_and(|desktop| desktop.split(':').any(|name| name == "GNOME")))
    }

    /// Where the main screen's wallpaper is.
    pub fn path() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            use objc2::MainThreadMarker;
            use objc2_app_kit::{NSScreen, NSWorkspace};
            let screen = NSScreen::mainScreen(MainThreadMarker::new()?)?;
            let url = NSWorkspace::sharedWorkspace().desktopImageURLForScreen(&screen)?;
            Some(PathBuf::from(url.path()?.to_string()))
        }
        #[cfg(target_os = "linux")]
        {
            gnome::path()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        None
    }

    /// GNOME keeps a light and a dark wallpaper, worn by the colour scheme it is set to.
    #[cfg(target_os = "linux")]
    mod gnome {
        use std::path::PathBuf;

        fn setting(schema: &str, key: &str) -> Option<String> {
            let output = sikemux_process::user_environment::command("gsettings")
                .args(["get", schema, key])
                .output()
                .ok()?;
            output.status.success().then(|| {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .trim_matches('\'')
                    .to_owned()
            })
        }

        pub fn path() -> Option<PathBuf> {
            let dark = setting("org.gnome.desktop.interface", "color-scheme").as_deref()
                == Some("prefer-dark");
            let key = if dark {
                "picture-uri-dark"
            } else {
                "picture-uri"
            };
            let uri = setting("org.gnome.desktop.background", key)?;
            let path = url::Url::parse(&uri).ok()?.to_file_path().ok()?;
            Some(first_slide(path))
        }

        /// A wallpaper that changes through the day is an XML list of pictures; its first one
        /// stands for it.
        fn first_slide(path: PathBuf) -> PathBuf {
            if path.extension().is_none_or(|extension| extension != "xml") {
                return path;
            }
            let Ok(list) = std::fs::read_to_string(&path) else {
                return path;
            };
            list.split("<file>")
                .nth(1)
                .and_then(|file| file.split("</file>").next())
                .and_then(|file| {
                    file.split(['<', '>'])
                        .map(str::trim)
                        .find(|part| part.starts_with('/'))
                })
                .map(PathBuf::from)
                .unwrap_or(path)
        }
    }

    /// An aerial wallpaper is a `.madesktop` plist; its still frame lives at `thumbnailPath`.
    fn still(path: &Path) -> PathBuf {
        if path
            .extension()
            .is_none_or(|extension| extension != "madesktop")
        {
            return path.to_owned();
        }
        let thumbnail = sikemux_process::user_environment::command("/usr/bin/plutil")
            .args(["-extract", "thumbnailPath", "raw", "-o", "-"])
            .arg(path)
            .output()
            .ok()
            .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()));
        thumbnail
            .filter(|thumbnail| thumbnail.is_file())
            .unwrap_or_else(|| path.to_owned())
    }

    /// The picture's name and its pixels, shrunk, as base64 RGBA.
    pub fn pixels(path: &Path) -> Option<(String, String)> {
        let path = still(path);
        let name = path.file_stem()?.to_string_lossy().into_owned();
        let image = shrunk(&path)?;
        let pixels = base64::engine::general_purpose::STANDARD.encode(image.as_raw());
        Some((name, pixels))
    }

    /// macOS wallpapers are often HEIC, which only `sips` reads, so it turns them into a small
    /// PNG first.
    #[cfg(target_os = "macos")]
    fn shrunk(path: &Path) -> Option<image::RgbaImage> {
        let out = tempfile::Builder::new().suffix(".png").tempfile().ok()?;
        let shrunk = sikemux_process::user_environment::command("/usr/bin/sips")
            .args(["-s", "format", "png", "-Z", &SIZE.to_string()])
            .arg(path)
            .arg("--out")
            .arg(out.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        if !shrunk.success() {
            return None;
        }
        Some(image::open(out.path()).ok()?.to_rgba8())
    }

    #[cfg(not(target_os = "macos"))]
    fn shrunk(path: &Path) -> Option<image::RgbaImage> {
        Some(image::open(path).ok()?.thumbnail(SIZE, SIZE).to_rgba8())
    }
}
