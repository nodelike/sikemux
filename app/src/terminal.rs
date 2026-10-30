//! A terminal pane: the screen the core sends, drawn by gpui-terminal-grid.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Copy, InputEvent, InputState, Paste, SelectAll};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, canvas, div, font, px, AnyElement, AnyView, App, AppContext, Bounds, ClickEvent,
    ClipboardEntry, ClipboardItem, Context, CursorStyle, Entity, ExternalPaths, FocusHandle,
    FontFallbacks, Hsla, Image, InputHandler, InteractiveElement, IntoElement, KeyBinding,
    KeyDownEvent, Keystroke, Modifiers, MouseButton as Button, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, Rgba, ScrollEvent, SharedString,
    StyleRefinement, Styled, Subscription, Task, UTF16Selection, WeakEntity, Window,
};
use gpui_terminal_grid::{
    Cursor, CursorShape, Grid, Highlight, Layout, Look, Marked, Metrics, Span, Underline,
};
use sikemux_wire::grid::{
    self as wire, Color, FindOptions, FindResult, GridPoint, Mods, MouseAction, MouseButton, Row,
    Screen, TerminalInput,
};
use sikemux_wire::protocol::{
    SessionId, SessionInfo, DEFAULT_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE,
    MIN_TERMINAL_FONT_SIZE,
};
use sikemux_wire::pty::shell_protocol::{ShellBoundary, ShellMetadataSnapshot, ShellPhase};
use tokio::sync::{mpsc, oneshot};

use crate::core::{Ask, Screenful, TerminalAsk};
use crate::hosts::Place;
use crate::icons::Icon;
use crate::keymap::{TextSizeDecrease, TextSizeIncrease, TextSizeReset};
use crate::send_to::{self, Recipient};
use crate::theme::{self, Tokens, SPACE_0, SPACE_1H, SPACE_2};
use crate::ui;
use crate::workbench::{TakesKeys, Workbench};

const PAGE: u64 = 256;
/// History lines kept around the ones in view.
const HISTORY_KEPT: usize = 4_000;
const LINE_HEIGHT: f32 = 1.32;
/// Named by today's command id, so a shortcut the person set for it still finds it.
#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "terminal.new")]
pub struct NewTerminal;

/// The key context a terminal's keys are bound in.
const CONTEXT: &str = "Terminal";

actions!(terminal, [OpenFind, ResetModes, PageUp, PageDown]);

/// Binds a terminal's own keys. Copy, paste and select all are the Edit
/// menu's actions, so the menu works on a terminal too.
pub fn install(cx: &mut App) {
    let mac = cfg!(target_os = "macos");
    let keys = |mac_keys: &'static str, other: &'static str| if mac { mac_keys } else { other };
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new(keys("cmd-c", "ctrl-shift-c"), Copy, context),
        KeyBinding::new(keys("cmd-v", "ctrl-shift-v"), Paste, context),
        KeyBinding::new(keys("cmd-a", "ctrl-shift-a"), SelectAll, context),
        KeyBinding::new(keys("cmd-f", "ctrl-f"), OpenFind, context),
        KeyBinding::new("shift-pageup", PageUp, context),
        KeyBinding::new("shift-pagedown", PageDown, context),
        // Kit's root binds Tab to moving focus, which would otherwise win over the key handler.
        KeyBinding::new("tab", gpui::NoAction, context),
        KeyBinding::new("shift-tab", gpui::NoAction, context),
    ]);
    if mac {
        cx.bind_keys([KeyBinding::new("cmd-alt-r", ResetModes, context)]);
    }
}

/// How long a blinking cursor stays shown, and then hidden.
const BLINK: Duration = Duration::from_millis(600);
/// Wakes just past each flip, so a timer that lands a hair early does not redraw twice.
const BLINK_MARGIN: Duration = Duration::from_millis(5);
/// Characters that end a word picked by a double click.
const WORD_SEPARATORS: &str = " ()[]{}<>'\"`,;│|";

/// A terminal by the host it runs on and its session there, since sessions on different
/// hosts can share a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TermKey {
    /// The host, as numbered in [`Terminals::key`].
    place: u16,
    pub session: SessionId,
}

/// The ask `ask` for the terminal `key`, sent over its host's connection.
fn routed(places: &RefCell<Vec<Place>>, key: TermKey, ask: TerminalAsk) -> Ask {
    let ask = Ask::Terminal(key.session, ask);
    match places.borrow().get(usize::from(key.place)) {
        Some(Place::Account(host)) => Ask::On(host.clone(), Box::new(ask)),
        _ => ask,
    }
}

/// A call to the core of the host terminal `key` runs on, answered on `reply`.
fn requested(
    places: &RefCell<Vec<Place>>,
    key: TermKey,
    method: &'static str,
    params: serde_json::Value,
    reply: oneshot::Sender<Result<serde_json::Value, String>>,
) -> Ask {
    match places.borrow().get(usize::from(key.place)) {
        Some(Place::Account(host)) => Ask::HostRequest {
            host: host.clone(),
            method,
            params,
            reply,
        },
        _ => Ask::Request {
            method,
            params,
            reply,
        },
    }
}

/// Every terminal this window has attached, by host and session.
#[derive(Default)]
pub struct Terminals {
    /// The hosts terminals run on, numbered by where they are in the list.
    places: RefCell<Vec<Place>>,
    open: std::collections::HashMap<TermKey, Terminal>,
    metrics: Option<Metrics>,
    /// The terminal holding the keyboard, if one does.
    focused: Option<TermKey>,
    /// Redraws while the focused terminal's cursor blinks.
    blinking: Option<Task<()>>,
    /// Each terminal drawn on its own, so its output and cursor redraw only it.
    views: RefCell<HashMap<TermKey, Entity<TerminalView>>>,
    /// Text handed to a terminal before its first screen came, typed once it does.
    held: HashMap<TermKey, String>,
}

/// One terminal as a view of its own, which the stage keeps as last drawn until it changes.
pub struct TerminalView {
    bench: WeakEntity<Workbench>,
    id: TermKey,
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(bench) = self.bench.upgrade() else {
            return div().into_any_element();
        };
        let cx: &App = cx;
        draw(bench.read(cx), &bench, self.id, window, cx)
    }
}

struct Terminal {
    screen: Screen,
    /// History lines fetched so far, numbered as the core numbers them.
    history: BTreeMap<u64, Arc<Row>>,
    pages_asked: HashSet<u64>,
    /// The line on the top row while scrolled back; none follows the output.
    top: Option<u64>,
    selection: Option<(GridPoint, GridPoint)>,
    /// Where a selection being dragged started.
    anchor: Option<GridPoint>,
    /// The mouse moved since the button went down, so letting go opens no link.
    dragged: bool,
    /// The link under the mouse.
    link: Option<Link>,
    /// What an input method is composing.
    marked: Option<String>,
    find: Option<Find>,
    /// Where the shell says it is and how its last command went, when it says.
    shell: Option<ShellMetadataSnapshot>,
    /// When the cursor last came back into view: typing and focus restart its blink.
    blink_from: Instant,
    _focus_changes: [Subscription; 2],
    focus: FocusHandle,
    acked: u32,
    asked_size: (u16, u16),
    origin: Point<Pixels>,
    wheel: f32,
}

#[derive(Clone, PartialEq)]
struct Link {
    line: u64,
    columns: Range<u16>,
    uri: String,
}

impl Link {
    /// The address to open: only web pages, and none carrying a user name or password.
    fn web_address(&self) -> Option<&str> {
        let rest = self
            .uri
            .strip_prefix("https://")
            .or_else(|| self.uri.strip_prefix("http://"))?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        (!host.is_empty() && !host.contains('@')).then_some(self.uri.as_str())
    }
}

impl Terminal {
    fn first_line(&self) -> u64 {
        self.screen.header.history_end - u64::from(self.screen.header.history_len)
    }

    fn top_line(&self) -> u64 {
        self.top
            .unwrap_or(self.screen.header.history_end)
            .max(self.first_line())
    }

