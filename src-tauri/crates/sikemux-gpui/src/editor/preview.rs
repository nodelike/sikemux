//! A markdown, DBML or HTML file drawn the way it reads, beside its source. In markdown,
//! links open, a web address in the browser and a file in its editor.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, img, point, px, AnyElement, App, AppContext, Context, Entity, FocusHandle,
    Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, Pixels, RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, StyledImage, Subscription, Window,
};
use gpui_transcript::{DocFind, Markdown, Pen, Query, Style};
use serde_json::Value;
use sikemux_markdown::{Doc, Op, Options};
use tokio::sync::mpsc::UnboundedSender;

use super::{Document, FileKey, OpenAt};
use crate::core::Ask;
use crate::picture::{Picture, Remote};
use crate::theme::{self, Tokens, SPACE_0, SPACE_2, TEXT_XS};
use crate::ui::{self, FindBar};
use crate::workbench::Workbench;

actions!(markdown_preview, [FindInPreview]);

pub(super) const CONTEXT: &str = "MarkdownPreview";
/// The room above the preview's text, where a place found is brought to rest below.
const PAD_Y: f32 = 24.;
const TEXT_SIZE: f32 = 15.;
/// The widest the preview's text runs, as GitHub keeps a README's.
const WIDTH: f32 = 860.;
/// Frames spent bringing a place found into view: one from its block's top, one from its line.
const REVEAL_FRAMES: u8 = 2;

pub(super) fn install(cx: &mut App) {
    let find = if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    };
    cx.bind_keys([KeyBinding::new(find, FindInPreview, Some(CONTEXT))]);
}

const OPTIONS: Options = Options {
    gfm: true,
    html_as_text: false,
    file_links: true,
    html_images: true,
    html_layout: true,
    references: false,
    file_paths: false,
};

/// Whether a file's editor shows its text or the markdown it renders to.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Source,
    Preview,
}

/// How a file reads when it is not shown as its text.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Look {
    Markdown,
    /// A DBML schema, drawn as its tables.
    Diagram,
    /// An HTML file, drawn by WebKit as a browser would.
    Page,
}

pub(super) fn look(path: &Path) -> Option<Look> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "md" | "markdown" => Some(Look::Markdown),
        "dbml" => Some(Look::Diagram),
        "html" | "htm" => Some(Look::Page),
        _ => None,
    }
}

/// The editor a file opens in first: markdown and DBML read as themselves, and HTML, which
/// is mostly edited and often only draws once built, as its text.
pub(super) fn first_mode(path: &Path) -> Mode {
    match look(path) {
        Some(Look::Markdown | Look::Diagram) => Mode::Preview,
        Some(Look::Page) | None => Mode::Source,
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

/// The bar over a markdown, DBML or HTML file's editor that switches it between its
/// source and how it reads.
pub(super) fn toolbar(
    this: &Entity<Workbench>,
    key: &super::FileKey,
    look: Look,
    mode: Mode,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let choice = |id: &'static str, label: &'static str, shows: Mode| {
        let (this, key) = (this.clone(), key.clone());
        let chosen = mode == shows;
        div()
            .id(id)
            .px(SPACE_2)
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
        .gap(SPACE_0)
        .h(px(28.))
        .px(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(choice("markdown-source", "Source", Mode::Source))
        .child(choice(
            "markdown-preview",
            match look {
                Look::Markdown | Look::Page => "Preview",
                Look::Diagram => "Diagram",
            },
            Mode::Preview,
        ))
}

/// The markdown of the file at `path`, drawn to read, with the places a find found marked.
#[derive(IntoElement)]
pub(super) struct Body {
    pub(super) path: PathBuf,
    pub(super) markdown: Rc<Markdown>,
    pub(super) found: Option<Rc<DocFind>>,
    pub(super) scroll: ScrollHandle,
    pub(super) focus: FocusHandle,
    /// How pictures on the web are fetched; none while the file is on another host, whose
    /// pictures beside it this one cannot read.
    pub(super) asks: Option<UnboundedSender<Ask>>,
}

impl RenderOnce for Body {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let folder = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let quiet = Tokens::get(cx).text_tertiary;
        let (pictures, asks) = (folder.clone(), self.asks);
        let style = Rc::new(Style {
            on_link: Some(Rc::new(
                move |link: &str, window: &mut Window, cx: &mut App| {
                    follow(&folder, link, window, cx)
                },
            )),
            image: Some(Rc::new(
                move |src: &str, alt: &str, _: &mut Window, _: &mut App| {
                    picture(src, alt, &pictures, asks.as_ref(), quiet)
                },
            )),
            gap: px(14.),
            ruled_headings: true,
            ..reading_style(TEXT_SIZE, cx)
        });
        let pen = match &self.found {
            Some(found) => found.pen(style, &self.markdown),
            None => Pen::new(style),
        };
        let id = SharedString::from(format!("markdown-{}", self.path.display()));
        let focus = self.focus.clone();
        div()
            .id(id.clone())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx)
            })
            .child(
                div()
                    .mx_auto()
                    .max_w(px(WIDTH))
                    .px(px(28.))
                    .py(px(PAD_Y))
                    .child(pen.markdown(id, &self.markdown, window, cx)),
            )
    }
}

