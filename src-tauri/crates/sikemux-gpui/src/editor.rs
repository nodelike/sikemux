//! The editor's tab: a file read and saved through the core, edited on Kit's
//! code editor, and opened from anywhere at a line.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, Position, Replace, Search,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, AnyElement, AnyWindowHandle, App, AppContext, Context, Entity, FocusHandle,
    Focusable, FontWeight, InteractiveElement, IntoElement, KeyBinding, ParentElement,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, WeakEntity,
    Window,
};
use serde_json::{json, Value};
use sikemux_wire::protocol::{Pane, PaneKind, DEFAULT_TEXT_SCALE, MAX_TEXT_SCALE, MIN_TEXT_SCALE};

use crate::core::{Ask, Target};
use crate::hosts::Place as Host;
use crate::keymap::{TextSizeDecrease, TextSizeIncrease, TextSizeReset};
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_XS};
use crate::ui;
use crate::workbench::{Strip, TakesKeys, Workbench};

mod blame;
mod copies;
mod dbml;
pub(crate) mod diagram;
mod gutter;
mod insights;
mod lsp;

pub(crate) use lsp::{allow as allow_language_servers, allowed as language_servers_allowed};
pub(crate) mod preview;
mod viewer;

/// Opens a file in the editor with the cursor at a line, counted from 0, and
/// a column, or the line's first character that is not blank when there is
/// none. Every diff's "open in editor" sends this.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(name = "editor.openAt", no_json)]
pub struct OpenAt {
    pub path: PathBuf,
    pub line: u32,
    pub column: Option<u32>,
}

actions!(
    editor,
    [
        Save,
        GoToLine,
        CloseGoToLine,
        NavigateBack,
        NavigateForward,
        ClosePeek,
        AddNextMatch
    ]
);

const CONTEXT: &str = "FileEditor";
/// How many times a read starts again when the file moves under it.
const READ_TRIES: usize = 3;
/// A save goes in pieces of this many characters, as the core takes them.
const SAVE_PIECE: usize = 512 * 1024;
const FONT_SIZE: f32 = 13.;
const TEXT_SCALE_STEP: f64 = 0.1;
/// How many places back ⌘[ remembers.
const MAX_HISTORY: usize = 128;
/// The most places a peek lists.
const MAX_PEEKED: usize = 300;

pub fn install(cx: &mut App) {
    preview::install(cx);
    crate::ui::find_bar::install(cx);
    let save = if cfg!(target_os = "macos") {
        "cmd-s"
    } else {
        "ctrl-s"
    };
    let mac = cfg!(target_os = "macos");
    let keys = |mac_keys: &'static str, other: &'static str| if mac { mac_keys } else { other };
    let text = Some("FileEditor > Input");
    cx.bind_keys([
        KeyBinding::new(save, Save, Some(CONTEXT)),
        KeyBinding::new(keys("cmd-alt-g", "ctrl-alt-g"), GoToLine, Some(CONTEXT)),
        KeyBinding::new("escape", CloseGoToLine, Some("GoToLine > Input")),
        KeyBinding::new("escape", ClosePeek, Some("peek > Input")),
        KeyBinding::new("escape", ClosePeek, Some("peek")),
        KeyBinding::new(keys("cmd-h", "ctrl-h"), Replace, text),
        KeyBinding::new(keys("cmd-[", "ctrl-["), NavigateBack, text),
        KeyBinding::new(keys("cmd-]", "ctrl-]"), NavigateForward, text),
        KeyBinding::new(keys("cmd-d", "ctrl-d"), AddNextMatch, text),
        // ⇧⌘F searches the project from the editor too, rather than Kit's replace.
        KeyBinding::new(
            keys("cmd-shift-f", "ctrl-shift-f"),
            crate::search::FocusSearch,
            text,
        ),
    ]);
}

/// A file by the host it is on and its path there, since two hosts can hold the same path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub place: Host,
    pub path: PathBuf,
}

impl FileKey {
    pub fn new(place: Host, path: impl Into<PathBuf>) -> Self {
        Self {
            place,
            path: path.into(),
        }
    }
}

impl Editors {
    /// Puts the cursor at `at` once its file is read.
    pub fn reveal(&mut self, at: OpenAt) {
        self.reveal = Some(at);
    }
}

/// Changes the open file `key`, when it is open, and draws the editors again.
fn change<R>(
    editors: &Entity<Editors>,
    key: &FileKey,
    cx: &mut App,
    change: impl FnOnce(&mut Document, &mut Context<Editors>) -> R,
) -> Option<R> {
    editors.update(cx, |editors, cx| {
        let document = editors.open.get_mut(key)?;
        let changed = change(document, cx);
        cx.notify();
        Some(changed)
    })
}

/// Every file open in an editor, by its host and path.
#[derive(Default)]
pub struct Editors {
    open: HashMap<FileKey, Document>,
    /// Where a file asked to open at a line puts its cursor once it is read.
    reveal: Option<OpenAt>,
    /// The window the editors draw in, which reading and reloading update.
    window: Option<AnyWindowHandle>,
    /// Where the editor was before each jump, and where ⌘[ came back from.
    back: Vec<OpenAt>,
    forward: Vec<OpenAt>,
    insights: insights::Insights,
    /// Files that are not text, shown for what they are.
    viewers: HashMap<FileKey, viewer::Viewer>,
    /// What language servers failed with that the person was told, so each is said once.
    lsp_said: HashSet<String>,
    /// Languages whose server the core is installing.
    lsp_installing: HashSet<String>,
    copies: copies::Copies,
}

struct Document {
    state: Entity<EditorState>,
    /// The host the file is read from and saved to.
    target: Target,
    /// What the file held when it was last read or saved, and that text's version.
    saved: String,
    version: Option<String>,
    loaded: bool,
    dirty: bool,
    /// Why the file could not be read or saved, or that it changed on disk
    /// while it had unsaved edits.
    notice: Option<Notice>,
    /// The field asking which line to go to, while it is open.
    goto: Option<(Entity<InputState>, Subscription)>,
    /// The language server the file is open with, once it is.
    served: Option<lsp::Served>,
    /// The places a symbol leads to when there is more than one.
    peek: Option<Peek>,
    /// The selection ⌘D started from, and where the match it last added ends.
    next_match: Option<(std::ops::Range<usize>, usize)>,
    /// Whether it shows its text or, for markdown, DBML and HTML, how it reads, and that
    /// rendering.
    mode: preview::Mode,
    rendered: Option<Rendered>,
    /// The find bar over the markdown as it reads, while it is open.
    finding: Option<preview::Finding>,
    preview_focus: FocusHandle,
    preview_scroll: ScrollHandle,
    /// What changed since the last commit, marked beside the lines.
    gutter: gutter::Gutter,
    /// Who last changed each line.
    blame: blame::Blame,
    _changes: Subscription,
}