    fn row(&self, line: u64) -> Option<&Row> {
        let end = self.screen.header.history_end;
        if line >= end {
            self.screen
                .rows
                .get(usize::try_from(line - end).ok()?)
                .map(|row| &**row)
        } else {
            self.history.get(&line).map(|row| &**row)
        }
    }

    /// The row's text one entry per column; the second column of a wide character is empty.
    fn cells(&self, line: u64) -> Option<Vec<&str>> {
        let row = self.row(line)?;
        let mut cells = vec![" "; usize::from(self.screen.header.cols)];
        for span in &row.spans {
            let pieces: Vec<&str> = if span.cluster {
                vec![span.text.as_str()]
            } else {
                span.text
                    .char_indices()
                    .map(|(at, character)| &span.text[at..at + character.len_utf8()])
                    .collect()
            };
            let step = if span.wide { 2 } else { 1 };
            for (index, piece) in pieces.into_iter().enumerate() {
                let col = usize::from(span.col) + index * step;
                if let Some(cell) = cells.get_mut(col) {
                    *cell = piece;
                }
                if let Some(cell) = cells.get_mut(col + 1).filter(|_| span.wide) {
                    *cell = "";
                }
            }
        }
        Some(cells)
    }

    fn select_word(&mut self, point: GridPoint) {
        let Some(cells) = self.cells(point.line) else {
            return;
        };
        let word = |col: usize| {
            cells
                .get(col)
                .is_some_and(|cell| !cell.is_empty() && !WORD_SEPARATORS.contains(*cell))
        };
        let mut from = usize::from(point.col);
        let mut to = from;
        if word(from) {
            while from > 0 && (word(from - 1) || cells[from - 1].is_empty()) {
                from -= 1;
            }
            while to + 1 < cells.len() && (word(to + 1) || cells[to + 1].is_empty()) {
                to += 1;
            }
        }
        self.selection = Some((
            GridPoint {
                line: point.line,
                col: from as u16,
            },
            GridPoint {
                line: point.line,
                col: to as u16,
            },
        ));
    }

    fn select_line(&mut self, point: GridPoint) {
        let mut first = point.line;
        while first > 0 && self.row(first - 1).is_some_and(|row| row.wrapped) {
            first -= 1;
        }
        let mut last = point.line;
        while self.row(last).is_some_and(|row| row.wrapped) {
            last += 1;
        }
        let end = GridPoint {
            line: last,
            col: self.screen.header.cols.saturating_sub(1),
        };
        self.selection = Some((
            GridPoint {
                line: first,
                col: 0,
            },
            end,
        ));
    }

    /// The link at a cell: one the program marked, or a web address in the text.
    fn link_at(&self, point: GridPoint) -> Option<Link> {
        let row = self.row(point.line)?;
        let span = row
            .spans
            .iter()
            .find(|span| (span.col..span.col + span.width).contains(&point.col));
        if let Some((span, uri)) = span.and_then(|span| Some((span, span.style.link.clone()?))) {
            let mut columns = span.col..span.col + span.width;
            for other in row
                .spans
                .iter()
                .filter(|other| other.style.link.as_deref() == Some(&*uri))
            {
                if other.col + other.width == columns.start {
                    columns.start = other.col;
                }
                if other.col == columns.end {
                    columns.end = other.col + other.width;
                }
            }
            return Some(Link {
                line: point.line,
                columns,
                uri: uri.to_string(),
            });
        }
        let cells = self.cells(point.line)?;
        let text: String = cells.concat();
        let columns_at: Vec<u16> = cells
            .iter()
            .enumerate()
            .flat_map(|(col, cell)| std::iter::repeat_n(col as u16, cell.len()))
            .collect();
        let mut from = 0;
        while let Some(found) = ["https://", "http://"]
            .iter()
            .filter_map(|scheme| text[from..].find(scheme))
            .min()
        {
            let start = from + found;
            let length = text[start..]
                .find(|c: char| c.is_whitespace() || "\"'<>`".contains(c))
                .unwrap_or(text.len() - start);
            let uri =
                text[start..start + length].trim_end_matches(|c: char| ".,;:!?)]}".contains(c));
            let columns = columns_at[start]..columns_at[start + uri.len() - 1] + 1;
            if columns.contains(&point.col) {
                return Some(Link {
                    line: point.line,
                    columns,
                    uri: uri.to_owned(),
                });
            }
            from = start + length.max(1);
        }
        None
    }

    fn select_all(&mut self) {
        let header = &self.screen.header;
        let last = header.history_end + u64::from(header.rows.saturating_sub(1));
        self.selection = Some((
            GridPoint {
                line: self.first_line(),
                col: 0,
            },
            GridPoint {
                line: last,
                col: header.cols.saturating_sub(1),
            },
        ));
    }

    /// The selected text when it lies on one row, for a search to start from.
    fn selected_words(&self) -> Option<String> {
        let (start, end) = self
            .selection
            .filter(|(start, end)| start.line == end.line)?;
        let cells = self.cells(start.line)?;
        let text = cells
            .get(usize::from(start.col)..=usize::from(end.col).min(cells.len().saturating_sub(1)))?
            .concat();
        Some(text.trim().to_owned()).filter(|text| !text.is_empty())
    }

    fn reset(&mut self) {
        self.screen = Screen::default();
        self.history.clear();
        self.pages_asked.clear();
        self.top = None;
        self.selection = None;
        self.acked = 0;
        self.shell = None;
    }
}

impl Terminals {
    /// How many terminals this window shows.
    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    pub fn focus(&self, session: TermKey) -> Option<&FocusHandle> {
        self.open.get(&session).map(|terminal| &terminal.focus)
    }

    /// The size of one cell, once a terminal has been drawn.
    pub fn cell(&self) -> Option<gpui_kit::Size<Pixels>> {
        self.metrics.as_ref().map(|metrics| metrics.cell)
    }

    /// Draws the terminal `session` again where it was, leaving the rest of the window as it was.
    pub fn redraw(&self, session: TermKey, cx: &mut App) {
        let view = self.views.borrow().get(&session).cloned();
        if let Some(view) = view {
            view.update(cx, |_, cx| cx.repaint());
        }
    }

    /// Redraws every terminal, as when their font changes.
    pub fn redraw_all(&self, cx: &mut App) {
        let views: Vec<Entity<TerminalView>> = self.views.borrow().values().cloned().collect();
        for view in views {
            view.update(cx, |_, cx| cx.notify());
        }
    }

    pub fn screen(&self, session: TermKey) -> Option<&Screen> {
        self.open.get(&session).map(|terminal| &terminal.screen)
    }

