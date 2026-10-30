//! Files that are not text, shown for what they are: a picture to zoom, a document
//! in macOS's own viewer, and anything else as the first of its bytes. A bar over
//! each names the file and opens or reveals it. Another host's pictures and documents
//! are shown from a read-only copy brought over to this disk.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;
use gpui::PinchEvent;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, img, px, uniform_list, AnyElement, App, Context, Entity, FontWeight,
    ImgResourceLoader, InteractiveElement, IntoElement, ObjectFit, ParentElement, Pixels,
    RenderOnce, Resource, SharedString, Size, StatefulInteractiveElement, Styled, StyledImage,
    Window,
};
use serde_json::json;

use super::{copies, Editors, FileKey};
use crate::core::Target;
use crate::hosts::Place as Host;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_SM, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

/// How much of a file the byte view shows.
const SHOWN_BYTES: u64 = 64 * 1024;
const BYTES_PER_ROW: usize = 16;

/// Extensions that are never text, so the file goes straight to a viewer.
const NOT_TEXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif", "tif", "tiff", "heic",
    "heif", "psd", "pdf", "ttf", "otf", "woff", "woff2", "mp4", "m4v", "mov", "webm", "mkv", "avi",
    "mp3", "m4a", "aac", "wav", "flac", "aif", "aiff", "ogg", "oga", "opus", "doc", "docx", "xls",
    "xlsx", "ppt", "pptx", "rtf", "odt", "ods", "odp", "pages", "numbers", "epub", "zip", "gz",
    "tgz", "bz2", "xz", "7z", "rar", "tar", "jar", "dmg", "iso", "exe", "dll", "so", "dylib", "o",
    "a", "class", "pyc", "wasm", "sqlite", "sqlite3",
];

/// Whether a file is shown in a viewer without being read as text first.
pub(super) fn not_text(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| NOT_TEXT.contains(&extension.to_ascii_lowercase().as_str()))
}

/// A file shown for what it is, once the core says what that is.
#[derive(Default)]
pub(super) struct Viewer {
    about: Option<About>,
    zoom: Zoom,
    /// The room a picture is shown in, as last drawn.
    room: Rc<Cell<Size<Pixels>>>,
    bytes: Option<Vec<u8>>,
    /// Where a picture or document is read from on this disk: the file itself, or
    /// the copy of another host's.
    local: Option<PathBuf>,
    failed: Option<SharedString>,
    /// A document's viewer, which macOS draws under the window's overlays; none inside
    /// once macOS would not make one.
    #[cfg(target_os = "macos")]
    document: Option<Option<Opened>>,
}

struct About {
    mime: SharedString,
    size: u64,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Zoom {
    #[default]
    Fit,
    /// So many points to one of the picture's pixels, scrolled when larger.
    Scale(f32),
}

const ZOOM_STEP: f32 = 1.25;
const ZOOM_RANGE: (f32, f32) = (0.1, 8.);

/// A picture fitted to the pane, or at a scale of its own size.
#[derive(IntoElement)]
struct Picture {
    path: PathBuf,
    zoom: Zoom,
}

impl RenderOnce for Picture {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let picture = img(self.path.clone());
        let Zoom::Scale(scale) = self.zoom else {
            return picture.size_full().object_fit(ObjectFit::Contain);
        };
        let resource = Resource::Path(Arc::from(self.path.as_path()));
        let natural = window
            .use_asset::<ImgResourceLoader>(&resource, cx)
            .and_then(Result::ok)
            .map(|image| image.size(0));
        match natural {
            Some(size) => picture
                .flex_none()
                .m_auto()
                .w(px(size.width.0 as f32 * scale))
                .h(px(size.height.0 as f32 * scale))
                .object_fit(ObjectFit::Fill),
            None => picture.object_fit(ObjectFit::None),
        }
    }
}

#[derive(PartialEq)]
enum Kind {
    Image,
    /// A document, video or sound macOS's PDF or Quick Look viewer shows, and elsewhere a
    /// card opens.
    Document,
    /// Sound and video Quick Look cannot play, such as WebM.
    Elsewhere,
    Bytes,
}