enum Rendered {
    Markdown(Rc<gpui_transcript::Markdown>),
    Diagram(Entity<diagram::Diagram>),
    /// None once WebKit would not make the page's view.
    Page(Option<crate::page_view::View>),
}

impl Document {
    fn markdown(&self) -> Option<Rc<gpui_transcript::Markdown>> {
        match &self.rendered {
            Some(Rendered::Markdown(markdown)) => Some(markdown.clone()),
            _ => None,
        }
    }
}

#[derive(Clone)]
enum Notice {
    Failed(String),
    /// The disk moved on under unsaved edits; reloading drops them.
    Conflict(String),
    /// Whether the language servers of the project, by its folder, may run.
    Trust(String),
}

impl Editors {
    /// Each open file with its cursor's line, counted from one, and whether it has unsaved edits.
    #[cfg(feature = "drive")]
    pub fn open_files(&self, cx: &App) -> Vec<(PathBuf, u32, bool)> {
        (self.open.iter())
            .map(|(key, document)| {
                let line = document.state.read(cx).cursor_position().line + 1;
                (key.path.clone(), line, document.dirty)
            })
            .collect()
    }

    /// Whether the keys are in the editor showing `path`.
    pub fn has_keys(&self, path: &Path, window: &Window, cx: &App) -> bool {
        (self.open.iter()).any(|(key, document)| {
            key.path == path
                && (document.state.read(cx).focus_handle(cx).is_focused(window)
                    || document.preview_focus.contains_focused(window, cx))
        })
    }

    /// Whether the file has edits not yet saved.
    pub fn is_dirty(&self, key: &FileKey) -> bool {
        self.open.get(key).is_some_and(|document| document.dirty)
    }

    /// The files under `root` on `place` with changes not saved.
    pub fn unsaved_under<'a>(
        &'a self,
        place: &'a Host,
        root: &'a Path,
    ) -> impl Iterator<Item = &'a Path> + 'a {
        (self.open.iter())
            .filter(move |(key, document)| {
                document.dirty && key.place == *place && key.path.starts_with(root)
            })
            .map(|(key, _)| key.path.as_path())
    }

    /// The files on `place` its core keeps watching for changes on disk.
    fn watched(&self, place: &Host) -> Vec<String> {
        (self.open.keys())
            .chain(self.viewers.keys())
            .filter(|key| key.place == *place)
            .map(|key| key.path.to_string_lossy().into_owned())
            .collect()
    }
}