/// The find bar over a markdown preview, while it is open.
pub(super) struct Finding {
    input: Entity<InputState>,
    match_case: bool,
    whole_word: bool,
    /// The places found, once the bar holds something to find.
    found: Option<Rc<DocFind>>,
    /// Frames left to bring the place the reader is on into view.
    reveal: u8,
    _typed: Subscription,
}

impl Finding {
    pub(super) fn found(&self) -> Option<Rc<DocFind>> {
        self.found.clone()
    }

    pub(super) fn revealing(&self) -> bool {
        self.reveal > 0
    }
}

/// Opens the find bar over the preview of `key`, or puts the reader back in it.
pub(super) fn open_find(
    bench: &mut Workbench,
    key: &FileKey,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(finding) = (bench.editors.read(cx).open.get(key)).map(|document| {
        document
            .finding
            .as_ref()
            .map(|finding| finding.input.clone())
    }) else {
        return;
    };
    let input = finding.unwrap_or_else(|| {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in file"));
        let typed = {
            let key = key.clone();
            cx.subscribe(&input, move |bench, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    search(bench, &key, cx);
                }
            })
        };
        let finding = Finding {
            input: input.clone(),
            match_case: false,
            whole_word: false,
            found: None,
            reveal: 0,
            _typed: typed,
        };
        super::change(&bench.editors, key, cx, |document, _| {
            document.finding = Some(finding)
        });
        input
    });
    input.update(cx, |input, cx| input.select_all(window, cx));
    input.read(cx).focus_handle(cx).focus(window, cx);
}

/// Looks for what the bar holds, as its options say, in the preview as it reads now.
fn search(bench: &mut Workbench, key: &FileKey, cx: &mut Context<Workbench>) {
    super::change(&bench.editors, key, cx, |document, cx| refind(document, cx));
}

/// Finds again what the bar holds, as the preview's text changed or the bar did.
pub(super) fn refind(document: &mut Document, cx: &App) {
    let rendered = document.markdown();
    let Some(finding) = document.finding.as_mut() else {
        return;
    };
    let query = Query {
        text: finding.input.read(cx).value().to_string(),
        match_case: finding.match_case,
        whole_word: finding.whole_word,
    };
    finding.found = match (query.text.is_empty(), rendered) {
        (false, Some(markdown)) => Some(Rc::new(DocFind::new(&markdown, &query))),
        _ => None,
    };
    finding.reveal = REVEAL_FRAMES;
}

fn step_find(bench: &mut Workbench, key: &FileKey, forward: bool, cx: &mut Context<Workbench>) {
    super::change(&bench.editors, key, cx, |document, _| {
        let Some(finding) = document.finding.as_mut() else {
            return;
        };
        if let Some(found) = finding.found.as_mut() {
            Rc::make_mut(found).step(forward);
            finding.reveal = REVEAL_FRAMES;
        }
    });
}

fn flip_find(
    bench: &mut Workbench,
    key: &FileKey,
    flip: fn(&mut Finding),
    cx: &mut Context<Workbench>,
) {
    let flipped = bench.editors.update(cx, |editors, _| {
        let finding = (editors.open.get_mut(key)).and_then(|document| document.finding.as_mut());
        finding.map(flip).is_some()
    });
    if flipped {
        search(bench, key, cx);
    }
}

/// Closes the bar, takes the marks away, and puts the reader back in the preview.
fn close_find(
    bench: &mut Workbench,
    key: &FileKey,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    super::change(&bench.editors, key, cx, |document, cx| {
        document.finding = None;
        document.preview_focus.focus(window, cx);
    });
}

/// Scrolls the preview so the place the reader is on sits in view, aimed from where the last
/// frame drew it.
pub(super) fn reveal(bench: &mut Workbench, key: &FileKey, cx: &mut Context<Workbench>) {
    bench.editors.update(cx, |editors, cx| {
        if let Some(document) = editors.open.get_mut(key) {
            reveal_in(document, cx);
        }
    });
}

fn reveal_in(document: &mut Document, cx: &mut Context<super::Editors>) {
    let (Some(markdown), Some(finding)) = (document.markdown(), document.finding.as_mut()) else {
        return;
    };
    finding.reveal = finding.reveal.saturating_sub(1);
    let style = reading_style(TEXT_SIZE, cx);
    let offset = (finding.found.as_ref()).and_then(|found| found.current_offset(&markdown, &style));
    if let Some(offset) = offset {
        let scroll = &document.preview_scroll;
        let (line, room) = (px(PAD_Y) + offset, room(scroll));
        let scrolled = -scroll.offset().y;
        let view = scroll.bounds().size.height;
        if line < scrolled + room || line > scrolled + view - room {
            scroll.set_offset(point(px(0.), -(line - room).max(px(0.))));
        }
    }
    if finding.reveal > 0 {
        cx.notify();
    }
}