fn kind_of(mime: &str) -> Kind {
    let documents = mime == "application/pdf"
        || mime.starts_with("application/vnd.openxmlformats-")
        || mime.starts_with("application/vnd.oasis.opendocument.")
        || matches!(
            mime,
            "application/rtf"
                | "application/msword"
                | "application/vnd.ms-excel"
                | "application/vnd.ms-powerpoint"
                | "application/vnd.apple.pages"
                | "application/vnd.apple.numbers"
                | "application/vnd.apple.keynote"
                | "application/epub+zip"
                | "image/vnd.adobe.photoshop"
                | "video/mp4"
                | "video/quicktime"
                | "audio/mpeg"
                | "audio/mp4"
                | "audio/aac"
                | "audio/wav"
                | "audio/aiff"
                | "audio/flac"
        );
    if documents {
        return Kind::Document;
    }
    if mime.starts_with("audio/") || mime.starts_with("video/") {
        return Kind::Elsewhere;
    }
    if mime.starts_with("image/") {
        return Kind::Image;
    }
    Kind::Bytes
}

/// Asks the core what the file is, and reads the bytes a byte view shows.
pub(super) fn load(bench: &mut Workbench, key: FileKey, cx: &mut Context<Workbench>) {
    let added = bench.editors.update(cx, |editors, _| {
        let added = !editors.viewers.contains_key(&key);
        editors.viewers.entry(key.clone()).or_default();
        added
    });
    if added {
        super::show_files(bench, &key.place, cx);
    }
    let target = Target::new(&bench.asks, key.place.clone());
    let editors = bench.editors.downgrade();
    cx.spawn(async move |_, cx| {
        let name = key.path.to_string_lossy().into_owned();
        let about = target
            .request("files.preview", json!({ "path": name }))
            .await;
        let about = match about {
            Ok(about) => About {
                mime: about["mime"]
                    .as_str()
                    .unwrap_or("application/octet-stream")
                    .to_owned()
                    .into(),
                size: about["size"].as_u64().unwrap_or(0),
            },
            Err(error) => {
                let _ = editors.update(cx, |editors, cx| {
                    if let Some(viewer) = editors.viewers.get_mut(&key) {
                        viewer.failed = Some(error.into());
                    }
                    cx.notify();
                });
                return;
            }
        };
        let kind = kind_of(&about.mime);
        let shown_here = matches!(kind, Kind::Image | Kind::Document);
        let local = match &key.place {
            _ if !shown_here => None,
            Host::This => Some(key.path.clone()),
            Host::Account(_) => {
                match copies::bring(editors.clone(), target.clone(), key.clone(), about.size, cx)
                    .await
                {
                    Ok(copy) => Some(copy),
                    Err(error) => {
                        let _ = editors.update(cx, |editors, cx| {
                            if let Some(viewer) = editors.viewers.get_mut(&key) {
                                viewer.failed = Some(error.into());
                            }
                            cx.notify();
                        });
                        return;
                    }
                }
            }
        };
        let bytes = match kind {
            Kind::Bytes => {
                let read = json!({ "path": name, "at": 0, "length": SHOWN_BYTES });
                target
                    .request("files.bytes", read)
                    .await
                    .ok()
                    .and_then(|read| {
                        base64::engine::general_purpose::STANDARD
                            .decode(read["base64"].as_str()?)
                            .ok()
                    })
            }
            _ => None,
        };
        let document = kind == Kind::Document;
        let _ = editors.update(cx, |editors, cx| {
            if let Some(viewer) = editors.viewers.get_mut(&key) {
                viewer.about = Some(about);
                viewer.bytes = bytes;
                viewer.local = local;
                viewer.failed = None;
            }
            if document {
                show_document(editors, key.clone(), cx);
            }
            cx.notify();
        });
    })
    .detach();
}

/// Opens the document in macOS's own viewer, which needs the window to live in.
fn show_document(editors: &mut Editors, key: FileKey, cx: &mut Context<Editors>) {
    #[cfg(target_os = "macos")]
    {
        let Some(handle) = editors.window else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |editors, cx| {
                    let Some(viewer) = editors.viewers.get_mut(&key) else {
                        return;
                    };
                    let Some(local) = viewer.local.as_deref() else {
                        return;
                    };
                    let document = gpui_native_view::document::Document::new(local, window)
                        .map(std::rc::Rc::new);
                    let focus = cx.focus_handle();
                    viewer.document = Some(document.map(|document| {
                        let surface = gpui_native_view::NativeSurface::new(Shown(document.clone()));
                        (document, surface, focus)
                    }));
                    cx.notify();
                });
            });
        })
        .detach();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (editors, key, cx);
}