/// The editor for `path`, opened the first time it is drawn.
pub fn pane(
    bench: &Workbench,
    this: &Entity<Workbench>,
    path: &str,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let key = FileKey::new(bench.place(), path);
    let path = key.path.clone();
    let t = Tokens::get(cx).clone();
    if bench.editors.read(cx).viewers.contains_key(&key) {
        return viewer::pane(bench, this, &key, cx);
    }
    if viewer::not_text(&path) {
        let opening = key.clone();
        cx.defer_in(window, move |bench, window, cx| {
            let handle = window.window_handle();
            bench
                .editors
                .update(cx, |editors, _| editors.window = Some(handle));
            viewer::load(bench, opening, cx)
        });
        return div().size_full().into_any_element();
    }
    let Some(loaded) = (bench.editors.read(cx).open.get(&key)).map(|document| document.loaded)
    else {
        let opening = key.clone();
        cx.defer_in(window, move |bench, window, cx| {
            open(bench, opening, window, cx)
        });
        return div().size_full().into_any_element();
    };
    if bench.claims_keys(TakesKeys::File(path.clone())) {
        let focused = key.clone();
        cx.defer_in(window, move |bench, window, cx| {
            let Some(document) = bench.editors.read(cx).open.get(&focused) else {
                return;
            };
            let look = preview::look(&focused.path);
            match document.mode {
                preview::Mode::Preview
                    if matches!(look, Some(preview::Look::Markdown | preview::Look::Page)) =>
                {
                    document.preview_focus.clone().focus(window, cx)
                }
                _ => (document.state.clone()).update(cx, |state, cx| state.focus(window, cx)),
            }
        });
    }
    if loaded {
        let reveal = bench.editors.read(cx).reveal.clone();
        if let Some(at) = reveal.filter(|at| at.path == path) {
            let revealed = key.clone();
            cx.defer_in(window, move |bench, window, cx| {
                bench.editors.update(cx, |editors, _| editors.reveal = None);
                place_cursor(bench, &revealed, &at, window, cx);
            });
        }
    }
    let size = px((FONT_SIZE as f64 * text_scale(bench)) as f32);
    let project = lsp::project_of(bench, &path);
    let this = this.clone();
    let saving = key.clone();
    let Some(document) = bench.editors.read(cx).open.get(&key) else {
        return div().size_full().into_any_element();
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .key_context(match document.peek {
            Some(_) => "FileEditor peek",
            None => CONTEXT,
        })
        .capture_any_mouse_down({
            let key = key.clone();
            window.listener_for(
                &this,
                move |bench, event: &gpui_kit::MouseDownEvent, _, cx| {
                    let held = event.modifiers;
                    let lsp_click = event.button == gpui_kit::MouseButton::Left
                        && held.secondary(cx)
                        && (held.shift || held.alt);
                    if lsp_click && lsp::navigate(bench, &key, held.shift, cx) {
                        cx.stop_propagation();
                    }
                },
            )
        })
        .on_action(
            window.listener_for(&this, move |bench, _: &Save, window, cx| {
                save(bench, &saving, window, cx)
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &NavigateBack, _, cx| {
                step(bench, true, cx)
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &NavigateForward, _, cx| {
                step(bench, false, cx)
            }),
        )
        .on_action(window.listener_for(&this, |bench, _: &ClosePeek, _, cx| close_peeks(bench, cx)))
        .on_action({
            let key = key.clone();
            window.listener_for(&this, move |bench, _: &AddNextMatch, _, cx| {
                add_next_match(bench, &key, cx)
            })
        })
        .on_action({
            let key = key.clone();
            window.listener_for(&this, move |bench, _: &GoToLine, window, cx| {
                open_goto(bench, &key, window, cx)
            })
        })
        .on_action({
            let key = key.clone();
            window.listener_for(&this, move |bench, _: &CloseGoToLine, window, cx| {
                close_goto(bench, &key, window, cx)
            })
        })
        .on_pinch(
            window.listener_for(&this, |bench, event: &gpui::PinchEvent, _, _| {
                crate::pinch::resize(bench, crate::pinch::Surface::Editor, event)
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &TextSizeIncrease, _, _| {
                rescale(bench, Some(TEXT_SCALE_STEP))
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &TextSizeDecrease, _, _| {
                rescale(bench, Some(-TEXT_SCALE_STEP))
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &TextSizeReset, _, _| rescale(bench, None)),
        )
        .when_some(document.notice.clone(), |pane, notice| {
            pane.child(notice_bar(&this, &key, notice, cx))
        })
        .when_some(document.goto.as_ref(), |pane, (input, _)| {
            pane.child(
                div()
                    .key_context("GoToLine")
                    .flex()
                    .items_center()
                    .gap(SPACE_2H)
                    .px(SPACE_3)
                    .py(SPACE_1H)
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .child(div().text_color(t.text_tertiary).child("Go to line"))
                    .child(
                        Input::new(input)
                            .appearance(false)
                            .flex_1()
                            .p_0()
                            .text_color(t.text_primary),
                    ),
            )
        })
        .when_some(preview::look(&path), |pane, look| {
            pane.child(preview::toolbar(&this, &key, look, document.mode, cx))
        })
        .map(|pane| match (document.mode, document.rendered.as_ref()) {
            (preview::Mode::Preview, Some(Rendered::Markdown(markdown))) => {
                let finding = document.finding.as_ref();
                if finding.is_some_and(preview::Finding::revealing) {
                    let (this, key) = (this.clone(), key.clone());
                    window.on_next_frame(move |_, cx| {
                        this.update(cx, |bench, cx| preview::reveal(bench, &key, cx))
                    });
                }
                let find = {
                    let key = key.clone();
                    window.listener_for(
                        &this,
                        move |bench, _: &preview::FindInPreview, window, cx| {
                            preview::open_find(bench, &key, window, cx)
                        },
                    )
                };
                pane.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .key_context(preview::CONTEXT)
                        .on_action(find)
                        .child(preview::Body {
                            path: path.clone(),
                            markdown: markdown.clone(),
                            found: finding.and_then(preview::Finding::found),
                            scroll: document.preview_scroll.clone(),
                            focus: document.preview_focus.clone(),
                            asks: (bench.place() == crate::hosts::Place::This)
                                .then(|| bench.asks.clone()),
                        })
                        .children(finding.map(|finding| preview::bar(&this, &key, finding, cx))),
                )
            }
            (preview::Mode::Preview, Some(Rendered::Page(Some(page)))) => pane.child(
                gpui_native_view::native_view(
                    SharedString::from(format!("html-preview-{}", path.display())),
                    &page.surface,
                )
                .track_focus(&document.preview_focus)
                .flex_1()
                .min_h_0(),
            ),
            (preview::Mode::Preview, Some(Rendered::Diagram(diagram))) => pane.child(
                div().flex_1().min_h_0().child(
                    gpui_kit::AnyView::from(diagram.clone())
                        .cached(gpui_kit::StyleRefinement::default().size_full()),
                ),
            ),
            _ => pane.child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .capture_action(window.listener_for(&this, |bench, _: &Search, _, cx| {
                        bench.parts.redraw_stage(cx)
                    }))
                    .capture_action(window.listener_for(&this, |bench, _: &Replace, _, cx| {
                        bench.parts.redraw_stage(cx)
                    }))
                    .font_family(theme::MONO)
                    .text_size(size)
                    .text_color(t.text_primary)
                    .child(
                        Editor::new(&document.state)
                            .appearance(false)
                            .text_size(size)
                            .h_full(),
                    )
                    .child(gutter::overlay(
                        &this,
                        &key,
                        &document.state,
                        &document.gutter,
                        gpui_kit::component::input::RopeExt::lines_len(
                            document.state.read(cx).text(),
                        ),
                        &t,
                    ))
                    .child(blame::overlay(
                        &document.state,
                        &document.blame,
                        size,
                        t.text_disabled,
                    )),
            ),
        })
        .when_some(document.peek.as_ref(), |pane, peek| {
            pane.child(peek_panel(&this, peek, cx))
        })
        .child(insights::bar(
            bench,
            &this,
            &path,
            project.as_deref().map(Path::new),
            cx,
        ))
        .into_any_element()
}

/// The places a symbol leads to, by file, with each line's text once read.
struct Peek {
    title: SharedString,
    places: Vec<Spot>,
}

struct Spot {
    at: OpenAt,
    text: Option<SharedString>,
}

/// Opens `to`, remembering where the editor in front was so ⌘[ comes back.
pub fn go(bench: &mut Workbench, to: OpenAt, cx: &mut Context<Workbench>) {
    remember_here(bench, cx);
    show(bench, to, cx);
}

/// Opens `to`, a file of no project, in a tab of `strip` called `title`.
pub fn go_in(
    bench: &mut Workbench,
    to: OpenAt,
    strip: &Strip,
    title: &str,
    cx: &mut Context<Workbench>,
) {
    remember_here(bench, cx);
    let target = to.path.to_string_lossy().into_owned();
    bench.call(
        "openTab",
        strip.with(json!({ "kind": "files", "target": target, "title": title })),
    );
    bench.editors.update(cx, |editors, cx| {
        editors.reveal = Some(to);
        cx.notify();
    });
}

fn remember_here(bench: &mut Workbench, cx: &mut App) {
    if let Some(here) = here(bench, cx) {
        bench.editors.update(cx, |editors, _| {
            editors.back.push(here);
            if editors.back.len() > MAX_HISTORY {
                editors.back.remove(0);
            }
            editors.forward.clear();
        });
    }
}

fn show(bench: &mut Workbench, at: OpenAt, cx: &mut Context<Workbench>) {
    let path = at.path.clone();
    bench
        .editors
        .update(cx, |editors, _| editors.reveal = Some(at));
    bench.open_file(&path, cx);
}

/// Where the cursor is in the editor in front.
fn here(bench: &Workbench, cx: &App) -> Option<OpenAt> {
    let path = bench.front_file()?;
    let position = bench
        .editors
        .read(cx)
        .open
        .get(&FileKey::new(bench.place(), path.clone()))?
        .state
        .read(cx)
        .cursor_position();
    Some(OpenAt {
        path,
        line: position.line,
        column: Some(position.character),
    })
}

/// Goes back to where the editor was before its last jump, or forward again.
fn step(bench: &mut Workbench, back: bool, cx: &mut Context<Workbench>) {
    let here = here(bench, cx);
    let taken = bench.editors.update(cx, |editors, _| {
        let (from, to) = match back {
            true => (&mut editors.back, &mut editors.forward),
            false => (&mut editors.forward, &mut editors.back),
        };
        let taken = from.pop()?;
        to.extend(here);
        Some(taken)
    });
    if let Some(to) = taken {
        show(bench, to, cx);
    }
}

/// ⌘D, as on the Mac: with nothing selected it selects the word at the cursor, and with text
/// selected it adds the next place that text appears, coming round from the top, until every
/// place is selected.
fn add_next_match(bench: &mut Workbench, key: &FileKey, cx: &mut Context<Workbench>) {
    bench.editors.update(cx, |editors, cx| {
        if let Some(document) = editors.open.get_mut(key) {
            add_next_match_in(document, cx);
        }
    });
}

fn add_next_match_in(document: &mut Document, cx: &mut App) {
    let state = document.state.clone();
    let text = state.read(cx).value().to_string();
    let selected = state.read(cx).selected_range();
    if selected.is_empty() {
        document.next_match = None;
        let word = |letter: char| letter.is_alphanumeric() || letter == '_';
        let start = text[..selected.start]
            .char_indices()
            .rev()
            .take_while(|(_, letter)| word(*letter))
            .last()
            .map_or(selected.start, |(at, _)| at);
        let end = text[selected.start..]
            .char_indices()
            .find(|(_, letter)| !word(*letter))
            .map_or(text.len(), |(at, _)| selected.start + at);
        if start < end {
            state.update(cx, |state, cx| state.set_selected_range(start..end, cx));
        }
        return;
    }
    let after = match &document.next_match {
        Some((origin, after)) if *origin == selected => *after,
        _ => selected.end,
    };
    let needle = &text[selected.clone()];
    let found = text
        .get(after..)
        .and_then(|rest| rest.find(needle))
        .map(|at| after + at)
        .or_else(|| text.find(needle));
    let Some(at) = found.filter(|at| *at != selected.start) else {
        return;
    };
    let added = at..at + needle.len();
    document.next_match = Some((selected, added.end));
    state.update(cx, |state, cx| state.add_selection(added, cx));
}

/// Lists the places a symbol leads to under the editor of `path`, and reads
/// each one's line.
fn peek(
    bench: &mut Workbench,
    key: &FileKey,
    title: &str,
    places: Vec<OpenAt>,
    cx: &mut Context<Workbench>,
) {
    let places: Vec<Spot> = places
        .into_iter()
        .take(MAX_PEEKED)
        .map(|at| Spot { at, text: None })
        .collect();
    let mut files: Vec<PathBuf> = places.iter().map(|place| place.at.path.clone()).collect();
    files.dedup();
    let peek = Peek {
        title: title.to_owned().into(),
        places,
    };
    let Some(target) = change(&bench.editors, key, cx, |document, _| {
        document.peek = Some(peek);
        document.target.clone()
    }) else {
        return;
    };
    let (editors, peeked) = (bench.editors.downgrade(), key.clone());
    cx.spawn(async move |_, cx| {
        for file in files {
            let Ok((text, _)) = read_file(&target, file.to_string_lossy().into_owned()).await
            else {
                continue;
            };
            let lines: Vec<&str> = text.lines().collect();
            let _ = editors.update(cx, |editors, cx| {
                let Some(peek) =
                    (editors.open.get_mut(&peeked)).and_then(|document| document.peek.as_mut())
                else {
                    return;
                };
                for place in peek.places.iter_mut().filter(|place| place.at.path == file) {
                    let line = lines
                        .get(place.at.line as usize)
                        .copied()
                        .unwrap_or_default();
                    place.text = Some(line.trim().to_owned().into());
                }
                cx.notify();
            });
        }
    })
    .detach();
}

fn close_peeks(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.editors.update(cx, |editors, cx| {
        for document in editors.open.values_mut() {
            document.peek = None;
        }
        cx.notify();
    });
}

/// The panel under an editor listing the places a symbol leads to, by file.
fn peek_panel(this: &Entity<Workbench>, peek: &Peek, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut file: Option<&Path> = None;
    for (index, place) in peek.places.iter().enumerate() {
        if file != Some(place.at.path.as_path()) {
            file = Some(place.at.path.as_path());
            let name = place
                .at
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            let folder = place
                .at
                .path
                .parent()
                .map(|folder| folder.to_string_lossy().into_owned());
            rows.push(
                div()
                    .flex()
                    .gap(SPACE_2)
                    .px(SPACE_3)
                    .pt(SPACE_1H)
                    .child(div().text_color(t.text_primary).children(name))
                    .child(div().text_color(t.text_tertiary).children(folder))
                    .into_any_element(),
            );
        }
        let (this, at) = (this.clone(), place.at.clone());
        rows.push(
            div()
                .id(("peek", index))
                .flex()
                .gap(SPACE_3)
                .h(px(24.))
                .items_center()
                .px(SPACE_3)
                .cursor_pointer()
                .hover(move |style| style.bg(t.surface_hover))
                .on_click(move |_, _, cx| {
                    let at = at.clone();
                    this.update(cx, |bench, cx| {
                        close_peeks(bench, cx);
                        go(bench, at, cx);
                    })
                })
                .child(
                    div()
                        .w(px(40.))
                        .text_right()
                        .text_color(t.text_tertiary)
                        .child((place.at.line + 1).to_string()),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(t.text_secondary)
                        .children(place.text.clone()),
                )
                .into_any_element(),
        );
    }
    div()
        .flex()
        .flex_col()
        .max_h(px(320.))
        .border_t_1()
        .border_color(t.border)
        .font_family(theme::MONO)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(
            div()
                .flex()
                .items_center()
                .h(px(30.))
                .px(SPACE_3)
                .gap(SPACE_2)
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child(peek.title.to_uppercase()),
                )
                .child(
                    div()
                        .text_color(t.text_tertiary)
                        .child(peek.places.len().to_string()),
                )
                .child(div().flex_1())
                .child(
                    ui::IconButton::new("peek-close", crate::icons::Icon::Close, "Close — esc")
                        .small()
                        .on_click({
                            let this = this.clone();
                            move |_, _, cx| this.update(cx, close_peeks)
                        }),
                ),
        )
        .child(
            div()
                .id("peek-places")
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .pb(SPACE_1H)
                .children(rows),
        )
}

/// Opens the field asking which line to go to, as `line` or `line:column`
/// counted from 1.
fn open_goto(
    bench: &mut Workbench,
    key: &FileKey,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if !bench.editors.read(cx).open.contains_key(key) {
        return;
    }
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("line[:column]"));
    let asked_for = key.clone();
    let entered = cx.subscribe_in(
        &input,
        window,
        move |bench, input, event: &InputEvent, window, cx| {
            if !matches!(event, InputEvent::PressEnter { .. }) {
                return;
            }
            let asked = input.read(cx).value();
            let mut parts = asked
                .trim()
                .splitn(2, ':')
                .map(|part| part.trim().parse::<u32>().ok());
            let line = parts.next().flatten();
            let column = parts.next().flatten();
            close_goto(bench, &asked_for, window, cx);
            if let Some(line) = line {
                let at = OpenAt {
                    path: asked_for.path.clone(),
                    line: line.saturating_sub(1),
                    column: column.map(|column| column.saturating_sub(1)),
                };
                place_cursor(bench, &asked_for, &at, window, cx);
            }
        },
    );
    input.update(cx, |input, cx| input.focus(window, cx));
    change(&bench.editors, key, cx, |document, _| {
        document.goto = Some((input, entered))
    });
}

/// Closes the go-to-line field and gives the editor back the keyboard.
fn close_goto(
    bench: &mut Workbench,
    key: &FileKey,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    bench.editors.update(cx, |editors, cx| {
        let Some(document) = editors.open.get_mut(key) else {
            return;
        };
        if document.goto.take().is_some() {
            document
                .state
                .update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
        }
    });
}

/// The bar over an editor saying it could not read or save its file, or
/// that the file changed on disk under unsaved edits.
/// Something a notice bar's button does to the open file.
type Run = dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>);

fn notice_bar(
    this: &Entity<Workbench>,
    key: &FileKey,
    notice: Notice,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let path = key.path.as_path();
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let text: SharedString = match &notice {
        Notice::Failed(error) => error.clone().into(),
        Notice::Conflict(error) => {
            format!("{name} has an external change. Your editor buffer was preserved. {error}")
                .into()
        }
        Notice::Trust(project) => {
            let folder = Path::new(project).file_name().map_or_else(
                || project.clone(),
                |name| name.to_string_lossy().into_owned(),
            );
            format!(
                "Start language servers for {folder}? They can run code from this project, such as its build scripts and installed tools. Only allow this for projects you trust."
            )
            .into()
        }
    };
    let button = |id: &'static str, label: &'static str, variant, run: Rc<Run>| {
        let this = this.clone();
        ui::Button::new(id, label, variant).on_click(move |_, window, cx| {
            let run = run.clone();
            this.update(cx, |bench, cx| run(bench, window, cx))
        })
    };
    let reload = {
        let key = key.clone();
        button(
            "editor-reload",
            "Reload disk",
            ui::Variant::Outline,
            Rc::new(move |bench, window, cx| {
                bench.editors.update(cx, |editors, _| {
                    if let Some(document) = editors.open.get_mut(&key) {
                        document.notice = None;
                        document.dirty = false;
                        document.version = None;
                    }
                });
                read(bench, key.clone(), window, cx);
            }),
        )
    };
    let answers = match &notice {
        Notice::Trust(project) => {
            let answer = |allowed: bool| {
                let (key, project) = (key.clone(), project.clone());
                Rc::new(
                    move |bench: &mut Workbench,
                          window: &mut Window,
                          cx: &mut Context<Workbench>| {
                        lsp::trust(
                            bench,
                            key.path.clone(),
                            project.clone(),
                            allowed,
                            window,
                            cx,
                        )
                    },
                )
                    as Rc<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>
            };
            vec![
                button("editor-trust", "Allow", ui::Variant::Primary, answer(true)),
                button(
                    "editor-distrust",
                    "Don't allow",
                    ui::Variant::Outline,
                    answer(false),
                ),
            ]
        }
        Notice::Conflict(_) => vec![reload],
        Notice::Failed(_) => Vec::new(),
    };
    let tone = match notice {
        Notice::Trust(_) => t.text_primary,
        _ => t.warn,
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .px(SPACE_3)
        .py(SPACE_1H)
        .border_b_1()
        .border_color(t.border)
        .bg(t.surface_sunken)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(tone)
                .font_weight(FontWeight::MEDIUM)
                .child(text),
        )
        .children(answers)
}

/// Makes the editor for the file `key` and reads the file into it.
fn open(bench: &mut Workbench, key: FileKey, window: &mut Window, cx: &mut Context<Workbench>) {
    if bench.editors.read(cx).open.contains_key(&key) {
        return;
    }
    let name = key
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let language = crate::file_icons::language(&name, cx).unwrap_or_default();
    let factory = crate::syntax::highlighters(cx);
    let state = cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .language(language)
            .soft_wrap(false)
            .line_number(true);
        state.set_highlighter_factory(factory, cx);
        state
    });
    let changes = follow_edits(&state, key.clone(), cx);
    let target = Target::new(&bench.asks, key.place.clone());
    let document = Document {
        state,
        target,
        saved: String::new(),
        version: None,
        loaded: false,
        dirty: false,
        notice: None,
        goto: None,
        served: None,
        peek: None,
        next_match: None,
        mode: preview::first_mode(&key.path),
        rendered: None,
        finding: None,
        preview_focus: cx.focus_handle(),
        preview_scroll: ScrollHandle::new(),
        gutter: gutter::Gutter::default(),
        blame: blame::Blame::default(),
        _changes: changes,
    };
    let handle = window.window_handle();
    bench.editors.update(cx, |editors, _| {
        editors.window = Some(handle);
        editors.open.insert(key.clone(), document);
    });
    show_files(bench, &key.place, cx);
    read(bench, key, window, cx);
}

