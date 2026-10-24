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

use crate::core::{self, Ask, Watch};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{self, Tokens};
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

fn editor_of(bench: &mut Workbench) -> Option<&mut Editor> {
    bench
        .settings
        .as_mut()?
        .pages
        .appearance
        .as_mut()?
        .editor
        .as_mut()
}

fn new_id() -> String {
    let now = chrono::Utc::now().timestamp_millis();
    format!("custom-{now:x}")
}

/// `#rrggbb` for a colour the picker chose.
fn hex(color: Hsla) -> String {
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
    let Some(draft) = bench
        .settings
        .as_ref()
        .and_then(|settings| settings.pages.appearance.as_ref()?.editor.as_ref())
        .map(|editor| editor.draft.clone())
    else {
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
        Watch::new("theme", json!({}), &["preferences"]),
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
    let Some(editor) = editor_of(bench) else {
        return;
    };
    if editor.draft[group][key].as_str() == Some(value.as_str()) {
        return;
    }
    editor.draft[group][key] = json!(value);
    preview(bench, cx);
    bench.parts.redraw(&bench.parts.stage, cx);
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
            if let Some(editor) = editor_of(bench) {
                editor.draft["name"] = json!(value);
            }
        }
    });
    let shown = fields(&draft, 0, window, cx);
    let Some(appearance) = bench
        .settings
        .as_mut()
        .and_then(|settings| settings.pages.appearance.as_mut())
    else {
        return;
    };
    appearance.editor = Some(Editor {
        draft,
        original,
        new,
        base,
        group: 0,
        name,
        fields: shown,
        _naming: naming,
    });
    preview(bench, cx);
    bench.parts.redraw(&bench.parts.stage, cx);
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
        Picture::PaneImage => bench.preferences().pane_image.map(std::path::PathBuf::from),
    };
    let Some(path) = path else {
        return;
    };
    if let Some(appearance) = bench
        .settings
        .as_mut()
        .and_then(|settings| settings.pages.appearance.as_mut())
    {
        appearance.reading = Some(picture);
    }
    bench.parts.redraw(&bench.parts.stage, cx);
    let asks = bench.asks.clone();
    cx.spawn_in(window, async move |bench, cx| {
        let read = cx
            .background_executor()
            .spawn(async move { wallpaper::pixels(&path) })
            .await;
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
            if let Some(appearance) = bench
                .settings
                .as_mut()
                .and_then(|settings| settings.pages.appearance.as_mut())
            {
                appearance.reading = None;
            }
            if let Some(original) = theme {
                let mut draft = original.clone();
                draft["id"] = json!(new_id());
                open(bench, draft, original, true, window, cx);
            }
            bench.parts.redraw(&bench.parts.stage, cx);
        });
    })
    .detach();
}

/// Takes away one of the person's own themes, going back to the default one when it was worn.
pub fn delete(bench: &Workbench, id: &str) {
    let preferences = bench.preferences();
    let kept: Vec<_> = preferences
        .custom_themes
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
    if let Some(appearance) = bench
        .settings
        .as_mut()
        .and_then(|settings| settings.pages.appearance.as_mut())
    {
        appearance.editor = None;
    }
    wear_picked(bench);
    bench.parts.redraw(&bench.parts.stage, cx);
}

/// Keeps the draft among the person's themes and wears it.
fn save(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(mut draft) = editor_of(bench).map(|editor| editor.draft.clone()) else {
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
        .into_iter()
        .filter_map(|theme| serde_json::to_value(theme).ok())
        .collect();
    match themes.iter_mut().find(|theme| theme["id"] == id.as_str()) {
        Some(theme) => *theme = draft,
        None => themes.push(draft),
    }
    crate::settings::set(bench, "customThemes", Value::Array(themes));
    crate::settings::set(bench, "themeId", id);
    if let Some(appearance) = bench
        .settings
        .as_mut()
        .and_then(|settings| settings.pages.appearance.as_mut())
    {
        appearance.editor = None;
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

/// Shows group `group`'s colours.
fn show_group(
    bench: &mut Workbench,
    group: usize,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(draft) = editor_of(bench).map(|editor| editor.draft.clone()) else {
        return;
    };
    let shown = fields(&draft, group, window, cx);
    if let Some(editor) = editor_of(bench) {
        editor.group = group;
        editor.fields = shown;
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

/// Puts every colour back as the theme it started from had it, keeping its name.
fn reset(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(editor) = editor_of(bench) else {
        return;
    };
    let (id, name) = (editor.draft["id"].clone(), editor.draft["name"].clone());
    editor.draft = editor.original.clone();
    editor.draft["id"] = id;
    editor.draft["name"] = name;
    let group = editor.group;
    show_group(bench, group, window, cx);
    preview(bench, cx);
}

fn toggle_dark(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(editor) = editor_of(bench) else {
        return;
    };
    let dark = editor.draft["dark"] == true;
    editor.draft["dark"] = json!(!dark);
    preview(bench, cx);
    bench.parts.redraw(&bench.parts.stage, cx);
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
        .gap(px(6.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .p(px(12.))
                .rounded(theme::RADIUS_2)
                .bg(theme::color(
                    draft["editor"]["bg"].as_str().unwrap_or_default(),
                ))
                .text_color(theme::color(
                    draft["editor"]["fg"].as_str().unwrap_or_default(),
                ))
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(12.))
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
                .gap(px(2.))
                .p(px(6.))
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
        .gap(px(2.))
        .child(ui::Field::new(&editor.name))
        .child(
            div()
                .text_size(crate::theme::ui_text(11.))
                .text_color(t.text_tertiary)
                .child(format!("based on {}", editor.base)),
        );
    let head = div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(8.))
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
        .gap(px(8.))
        .children(editor.fields.iter().map(|field| {
            div()
                .w(px(204.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ColorPicker::new(&field.picker))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(crate::theme::ui_text(11.))
                                .text_color(t.text_tertiary)
                                .child(field.label),
                        )
                        .child(ui::Field::new(&field.text).mono()),
                )
        }));
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(14.))
        .border_t_1()
        .border_color(t.line_soft)
        .child(named)
        .child(head)
        .child(sample(&editor.draft))
        .child(tabs)
        .child(div().text_size(crate::theme::ui_text(11.5)).text_color(t.text_tertiary).child(hint))
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
        #[cfg(not(target_os = "macos"))]
        None
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

    /// The wallpaper's name and its pixels, shrunk, as base64 RGBA. macOS wallpapers are
    /// often HEIC, so `sips` turns them into a small PNG first.
    pub fn pixels(path: &Path) -> Option<(String, String)> {
        let path = still(path);
        let name = path.file_stem()?.to_string_lossy().into_owned();
        let out = tempfile::Builder::new().suffix(".png").tempfile().ok()?;
        let shrunk = sikemux_process::user_environment::command("/usr/bin/sips")
            .args(["-s", "format", "png", "-Z", &SIZE.to_string()])
            .arg(&path)
            .arg("--out")
            .arg(out.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        if !shrunk.success() {
            return None;
        }
        let image = image::open(out.path()).ok()?.to_rgba8();
        let pixels = base64::engine::general_purpose::STANDARD.encode(image.as_raw());
        Some((name, pixels))
    }
}