/// Shows a picture or document again after its file changed on its host's disk,
/// bringing another host's over again.
pub(super) fn reopen(bench: &mut Workbench, keys: &[FileKey], cx: &mut Context<Workbench>) {
    for key in keys {
        let shown_here =
            (bench.editors.read(cx).viewers.get(key)).is_some_and(|viewer| viewer.local.is_some());
        if shown_here && key.place != Host::This {
            load(bench, key.clone(), cx);
        }
    }
    let viewers = &bench.editors.read(cx).viewers;
    let pictures: Vec<&FileKey> = (keys.iter())
        .filter(|key| key.place == Host::This)
        .filter(|key| {
            (viewers.get(*key))
                .and_then(|viewer| viewer.about.as_ref())
                .is_some_and(|about| kind_of(&about.mime) == Kind::Image)
        })
        .collect();
    for key in &pictures {
        forget_picture(&key.path, cx);
    }
    if !pictures.is_empty() {
        bench.editors.update(cx, |_, cx| cx.notify());
    }
    #[cfg(target_os = "macos")]
    for key in keys.iter().filter(|key| key.place == Host::This) {
        if let Some((document, _, _)) = (bench.editors.read(cx).viewers.get(key))
            .and_then(|viewer| viewer.document.as_ref()?.as_ref())
        {
            document.open(&key.path);
        }
    }
}

/// Drops the picture drawn from `path`, so the next frame reads the file again.
fn forget_picture(path: &Path, cx: &mut App) {
    let resource = Resource::Path(Arc::from(path));
    if let Some(Ok(drawn)) = cx.fetch_asset::<ImgResourceLoader>(&resource) {
        cx.drop_image(drawn, None);
    }
    cx.remove_asset::<ImgResourceLoader>(&resource);
}

/// A document's viewer, and the surface its element places.
#[cfg(target_os = "macos")]
type Opened = (
    std::rc::Rc<gpui_native_view::document::Document>,
    gpui_native_view::NativeSurface,
    gpui_kit::FocusHandle,
);

/// A document the editor keeps, placed where its element is laid out.
#[cfg(target_os = "macos")]
struct Shown(std::rc::Rc<gpui_native_view::document::Document>);

#[cfg(target_os = "macos")]
impl gpui_native_view::Surface for Shown {
    fn place(&self, frame: Option<gpui_native_view::Frame>) {
        gpui_native_view::Surface::place(&*self.0, frame)
    }

    fn responder(&self) -> Option<gpui_native_view::Retained<gpui_native_view::NSView>> {
        gpui_native_view::Surface::responder(&*self.0)
    }
}