/// Keeps the file `key`'s unsaved mark, language server, gutter and blame in step with its
/// edits.
fn follow_edits(
    state: &Entity<EditorState>,
    watched: FileKey,
    cx: &mut Context<Workbench>,
) -> Subscription {
    cx.subscribe(state, move |bench, state, event: &InputEvent, cx| {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let asks = bench.asks.clone();
        let text = state.read(cx).value();
        let Some((dirty, flipped)) = bench.editors.update(cx, |editors, cx| {
            let document = editors.open.get_mut(&watched)?;
            let dirty = text.len() != document.saved.len() || text.as_ref() != document.saved;
            lsp::changed(document, &asks, cx);
            let flipped = dirty != document.dirty;
            document.dirty = dirty;
            Some((dirty, flipped))
        }) else {
            return;
        };
        if flipped {
            cx.notify();
        }
        if dirty {
            keep_preview(bench, &watched.path);
        }
        gutter::changed(&bench.editors, &watched, cx);
        blame::typed(bench, &watched, cx);
    })
}

/// Moves the editors of `from` on `place`, or of the files under it, to the same names under
/// `to`, with their unsaved edits.
pub fn renamed(
    bench: &mut Workbench,
    place: &Host,
    from: &Path,
    to: &Path,
    cx: &mut Context<Workbench>,
) {
    let moved: Vec<(FileKey, FileKey)> = (bench.editors.read(cx).open.keys())
        .filter(|key| key.place == *place)
        .filter_map(|key| {
            let rest = key.path.strip_prefix(from).ok()?;
            Some((key.clone(), FileKey::new(place.clone(), to.join(rest))))
        })
        .collect();
    if moved.is_empty() {
        return;
    }
    let taken: Vec<(FileKey, Document)> = bench.editors.update(cx, |editors, _| {
        (moved.iter())
            .filter_map(|(old, new)| Some((new.clone(), editors.open.remove(old)?)))
            .collect()
    });
    let mut kept = Vec::with_capacity(taken.len());
    for (new, mut document) in taken {
        lsp::closed(&document, &bench.asks);
        document.served = None;
        document._changes = follow_edits(&document.state, new.clone(), cx);
        kept.push((new, document));
    }
    bench
        .editors
        .update(cx, |editors, _| editors.open.extend(kept));
    show_files(bench, place, cx);
    let Some(window) = bench.editors.read(cx).window else {
        return;
    };
    cx.spawn(async move |this, cx| {
        let _ = window.update(cx, |_, window, cx| {
            let _ = this.update(cx, |bench, cx| {
                for (_, key) in moved {
                    if key.place == Host::This {
                        lsp::attach(bench, key.path.clone(), window, cx);
                    }
                    gutter::read_base(bench, &key, cx);
                    blame::read(bench, &key, cx);
                }
            });
        });
    })
    .detach();
}

