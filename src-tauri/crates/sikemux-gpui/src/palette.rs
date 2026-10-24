//! The pickers that rise over the stage, one at a time: the command deck, the
//! agent palette, the project picker, New… and the themes. Each fills a list of
//! rows; the person types to narrow it, moves through it and picks one.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, list, px, AnyElement, App, AppContext, Context, Entity, FocusHandle, FontWeight,
    Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, ListAlignment, ListState,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window,
};

use crate::icons::Icon;
use crate::plugin_pane::Failure;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

actions!(palette, [Next, Previous, PageDown, PageUp, Choose, Close]);

const ROW_HEIGHT: f32 = 30.;
/// A hairline between groups, with the room around it.
const RULE_HEIGHT: f32 = 17.;
const SHOWN: usize = 200;
const PAGE: isize = 8;

pub fn install(cx: &mut App) {
    let context = Some("Palette > Input");
    cx.bind_keys([
        KeyBinding::new("down", Next, context),
        KeyBinding::new("tab", Next, context),
        KeyBinding::new("up", Previous, context),
        KeyBinding::new("shift-tab", Previous, context),
        KeyBinding::new("pagedown", PageDown, context),
        KeyBinding::new("pageup", PageUp, context),
        KeyBinding::new("enter", Choose, context),
        KeyBinding::new("escape", Close, context),
    ]);
}

/// What a row shows before its title.
#[derive(Clone)]
pub enum Mark {
    Icon(Icon),
    /// An agent's CLI, in its brand colour.
    Agent(String),
    /// An open agent: its face, or its CLI's mark when faces are off.
    Open(Box<crate::workbench::AgentRow>),
}

#[derive(Clone)]
pub struct Row {
    /// What the palette's chooser is told was picked.
    pub key: SharedString,
    pub title: SharedString,
    /// The small words after the title, such as a command's category.
    pub meta: SharedString,
    /// The keys that do the same, drawn at the end.
    pub keys: Option<SharedString>,
    /// An agent's @name, dimmed after its title.
    pub handle: Option<SharedString>,
    /// What an open agent is doing, drawn in place of the meta as the rail draws it.
    pub state: Option<sikemux_wire::protocol::AgentStatus>,
    pub mark: Option<Mark>,
    /// The heading the row sits under while nothing is typed.
    pub group: Option<SharedString>,
    pub disabled: bool,
    /// The palette's armed choice does not reach this row, which says so while it is chosen.
    pub unarmed: bool,
    /// Everything typing matches against.
    pub search: String,
}

impl Row {
    pub fn new(key: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        let title = title.into();
        Self {
            key: key.into(),
            search: title.to_string(),
            title,
            meta: SharedString::default(),
            keys: None,
            handle: None,
            state: None,
            mark: None,
            group: None,
            disabled: false,
            unarmed: false,
        }
    }

    pub fn unarmed(mut self, unarmed: bool) -> Self {
        self.unarmed = unarmed;
        self
    }

    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = meta.into();
        self
    }

    pub fn handle(mut self, handle: Option<SharedString>) -> Self {
        self.handle = handle;
        self
    }

    pub fn state(mut self, state: sikemux_wire::protocol::AgentStatus) -> Self {
        self.state = Some(state);
        self
    }

    pub fn keys(mut self, keys: Option<SharedString>) -> Self {
        self.keys = keys;
        self
    }

    pub fn mark(mut self, mark: Mark) -> Self {
        self.mark = Some(mark);
        self
    }

    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Words typing finds the row by besides its title.
    pub fn also(mut self, words: &str) -> Self {
        self.search.push(' ');
        self.search.push_str(words);
        self
    }
}

type Pick = Rc<dyn Fn(&Row, &mut Workbench, &mut Window, &mut Context<Workbench>)>;
type Leave = Rc<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>;
type Typed = Rc<dyn Fn(&str) -> Option<Row>>;