    pub fn apply(&mut self, session: TermKey, screenful: Screenful, cx: &mut App) {
        let Some(terminal) = self.open.get_mut(&session) else {
            return;
        };
        match screenful {
            Screenful::Frame(frame) => {
                if frame.header.epoch != terminal.screen.header.epoch {
                    terminal.history.clear();
                    terminal.pages_asked.clear();
                    terminal.top = None;
                    terminal.selection = None;
                }
                terminal.screen.apply(frame);
            }
            Screenful::Lines(lines) => {
                terminal.pages_asked.remove(&lines.start);
                if lines.epoch == terminal.screen.header.epoch {
                    terminal
                        .history
                        .extend((lines.start..).zip(lines.rows.into_iter().map(Arc::new)));
                }
                if terminal.history.len() > HISTORY_KEPT {
                    let middle = terminal.top_line();
                    let half = HISTORY_KEPT as u64 / 2;
                    terminal
                        .history
                        .retain(|line, _| line.abs_diff(middle) < half);
                }
            }
            Screenful::Copied(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Screenful::Shell(shell) => {
                if terminal
                    .shell
                    .as_ref()
                    .is_none_or(|current| current.revision <= shell.revision)
                {
                    terminal.shell = Some(shell);
                }
            }
            Screenful::ShellChanged(event) => {
                let current = terminal.shell.as_ref();
                if current.is_some_and(|current| event.revision <= current.revision) {
                    return;
                }
                let last_exit_code = match event.boundary {
                    ShellBoundary::CommandFinished => event.exit_code,
                    _ => current.and_then(|current| current.last_exit_code),
                };
                terminal.shell = Some(ShellMetadataSnapshot {
                    revision: event.revision,
                    cwd: event.cwd,
                    phase: event.phase,
                    last_exit_code,
                });
            }
            Screenful::Found(found) => {
                let rows = u64::from(terminal.screen.header.rows);
                let current = found
                    .current
                    .and_then(|current| found.matches.get(current))
                    .map(|(start, _)| start.line);
                if let Some(line) = current.filter(|line| {
                    !(terminal.top_line()..terminal.top_line() + rows).contains(line)
                }) {
                    let end = terminal.screen.header.history_end;
                    let top = line.saturating_sub(rows / 2).max(terminal.first_line());
                    terminal.top = (top < end).then_some(top);
                }
                if let Some(find) = &mut terminal.find {
                    find.found = found;
                }
            }
        }
    }

    /// The key of `session` on `place`.
    pub fn key(&self, place: &Place, session: SessionId) -> TermKey {
        let mut places = self.places.borrow_mut();
        let at = places
            .iter()
            .position(|known| known == place)
            .unwrap_or_else(|| {
                places.push(place.clone());
                places.len() - 1
            });
        TermKey {
            place: at as u16,
            session,
        }
    }

    /// The host the terminal `key` runs on.
    pub fn place(&self, key: TermKey) -> Place {
        self.places
            .borrow()
            .get(usize::from(key.place))
            .cloned()
            .unwrap_or_default()
    }

    /// Attaches again after the connection came back, from a blank screen.
    pub fn reattach(&mut self, asks: &mpsc::UnboundedSender<Ask>) {
        for (session, terminal) in &mut self.open {
            terminal.reset();
            let _ = asks.send(routed(&self.places, *session, TerminalAsk::Attach));
        }
    }

    /// Lets go of terminals whose sessions are gone from their host.
    pub fn keep(
        &mut self,
        live: impl Fn(&Place, SessionId) -> bool,
        asks: &mpsc::UnboundedSender<Ask>,
    ) {
        let places = self.places.get_mut().clone();
        let place = |key: &TermKey| {
            places
                .get(usize::from(key.place))
                .cloned()
                .unwrap_or_default()
        };
        self.views
            .get_mut()
            .retain(|key, _| live(&place(key), key.session));
        self.open.retain(|session, _| {
            let keep = live(&place(session), session.session);
            if !keep {
                let _ = asks.send(routed(&self.places, *session, TerminalAsk::Detach));
            }
            keep
        });
    }
}

/// The terminal a pane shows, as a view of its own that attaches it the first time it is drawn.
pub fn pane(
    bench: &Workbench,
    this: &Entity<Workbench>,
    place: &Place,
    session: &str,
    cx: &mut App,
) -> AnyElement {
    let Ok(session) = session.parse::<SessionId>() else {
        return div().into_any_element();
    };
    let id = bench.terminals.key(place, session);
    let view = bench
        .terminals
        .views
        .borrow_mut()
        .entry(id)
        .or_insert_with(|| {
            cx.new(|_| TerminalView {
                bench: this.downgrade(),
                id,
            })
        })
        .clone();
    AnyView::from(view)
        .cached(StyleRefinement::default().size_full())
        .into_any_element()
}

fn draw(
    bench: &Workbench,
    this: &Entity<Workbench>,
    id: TermKey,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let this = this.clone();
    let mut pane = div()
        .id(("terminal", id.session))
        .group(CHIP_GROUP)
        .key_context(CONTEXT)
        .size_full()
        .p(SPACE_2)
        .on_pinch(
            window.listener_for(&this, |bench, event: &gpui::PinchEvent, _, _| {
                crate::pinch::resize(bench, crate::pinch::Surface::Terminal, event)
            }),
        )
        .on_action(window.listener_for(&this, move |bench, _: &Copy, _, _| copy(bench, id)))
        .on_action(
            window.listener_for(&this, move |bench, _: &Paste, window, cx| {
                paste(bench, id, window, cx)
            }),
        )
        .on_action(
            window.listener_for(&this, move |bench, _: &SelectAll, _, cx| {
                if let Some(terminal) = bench.terminals.open.get_mut(&id) {
                    terminal.select_all();
                    bench.terminals.redraw(id, cx);
                }
            }),
        )
        .on_action(
            window.listener_for(&this, move |bench, _: &OpenFind, window, cx| {
                open_find(bench, id, window, cx);
                bench.terminals.redraw(id, cx);
            }),
        )
        .on_action(window.listener_for(&this, move |bench, _: &PageUp, _, cx| {
            page(bench, id, true);
            bench.terminals.redraw(id, cx);
        }))
        .on_action(
            window.listener_for(&this, move |bench, _: &PageDown, _, cx| {
                page(bench, id, false);
                bench.terminals.redraw(id, cx);
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &TextSizeIncrease, _, _| {
                resize_text(bench, Some(true))
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &TextSizeDecrease, _, _| {
                resize_text(bench, Some(false))
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &TextSizeReset, _, _| {
                resize_text(bench, None)
            }),
        )
        .on_action(
            window.listener_for(&this, move |bench, _: &ResetModes, _, _| {
                let _ =
                    bench
                        .asks
                        .send(routed(&bench.terminals.places, id, TerminalAsk::ResetModes));
            }),
        )
        .on_key_down(
            window.listener_for(&this, move |bench, event: &KeyDownEvent, window, cx| {
                if key(bench, id, &event.keystroke, window) {
                    cx.stop_propagation();
                    bench.terminals.redraw(id, cx);
                }
            }),
        )
        .on_mouse_down(
            Button::Left,
            window.listener_for(&this, move |bench, event: &MouseDownEvent, window, cx| {
                mouse_down(bench, id, event, window, cx);
                bench.terminals.redraw(id, cx);
            }),
        )
        .on_mouse_move(
            window.listener_for(&this, move |bench, event: &MouseMoveEvent, _, cx| {
                if mouse_move(bench, id, event) {
                    bench.terminals.redraw(id, cx);
                }
            }),
        )
        .on_mouse_down(
            Button::Right,
            window.listener_for(&this, move |bench, _: &MouseDownEvent, window, cx| {
                if let Some(terminal) = bench.terminals.open.get(&id) {
                    window.focus(&terminal.focus, cx);
                }
            }),
        )
        .on_mouse_up(
            Button::Left,
            window.listener_for(&this, move |bench, event: &MouseUpEvent, _, cx| {
                mouse_up(bench, id, event, cx);
                bench.terminals.redraw(id, cx);
            }),
        )
        .on_scroll(
            window.listener_for(&this, move |bench, event: &ScrollEvent, _, cx| {
                if wheel(bench, id, event) {
                    bench.terminals.redraw(id, cx);
                }
            }),
        )
        .on_drop(window.listener_for(
            &this,
            move |bench, dropped: &crate::drag::DraggedPath, _, _| {
                let paths = vec![dropped.path.to_string_lossy().into_owned()];
                send(bench, id, TerminalInput::Paths { paths });
            },
        ))
        .on_drop(
            window.listener_for(&this, move |bench, dropped: &ExternalPaths, _, _| {
                take_dropped(bench, id, dropped.paths())
            }),
        );
    if let Some(terminal) = bench.terminals.open.get(&id) {
        pane = pane.track_focus(&terminal.focus);
        if terminal.link.is_some() {
            pane = pane.cursor(CursorStyle::PointingHand);
        }
        if let Some(find) = &terminal.find {
            pane = pane.child(find_bar(&this, id, find));
        }
        let over_ssh =
            || session_info(bench, id).is_some_and(|session| session.ssh_alias.is_some());
        if let Some(shell) = terminal.shell.as_ref().filter(|_| !over_ssh()) {
            pane = pane.child(shell_chip(shell, cx));
        }
    }
    let menu = terminal_menu(this.downgrade(), id);
    pane.relative()
        .child(
            div().size_full().overflow_hidden().child(
                canvas(
                    {
                        let this = this.clone();
                        move |bounds, window, cx| {
                            this.update(cx, |bench, cx| layout(bench, id, bounds, window, cx))
                        }
                    },
                    move |_, (layout, focus), window, cx| {
                        let cursor = layout.cursor_bounds();
                        window.handle_input(
                            &focus,
                            Typing {
                                bench: this,
                                id,
                                cursor,
                            },
                            cx,
                        );
                        layout.paint(window, cx);
                    },
                )
                .size_full(),
            ),
        )
        .context_menu(ui::menu(menu))
        .into_any_element()
}

/// The terminal's session as its host's view lists it.
fn session_info(bench: &Workbench, id: TermKey) -> Option<&SessionInfo> {
    let view = match bench.terminals.place(id) {
        Place::This => bench.view.as_ref()?,
        Place::Account(host) => bench.hosts.views.get(&host)?,
    };
    view.sessions
        .iter()
        .find(|session| session.id == id.session)
}

/// The shell's folder and how its last command went, faint in the corner until the pointer
/// is over the terminal.
fn shell_chip(shell: &ShellMetadataSnapshot, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx);
    let folder = shell.cwd.as_deref().map(|cwd| {
        std::path::Path::new(cwd).file_name().map_or_else(
            || cwd.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    });
    let phase = match (shell.phase, shell.last_exit_code) {
        (ShellPhase::Finished, Some(0)) => Some("done".to_owned()),
        (ShellPhase::Finished, Some(code)) => Some(format!("exit {code}")),
        (ShellPhase::Unknown, _) => None,
        (ShellPhase::Prompt, _) => Some("prompt".to_owned()),
        (ShellPhase::Input, _) => Some("input".to_owned()),
        (ShellPhase::Running, _) => Some("running".to_owned()),
        (ShellPhase::Finished, None) => Some("finished".to_owned()),
    };
    let dot = match shell.phase {
        ShellPhase::Running | ShellPhase::Input => t.live,
        ShellPhase::Finished => t.warn,
        ShellPhase::Prompt | ShellPhase::Unknown => t.ink_faint,
    };
    div()
        .absolute()
        .right(px(13.))
        .bottom(px(6.))
        .max_w(px(360.))
        .flex()
        .items_center()
        .gap(px(5.))
        .px(SPACE_1H)
        .py(SPACE_0)
        .rounded_full()
        .border_1()
        .border_color(t.line)
        .bg(t.surface)
        .font_family(theme::MONO)
        .text_size(px(9.))
        .text_color(t.text_secondary)
        .opacity(0.58)
        .group_hover(CHIP_GROUP, |chip| chip.opacity(0.9))
        .child(div().flex_none().size(px(5.)).rounded_full().bg(dot))
        .when_some(folder, |chip, folder| {
            chip.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_secondary)
                    .child(folder),
            )
        })
        .when_some(phase, |chip, phase| {
            chip.child(div().flex_none().text_color(t.text_tertiary).child(phase))
        })
}

const CHIP_GROUP: &str = "terminal";

/// Attaches the terminal the first time, fits it to its bounds, acknowledges
/// the frame about to be drawn, asks for history lines scrolled into view,
/// and shapes the grid.
fn layout(
    bench: &mut Workbench,
    id: TermKey,
    bounds: gpui_kit::Bounds<Pixels>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Layout, FocusHandle) {
    let size = px(font_size(bench) as f32);
    if bench
        .terminals
        .metrics
        .as_ref()
        .is_some_and(|metrics| metrics.font_size != size)
    {
        bench.terminals.metrics = None;
    }
    let metrics = bench
        .terminals
        .metrics
        .get_or_insert_with(|| {
            let mut mono = font(theme::MONO);
            mono.fallbacks = Some(FontFallbacks::from_fonts(vec![theme::MONO_ICONS.into()]));
            Metrics::new(mono, size, LINE_HEIGHT, window)
        })
        .clone();
    let asks = bench.asks.clone();
    if !bench.terminals.open.contains_key(&id) && bench.claims_keys(TakesKeys::Terminal) {
        cx.defer_in(window, move |bench, window, cx| {
            if let Some(terminal) = bench.terminals.open.get(&id) {
                window.focus(&terminal.focus, cx);
            }
        });
    }
    let terminal = bench.terminals.open.entry(id).or_insert_with(|| {
        let _ = asks.send(routed(&bench.terminals.places, id, TerminalAsk::Attach));
        let focus = cx.focus_handle();
        Terminal {
            screen: Screen::default(),
            history: BTreeMap::new(),
            pages_asked: HashSet::new(),
            top: None,
            selection: None,
            anchor: None,
            dragged: false,
            link: None,
            marked: None,
            find: None,
            shell: None,
            blink_from: Instant::now(),
            _focus_changes: [
                cx.on_focus_in(&focus, window, move |bench, _, cx| {
                    focus_changed(bench, id, true, cx)
                }),
                cx.on_focus_out(&focus, window, move |bench, _, _, cx| {
                    focus_changed(bench, id, false, cx)
                }),
            ],
            focus,
            acked: 0,
            asked_size: (0, 0),
            origin: Point::default(),
            wheel: 0.,
        }
    });
    terminal.origin = bounds.origin;
    if terminal.screen.header.rows > 0 && bench.terminals.held.contains_key(&id) {
        cx.defer_in(window, move |bench, _, cx| {
            if let Some(text) = bench.terminals.held.remove(&id) {
                type_text(bench, id, text, cx);
            }
        });
    }
    let size = metrics.fit(bounds.size);
    if size != terminal.asked_size {
        terminal.asked_size = size;
        let _ = asks.send(routed(
            &bench.terminals.places,
            id,
            TerminalAsk::Resize(size.0, size.1),
        ));
    }
    let header = &terminal.screen.header;
    if header.frame != terminal.acked && header.rows > 0 {
        terminal.acked = header.frame;
        let _ = asks.send(routed(
            &bench.terminals.places,
            id,
            TerminalAsk::Ack(header.frame),
        ));
    }
    let top = terminal.top_line();
    let end = top + u64::from(header.rows);
    let mut page = top / PAGE * PAGE;
    while page < end.min(header.history_end) {
        let start = page.max(terminal.first_line());
        let missing = (start..(page + PAGE).min(header.history_end))
            .any(|line| !terminal.history.contains_key(&line));
        if missing && terminal.pages_asked.insert(start) {
            let count = (page + PAGE).min(header.history_end) - start;
            let _ = asks.send(routed(
                &bench.terminals.places,
                id,
                TerminalAsk::Lines {
                    start,
                    count: count as u32,
                },
            ));
        }
        page += PAGE;
    }
    let tokens = Tokens::get(cx);
    let view = View {
        terminal,
        top,
        tokens,
        focused: terminal.focus.is_focused(window),
        in_front: window.is_window_active() && window.is_visible(),
    };
    (
        Layout::new(&view, bounds.origin, &metrics, window),
        terminal.focus.clone(),
    )
}

/// A terminal as the grid draws it: the rows from `top`, coloured by the theme.
struct View<'a> {
    terminal: &'a Terminal,
    top: u64,
    tokens: &'a Tokens,
    focused: bool,
    /// The window is active and shown, so a blinking cursor blinks.
    in_front: bool,
}

impl View<'_> {
    /// Tints the cells from one point to another, inclusive, where they are in view.
    fn tint(
        &self,
        (start, end): (GridPoint, GridPoint),
        color: Hsla,
        each: &mut dyn FnMut(Highlight),
    ) {
        let header = &self.terminal.screen.header;
        let first = start.line.max(self.top);
        let last = end
            .line
            .min(self.top + u64::from(header.rows).saturating_sub(1));
        for line in first..=last {
            let from = if line == start.line { start.col } else { 0 };
            let to = if line == end.line {
                end.col + 1
            } else {
                header.cols
            };
            each(Highlight {
                row: (line - self.top) as u16,
                columns: from..to,
                color,
                underline: false,
            });
        }
    }