/// How far from the edge of the view a place found is kept.
fn room(scroll: &ScrollHandle) -> Pixels {
    (scroll.bounds().size.height / 4.).min(px(96.))
}

/// The find bar over the preview of `key`, while it is open.
pub(super) fn bar(this: &Entity<Workbench>, key: &FileKey, finding: &Finding, cx: &App) -> FindBar {
    let label = match finding.found.as_ref().map(|found| found.found()) {
        Some((current, total)) if total > 0 => format!("{}/{total}", current + 1),
        _ if finding.input.read(cx).value().is_empty() => "0/0".to_owned(),
        _ => "No results".to_owned(),
    };
    let on = |act: fn(&mut Workbench, &FileKey, &mut Window, &mut Context<Workbench>)| {
        let (this, key) = (this.clone(), key.clone());
        move |window: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| act(bench, &key, window, cx))
        }
    };
    let step = {
        let (this, key) = (this.clone(), key.clone());
        move |forward: bool, _: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| step_find(bench, &key, forward, cx))
        }
    };
    FindBar::new(&finding.input, label)
        .match_case(
            finding.match_case,
            on(|bench, key, _, cx| flip_find(bench, key, |finding| finding.match_case ^= true, cx)),
        )
        .whole_word(
            finding.whole_word,
            on(|bench, key, _, cx| flip_find(bench, key, |finding| finding.whole_word ^= true, cx)),
        )
        .on_step(step)
        .on_close(on(close_find))
}

/// [`reading_style`] whose web links open what they name in the app, as
/// [`crate::links::web_opener`] does.
pub(crate) fn reading_linked(
    text_size: f32,
    bench: &gpui_kit::WeakEntity<crate::workbench::Workbench>,
    cx: &App,
) -> Style {
    Style {
        on_link: Some(crate::links::web_opener(bench.clone())),
        ..reading_style(text_size, cx)
    }
}

/// How the app reads markdown at `text_size` points at the default text size, growing and
/// shrinking with the person's choice as the rest of the interface's text does.
pub(crate) fn reading_style(text_size: f32, cx: &App) -> Style {
    let t = Tokens::get(cx);
    let mut mono = gpui_kit::font(theme::MONO);
    mono.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(vec![
        theme::MONO_ICONS.into()
    ]));
    Style {
        font: theme::UI.into(),
        mono,
        text_size: px(text_size * theme::text_scale(cx)),
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

/// A picture the markdown shows at its own size, no wider than the text: a file beside the
/// one being read, or one on the web, which the core fetches.
fn picture(
    src: &str,
    alt: &str,
    folder: &Path,
    asks: Option<&UnboundedSender<Ask>>,
    quiet: Hsla,
) -> AnyElement {
    let picture = if src.starts_with("http://") || src.starts_with("https://") {
        asks.map(|asks| Picture::Remote(Remote::new(src.to_owned(), asks.clone())))
    } else {
        let local = src.strip_prefix("file://").unwrap_or(src);
        let local = local.split(['#', '?']).next().unwrap_or(local);
        asks.map(|_| Picture::File(folder.join(local)))
    };
    let alt = SharedString::from(alt.to_owned());
    let unloaded = move || {
        ui::text(alt.clone(), TEXT_XS, FontWeight::NORMAL)
            .text_color(quiet)
            .into_any_element()
    };
    match picture {
        Some(picture) => img(picture.source_at(px(WIDTH)))
            .max_w_full()
            .with_fallback(unloaded)
            .into_any_element(),
        None => unloaded(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str, match_case: bool, whole_word: bool) -> Query {
        Query {
            text: text.to_owned(),
            match_case,
            whole_word,
        }
    }

    const TEXT: &str = "# Notes\n\nFoo the food, then foo again.\n\n- one foo\n- `foo()` in code\n";

    #[test]
    fn find_counts_every_place_in_the_preview_as_it_reads() {
        let markdown = render(TEXT);
        let count = |query: Query| DocFind::new(&markdown, &query).found().1;
        assert_eq!(count(query("foo", false, false)), 5);
        assert_eq!(count(query("Foo", true, false)), 1);
        assert_eq!(count(query("foo", false, true)), 4);
        assert_eq!(count(query("absent", false, false)), 0);
        assert_eq!(count(query("#", false, false)), 0);
    }

    #[test]
    fn stepping_goes_round_the_places_found() {
        let markdown = render(TEXT);
        let mut found = DocFind::new(&markdown, &query("foo", false, true));
        found.step(false);
        assert_eq!(found.found(), (3, 4));
        found.step(true);
        assert_eq!(found.found(), (0, 4));
        found.step(true);
        assert_eq!(found.found(), (1, 4));
    }
}