/// Tells the core of `place` which files to watch: every one of its files open in an editor.
fn show_files(bench: &Workbench, place: &Host, cx: &App) {
    Target::new(&bench.asks, place.clone()).call(
        "showFiles",
        json!({ "paths": bench.editors.read(cx).watched(place) }),
    );
}

/// Tells the cores again which files to watch and claims back the files open with language
/// servers, as a core forgets both for a connection that drops.
pub fn reconnected(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let mut places: Vec<Host> = Vec::new();
    let editors = bench.editors.read(cx);
    for key in (editors.open.keys()).chain(editors.viewers.keys()) {
        if !places.contains(&key.place) {
            places.push(key.place.clone());
        }
    }
    for place in places {
        show_files(bench, &place, cx);
    }
    lsp::claim(bench, cx);
}

/// Tells an account host again which of its files to watch, once it is back in reach.
pub fn host_reached(bench: &Workbench, place: &Host, cx: &App) {
    if !bench.editors.read(cx).watched(place).is_empty() {
        show_files(bench, place, cx);
    }
}

/// Reads the whole file in the pieces the core gives, starting again when it
/// changes under the read.
async fn read_file(target: &Target, path: String) -> Result<(String, String), String> {
    for _ in 0..READ_TRIES {
        let mut text = String::new();
        let mut at = 0u64;
        let mut stamp: Option<String> = None;
        let moved = loop {
            let piece = target
                .request("files.read", json!({ "path": path, "at": at }))
                .await?;
            let piece_stamp = piece["stamp"].as_str().map(str::to_owned);
            if stamp.is_some() && piece_stamp != stamp {
                break true;
            }
            stamp = piece_stamp;
            text.push_str(piece["text"].as_str().unwrap_or_default());
            at = piece["next"].as_u64().unwrap_or(at);
            if let Some(version) = piece["version"].as_str() {
                return Ok((text, version.to_owned()));
            }
            if piece["text"].as_str().is_none_or(str::is_empty) {
                break false;
            }
        };
        if !moved {
            break;
        }
    }
    Err(format!("{path} kept changing while it was read"))
}