    fn rgb(&self, color: Color) -> Hsla {
        let [r, g, b] = match color {
            Color::Rgb(rgb) => rgb,
            Color::Named(index) => self.named(index),
            Color::Indexed(index) => self.named(u16::from(index)),
        };
        Rgba {
            r: f32::from(r) / 255.,
            g: f32::from(g) / 255.,
            b: f32::from(b) / 255.,
            a: 1.,
        }
        .into()
    }

    fn named(&self, index: u16) -> [u8; 3] {
        let palette = &self.terminal.screen.header.palette;
        palette
            .iter()
            .find(|(named, _)| *named == index)
            .map_or_else(
                || self.tokens.terminal.color(usize::from(index)),
                |(_, rgb)| *rgb,
            )
    }

    fn look(&self, style: &wire::Style) -> Look {
        let flags = style.flags;
        let default_bg = style.bg == Color::Named(wire::NAMED_BACKGROUND);
        let (mut fg, mut bg) = (
            self.rgb(style.fg),
            (!default_bg).then(|| self.rgb(style.bg)),
        );
        if flags & wire::STYLE_INVERSE != 0 {
            (fg, bg) = (bg.unwrap_or_else(|| self.rgb(style.bg)), Some(fg));
        }
        if flags & wire::STYLE_DIM != 0 {
            fg.a *= 0.6;
        }
        if flags & wire::STYLE_HIDDEN != 0 {
            fg.a = 0.;
        }
        let underline = [
            (wire::STYLE_UNDERLINE, Underline::Single),
            (wire::STYLE_DOUBLE_UNDERLINE, Underline::Double),
            (wire::STYLE_CURLY_UNDERLINE, Underline::Curly),
            (wire::STYLE_DOTTED_UNDERLINE, Underline::Dotted),
            (wire::STYLE_DASHED_UNDERLINE, Underline::Dashed),
        ]
        .into_iter()
        .find_map(|(flag, underline)| (flags & flag != 0).then_some(underline));
        Look {
            fg,
            bg,
            bold: flags & wire::STYLE_BOLD != 0,
            italic: flags & wire::STYLE_ITALIC != 0,
            underline,
            underline_color: style.underline_color.map(|color| self.rgb(color)),
            strike: flags & wire::STYLE_STRIKE != 0,
        }
    }
}