/// One palette as its opener describes it.
pub struct Spec {
    /// Which palette it is, so the shortcut that opened it closes it again.
    pub id: &'static str,
    pub placeholder: SharedString,
    /// What the list says when typing matches nothing.
    pub empty: SharedString,
    pub rows: Vec<Row>,
    pub width: f32,
    pub pick: Pick,
    /// Heard each time the highlight moves, for a palette that shows its row
    /// before it is picked.
    pub browse: Option<Pick>,
    /// Heard when the palette closes with nothing picked.
    pub cancel: Option<Leave>,
    /// The row highlighted while nothing is typed, such as the theme worn now.
    pub start: Option<SharedString>,
    /// A row made of what is typed, offered first when no row has its key.
    pub typed: Option<Typed>,
    /// A choice beside the field that the pick reads, such as the mode an agent starts in.
    pub choice: Option<Choice>,
    /// A button beside the field that does something the rows cannot, such as opening a folder.
    pub aside: Option<Aside>,
    /// Drawn as the command deck: wider and taller, its rows' words in tracked capitals and
    /// their keys in keycaps.
    deck: bool,
    /// Rises over the stage alone, leaving the rails as they are.
    on_stage: bool,
    /// Its groups are parted by hairlines rather than headed by their names.
    ruled: bool,
    /// Its rows are numbered while nothing is typed, and a number's key picks its row.
    numbered: bool,
}

/// A button beside the palette's field: its icon, what it says it does, and what it does.
pub struct Aside {
    pub icon: Icon,
    pub label: SharedString,
    pub press: Leave,
}

/// A few words to choose between beside the palette's field, and which is chosen now.
#[derive(Clone)]
pub struct Choice {
    pub options: Vec<(Icon, SharedString)>,
    pub chosen: Rc<Cell<usize>>,
    /// The option that lets go of a safeguard, drawn loud while chosen.
    pub armed: Option<usize>,
}

impl Choice {
    fn armed_now(&self) -> bool {
        self.armed == Some(self.chosen.get())
    }
}