/// Reads the file into its editor, replacing what it shows.
fn read(bench: &mut Workbench, key: FileKey, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(target) =
        (bench.editors.read(cx).open.get(&key)).map(|document| document.target.clone())
    else {
        return;
    };
    let name = key.path.to_string_lossy().into_owned();
    cx.spawn_in(window, async move |bench, cx| {
        let read = read_file(&target, name).await;
        let _ = bench.update_in(cx, |bench, window, cx| {
            let (asks, this) = (bench.asks.clone(), cx.weak_entity());
            let read = bench.editors.update(cx, |editors, cx| {
                let document = editors.open.get_mut(&key)?;
                let first = !document.loaded;
                let was_dirty = document.dirty;
                match read {
                    Ok((_, version)) if document.version.as_ref() == Some(&version) => {}
                    Ok(_) if document.dirty => {
                        document.notice = Some(Notice::Conflict(
                            "The disk version changed while this editor had unsaved work.".into(),
                        ));
                    }
                    Ok((text, version)) => {
                        document
                            .state
                            .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
                        document.saved = text;
                        document.version = Some(version);
                        document.loaded = true;
                        document.dirty = false;
                        document.notice = None;
                        if document.mode == preview::Mode::Preview {
                            let text = document.saved.clone();
                            show_rendered(document, &key, &text, &asks, this, window, cx);
                            preview::refind(document, cx);
                        }
                        lsp::changed(document, &asks, cx);
                    }
                    Err(error) if error.starts_with("not-text:") => {
                        editors.open.remove(&key);
                        return Some(None);
                    }
                    Err(error) => document.notice = Some(Notice::Failed(error)),
                }
                cx.notify();
                Some(Some((first, document.loaded, document.dirty != was_dirty)))
            });
            let (first, loaded, flipped) = match read {
                Some(Some(read)) => read,
                Some(None) => return viewer::load(bench, key, cx),
                None => return,
            };
            if first && loaded {
                if key.place == Host::This {
                    lsp::attach(bench, key.path.clone(), window, cx);
                }
                gutter::read_base(bench, &key, cx);
                blame::read(bench, &key, cx);
            } else {
                gutter::changed(&bench.editors, &key, cx);
            }
            if flipped {
                bench.parts.redraw(&bench.parts.top, cx);
            }
        });
    })
    .detach();
}

/// Saves the editor's text over the file, refused by the core when the file
/// moved on since it was read. A save of an unchanged file reads it again.
fn save(bench: &mut Workbench, key: &FileKey, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(document) = bench.editors.read(cx).open.get(key) else {
        return;
    };
    if !document.dirty {
        bench.editors.update(cx, |editors, _| {
            if let Some(document) = editors.open.get_mut(key) {
                document.version = None;
            }
        });
        return read(bench, key.clone(), window, cx);
    }
    let text = document.state.read(cx).value().to_string();
    let version = document.version.clone();
    let target = document.target.clone();
    let key = key.clone();
    cx.spawn_in(window, async move |bench, cx| {
        let saved = write_file(&target, &key.path.to_string_lossy(), &text, version).await;
        let _ = bench.update_in(cx, |bench, _, cx| {
            let landed = saved.is_ok();
            let asks = bench.asks.clone();
            let Some(flipped) = change(&bench.editors, &key, cx, |document, cx| {
                let was_dirty = document.dirty;
                match saved {
                    Ok(version) => {
                        lsp::saved(document, &text, &asks);
                        document.dirty = document.state.read(cx).value().as_ref() != text;
                        document.saved = text;
                        document.version = Some(version);
                        document.notice = None;
                    }
                    Err(error) if error.starts_with("file-conflict:") => {
                        let error = error.trim_start_matches("file-conflict:").trim().to_owned();
                        document.notice = Some(Notice::Conflict(error));
                    }
                    Err(error) => document.notice = Some(Notice::Failed(error)),
                }
                document.dirty != was_dirty
            }) else {
                return;
            };
            if landed {
                blame::read(bench, &key, cx);
            }
            if flipped {
                bench.parts.redraw(&bench.parts.top, cx);
            }
        });
    })
    .detach();
}