impl Grid for View<'_> {
    fn size(&self) -> (u16, u16) {
        (
            self.terminal.screen.header.cols,
            self.terminal.screen.header.rows,
        )
    }

    fn row(&self, row: u16, each: &mut dyn FnMut(Span<'_>)) {
        let Some(line) = self.terminal.row(self.top + u64::from(row)) else {
            return;
        };
        for span in &line.spans {
            each(Span {
                column: span.col,
                text: &span.text,
                wide: span.wide,
                cluster: span.cluster,
                look: self.look(&span.style),
            });
        }
    }

    fn cursor(&self) -> Option<Cursor> {
        let header = &self.terminal.screen.header;
        let line = header.history_end + u64::from(header.cursor.row);
        let row = u16::try_from(line.checked_sub(self.top)?)
            .ok()
            .filter(|row| *row < header.rows)?;
        if self.focused
            && self.in_front
            && header.cursor.shape & wire::CURSOR_BLINKS != 0
            && blinked_off(self.terminal.blink_from)
        {
            return None;
        }
        let shape = match header.cursor.shape & !wire::CURSOR_BLINKS {
            wire::CURSOR_HIDDEN => return None,
            _ if !self.focused => CursorShape::Hollow,
            wire::CURSOR_UNDERLINE => CursorShape::Underline,
            wire::CURSOR_BEAM => CursorShape::Beam,
            wire::CURSOR_HOLLOW => CursorShape::Hollow,
            _ => CursorShape::Block,
        };
        let color = self.rgb(Color::Named(258));
        Some(Cursor {
            row,
            column: header.cursor.col,
            shape,
            color,
            text: self.tokens.terminal_cursor_text,
        })
    }

    fn highlights(&self, each: &mut dyn FnMut(Highlight)) {
        if let Some(link) = &self.terminal.link {
            if let Some(row) = link
                .line
                .checked_sub(self.top)
                .and_then(|row| u16::try_from(row).ok())
            {
                let color = self.rgb(Color::Named(wire::NAMED_FOREGROUND));
                each(Highlight {
                    row,
                    columns: link.columns.clone(),
                    color,
                    underline: true,
                });
            }
        }
        if let Some(find) = &self.terminal.find {
            for (index, &range) in find.found.matches.iter().enumerate() {
                let color = if find.found.current == Some(index) {
                    self.tokens.terminal_match_current
                } else {
                    self.tokens.terminal_match
                };
                self.tint(range, color, each);
            }
        }
        if let Some(range) = self.terminal.selection {
            self.tint(range, self.tokens.terminal_selection, each);
        }
    }

    fn marked(&self) -> Option<Marked<'_>> {
        let text = self.terminal.marked.as_deref()?;
        let header = &self.terminal.screen.header;
        let row = u16::try_from(
            (header.history_end + u64::from(header.cursor.row)).checked_sub(self.top)?,
        )
        .ok()?;
        Some(Marked {
            row,
            column: header.cursor.col,
            text,
            fg: self.rgb(Color::Named(wire::NAMED_FOREGROUND)),
            bg: self.rgb(Color::Named(wire::NAMED_BACKGROUND)),
        })
    }
}

/// The terminal holding the keyboard, if one does.
pub fn focused(bench: &Workbench) -> Option<TermKey> {
    bench.terminals.focused
}

/// Types the paths of files dropped from Finder into terminal `id`.
pub fn take_dropped(bench: &mut Workbench, id: TermKey, paths: &[std::path::PathBuf]) {
    let paths = paths
        .iter()
        .map(|path| crate::chat::lasting(path).to_string_lossy().into_owned())
        .collect();
    send(bench, id, TerminalInput::Paths { paths });
}

/// Types `text` into terminal `id` as [`type_text`] does, once it shows its first screen.
pub fn hand_to(bench: &mut Workbench, id: TermKey, text: String, cx: &mut App) {
    let ready = bench
        .terminals
        .open
        .get(&id)
        .is_some_and(|terminal| terminal.screen.header.rows > 0);
    if ready {
        type_text(bench, id, text, cx);
        return;
    }
    bench
        .terminals
        .held
        .entry(id)
        .and_modify(|held| *held = format!("{held}\n\n{text}"))
        .or_insert(text);
}

/// Types `text` into terminal `id` as one paste, as dictation does: between paste markers
/// when its program has turned bracketed paste on, else on one line, since a bare newline
/// would send it. Says whether the terminal is still open.
pub fn type_text(bench: &mut Workbench, id: TermKey, text: String, cx: &mut App) -> bool {
    let Some(terminal) = bench.terminals.open.get(&id) else {
        return false;
    };
    let text: String = text
        .chars()
        .filter(|char| matches!(char, '\n' | '\t') || !char.is_control())
        .collect();
    let input = if terminal.screen.header.modes & wire::MODE_BRACKETED_PASTE != 0 {
        TerminalInput::Paste { text }
    } else {
        TerminalInput::Text {
            text: text.replace('\n', " "),
        }
    };
    send(bench, id, input);
    bench.terminals.redraw(id, cx);
    true
}

fn send(bench: &mut Workbench, id: TermKey, input: TerminalInput) {
    if let Some(terminal) = bench.terminals.open.get_mut(&id) {
        terminal.top = None;
        terminal.blink_from = Instant::now();
    }
    let _ = bench.asks.send(routed(
        &bench.terminals.places,
        id,
        TerminalAsk::Input(input),
    ));
}

fn mods(modifiers: &Modifiers) -> Mods {
    Mods {
        shift: modifiers.shift,
        alt: modifiers.alt,
        ctrl: modifiers.control,
        meta: modifiers.platform,
    }
}