impl Spec {
    pub fn new(
        id: &'static str,
        placeholder: impl Into<SharedString>,
        rows: Vec<Row>,
        pick: impl Fn(&Row, &mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
    ) -> Self {
        Self {
            id,
            placeholder: placeholder.into(),
            empty: "no matches".into(),
            rows,
            width: 560.,
            pick: Rc::new(pick),
            browse: None,
            cancel: None,
            start: None,
            typed: None,
            choice: None,
            aside: None,
            deck: false,
            numbered: false,
            on_stage: false,
            ruled: false,
        }
    }

    pub fn on_stage(mut self) -> Self {
        self.on_stage = true;
        self
    }

    pub fn ruled(mut self) -> Self {
        self.ruled = true;
        self
    }

    pub fn numbered(mut self) -> Self {
        self.numbered = true;
        self
    }

    pub fn deck(mut self) -> Self {
        self.deck = true;
        self.width = 680.;
        self
    }

    pub fn typed(mut self, typed: impl Fn(&str) -> Option<Row> + 'static) -> Self {
        self.typed = Some(Rc::new(typed));
        self
    }

    pub fn choice(mut self, choice: Choice) -> Self {
        self.choice = Some(choice);
        self
    }

    pub fn aside(
        mut self,
        icon: Icon,
        label: impl Into<SharedString>,
        press: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
    ) -> Self {
        self.aside = Some(Aside {
            icon,
            label: label.into(),
            press: Rc::new(press),
        });
        self
    }

    pub fn start(mut self, key: SharedString) -> Self {
        self.start = Some(key);
        self
    }

    pub fn empty(mut self, empty: impl Into<SharedString>) -> Self {
        self.empty = empty.into();
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn browse(
        mut self,
        browse: impl Fn(&Row, &mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
    ) -> Self {
        self.browse = Some(Rc::new(browse));
        self
    }

    pub fn cancel(
        mut self,
        cancel: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
    ) -> Self {
        self.cancel = Some(Rc::new(cancel));
        self
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Line {
    Heading(usize),
    Row(usize),
    Rule,
}

impl Line {
    fn height(self) -> f32 {
        match self {
            Line::Rule => RULE_HEIGHT,
            _ => ROW_HEIGHT,
        }
    }
}

/// The open palette: what it offers, what the person typed, and the lines
/// that shows.
pub struct Palette {
    spec: Spec,
    input: Entity<InputState>,
    lines: Vec<Line>,
    selected: Option<usize>,
    list: ListState,
    /// What had focus before the palette took it, to hand it back.
    before: Option<FocusHandle>,
    /// The last row is the one made of what was typed.
    typed_shown: bool,
    /// Why its rows could not be read, and how to read them again.
    failed: Option<(Failure, Leave)>,
    _typing: Subscription,
}

impl Palette {
    pub fn id(&self) -> &'static str {
        self.spec.id
    }

    /// The number `line` goes by in a numbered palette while nothing is typed.
    fn number_of(&self, line: usize, cx: &App) -> Option<usize> {
        let numbering = self.spec.numbered && self.input.read(cx).value().is_empty();
        let rows = self.lines[..=line.min(self.lines.len().saturating_sub(1))]
            .iter()
            .filter(|line| matches!(line, Line::Row(_)))
            .count();
        let numbered = numbering && matches!(self.lines.get(line), Some(Line::Row(_)));
        numbered.then_some(rows).filter(|number| *number <= 9)
    }

    /// The line a number's key picks in a numbered palette while nothing is typed.
    fn line_numbered(&self, number: usize, cx: &App) -> Option<usize> {
        (0..self.lines.len()).find(|&line| self.number_of(line, cx) == Some(number))
    }

    fn filter(&mut self, query: &str) {
        if self.typed_shown {
            self.spec.rows.pop();
            self.typed_shown = false;
        }
        let typed = self
            .spec
            .typed
            .as_ref()
            .and_then(|typed| typed(query.trim()))
            .filter(|row| !self.spec.rows.iter().any(|kept| kept.key == row.key));
        let rows = &self.spec.rows;
        self.lines = if self.spec.ruled {
            ruled(rows, query.trim())
        } else if query.trim().is_empty() {
            let mut lines = Vec::with_capacity(rows.len() + 4);
            let mut group = None;
            for (place, row) in rows.iter().enumerate() {
                if row.group.is_some() && row.group != group {
                    lines.push(Line::Heading(place));
                    group = row.group.clone();
                }
                lines.push(Line::Row(place));
            }
            lines
        } else {
            crate::fuzzy::rank(query, rows.iter().map(|row| row.search.as_str()), SHOWN)
                .into_iter()
                .map(Line::Row)
                .collect()
        };
        if let Some(row) = typed {
            self.spec.rows.push(row);
            self.lines.insert(0, Line::Row(self.spec.rows.len() - 1));
            self.typed_shown = true;
        }
        self.list.reset(self.lines.len());
        let start = self
            .spec
            .start
            .clone()
            .filter(|_| query.trim().is_empty())
            .and_then(|key| self.line_of(&key));
        self.select(start.or_else(|| self.step_from(None, 1)));
    }

    fn line_of(&self, key: &SharedString) -> Option<usize> {
        let rows = &self.spec.rows;
        self.lines
            .iter()
            .position(|line| matches!(line, Line::Row(place) if rows[*place].key == *key))
    }

    /// The key of the row highlighted now.
    pub fn highlighted(&self) -> Option<SharedString> {
        self.selected
            .and_then(|line| self.row(line))
            .map(|row| row.key.clone())
    }

    /// Offers `rows` in place of what it offered, keeping what the person typed
    /// and the row they had reached.
    pub fn refill(&mut self, rows: Vec<Row>, cx: &App) {
        let reached = self.highlighted();
        self.failed = None;
        self.spec.rows = rows;
        self.typed_shown = false;
        let query = self.input.read(cx).value().to_string();
        self.filter(&query);
        if let Some(line) = reached.and_then(|key| self.line_of(&key)) {
            self.select(Some(line));
        }
    }

    /// Offers nothing, saying why in place of the rows, with `retry` to read them again.
    pub fn failed(
        &mut self,
        failure: Failure,
        retry: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
        cx: &App,
    ) {
        self.refill(Vec::new(), cx);
        self.failed = Some((failure, Rc::new(retry)));
    }

    fn choosable(&self, line: usize) -> bool {
        matches!(self.lines.get(line), Some(Line::Row(place)) if !self.spec.rows[*place].disabled)
    }

    /// The next row that can be picked `by` lines on from `from`, wrapping.
    fn step_from(&self, from: Option<usize>, by: isize) -> Option<usize> {
        let count = self.lines.len() as isize;
        let start = from.map_or(if by > 0 { -1 } else { count }, |from| from as isize);
        (1..=count)
            .map(|step| (start + by.signum() * step).rem_euclid(count) as usize)
            .find(|&line| self.choosable(line))
    }

    fn step(&mut self, by: isize) {
        let mut at = self.selected;
        for _ in 0..by.unsigned_abs() {
            at = self.step_from(at, by);
        }
        self.select(at);
    }

    fn select(&mut self, line: Option<usize>) {
        self.selected = line;
        if let Some(line) = line {
            self.list.scroll_to_reveal_item(line);
        }
    }

    fn row(&self, line: usize) -> Option<&Row> {
        match self.lines.get(line)? {
            Line::Row(place) => self.spec.rows.get(*place),
            Line::Heading(_) | Line::Rule => None,
        }
    }
}

/// The lines of a palette whose groups are parted by hairlines: each group in its order,
/// narrowed to what `query` finds in it and best first, a rule between groups that show.
fn ruled(rows: &[Row], query: &str) -> Vec<Line> {
    let mut lines = Vec::with_capacity(rows.len() + 2);
    let mut start = 0;
    while start < rows.len() {
        let group = &rows[start].group;
        let end = rows[start..]
            .iter()
            .position(|row| row.group != *group)
            .map_or(rows.len(), |length| start + length);
        let found: Vec<usize> = if query.is_empty() {
            (start..end).collect()
        } else {
            crate::fuzzy::rank(
                query,
                rows[start..end].iter().map(|row| row.search.as_str()),
                SHOWN,
            )
            .into_iter()
            .map(|place| start + place)
            .collect()
        };
        if !found.is_empty() && !lines.is_empty() {
            lines.push(Line::Rule);
        }
        lines.extend(found.into_iter().map(Line::Row));
        start = end;
    }
    lines
}

impl Workbench {
    /// Opens `spec`'s palette in place of any other, or closes it when it is
    /// the one open.
    pub fn toggle_palette(&mut self, spec: Spec, window: &mut Window, cx: &mut Context<Self>) {
        let same = self
            .palette
            .as_ref()
            .is_some_and(|open| open.id() == spec.id);
        self.close_palette(window, cx);
        if same {
            return;
        }
        self.close_finder(window, cx);
        let before = window.focused(cx);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(spec.placeholder.clone()));
        let typing = cx.subscribe_in(
            &input,
            window,
            |bench, input, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    let query = input.read(cx).value().to_string();
                    if let Some(palette) = bench.palette.as_mut() {
                        palette.filter(&query);
                    }
                    bench.browse(window, cx);
                    bench.parts.redraw(&bench.parts.finder, cx);
                }
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        let mut palette = Palette {
            spec,
            input,
            lines: Vec::new(),
            selected: None,
            list: ListState::new(0, ListAlignment::Top, px(ROW_HEIGHT * 8.)),
            before,
            typed_shown: false,
            failed: None,
            _typing: typing,
        };
        palette.filter("");
        self.palette = Some(palette);
        self.parts.redraw(&self.parts.finder, cx);
        cx.notify();
    }

    /// Closes the open palette with nothing picked.
    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancel) = self.leave_palette(window, cx).and_then(|spec| spec.cancel) {
            cancel(self, window, cx);
        }
    }

    fn leave_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Spec> {
        let palette = self.palette.take()?;
        palette
            .before
            .as_ref()
            .unwrap_or(&self.focus)
            .focus(window, cx);
        self.parts.redraw(&self.parts.finder, cx);
        cx.notify();
        Some(palette.spec)
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        let (Some(browse), Some(row)) = (
            palette.spec.browse.clone(),
            palette.selected.and_then(|line| palette.row(line)).cloned(),
        ) else {
            return;
        };
        browse(&row, self, window, cx);
    }

    fn step_palette(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.as_mut() {
            palette.step(by);
        }
        self.browse(window, cx);
        self.parts.redraw(&self.parts.finder, cx);
    }

    /// Picks the row a number's key names in a numbered palette while nothing is typed.
    fn pick_numbered(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let plain = !keystroke.modifiers.modified();
        let Some(number) = keystroke.key.parse::<usize>().ok().filter(|_| plain) else {
            return;
        };
        let line = self
            .palette
            .as_ref()
            .and_then(|palette| palette.line_numbered(number, cx));
        if let Some(line) = line {
            cx.stop_propagation();
            self.pick(Some(line), window, cx);
        }
    }

    fn pick(&mut self, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .palette
            .as_ref()
            .and_then(|palette| {
                line.filter(|&line| palette.choosable(line))
                    .and_then(|line| palette.row(line))
            })
            .cloned()
        else {
            return;
        };
        if let Some(spec) = self.leave_palette(window, cx) {
            let pick = spec.pick;
            cx.defer_in(window, move |bench, window, cx| {
                pick(&row, bench, window, cx)
            });
        }
    }
}

pub fn overlay(
    bench: &Workbench,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let palette = bench.palette.as_ref()?;
    let t = Tokens::get(cx).clone();
    let count = palette.lines.len();
    let deck = palette.spec.deck;
    let list_pad = if deck { 6. } else { 4. };
    let lines_height: f32 = palette.lines.iter().map(|line| line.height()).sum();
    let viewport = window.viewport_size();
    let stage = palette
        .spec
        .on_stage
        .then(|| bench.stage_frame.get())
        .filter(|stage| stage.size.width > px(0.));
    let close = cx.listener(|bench: &mut Workbench, _, window, cx| bench.close_palette(window, cx));
    Some(
        div()
            .id("palette")
            .absolute()
            .map(|backdrop| match stage {
                Some(stage) => backdrop
                    .left(stage.origin.x)
                    .top(stage.origin.y)
                    .w(stage.size.width)
                    .h(stage.size.height),
                None => backdrop.inset_0(),
            })
            .flex()
            .justify_center()
            .items_start()
            .pt(viewport.height * 0.13)
            .bg(Hsla {
                a: 0.68,
                ..t.surface_sunken
            })
            .occlude()
            .on_mouse_down(MouseButton::Left, close)
            .child(
                ui::float(cx)
                    .key_context("Palette Modal")
                    .w(px(palette.spec.width))
                    .map(|panel| match (deck, stage) {
                        (true, _) => panel
                            .max_w(viewport.width * 0.9)
                            .max_h(px(680.).min(viewport.height * 0.72)),
                        (false, Some(stage)) => panel
                            .max_w(stage.size.width - px(32.))
                            .max_h(viewport.height * 0.62),
                        (false, None) => panel
                            .max_w(viewport.width * 0.86)
                            .max_h(viewport.height * 0.62),
                    })
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .when(palette.spec.numbered, |float| {
                        float.capture_key_down(cx.listener(|bench, event, window, cx| {
                            bench.pick_numbered(event, window, cx)
                        }))
                    })
                    .on_action(
                        cx.listener(|bench, _: &Next, window, cx| {
                            bench.step_palette(1, window, cx)
                        }),
                    )
                    .on_action(cx.listener(|bench, _: &Previous, window, cx| {
                        bench.step_palette(-1, window, cx)
                    }))
                    .on_action(cx.listener(|bench, _: &PageDown, window, cx| {
                        bench.step_palette(PAGE, window, cx)
                    }))
                    .on_action(cx.listener(|bench, _: &PageUp, window, cx| {
                        bench.step_palette(-PAGE, window, cx)
                    }))
                    .on_action(cx.listener(|bench, _: &Choose, window, cx| {
                        let selected = bench.palette.as_ref().and_then(|palette| palette.selected);
                        bench.pick(selected, window, cx);
                    }))
                    .on_action(
                        cx.listener(|bench, _: &Close, window, cx| bench.close_palette(window, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(16.))
                            .py(px(14.))
                            .border_b_1()
                            .border_color(t.border)
                            .child(ui::icon(Icon::Search, 15.).text_color(t.text_tertiary))
                            .child(
                                div()
                                    .flex_1()
                                    .h(crate::theme::ui_text(20.))
                                    .flex()
                                    .items_center()
                                    .child(
                                        Input::new(&palette.input)
                                            .appearance(false)
                                            .flex_1()
                                            .p_0()
                                            .text_size(crate::theme::ui_text(14.))
                                            .text_color(t.text_primary),
                                    ),
                            )
                            .children(palette.spec.aside.as_ref().map(|aside| {
                                let press = aside.press.clone();
                                ui::IconButton::new(
                                    "palette-aside",
                                    aside.icon,
                                    aside.label.clone(),
                                )
                                .on_click(
                                    cx.listener(move |bench, _, window, cx| {
                                        press(bench, window, cx)
                                    }),
                                )
                            }))
                            .children(
                                palette
                                    .spec
                                    .choice
                                    .clone()
                                    .map(|choice| mode_switch(choice, cx)),
                            ),
                    )
                    .map(|panel| {
                        if let Some((failure, retry)) = palette.failed.clone() {
                            return panel.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .pb(px(20.))
                                    .child(crate::plugin_pane::failure(&failure, cx))
                                    .child(
                                        ui::Button::new(
                                            "palette-retry",
                                            "Try again",
                                            ui::Variant::Outline,
                                        )
                                        .on_click(
                                            cx.listener(move |bench, _, window, cx| {
                                                retry(bench, window, cx)
                                            }),
                                        ),
                                    ),
                            );
                        }
                        if count == 0 {
                            return panel.child(
                                div()
                                    .p(px(28.))
                                    .text_center()
                                    .text_color(t.text_tertiary)
                                    .child(palette.spec.empty.clone()),
                            );
                        }
                        let bench = cx.entity();
                        panel.child(
                            list(palette.list.clone(), move |line, _, cx| {
                                bench.update(cx, |bench, cx| line_of(bench, line, cx))
                            })
                            .h(px(lines_height + 2. * list_pad))
                            .min_h_0()
                            .flex_shrink(1.)
                            .py(px(list_pad))
                            .px(px(6.)),
                        )
                    }),
            )
            .into_any_element(),
    )
}

fn line_of(bench: &Workbench, line: usize, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(palette) = bench.palette.as_ref() else {
        return div().into_any_element();
    };
    let row = match palette.lines.get(line) {
        Some(Line::Heading(place)) => {
            let group = palette.spec.rows[*place].group.clone().unwrap_or_default();
            return div()
                .h(px(ROW_HEIGHT))
                .px(px(8.))
                .flex()
                .items_end()
                .pb(px(4.))
                .child(ui::caps(&group, cx))
                .into_any_element();
        }
        Some(Line::Row(place)) => &palette.spec.rows[*place],
        Some(Line::Rule) => {
            return div()
                .w_full()
                .h(px(RULE_HEIGHT))
                .px(px(12.))
                .flex()
                .items_center()
                .child(div().h(px(1.)).flex_1().bg(t.border))
                .into_any_element();
        }
        None => return div().into_any_element(),
    };
    let armed = palette.spec.choice.as_ref().is_some_and(Choice::armed_now);
    let meta: SharedString = match row.unarmed && armed {
        true => "opens safe".into(),
        false => row.meta.clone(),
    };
    let selected = palette.selected == Some(line);
    let ink = if row.disabled {
        t.text_tertiary
    } else if selected {
        t.text_primary
    } else {
        t.text_secondary
    };
    div()
        .id(("palette-line", line))
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(ROW_HEIGHT))
        .px(px(8.))
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(13.))
        .text_color(ink)
        .when(selected, |element| element.bg(t.surface_chosen))
        .when(!row.disabled, |element| {
            element
                .on_mouse_move(cx.listener(move |bench, _, window, cx| {
                    let Some(palette) = bench
                        .palette
                        .as_mut()
                        .filter(|palette| palette.selected != Some(line))
                    else {
                        return;
                    };
                    palette.selected = Some(line);
                    bench.browse(window, cx);
                    bench.parts.redraw(&bench.parts.finder, cx);
                }))
                .on_click(
                    cx.listener(move |bench, _, window, cx| bench.pick(Some(line), window, cx)),
                )
        })
        .when_some(palette.number_of(line, cx), |element, number| {
            element.child(ui::kbd(number.to_string(), cx).flex_none())
        })
        .when_some(row.mark.clone(), |element, mark| {
            element.child(
                div()
                    .w(px(18.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(match mark {
                        Mark::Icon(glyph) => ui::picture(glyph, 14., t.text_tertiary),
                        Mark::Agent(provider) => ui::mark(&provider, 14.),
                        Mark::Open(agent) => ui::agent_mark(&agent, 14., cx),
                    }),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(row.title.clone()),
        )
        .when_some(row.handle.clone(), |element, handle| {
            element.child(ui::meta(handle, cx).flex_none())
        })
        .map(|element| {
            match row
                .state
                .and_then(|state| ui::status_mark(&row.key, state, cx))
            {
                Some(state) => element.child(div().flex_none().child(state)),
                None if meta.is_empty() => element,
                None if palette.spec.deck => element.child(
                    ui::text(meta.to_uppercase(), 10., FontWeight::SEMIBOLD)
                        .letter_spacing(0.08)
                        .text_color(t.text_tertiary)
                        .flex_none(),
                ),
                None => element.child(ui::meta(meta, cx).flex_none()),
            }
        })
        .when_some(row.keys.clone(), |element, keys| {
            element.child(match palette.spec.deck {
                true => keycap(keys, selected, cx),
                false => ui::kbd(keys, cx).flex_none().pl(px(6.)),
            })
        })
        .into_any_element()
}

/// A shortcut in a key's box, inked as the row is.
fn keycap(keys: SharedString, selected: bool, cx: &App) -> gpui_kit::Div {
    let t = Tokens::get(cx);
    ui::kbd(keys, cx)
        .flex_none()
        .min_w(px(40.))
        .px(px(6.))
        .py(px(1.))
        .line_height(theme::ui_text(13.))
        .flex()
        .justify_center()
        .border_1()
        .border_color(t.border)
        .bg(t.surface_sunken)
        .rounded(theme::RADIUS_1)
        .when(selected, |keycap| keycap.text_color(t.text_primary))
}

/// The choice beside the field as main's safety switch draws it: a sunken box of tracked mono
/// capitals, the chosen one raised, the armed one in the rainbow while chosen.
fn mode_switch(choice: Choice, cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let picked = choice.chosen.get();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_2)
        .bg(Hsla {
            a: t.surface_sunken.a * 0.46,
            ..t.surface_sunken
        })
        .children(
            choice
                .options
                .iter()
                .enumerate()
                .map(|(at, (glyph, word))| {
                    let on = at == picked;
                    let armed = on && choice.armed == Some(at);
                    let chosen = choice.chosen.clone();
                    let ink = if on { t.text_primary } else { t.text_secondary };
                    div()
                        .id(("palette-choice", at))
                        .h(px(23.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .rounded(theme::RADIUS_2)
                        .text_color(ink)
                        .when(on, |option| {
                            option
                                .bg(t.surface_raised)
                                .border_1()
                                .border_color(t.border)
                        })
                        .when(!on, |option| {
                            option.hover(move |style| style.text_color(t.text_primary))
                        })
                        .on_click(cx.listener(move |bench, _, window, cx| {
                            chosen.set(at);
                            if let Some(palette) = &bench.palette {
                                let input = palette.input.clone();
                                input.update(cx, |input, cx| input.focus(window, cx));
                            }
                            bench.parts.redraw(&bench.parts.finder, cx);
                        }))
                        .child(ui::icon(*glyph, 12.).text_color(if armed { t.accent } else { ink }))
                        .child(
                            match armed {
                                true => ui::rainbow(&word.to_uppercase(), 10., cx),
                                false => ui::text(word.to_uppercase(), 10., FontWeight::SEMIBOLD)
                                    .letter_spacing(0.08),
                            }
                            .font_family(theme::MONO),
                        )
                }),
        )
}