/// Writes `text` in the pieces the core takes, never splitting a character,
/// answering the saved file's version.
async fn write_file(
    target: &Target,
    path: &str,
    text: &str,
    version: Option<String>,
) -> Result<String, String> {
    let mut upload: Option<String> = None;
    let mut rest = text;
    loop {
        let mut end = rest.len().min(SAVE_PIECE);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (piece, after) = rest.split_at(end);
        let more = !after.is_empty();
        let written = target
            .request(
                "files.write",
                json!({ "path": path, "text": piece, "version": version, "upload": upload, "more": more }),
            )
            .await?;
        if !more {
            return written["version"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{path} was saved without a version"));
        }
        upload = written["upload"].as_str().map(str::to_owned);
        rest = after;
    }
}

/// Keeps the tab showing `path` when it was opened only to look at, as an
/// edit there means the person is working in it.
fn keep_preview(bench: &Workbench, path: &Path) {
    let Some(strip) = bench.strip() else {
        return;
    };
    let target = path.to_string_lossy();
    let previewed = bench.tabs(&strip).iter().find(|tab| {
        tab.preview
            && matches!(
                &tab.pane,
                Pane::View { kind: PaneKind::Files, target: shown, .. } if *shown == target
            )
    });
    if let Some(tab) = previewed {
        bench.keep_tab(&strip, &tab.id);
    }
}

/// Every file a tab or desk shows, once for each view of it.
fn shown(bench: &Workbench, cx: &App) -> Vec<FileKey> {
    let mut shown = Vec::new();
    let views = bench.view.iter().map(|view| (Host::This, view)).chain(
        bench
            .hosts
            .views
            .iter()
            .map(|(host, view)| (Host::Account(host.clone()), view)),
    );
    for (place, view) in views {
        let strips = view
            .layout
            .projects
            .values()
            .chain(view.layout.hosts.values());
        for tab in strips.flat_map(|strip| &strip.tabs) {
            files_in(&place, &tab.pane, &mut shown);
        }
    }
    let place = bench.place();
    shown.extend(
        bench
            .desk_files(cx)
            .map(|path| FileKey::new(place.clone(), path.clone())),
    );
    shown
}

/// Closes what hides the files `closing`, asking first when that loses unsaved edits nothing
/// else shows, and letting those edits go once the person says so.
pub fn close_unsaved(
    bench: &mut Workbench,
    closing: Vec<FileKey>,
    close: impl Fn(&mut Workbench, &mut Context<Workbench>) + 'static,
    cx: &mut Context<Workbench>,
) {
    let mut left = shown(bench, cx);
    for key in &closing {
        if let Some(at) = left.iter().position(|shown| shown == key) {
            left.swap_remove(at);
        }
    }
    let mut lost: Vec<FileKey> = Vec::new();
    for key in closing {
        if bench.editors.read(cx).is_dirty(&key) && !left.contains(&key) && !lost.contains(&key) {
            lost.push(key);
        }
    }
    let (title, body) = match lost.as_slice() {
        [] => return close(bench, cx),
        [one] => (
            format!("Close {} without saving?", name_of(&one.path)),
            "Its unsaved changes are lost.",
        ),
        many => (
            format!("Close {} files without saving?", many.len()),
            "Their unsaved changes are lost.",
        ),
    };
    bench.confirm(
        title,
        body,
        "Close",
        true,
        move |bench, yes, _, cx| {
            if !yes {
                return;
            }
            bench.editors.update(cx, |editors, _| {
                for key in &lost {
                    if let Some(document) = editors.open.get_mut(key) {
                        document.dirty = false;
                    }
                }
            });
            close(bench, cx);
            keep(bench, cx);
        },
        cx,
    );
}

/// The files in `pane`, its own or every one of its splits'.
pub fn files_of(place: &Host, pane: &Pane) -> Vec<FileKey> {
    let mut found = Vec::new();
    files_in(place, pane, &mut found);
    found
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy())
        .into_owned()
}

/// Lets go of every file with no unsaved edits that no tab shows any more.
pub fn keep(bench: &mut Workbench, cx: &mut App) {
    let editors = bench.editors.read(cx);
    if editors.open.is_empty() && editors.viewers.is_empty() {
        return;
    }
    let shown = shown(bench, cx);
    let asks = bench.asks.clone();
    let places = bench.editors.update(cx, |editors, _| {
        let closed: Vec<FileKey> = (editors.viewers.keys())
            .filter(|key| !shown.contains(*key))
            .cloned()
            .collect();
        let mut places: Vec<Host> = Vec::new();
        for key in closed {
            editors.viewers.remove(&key);
            editors.copies.forget(&key);
            if !places.contains(&key.place) {
                places.push(key.place);
            }
        }
        let gone: Vec<FileKey> = (editors.open.iter())
            .filter(|(key, document)| !document.dirty && !shown.contains(*key))
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            if let Some(document) = editors.open.remove(&key) {
                lsp::closed(&document, &asks);
            }
            if !places.contains(&key.place) {
                places.push(key.place);
            }
        }
        places
    });
    for place in places {
        show_files(bench, &place, cx);
    }
}

fn files_in(place: &Host, pane: &Pane, found: &mut Vec<FileKey>) {
    found.extend(
        crate::panes::views(pane)
            .filter(|view| view.kind == PaneKind::Files)
            .map(|view| FileKey::new(place.clone(), view.target)),
    );
}

/// Reads again what open files were at their last commit, when the repository of the
/// project folder `project` moved, or any when none: `paths` from its root, `.git` for its
/// head, or none for anything.
pub fn git_changed(
    bench: &mut Workbench,
    project: Option<&str>,
    paths: Option<&[String]>,
    cx: &mut Context<Workbench>,
) {
    let open: Vec<FileKey> = (bench.editors.read(cx).open.keys())
        .filter(|key| key.place == Host::This)
        .filter(|key| project.is_none_or(|project| key.path.starts_with(project)))
        .filter(|key| {
            paths.is_none_or(|paths| {
                paths
                    .iter()
                    .any(|changed| changed == ".git" || key.path.ends_with(changed))
            })
        })
        .cloned()
        .collect();
    for key in open {
        gutter::read_base(bench, &key, cx);
        blame::read(bench, &key, cx);
    }
}