/// A key GPUI names as the `KeyboardEvent.key` the core's encoder takes.
fn named_key(key: &str) -> Option<String> {
    Some(match key {
        "enter" => "Enter".into(),
        "backspace" => "Backspace".into(),
        "tab" => "Tab".into(),
        "escape" => "Escape".into(),
        "up" => "ArrowUp".into(),
        "down" => "ArrowDown".into(),
        "left" => "ArrowLeft".into(),
        "right" => "ArrowRight".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "insert" => "Insert".into(),
        "delete" => "Delete".into(),
        _ if key.len() > 1
            && key.starts_with('f')
            && key[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            key.to_uppercase()
        }
        _ => return None,
    })
}

/// Handles a key, answering whether the terminal took it.
fn key(bench: &mut Workbench, id: TermKey, keystroke: &Keystroke, window: &Window) -> bool {
    let focused = bench
        .terminals
        .open
        .get(&id)
        .is_some_and(|terminal| terminal.focus.is_focused(window));
    if !focused {
        return false;
    }
    let modifiers = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    if modifiers.platform && !matches!(key, "left" | "right" | "backspace") {
        return false;
    }
    let input = if let Some(name) = named_key(key) {
        TerminalInput::Key {
            key: name,
            mods: mods(modifiers),
        }
    } else if modifiers.control || modifiers.alt {
        let typed = if key == "space" { " " } else { key };
        if typed.chars().count() != 1 {
            return false;
        }
        let typed = if modifiers.shift {
            typed.to_uppercase()
        } else {
            typed.to_owned()
        };
        TerminalInput::Key {
            key: typed,
            mods: mods(modifiers),
        }
    } else {
        return false;
    };
    send(bench, id, input);
    true
}

/// Text typed into a terminal, through the input method so accents and
/// composed characters arrive whole. What it is still composing shows at the cursor.
struct Typing {
    bench: Entity<Workbench>,
    id: TermKey,
    cursor: Option<Bounds<Pixels>>,
}

impl Typing {
    fn marked(&self, cx: &App) -> Option<String> {
        self.bench
            .read(cx)
            .terminals
            .open
            .get(&self.id)?
            .marked
            .clone()
    }

    fn set_marked(&self, marked: Option<String>, cx: &mut App) {
        let id = self.id;
        self.bench.update(cx, |bench, cx| {
            if let Some(terminal) = bench.terminals.open.get_mut(&id) {
                terminal.marked = marked.filter(|text| !text.is_empty());
                bench.terminals.redraw(id, cx);
            }
        });
    }
}

impl InputHandler for Typing {
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        let end = self
            .marked(cx)
            .map_or(0, |text| text.encode_utf16().count());
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.marked(cx).map(|text| 0..text.encode_utf16().count())
    }

    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        let units: Vec<u16> = self.marked(cx)?.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted = Some(range.clone());
        String::from_utf16(&units[range]).ok()
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.set_marked(None, cx);
        if !text.is_empty() {
            let (id, text) = (self.id, text.to_owned());
            self.bench
                .update(cx, |bench, _| send(bench, id, TerminalInput::Text { text }));
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.set_marked(Some(text.to_owned()), cx);
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut App) {
        self.set_marked(None, cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.cursor
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        None
    }
}

/// Moves the view by `lines`, negative toward older output.
fn scroll(terminal: &mut Terminal, lines: i64) {
    let end = terminal.screen.header.history_end;
    let top = terminal
        .top_line()
        .saturating_add_signed(lines)
        .clamp(terminal.first_line(), end);
    terminal.top = (top < end).then_some(top);
}

/// The cell under a window position, as a line number and column.
fn point_at(terminal: &Terminal, metrics: &Metrics, position: Point<Pixels>) -> (GridPoint, u16) {
    let header = &terminal.screen.header;
    let (col, row) = metrics.cell_at(position - terminal.origin, (header.cols, header.rows));
    (
        GridPoint {
            line: terminal.top_line() + u64::from(row),
            col,
        },
        row,
    )
}

fn reports_mouse(terminal: &Terminal, modifiers: &Modifiers) -> bool {
    terminal.screen.header.modes & wire::MODE_MOUSE_ANY != 0 && !modifiers.shift && !modifiers.alt
}

fn mouse_down(
    bench: &mut Workbench,
    id: TermKey,
    event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(metrics) = bench.terminals.metrics.clone() else {
        return;
    };
    let Some(terminal) = bench.terminals.open.get_mut(&id) else {
        return;
    };
    window.focus(&terminal.focus, cx);
    let (point, row) = point_at(terminal, &metrics, event.position);
    if reports_mouse(terminal, &event.modifiers) {
        let input = TerminalInput::Mouse {
            action: MouseAction::Press,
            button: MouseButton::Left,
            col: point.col,
            row,
            mods: mods(&event.modifiers),
        };
        let _ = bench.asks.send(routed(
            &bench.terminals.places,
            id,
            TerminalAsk::Input(input),
        ));
        return;
    }
    terminal.dragged = false;
    terminal.anchor = Some(point);
    match event.click_count {
        2 => terminal.select_word(point),
        3.. => terminal.select_line(point),
        _ => terminal.selection = None,
    }
}

fn mouse_move(bench: &mut Workbench, id: TermKey, event: &MouseMoveEvent) -> bool {
    let Some(metrics) = bench.terminals.metrics.clone() else {
        return false;
    };
    let Some(terminal) = bench.terminals.open.get_mut(&id) else {
        return false;
    };
    let (point, row) = point_at(terminal, &metrics, event.position);
    let modes = terminal.screen.header.modes;
    if reports_mouse(terminal, &event.modifiers) {
        let pressed = event.pressed_button == Some(Button::Left);
        if modes & wire::MODE_MOUSE_MOTION != 0 || (pressed && modes & wire::MODE_MOUSE_DRAG != 0) {
            let button = if pressed {
                MouseButton::Left
            } else {
                MouseButton::None
            };
            let input = TerminalInput::Mouse {
                action: MouseAction::Move,
                button,
                col: point.col,
                row,
                mods: mods(&event.modifiers),
            };
            let _ = bench.asks.send(routed(
                &bench.terminals.places,
                id,
                TerminalAsk::Input(input),
            ));
        }
        return false;
    }
    let Some(anchor) = terminal
        .anchor
        .filter(|_| event.pressed_button == Some(Button::Left))
    else {
        let link = terminal.link_at(point);
        let changed = link != terminal.link;
        terminal.link = link;
        return changed;
    };
    if point == anchor && !terminal.dragged {
        return false;
    }
    terminal.dragged = true;
    terminal.selection = Some(if point < anchor {
        (point, anchor)
    } else {
        (anchor, point)
    });
    true
}

fn mouse_up(bench: &mut Workbench, id: TermKey, event: &MouseUpEvent, cx: &mut App) {
    let Some(metrics) = bench.terminals.metrics.clone() else {
        return;
    };
    let Some(terminal) = bench.terminals.open.get_mut(&id) else {
        return;
    };
    let clicked = terminal.anchor.take().is_some() && !terminal.dragged && event.click_count == 1;
    if let Some(address) = terminal
        .link
        .as_ref()
        .filter(|_| clicked)
        .and_then(Link::web_address)
    {
        cx.open_url(address);
    }
    if reports_mouse(terminal, &event.modifiers) {
        let (point, row) = point_at(terminal, &metrics, event.position);
        let input = TerminalInput::Mouse {
            action: MouseAction::Release,
            button: MouseButton::Left,
            col: point.col,
            row,
            mods: mods(&event.modifiers),
        };
        let _ = bench.asks.send(routed(
            &bench.terminals.places,
            id,
            TerminalAsk::Input(input),
        ));
    }
}