/// The viewer of the file at `path`.
pub(super) fn pane(
    bench: &Workbench,
    this: &Entity<Workbench>,
    key: &FileKey,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let path = key.path.as_path();
    let Some(viewer) = bench.editors.read(cx).viewers.get(key) else {
        return div().size_full().into_any_element();
    };
    let kind = viewer.about.as_ref().map(|about| kind_of(&about.mime));
    let body: AnyElement = match (&viewer.failed, kind) {
        (Some(error), _) => message(error.clone(), &t),
        (None, None) => message("Reading…".into(), &t),
        (None, Some(Kind::Image)) => {
            let picture = Picture {
                path: viewer.local.clone().unwrap_or_else(|| path.to_owned()),
                zoom: viewer.zoom,
            };
            let (room, pinched, this) = (viewer.room.clone(), key.clone(), this.clone());
            div()
                .size_full()
                .relative()
                .child(
                    canvas(move |bounds, _, _| room.set(bounds.size), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
                .child(
                    div()
                        .id("viewer-image")
                        .size_full()
                        .flex()
                        .overflow_scroll()
                        .on_pinch(move |event: &PinchEvent, _, cx| {
                            this.update(cx, |bench, cx| {
                                pinch(bench, &pinched, 1. + event.delta, cx)
                            })
                        })
                        .child(picture),
                )
                .into_any_element()
        }
        (None, Some(Kind::Document)) => document(viewer, key, &t),
        (None, Some(Kind::Elsewhere)) => card(viewer, key, &t),
        (None, Some(Kind::Bytes)) => bytes(viewer.bytes.clone().unwrap_or_default(), &t),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(bar(this, key, viewer, cx))
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// Scales the picture by `factor`, starting from the size it is fitted to while it is fitted.
fn pinch(bench: &mut Workbench, key: &FileKey, factor: f32, cx: &mut Context<Workbench>) {
    let Some(viewer) = bench.editors.read(cx).viewers.get(key) else {
        return;
    };
    let (zoom, room) = (viewer.zoom, viewer.room.get());
    let path = viewer.local.clone().unwrap_or_else(|| key.path.clone());
    let scale = match zoom {
        Zoom::Scale(scale) => scale,
        Zoom::Fit => {
            let resource = Resource::Path(Arc::from(path.as_path()));
            let Some(natural) = cx
                .fetch_asset::<ImgResourceLoader>(&resource)
                .and_then(Result::ok)
                .map(|image| image.size(0))
            else {
                return;
            };
            let wide = f32::from(room.width) / natural.width.0 as f32;
            let tall = f32::from(room.height) / natural.height.0 as f32;
            wide.min(tall)
        }
    };
    let (least, most) = ZOOM_RANGE;
    bench.editors.update(cx, |editors, cx| {
        if let Some(viewer) = editors.viewers.get_mut(key) {
            viewer.zoom = Zoom::Scale((scale * factor).clamp(least, most));
        }
        cx.notify();
    });
}

/// A document in macOS's own viewer, once it is open.
fn document(viewer: &Viewer, key: &FileKey, t: &Tokens) -> AnyElement {
    #[cfg(target_os = "macos")]
    match &viewer.document {
        Some(Some((_, surface, focus))) => {
            return gpui_native_view::native_view(
                SharedString::from(format!("document-{}", key.path.display())),
                surface,
            )
            .track_focus(focus)
            .size_full()
            .into_any_element()
        }
        Some(None) => {}
        None => return message("Opening…".into(), t),
    }
    card(viewer, key, t)
}

/// A file the window does not draw: its name, size and type, and the app the system opens
/// it with.
fn card(viewer: &Viewer, key: &FileKey, t: &Tokens) -> AnyElement {
    let about = viewer
        .about
        .as_ref()
        .map(|about| format!("{} · {}", size(about.size), about.mime));
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_1H)
        .child(ui::icon(Icon::File, 28.).text_color(t.text_tertiary))
        .child(
            ui::text(name_of(&key.path), TEXT_SM, FontWeight::MEDIUM)
                .mt(SPACE_1)
                .text_color(t.text_primary),
        )
        .children(about.map(|about| {
            div()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(about)
        }))
        .children(openable(viewer, key).map(|open| {
            div().mt(SPACE_2).child(
                ui::Button::new(
                    "viewer-open-default",
                    "Open in default app",
                    ui::Variant::Outline,
                )
                .icon(Icon::Share)
                .on_click(move |_, _, cx| cx.open_with_system(&open)),
            )
        }))
        .into_any_element()
}

/// Where the system can open the file from: its copy here, or the file itself on this host.
fn openable(viewer: &Viewer, key: &FileKey) -> Option<PathBuf> {
    viewer
        .local
        .clone()
        .or_else(|| (key.place == Host::This).then(|| key.path.clone()))
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn message(text: SharedString, t: &Tokens) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(crate::theme::ui_text(TEXT_SM))
        .text_color(t.text_tertiary)
        .child(text)
        .into_any_element()
}

/// The first bytes of a file, sixteen to a row: where each row starts, the
/// bytes, and the characters they would print as.
fn bytes(bytes: Vec<u8>, t: &Tokens) -> AnyElement {
    let rows = bytes.len().div_ceil(BYTES_PER_ROW);
    let (ink, dim) = (t.text_secondary, t.text_tertiary);
    uniform_list("viewer-bytes", rows, move |range, _, _| {
        range
            .map(|row| {
                let start = row * BYTES_PER_ROW;
                let chunk = &bytes[start..(start + BYTES_PER_ROW).min(bytes.len())];
                let hex: Vec<String> = chunk.iter().map(|byte| format!("{byte:02x}")).collect();
                let printed: String = chunk
                    .iter()
                    .map(|&byte| {
                        if byte.is_ascii_graphic() || byte == b' ' {
                            byte as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                div()
                    .flex()
                    .gap(SPACE_4)
                    .px(SPACE_4)
                    .h(px(20.))
                    .items_center()
                    .child(
                        div()
                            .w(px(72.))
                            .text_color(dim)
                            .child(format!("{start:08x}")),
                    )
                    .child(div().w(px(392.)).text_color(ink).child(hex.join(" ")))
                    .child(div().text_color(dim).child(printed))
            })
            .collect()
    })
    .size_full()
    .font_family(theme::MONO)
    .text_size(crate::theme::ui_text(TEXT_XS))
    .into_any_element()
}

/// The bar over a viewer: the file's name, size and type, the picture's
/// zoom, and buttons that read it again, open it in its own app, or show it
/// in Finder.
fn bar(this: &Entity<Workbench>, key: &FileKey, viewer: &Viewer, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let path = key.path.as_path();
    let name = name_of(path);
    let about = viewer.about.as_ref();
    let zoom = |id: &'static str, label: &'static str, to: fn(Zoom) -> Zoom| {
        let (this, key) = (this.clone(), key.clone());
        div()
            .id(id)
            .px(SPACE_1H)
            .cursor_pointer()
            .text_color(t.text_secondary)
            .hover(move |style| style.text_color(t.text_primary))
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    bench.editors.update(cx, |editors, cx| {
                        if let Some(viewer) = editors.viewers.get_mut(&key) {
                            viewer.zoom = to(viewer.zoom);
                        }
                        cx.notify();
                    })
                })
            })
            .child(label)
    };
    let image = about.is_some_and(|about| kind_of(&about.mime) == Kind::Image);
    let reload = key.clone();
    let open = openable(viewer, key);
    let reveal = (key.place == Host::This).then(|| path.to_owned());
    let this_reload = this.clone();
    div()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .h(px(32.))
        .px(SPACE_3)
        .border_b_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(ui::icon(Icon::File, 14.).text_color(t.text_tertiary))
        .child(div().text_color(t.text_primary).child(name))
        .children(about.map(|about| div().text_color(t.text_tertiary).child(size(about.size))))
        .children(about.map(|about| div().text_color(t.text_tertiary).child(about.mime.clone())))
        .child(div().flex_1())
        .when(image, |bar| {
            bar.child(zoom("viewer-fit", "Fit", |_| Zoom::Fit))
                .child(zoom("viewer-actual", "1:1", |_| Zoom::Scale(1.)))
                .child(zoom("viewer-out", "−", |zoom| match zoom {
                    Zoom::Fit => Zoom::Scale(1. / ZOOM_STEP),
                    Zoom::Scale(scale) => Zoom::Scale(scale / ZOOM_STEP),
                }))
                .child(zoom("viewer-in", "+", |zoom| match zoom {
                    Zoom::Fit => Zoom::Scale(ZOOM_STEP),
                    Zoom::Scale(scale) => Zoom::Scale(scale * ZOOM_STEP),
                }))
        })
        .child(
            ui::IconButton::new("viewer-reload", Icon::Refresh, "Read it again")
                .small()
                .on_click(move |_, _, cx| {
                    let path = reload.clone();
                    this_reload.update(cx, |bench, cx| load(bench, path, cx))
                }),
        )
        .children(open.map(|open| {
            ui::IconButton::new("viewer-open", Icon::Share, "Open in its own app")
                .small()
                .on_click(move |_, _, cx| cx.open_with_system(&open))
        }))
        .children(reveal.map(|reveal| {
            ui::IconButton::new("viewer-reveal", Icon::Folder, ui::SHOW_IN_FILES)
                .small()
                .on_click(move |_, _, cx| cx.reveal_path(&reveal))
        }))
}

/// A size in bytes as people read it.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ => format!("{value:.1} {}", UNITS[unit]),
    }
}
