//! A markdown, DBML or HTML file drawn the way it reads, beside its source. In markdown,
//! links open, a web address in the browser and a file in its editor.

use crate::reading::reading_style;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, img, point, px, AnyElement, App, AppContext, ClipboardItem, Context, Entity,
    FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, KeyBinding,
    MouseButton, ParentElement, Pixels, RenderOnce, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Subscription, Window,
};
use gpui_transcript::{Copy, DocFind, DocSelect, Markdown, Query, Style};
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
    let (find, copy) = if cfg!(target_os = "macos") {
        ("cmd-f", "cmd-c")
    } else {
        ("ctrl-f", "ctrl-c")
    };
    cx.bind_keys([
        KeyBinding::new(find, FindInPreview, Some(CONTEXT)),
        KeyBinding::new(copy, Copy, Some(CONTEXT)),
    ]);
}

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
        .child(choice("markdown-source", t!("Source"), Mode::Source))
        .child(choice(
            "markdown-preview",
            match look {
                Look::Markdown | Look::Page => t!("Preview"),
                Look::Diagram => t!("Diagram"),
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
    pub(super) select: DocSelect,
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
        let pen = (self.select).pen(style, &self.markdown, self.found.as_deref());
        let id = SharedString::from(format!("markdown-{}", self.path.display()));
        let focus = self.focus.clone();
        let (pressed, dragged, released, copied) = (
            self.select.clone(),
            self.select.clone(),
            self.select.clone(),
            self.select,
        );
        let (markdown, copying) = (self.markdown.clone(), self.markdown.clone());
        div()
            .id(id.clone())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                focus.focus(window, cx);
                pressed.press(event, &markdown);
                window.refresh();
            })
            .on_mouse_move(move |event, window, _| {
                if dragged.drag(event) {
                    window.refresh();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, _| released.release())
            .on_action(move |_: &Copy, _, cx| {
                if let Some(text) = copied.selected_text(&copying) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Find in file")));
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
    let offset = (finding.found.as_ref())
        .and_then(|found| found.current_offset(&markdown, &style, &document.preview_select));
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
        _ => t!("No results").to_owned(),
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