fn wheel(bench: &mut Workbench, id: TermKey, event: &ScrollEvent) -> bool {
    let Some(metrics) = bench.terminals.metrics.clone() else {
        return false;
    };
    let Some(terminal) = bench.terminals.open.get_mut(&id) else {
        return false;
    };
    if event.touch_phase == gpui::TouchPhase::Started {
        terminal.wheel = 0.;
    }
    terminal.wheel += event.delta.pixel_delta(metrics.cell.height).y / metrics.cell.height;
    let lines = terminal.wheel.trunc();
    terminal.wheel -= lines;
    if lines == 0. {
        return false;
    }
    let modes = terminal.screen.header.modes;
    let alternate = modes & wire::MODE_ALT_SCREEN != 0 && modes & wire::MODE_ALTERNATE_SCROLL != 0;
    if reports_mouse(terminal, &event.modifiers) || alternate {
        let (point, row) = point_at(terminal, &metrics, event.position);
        let input = TerminalInput::Wheel {
            col: point.col,
            row,
            lines: -(lines as i32),
            mods: mods(&event.modifiers),
        };
        let _ = bench.asks.send(routed(
            &bench.terminals.places,
            id,
            TerminalAsk::Input(input),
        ));
        return false;
    }
    scroll(terminal, -(lines as i64));
    true
}

/// The find bar's state: what is typed in it, how it matches, and what the core found.
struct Find {
    input: Entity<InputState>,
    options: FindOptions,
    found: FindResult,
    _typing: Subscription,
}

impl Find {
    fn label(&self) -> SharedString {
        match self.found.current {
            Some(current) => format!(
                "{}/{}{}",
                current + 1,
                self.found.matches.len(),
                if self.found.more { "+" } else { "" }
            )
            .into(),
            None => "0/0".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    /// The query changed: the current match stays if it still matches.
    Typed,
    Next,
    Previous,
}

fn open_find(bench: &mut Workbench, id: TermKey, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(terminal) = bench.terminals.open.get_mut(&id) else {
        return;
    };
    let words = terminal.selected_words();
    let input = match &terminal.find {
        Some(find) => find.input.clone(),
        None => {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in terminal"));
            let typing = cx.subscribe(&input, move |bench, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    find_in(bench, id, Step::Typed, cx);
                }
            });
            terminal.find = Some(Find {
                input: input.clone(),
                options: FindOptions::default(),
                found: FindResult::default(),
                _typing: typing,
            });
            input
        }
    };
    input.update(cx, |state, cx| {
        if let Some(words) = words {
            state.set_value(words, window, cx);
        }
        state.focus(window, cx);
        state.select_all(window, cx);
    });
    find_in(bench, id, Step::Typed, cx);
}

fn close_find(bench: &mut Workbench, id: TermKey, window: &mut Window, cx: &mut App) {
    if let Some(terminal) = bench.terminals.open.get_mut(&id) {
        terminal.find = None;
        window.focus(&terminal.focus, cx);
    }
    bench.terminals.redraw(id, cx);
}

fn find_in(bench: &mut Workbench, id: TermKey, step: Step, cx: &mut Context<Workbench>) {
    let Some(terminal) = bench.terminals.open.get_mut(&id) else {
        return;
    };
    let top = terminal.top_line();
    let Some(find) = &mut terminal.find else {
        return;
    };
    let query = find.input.read(cx).value().to_string();
    if query.is_empty() {
        find.found = FindResult::default();
        return bench.terminals.redraw(id, cx);
    }
    let current = find
        .found
        .current
        .and_then(|current| find.found.matches.get(current))
        .map(|(start, _)| *start);
    let ask = TerminalAsk::Find {
        query,
        options: find.options.clone(),
        from: Some(current.unwrap_or(GridPoint { line: top, col: 0 })),
        backwards: step == Step::Previous,
        inclusive: step == Step::Typed || current.is_none(),
    };
    let _ = bench.asks.send(routed(&bench.terminals.places, id, ask));
}

fn copy(bench: &mut Workbench, id: TermKey) {
    if let Some(selection) = bench
        .terminals
        .open
        .get(&id)
        .and_then(|terminal| terminal.selection)
    {
        let _ = bench.asks.send(routed(
            &bench.terminals.places,
            id,
            TerminalAsk::Copy(Some(selection)),
        ));
    }
}

/// A page of history up or down, or the key itself to a program on the
/// alternate screen, which scrolls its own way.
fn page(bench: &mut Workbench, id: TermKey, up: bool) {
    let Some(terminal) = bench.terminals.open.get_mut(&id) else {
        return;
    };
    let header = &terminal.screen.header;
    if header.modes & wire::MODE_ALT_SCREEN != 0 {
        let key = if up { "PageUp" } else { "PageDown" };
        let mods = Mods {
            shift: true,
            ..Mods::default()
        };
        send(
            bench,
            id,
            TerminalInput::Key {
                key: key.into(),
                mods,
            },
        );
        return;
    }
    let rows = i64::from(header.rows);
    scroll(terminal, if up { -rows } else { rows });
}

/// The terminals' font size the person chose.
fn font_size(bench: &Workbench) -> u32 {
    bench.preferences().terminal_font_size
}

/// Makes the terminals' text bigger or smaller a step, or back to the default size without
/// either.
pub(crate) fn resize_text(bench: &Workbench, grow: Option<bool>) {
    let size = grow.map_or(DEFAULT_TERMINAL_FONT_SIZE, |grow| {
        font_size(bench)
            .saturating_add_signed(if grow { 1 } else { -1 })
            .clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE)
    });
    let _ = bench.asks.send(Ask::Call {
        method: "setPreference",
        params: serde_json::json!({ "key": "terminalFontSize", "value": size }),
    });
}

fn paste(bench: &mut Workbench, id: TermKey, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(item) = cx.read_from_clipboard() else {
        return;
    };
    if let Some(text) = item.text() {
        send(bench, id, TerminalInput::Paste { text });
        bench.terminals.redraw(id, cx);
        return;
    }
    for entry in item.entries() {
        if let ClipboardEntry::Image(image) = entry {
            paste_picture(id, image.clone(), window, cx);
        }
    }
}

/// Keeps a pasted picture on the terminal's host and types its path, which is how agents
/// in a terminal take a picture.
fn paste_picture(id: TermKey, image: Image, window: &mut Window, cx: &mut Context<Workbench>) {
    let stamp = chrono::Local::now()
        .format("pasted-%Y%m%d-%H%M%S%3f")
        .to_string();
    let readable = cx.background_spawn(async move { crate::pasted::readable(image) });
    cx.spawn_in(window, async move |this, cx| {
        use base64::Engine;
        let image = readable.await;
        let params = serde_json::json!({
            "name": format!("{stamp}.{}", image.format.extension()),
            "mime": image.format.mime_type(),
            "data": base64::engine::general_purpose::STANDARD.encode(&image.bytes),
        });
        let (reply, answer) = oneshot::channel();
        this.update(cx, |bench, _| {
            let ask = requested(&bench.terminals.places, id, "keepPasted", params, reply);
            let _ = bench.asks.send(ask);
        })
        .ok();
        let kept = match answer.await {
            Ok(Ok(kept)) => kept,
            Ok(Err(error)) => {
                let note = format!("The pasted picture was not kept: {error}");
                cx.update(|window, cx| window.push_notification(note, cx))
                    .ok();
                return;
            }
            Err(_) => return,
        };
        if let Some(path) = kept["path"].as_str() {
            let paths = vec![path.to_owned()];
            this.update(cx, |bench, cx| {
                send(bench, id, TerminalInput::Paths { paths });
                bench.terminals.redraw(id, cx);
            })
            .ok();
        }
    })
    .detach();
}