/// Reads again the problems of files whose language server said they changed.
pub fn diagnostics_changed(bench: &mut Workbench, paths: &[String], cx: &mut Context<Workbench>) {
    insights::diagnostics_changed(bench, paths, cx);
}

/// Files that changed on disk: clean editors take the new text, and one with
/// unsaved edits says the disk moved on when its version did.
pub fn changed(bench: &mut Workbench, place: &Host, paths: &[String], cx: &mut Context<Workbench>) {
    let shown: Vec<FileKey> = paths
        .iter()
        .map(|path| FileKey::new(place.clone(), path))
        .collect();
    viewer::reopen(bench, &shown, cx);
    let editors = bench.editors.read(cx);
    let Some(window) = editors.window else {
        return;
    };
    let changed: Vec<FileKey> = shown
        .into_iter()
        .filter(|key| editors.open.contains_key(key))
        .collect();
    if changed.is_empty() {
        return;
    }
    cx.spawn(async move |this, cx| {
        let _ = window.update(cx, |_, window, cx| {
            let _ = this.update(cx, |bench, cx| {
                for key in changed {
                    read(bench, key, window, cx);
                }
            });
        });
    })
    .detach();
}

/// Puts the cursor where `at` says and brings its line into view.
fn place_cursor(
    bench: &mut Workbench,
    key: &FileKey,
    at: &OpenAt,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    change(&bench.editors, key, cx, |document, cx| {
        if at.line > 0 || at.column.is_some() {
            document.mode = preview::Mode::Source;
        }
        document.state.update(cx, |state, cx| {
            let column = at.column.unwrap_or_else(|| {
                let text = state.value();
                let line = text.lines().nth(at.line as usize).unwrap_or_default();
                (line.len() - line.trim_start().len()) as u32
            });
            state.set_cursor_position(Position::new(at.line, column), window, cx);
        });
    });
}

/// Shows the file's text, or how its markdown reads, rendered from what the
/// editor holds now.
fn show_mode(
    bench: &mut Workbench,
    key: &FileKey,
    mode: preview::Mode,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let (asks, this) = (bench.asks.clone(), cx.weak_entity());
    change(&bench.editors, key, cx, |document, cx| {
        document.mode = mode;
        match mode {
            preview::Mode::Preview => {
                let text = document.state.read(cx).value();
                show_rendered(document, key, &text, &asks, this, window, cx);
                document.goto = None;
                preview::refind(document, cx);
                if matches!(
                    document.rendered,
                    Some(Rendered::Markdown(_) | Rendered::Page(Some(_)))
                ) {
                    document.preview_focus.focus(window, cx);
                }
            }
            preview::Mode::Source => document
                .state
                .update(cx, |state, cx| state.focus(window, cx)),
        }
    });
}

/// Renders `text` as the file reads: markdown as a page, DBML as a diagram that keeps
/// where its tables were moved to, and HTML in WebKit, which opens its web links in the browser.
fn show_rendered(
    document: &mut Document,
    key: &FileKey,
    text: &str,
    asks: &tokio::sync::mpsc::UnboundedSender<Ask>,
    bench: WeakEntity<Workbench>,
    window: &mut Window,
    cx: &mut App,
) {
    match preview::look(&key.path) {
        Some(preview::Look::Page) => {
            if let Some(Rendered::Page(Some(page))) = &document.rendered {
                page.show(text);
                return;
            }
            let file = (key.place == Host::This).then_some(key.path.as_path());
            let (events, mut heard) = tokio::sync::mpsc::unbounded_channel();
            let page = crate::page_view::open(
                text,
                crate::page_view::Room::Fills(file),
                events,
                window,
                cx,
            );
            cx.spawn(async move |cx| {
                while let Some(event) = heard.recv().await {
                    if let crate::page_view::PageEvent::Link(url) = event {
                        cx.update(|cx| cx.open_url(&url));
                    }
                }
            })
            .detach();
            document.rendered = Some(Rendered::Page(page));
        }
        Some(preview::Look::Diagram) => {
            if let Some(Rendered::Diagram(diagram)) = &document.rendered {
                diagram.update(cx, |diagram, cx| diagram.set_source(text, cx));
                return;
            }
            let host = match &key.place {
                Host::This => "this".to_owned(),
                Host::Account(host) => format!("host:{host}"),
            };
            let kept = diagram::Kept {
                asks: asks.clone(),
                key: diagram::KeptKey::Fixed(format!("file:{host}:{}", key.path.display())),
            };
            let key = key.clone();
            let open: diagram::OpenLine = Rc::new(move |line, window, cx| {
                let at = OpenAt {
                    path: key.path.clone(),
                    line,
                    column: None,
                };
                let _ = bench.update(cx, |bench, cx| {
                    bench.editors.update(cx, |editors, _| {
                        if let Some(document) = editors.open.get_mut(&key) {
                            document.mode = preview::Mode::Source;
                        }
                    });
                    place_cursor(bench, &key, &at, window, cx)
                });
            });
            let diagram = cx.new(|cx| {
                let mut diagram = diagram::Diagram::new(text, open);
                diagram.kept_by(kept, cx);
                diagram
            });
            document.rendered = Some(Rendered::Diagram(diagram));
        }
        _ => document.rendered = Some(Rendered::Markdown(Rc::new(preview::render(text)))),
    }
}

/// The editors' text scale the person chose.
fn text_scale(bench: &Workbench) -> f64 {
    bench.preferences().editor_text_scale
}

/// Makes the editors' text bigger or smaller by `step`, or back to its size without one.
/// Makes the editors' text bigger or smaller a step, or back to the default size without either.
pub(crate) fn resize_text(bench: &Workbench, grow: Option<bool>) {
    rescale(
        bench,
        grow.map(|grow| {
            if grow {
                TEXT_SCALE_STEP
            } else {
                -TEXT_SCALE_STEP
            }
        }),
    );
}

fn rescale(bench: &Workbench, step: Option<f64>) {
    let scale = step.map_or(DEFAULT_TEXT_SCALE, |step| {
        ((text_scale(bench) + step) * 10.).round() / 10.
    });
    let _ = bench.asks.send(Ask::Call {
        method: "setPreference",
        params: json!({ "key": "editorTextScale", "value": Value::from(scale.clamp(MIN_TEXT_SCALE, MAX_TEXT_SCALE)) }),
    });
}
