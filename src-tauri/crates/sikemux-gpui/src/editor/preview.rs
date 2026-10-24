//! A markdown file drawn the way it reads, beside its source: links open,
//! a web address in the browser and a file in its editor.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, App, Context, Entity, Hsla, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};
use gpui_transcript::{Markdown, Pen, Style};
use serde_json::Value;
use sikemux_markdown::{Doc, Op, Options};

use super::OpenAt;
use crate::theme::{self, Tokens};
use crate::workbench::Workbench;

const OPTIONS: Options = Options {
    gfm: true,
    html_as_text: false,
    file_links: true,
    html_images: false,
    file_paths: false,
};

/// Whether a file's editor shows its text or the markdown it renders to.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Source,
    Preview,
}

/// The editor a file opens in first: markdown reads as itself.
pub(super) fn first_mode(path: &Path) -> Mode {
    let markdown = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
        });
    match markdown {
        true => Mode::Preview,
        false => Mode::Source,
    }
}

/// `text` as the tree the transcript draws.
pub(crate) fn render(text: &str) -> Markdown {
    let blocks = match Doc::read(text, OPTIONS).into_iter().next() {
        Some(Op::List { c, .. }) => Value::Array(c),
        _ => Value::Array(Vec::new()),
    };
    Markdown::new(&blocks, text.encode_utf16().count())
}

/// The bar over a markdown file's editor that switches it between its
/// source and how it reads.
pub(super) fn toolbar(
    this: &Entity<Workbench>,
    key: &super::FileKey,
    mode: Mode,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let choice = |id: &'static str, label: &'static str, shows: Mode| {
        let (this, key) = (this.clone(), key.clone());
        let chosen = mode == shows;
        div()
            .id(id)
            .px(px(8.))
            .h(px(20.))
            .flex()
            .items_center()
            .rounded(theme::RADIUS_2)
            .cursor_pointer()
            .text_color(if chosen {
                t.text_primary
            } else {
                t.text_tertiary
            })
            .when(chosen, |choice| choice.bg(t.surface_chosen))
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| {
                    super::show_mode(bench, &key, shows, window, cx)
                })
            })
            .child(label)
    };
    div()
        .flex()
        .justify_end()
        .items_center()
        .gap(px(2.))
        .h(px(28.))
        .px(px(8.))
        .border_b_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(12.))
        .child(choice("markdown-source", "Source", Mode::Source))
        .child(choice("markdown-preview", "Preview", Mode::Preview))
}

/// The markdown of the file at `path`, drawn to read.
#[derive(IntoElement)]
pub(super) struct Body {
    pub(super) path: PathBuf,
    pub(super) markdown: Rc<Markdown>,
}

impl RenderOnce for Body {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let folder = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let style = Style {
            on_link: Some(Rc::new(
                move |link: &str, window: &mut Window, cx: &mut App| {
                    follow(&folder, link, window, cx)
                },
            )),
            ..reading_style(15., &t)
        };
        let pen = Pen::new(Rc::new(style));
        let id = SharedString::from(format!("markdown-{}", self.path.display()));
        div().id(id.clone()).size_full().overflow_y_scroll().child(
            div()
                .mx_auto()
                .max_w(px(860.))
                .px(px(28.))
                .py(px(24.))
                .child(pen.markdown(id, &self.markdown, window, cx)),
        )
    }
}

/// How the app reads markdown at `text_size`: its type, inks and grounds.
pub(crate) fn reading_style(text_size: f32, t: &Tokens) -> Style {
    let mut mono = gpui_kit::font(theme::MONO);
    mono.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(vec![
        theme::MONO_ICONS.into()
    ]));
    Style {
        font: theme::UI.into(),
        mono,
        text_size: px(text_size),
        line_height: 1.72,
        ink: t.text_primary,
        ink_dim: t.text_tertiary,
        ink_faint: t.text_disabled,
        accent: t.accent,
        line: Hsla { a: 0.8, ..t.line },
        line_soft: t.line_soft,
        selection: Hsla { a: 0.3, ..t.accent },
        mark_ground: Hsla { a: 0.1, ..t.accent },
        table_ground: Hsla {
            a: 0.72,
            ..t.line_soft
        },
        code_ground: Hsla {
            a: 0.72,
            ..t.surface_sunken
        },
        ..Style::default()
    }
}

/// Opens a link from the preview: a web address in the browser, anything
/// else as a file beside the one being read.
fn follow(folder: &Path, link: &str, window: &mut Window, cx: &mut App) {
    if link.starts_with("http://") || link.starts_with("https://") || link.starts_with("mailto:") {
        cx.open_url(link);
        return;
    }
    if link.starts_with('#') {
        return;
    }
    let local = link.strip_prefix("file://").unwrap_or(link);
    let local = local.split('#').next().unwrap_or(local);
    let path = match Path::new(local).is_absolute() {
        true => PathBuf::from(local),
        false => folder.join(local),
    };
    let at = OpenAt {
        path,
        line: 0,
        column: None,
    };
    window.dispatch_action(Box::new(at), cx);
}