/// The find bar over the pane's top right corner.
fn find_bar(bench: &Entity<Workbench>, id: TermKey, find: &Find) -> ui::FindBar {
    let act = |change: fn(&mut Workbench, TermKey, &mut Window, &mut Context<Workbench>)| {
        let bench = bench.clone();
        move |window: &mut Window, cx: &mut App| {
            bench.update(cx, |bench, cx| change(bench, id, window, cx))
        }
    };
    let flip = |flip: fn(&mut FindOptions)| {
        let bench = bench.clone();
        move |_: &mut Window, cx: &mut App| {
            bench.update(cx, |bench, cx| {
                if let Some(find) = bench
                    .terminals
                    .open
                    .get_mut(&id)
                    .and_then(|terminal| terminal.find.as_mut())
                {
                    flip(&mut find.options);
                    find.found = FindResult::default();
                }
                find_in(bench, id, Step::Typed, cx);
                bench.terminals.redraw(id, cx);
            })
        }
    };
    let stepping = bench.clone();
    ui::FindBar::new(&find.input, find.label())
        .match_case(
            find.options.case_sensitive,
            flip(|options| options.case_sensitive ^= true),
        )
        .whole_word(
            find.options.whole_word,
            flip(|options| options.whole_word ^= true),
        )
        .regex(find.options.regex, flip(|options| options.regex ^= true))
        .on_step(move |forward, _, cx| {
            let step = if forward { Step::Next } else { Step::Previous };
            stepping.update(cx, |bench, cx| find_in(bench, id, step, cx));
        })
        .on_close(act(|bench, id, window, cx| {
            close_find(bench, id, window, cx)
        }))
}

/// Puts the selected text in an agent's composer, under a line saying which
/// terminal it came from, and brings the agent forward.
fn send_selection(
    bench: &mut Workbench,
    id: TermKey,
    to: Recipient,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(range) = bench
        .terminals
        .open
        .get(&id)
        .and_then(|terminal| terminal.selection)
    else {
        return;
    };
    let folder = bench
        .terminals
        .open
        .get(&id)
        .and_then(|terminal| terminal.shell.as_ref()?.cwd.clone());
    let heading = session_info(bench, id).map_or_else(
        || "From a terminal:".to_owned(),
        |session| {
            let name = session
                .rail_name
                .clone()
                .or_else(|| session.window_title.clone())
                .unwrap_or_else(|| "Terminal".to_owned());
            match folder.as_ref().or(session.project.as_ref()) {
                Some(folder) => format!("From the terminal \"{name}\" in {folder}:"),
                None => format!("From the terminal \"{name}\":"),
            }
        },
    );
    let (reply, text) = oneshot::channel();
    let _ = bench.asks.send(routed(
        &bench.terminals.places,
        id,
        TerminalAsk::Text {
            range: Some(range),
            reply,
        },
    ));
    let text = async move {
        let text = text.await.ok()?;
        Some(format!("{heading}\n\n{}\n", send_to::code_fence(&text)))
    };
    send_to::send(bench, to, text, window, cx);
}

/// What right-clicking a terminal offers. Its edit and find items are the terminal's own
/// actions, so they show and do what the terminal's keys do.
fn terminal_menu(
    bench: WeakEntity<Workbench>,
    id: TermKey,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |popup, window, cx| {
        let Some(this) = bench.upgrade() else {
            return popup;
        };
        let (focus, selected, in_project) = {
            let bench = this.read(cx);
            let terminal = bench.terminals.open.get(&id);
            let in_project = bench
                .session(&id.session.to_string())
                .is_some_and(|session| session.project.is_some());
            (
                terminal.map(|terminal| terminal.focus.clone()),
                terminal.is_some_and(|terminal| terminal.selection.is_some()),
                in_project,
            )
        };
        let ask = |ask: fn() -> TerminalAsk| {
            let this = this.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                this.update(cx, |bench, _| {
                    let _ = bench.asks.send(routed(&bench.terminals.places, id, ask()));
                })
            }
        };
        let mut popup = popup
            .when_some(focus, PopupMenu::action_context)
            .item(
                PopupMenuItem::new("Copy")
                    .icon(Icon::Copy)
                    .action(Box::new(Copy))
                    .disabled(!selected),
            )
            .item(
                PopupMenuItem::new("Paste")
                    .icon(Icon::Text)
                    .action(Box::new(Paste)),
            )
            .item(
                PopupMenuItem::new("Select All")
                    .icon(Icon::Text)
                    .action(Box::new(SelectAll)),
            );
        if selected && in_project {
            let chosen = move |bench: &mut Workbench,
                               to,
                               window: &mut Window,
                               cx: &mut Context<Workbench>| {
                send_selection(bench, id, to, window, cx)
            };
            popup = send_to::submenu(
                popup,
                "Send Selection to Agent…",
                bench.clone(),
                chosen,
                window,
                cx,
            );
        }
        popup
            .separator()
            .item(
                PopupMenuItem::new("Find")
                    .icon(Icon::Search)
                    .action(Box::new(OpenFind)),
            )
            .item(
                PopupMenuItem::new("Copy Scrollback")
                    .icon(Icon::Copy)
                    .on_click(ask(|| TerminalAsk::Copy(None))),
            )
            .separator()
            .item(
                PopupMenuItem::new("Clear Terminal")
                    .icon(Icon::Discard)
                    .on_click(ask(|| TerminalAsk::Clear)),
            )
    }
}

impl Workbench {
    /// Draws the terminal's cursor as one tick of its blink timer would.
    pub fn blink_now(&mut self, id: TermKey, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminals.open.get_mut(&id) {
            terminal.blink_from -= BLINK;
        }
        self.terminals.redraw(id, cx);
    }
}

/// How far a blinking cursor is into being shown, or hidden.
fn phase(terminal: &Terminal) -> Duration {
    Duration::from_millis((terminal.blink_from.elapsed().as_millis() % BLINK.as_millis()) as u64)
}

fn blinked_off(since: Instant) -> bool {
    since.elapsed().as_millis() / BLINK.as_millis() % 2 == 1
}

/// Tells the program, if it asked, and keeps a timer redrawing a blinking cursor while it is focused.
fn focus_changed(bench: &mut Workbench, id: TermKey, focused: bool, cx: &mut Context<Workbench>) {
    let Some(terminal) = bench.terminals.open.get_mut(&id) else {
        return;
    };
    terminal.blink_from = Instant::now();
    if terminal.screen.header.modes & wire::MODE_FOCUS_REPORTS != 0 {
        let _ = bench.asks.send(routed(
            &bench.terminals.places,
            id,
            TerminalAsk::Input(TerminalInput::Focus { focused }),
        ));
    }
    let ssh = bench
        .session(&id.session.to_string())
        .is_some_and(|session| session.ssh_alias.is_some());
    if focused && ssh {
        let _ = bench.asks.send(Ask::Call {
            method: "reconnectSsh",
            params: serde_json::json!({ "id": id.session }),
        });
    }
    if focused {
        bench.terminals.focused = Some(id);
    } else if bench.terminals.focused == Some(id) {
        bench.terminals.focused = None;
    }
    keep_blinking(bench, cx);
    bench.terminals.redraw(id, cx);
}

/// Keeps a timer redrawing the focused terminal's blinking cursor while the window is active
/// and shown; it rests otherwise, and starts again when the window comes back.
pub fn keep_blinking(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if bench.terminals.focused.is_none() || bench.terminals.blinking.is_some() {
        return;
    }
    let window = bench.window_handle();
    bench.terminals.blinking = Some(cx.spawn(async move |this, cx| loop {
        let in_front = window
            .update(cx, |_, window, _| {
                window.is_window_active() && window.is_visible()
            })
            .unwrap_or(false);
        let Ok(wait) = this.update(cx, |bench, cx| {
            let focused = bench.terminals.focused;
            let blinking = focused
                .and_then(|id| bench.terminals.open.get(&id))
                .filter(|terminal| terminal.screen.header.cursor.shape & wire::CURSOR_BLINKS != 0);
            let wait = blinking
                .filter(|_| in_front)
                .map(|terminal| BLINK - phase(terminal) + BLINK_MARGIN);
            if wait.is_none() {
                bench.terminals.blinking = None;
            }
            if let Some(id) = focused {
                bench.terminals.redraw(id, cx);
            }
            wait
        }) else {
            return;
        };
        match wait {
            Some(wait) => cx.background_executor().timer(wait).await,
            None => return,
        }
    }));
}
