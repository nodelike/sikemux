//! A DBML file drawn as its tables on a canvas that pans and zooms, with a line from each
//! column that refers to another to the column it names.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, point, px, quad, App, BorderStyle, Bounds, ContentMask, Context, Corners,
    CursorStyle, DispatchPhase, Font, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    PathBuilder, PinchEvent, Pixels, Point, Render, RenderImage, ScrollEvent, SharedString,
    StatefulInteractiveElement, Styled, TextAlign, TextRun, WeakEntity, Window,
};

use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::dbml::{self, Ref, Schema};
use crate::core::{self, Ask};
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_0, SPACE_2, TEXT_2XS, TEXT_SM};
use crate::ui::IconButton;

const HEADER: f32 = 36.;
const ROW: f32 = 28.;
const FOOT: f32 = 6.;
const PAD: f32 = 14.;
const KEY_SPACE: f32 = 18.;
const MIN_WIDTH: f32 = 200.;
const MAX_WIDTH: f32 = 440.;
const NAME_SIZE: f32 = 13.;
const COLUMN_SIZE: f32 = 12.5;
const TYPE_SIZE: f32 = 11.5;
const RADIUS: f32 = 8.;
const GAP_X: f32 = 140.;
const GAP_Y: f32 = 44.;
const STUB: f32 = 24.;
/// How tall a column of tables may grow before the rest start a column beside it.
const TALLEST_STACK: f32 = 900.;
const MIN_ZOOM: f32 = 0.12;
const MAX_ZOOM: f32 = 2.5;
/// Below this zoom a table shows only its name and nothing of its columns.
const BARS_ZOOM: f32 = 0.25;
/// Below this zoom a table shows only its name, its columns as quiet bars.
const DETAIL_ZOOM: f32 = 0.45;
const MAP_WIDTH: f32 = 176.;
const MAP_HEIGHT: f32 = 116.;
const MAP_MARGIN: f32 = 12.;
const GROUP_PAD: f32 = 26.;
/// Room above a group's tables for its name.
const GROUP_LABEL: f32 = 18.;
/// How many measured widths a diagram keeps before it starts again.
const MEASURED_MAX: usize = 4096;
const EMBEDDED_HEIGHT: std::ops::Range<f32> = 240.0..560.0;

/// Where a table sits and how big it is, in the diagram's own units.
#[derive(Clone, Copy, Default)]
struct Card {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// How wide the table's name is at its own size.
    name_w: f32,
    placed: bool,
}

impl Card {
    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn contains(&self, at: Point<f32>) -> bool {
        at.x >= self.x && at.x <= self.right() && at.y >= self.y && at.y <= self.bottom()
    }
}

/// What the pointer is over: a table, and the column in it when it is over one.
#[derive(Clone, Copy, PartialEq)]
struct Spot {
    table: usize,
    column: Option<usize>,
}

enum Drag {
    Pan {
        from: Point<Pixels>,
        pan: Point<f32>,
    },
    Card {
        table: usize,
        from: Point<Pixels>,
        at: Point<f32>,
    },
    Map,
}

/// Shows the file's text at a line, counted from 0.
pub(crate) type OpenLine = Rc<dyn Fn(u32, &mut Window, &mut App)>;

/// How long after the last move, pan or zoom the arrangement is kept, so a drag writes once.
const SAVE_AFTER: Duration = Duration::from_millis(400);
/// How long a diagram waits for its kept arrangement before it lays itself out.
const LOAD_WAIT: Duration = Duration::from_millis(800);

/// Where the core keeps a diagram's arrangement across restarts.
pub(crate) struct Kept {
    pub asks: UnboundedSender<Ask>,
    pub key: KeptKey,
}

pub(crate) enum KeptKey {
    /// One arrangement, such as a file's, whatever its text.
    Fixed(String),
    /// An arrangement for each text under this prefix, such as a chat's blocks.
    ByText(String),
}

pub(crate) struct Diagram {
    schema: Rc<Schema>,
    /// The row each reference meets at either end, from and to.
    anchors: Rc<Vec<Anchors>>,
    cards: Vec<Card>,
    /// The tables in the order they are drawn, the last on top.
    order: Vec<usize>,
    measured: bool,
    /// Where each table was, by name, so a table keeps its place when the file is read again.
    kept: HashMap<String, Point<f32>>,
    /// Where the diagram's origin is drawn, from the canvas's top left corner.
    pan: Point<f32>,
    zoom: f32,
    viewport: Rc<Cell<Bounds<Pixels>>>,
    fit_pending: bool,
    /// The canvas's size when the tables were last fitted to it.
    fitted: gpui_kit::Size<Pixels>,
    /// A new canvas size seen once; the tables are fitted to it only if the next frame has it
    /// too, so a size a layout holds for a single frame never moves them.
    settling: Option<gpui_kit::Size<Pixels>>,
    fitted_once: bool,
    /// Whether the next fit follows the text growing, which only ever zooms out, so a schema an
    /// agent is writing does not pulse in and out.
    growing: bool,
    /// The tallest an embedded diagram has drawn at its width, so it never shrinks as its text
    /// grows; with the width it was measured at.
    tallest: (f32, f32),
    latch: gpui_transcript::scroll_latch::ScrollLatch,
    hover: Option<Spot>,
    pointer: Option<Point<Pixels>>,
    drag: Option<Drag>,
    open: Option<OpenLine>,
    /// Drawn inside other content, such as a chat, as tall as its tables need.
    embedded: bool,
    /// Whether the person moved, panned or zoomed it; until they do, it lays itself out
    /// again whenever its text changes.
    touched: bool,
    /// How wide each name and type measured, so text that grows as an agent writes it is
    /// measured once rather than on every change.
    widths: HashMap<(String, u8), f32>,
    /// The text it was last drawn from.
    source: String,
    kept_by: Option<Kept>,
    /// Waiting for the kept arrangement, which it is drawn with once it comes.
    loading: bool,
    /// When the arrangement is next kept, while a move has not been kept yet.
    save_at: Option<Instant>,
}

impl Diagram {
    /// The diagram of `source`; `open` shows the file's text at a line.
    pub(super) fn new(source: &str, open: OpenLine) -> Self {
        Self::with(source, Some(open), false)
    }

    /// The diagram of `source` drawn inside other content.
    pub(crate) fn embedded(source: &str) -> Self {
        Self::with(source, None, true)
    }

    fn with(source: &str, open: Option<OpenLine>, embedded: bool) -> Self {
        let mut diagram = Diagram {
            schema: Rc::new(Schema::default()),
            anchors: Rc::default(),
            cards: Vec::new(),
            order: Vec::new(),
            measured: false,
            kept: HashMap::new(),
            pan: point(0., 0.),
            zoom: 1.,
            viewport: Rc::new(Cell::new(Bounds::default())),
            fit_pending: true,
            fitted: gpui_kit::Size::default(),
            settling: None,
            fitted_once: false,
            growing: false,
            tallest: (0., 0.),
            latch: Default::default(),
            hover: None,
            pointer: None,
            drag: None,
            open,
            embedded,
            touched: false,
            widths: HashMap::new(),
            source: String::new(),
            kept_by: None,
            loading: false,
            save_at: None,
        };
        diagram.read(source);
        diagram
    }

    /// Keeps where the person moves the tables with the core, and draws the arrangement kept
    /// there when one is, waiting a moment for it before laying the tables out.
    pub(crate) fn kept_by(&mut self, kept: Kept, cx: &mut Context<Self>) {
        let key = self.key_for(&kept.key);
        let asked = core::request(&kept.asks, "diagramLayout", json!({ "key": key }));
        self.kept_by = Some(kept);
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let answer = asked.await.ok();
            let _ = this.update(cx, |diagram, cx| {
                if !diagram.loading {
                    return;
                }
                let layout = answer.and_then(|answer| answer.get("layout").cloned());
                if let Some(layout) = layout.filter(|layout| !layout.is_null()) {
                    diagram.restore(&layout);
                }
                diagram.loading = false;
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LOAD_WAIT).await;
            let _ = this.update(cx, |diagram, cx| {
                if std::mem::take(&mut diagram.loading) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn key_for(&self, key: &KeptKey) -> String {
        match key {
            KeptKey::Fixed(key) => key.clone(),
            KeptKey::ByText(prefix) => format!("{prefix}:{:016x}", fnv(&self.source)),
        }
    }

    /// Draws the tables where the kept arrangement put them, at its pan and zoom.
    fn restore(&mut self, layout: &Value) {
        let Some(tables) = layout["tables"].as_object() else {
            return;
        };
        self.kept = (tables.iter())
            .filter_map(|(name, at)| {
                let at = at.as_array()?;
                Some((
                    name.clone(),
                    point(at.first()?.as_f64()? as f32, at.get(1)?.as_f64()? as f32),
                ))
            })
            .collect();
        let source = std::mem::take(&mut self.source);
        self.read(&source);
        self.touched = true;
        let pan = layout["pan"].as_array();
        let zoom = layout["zoom"].as_f64();
        if let (Some(pan), Some(zoom)) = (pan, zoom) {
            let x = pan.first().and_then(Value::as_f64).unwrap_or_default();
            let y = pan.get(1).and_then(Value::as_f64).unwrap_or_default();
            self.pan = point(x as f32, y as f32);
            self.zoom = (zoom as f32).clamp(MIN_ZOOM, MAX_ZOOM);
            self.fit_pending = false;
            self.fitted_once = true;
        }
    }

    /// Keeps the arrangement once the person has stopped moving it for a moment.
    fn remember(&mut self, cx: &mut Context<Self>) {
        if self.kept_by.is_none() {
            return;
        }
        let waiting = self.save_at.is_some();
        self.save_at = Some(Instant::now() + SAVE_AFTER);
        if waiting {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            let due = this.update(cx, |diagram, _| diagram.save_at).ok().flatten();
            let Some(due) = due else {
                return;
            };
            let now = Instant::now();
            if now < due {
                cx.background_executor().timer(due - now).await;
                continue;
            }
            let _ = this.update(cx, |diagram, _| {
                diagram.save_at = None;
                diagram.keep(true);
            });
            return;
        })
        .detach();
    }

    /// Tells the core where the tables are, or that there is no arrangement to keep.
    fn keep(&self, arranged: bool) {
        let Some(kept) = &self.kept_by else {
            return;
        };
        let layout = arranged.then(|| {
            let tables: serde_json::Map<String, Value> = (self.schema.tables.iter())
                .zip(&self.cards)
                .map(|(table, card)| (table.name.clone(), json!([card.x.round(), card.y.round()])))
                .collect();
            json!({
                "tables": tables,
                "pan": [self.pan.x.round(), self.pan.y.round()],
                "zoom": self.zoom,
            })
        });
        let key = self.key_for(&kept.key);
        let _ = kept.asks.send(Ask::Call {
            method: "setDiagramLayout",
            params: json!({ "key": key, "layout": layout }),
        });
    }

    pub(crate) fn set_source(&mut self, source: &str, cx: &mut Context<Self>) {
        match self.touched {
            true => {
                for (table, card) in self.schema.tables.iter().zip(&self.cards) {
                    self.kept.insert(table.name.clone(), point(card.x, card.y));
                }
            }
            false => {
                self.kept.clear();
                self.fit_pending = true;
                self.growing = true;
            }
        }
        self.read(source);
        cx.notify();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.schema.tables.is_empty()
    }

    fn read(&mut self, source: &str) {
        self.source = source.to_owned();
        self.schema = Rc::new(dbml::parse(source));
        self.anchors = Rc::new(anchors(&self.schema));
        self.cards = (self.schema.tables.iter())
            .map(|table| match self.kept.get(&table.name) {
                Some(at) => Card {
                    x: at.x,
                    y: at.y,
                    placed: true,
                    ..Card::default()
                },
                None => Card::default(),
            })
            .collect();
        self.order = (0..self.cards.len()).collect();
        self.measured = false;
        self.hover = None;
        self.drag = None;
    }

    /// Sizes each table to its longest line, then places the tables that have no place yet.
    fn measure(&mut self, window: &mut Window) {
        if self.widths.len() > MEASURED_MAX {
            self.widths.clear();
        }
        let widths = &mut self.widths;
        let mut width = |text: &str, kind: u8, font: Font, size: f32| -> f32 {
            if text.is_empty() {
                return 0.;
            }
            if let Some(width) = widths.get(&(text.to_owned(), kind)) {
                return *width;
            }
            let run = TextRun {
                len: text.len(),
                font,
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                SharedString::from(text.to_string()),
                px(size),
                &[run],
                None,
            );
            let width = f32::from(line.width);
            widths.insert((text.to_owned(), kind), width);
            width
        };
        for (table, card) in self.schema.tables.iter().zip(self.cards.iter_mut()) {
            let dot = if table.color.is_some() { 14. } else { 0. };
            card.name_w = width(&table.name, 0, name_font(), NAME_SIZE);
            let mut w = PAD * 2. + dot + card.name_w + 28.;
            for column in &table.columns {
                let line = PAD * 2.
                    + KEY_SPACE
                    + width(
                        &column.name,
                        1 + column.pk as u8,
                        column_font(column.pk),
                        COLUMN_SIZE,
                    )
                    + 28.
                    + width(&column.kind, 3, type_font(), TYPE_SIZE);
                w = w.max(line);
            }
            card.w = w.clamp(MIN_WIDTH, MAX_WIDTH).ceil();
            card.h = HEADER + table.columns.len() as f32 * ROW + FOOT;
        }
        self.measured = true;
        if self.cards.iter().all(|card| !card.placed) {
            arrange(&self.schema, &mut self.cards);
        } else {
            place_new(&mut self.cards);
        }
    }

    fn to_world(&self, at: Point<Pixels>) -> Point<f32> {
        let origin = self.viewport.get().origin;
        point(
            (f32::from(at.x - origin.x) - self.pan.x) / self.zoom,
            (f32::from(at.y - origin.y) - self.pan.y) / self.zoom,
        )
    }

    fn spot_at(&self, at: Point<Pixels>) -> Option<Spot> {
        let world = self.to_world(at);
        let table = *(self.order.iter())
            .rev()
            .find(|&&index| self.cards[index].contains(world))?;
        let card = self.cards[table];
        let row = ((world.y - card.y - HEADER) / ROW).floor();
        let column = (row >= 0. && (row as usize) < self.schema.tables[table].columns.len())
            .then_some(row as usize);
        Some(Spot { table, column })
    }

    /// Whether pointing at `spot` shows a tip, which follows the pointer.
    fn tells(&self, spot: Spot) -> bool {
        let table = &self.schema.tables[spot.table];
        match spot.column.and_then(|column| table.columns.get(column)) {
            Some(column) => {
                column.pk
                    || column.not_null
                    || column.unique
                    || column.increment
                    || column.default.is_some()
                    || column.note.is_some()
            }
            None => table.note.is_some(),
        }
    }

    fn zoom_at(&mut self, at: Point<Pixels>, factor: f32, cx: &mut Context<Self>) {
        let world = self.to_world(at);
        let origin = self.viewport.get().origin;
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.touched = true;
        self.remember(cx);
        self.pan = point(
            f32::from(at.x - origin.x) - world.x * self.zoom,
            f32::from(at.y - origin.y) - world.y * self.zoom,
        );
        cx.notify();
    }

    fn zoom_centre(&mut self, factor: f32, cx: &mut Context<Self>) {
        let bounds = self.viewport.get();
        self.zoom_at(bounds.center(), factor, cx);
    }

    fn fit(&mut self, cx: &mut Context<Self>) {
        let size = self.viewport.get().size;
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        let Some(extent) = extent(&self.cards) else {
            self.pan = point(width / 2., height / 2.);
            return;
        };
        let (w, h) = (extent.2 - extent.0, extent.3 - extent.1);
        let margin = self.margin();
        let zoom = ((width - margin) / w)
            .min((height - margin) / h)
            .clamp(MIN_ZOOM, 1.);
        self.zoom = match self.growing && self.fitted_once {
            true => zoom.min(self.zoom),
            false => zoom,
        };
        self.growing = false;
        self.fitted_once = true;
        self.settling = None;
        self.pan = point(
            width / 2. - (extent.0 + w / 2.) * self.zoom,
            height / 2. - (extent.1 + h / 2.) * self.zoom,
        );
        self.fit_pending = false;
        self.fitted = size;
        cx.notify();
    }

    /// Room left around the tables when they are fitted to the canvas.
    fn margin(&self) -> f32 {
        if self.embedded {
            56.
        } else {
            96.
        }
    }

    /// How tall an embedded diagram draws: as tall as its tables need at the width it has,
    /// within bounds a reader can scroll past.
    pub(crate) fn height(&self) -> f32 {
        let width = f32::from(self.viewport.get().size.width);
        let Some(extent) = extent(&self.cards).filter(|_| width > 0.) else {
            return EMBEDDED_HEIGHT.start.max(self.tallest.1);
        };
        let zoom = ((width - self.margin()) / (extent.2 - extent.0)).min(1.);
        let needed = ((extent.3 - extent.1) * zoom + self.margin() + 40.)
            .clamp(EMBEDDED_HEIGHT.start, EMBEDDED_HEIGHT.end);
        match self.tallest.0 == width {
            true => needed.max(self.tallest.1),
            false => needed,
        }
    }

    fn rearrange(&mut self, cx: &mut Context<Self>) {
        arrange(&self.schema, &mut self.cards);
        self.kept.clear();
        self.touched = false;
        self.save_at = None;
        self.keep(false);
        self.fit(cx);
    }

    fn map(&self) -> Option<Map> {
        if self.embedded {
            return None;
        }
        minimap(&self.cards, self.viewport.get(), self.pan, self.zoom)
    }

    fn centre_on_map(&mut self, at: Point<Pixels>, map: &Map, cx: &mut Context<Self>) {
        let size = self.viewport.get().size;
        let world = map.to_world(at);
        self.pan = point(
            f32::from(size.width) / 2. - world.x * self.zoom,
            f32::from(size.height) / 2. - world.y * self.zoom,
        );
        cx.notify();
    }

    fn press(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) -> Option<u32> {
        if let Some(map) = self.map().filter(|map| map.frame.contains(&event.position)) {
            self.drag = Some(Drag::Map);
            self.centre_on_map(event.position, &map, cx);
            return None;
        }
        let spot = self.spot_at(event.position);
        if event.click_count == 2 {
            match spot {
                Some(spot) => return Some(self.schema.tables[spot.table].line),
                None => self.fit(cx),
            }
            return None;
        }
        self.drag = Some(match spot {
            Some(spot) => {
                let card = self.cards[spot.table];
                self.order.retain(|table| *table != spot.table);
                self.order.push(spot.table);
                Drag::Card {
                    table: spot.table,
                    from: event.position,
                    at: point(card.x, card.y),
                }
            }
            None => Drag::Pan {
                from: event.position,
                pan: self.pan,
            },
        });
        cx.notify();
        None
    }

    fn moved(&mut self, event: &MouseMoveEvent, hovered: bool, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) && self.drag.is_some() {
            self.drag = None;
            cx.notify();
        }
        match &self.drag {
            Some(Drag::Pan { from, pan }) => {
                self.touched = true;
                self.pan = point(
                    pan.x + f32::from(event.position.x - from.x),
                    pan.y + f32::from(event.position.y - from.y),
                );
                cx.notify();
            }
            Some(Drag::Card { table, from, at }) => {
                self.touched = true;
                let card = &mut self.cards[*table];
                card.x = at.x + f32::from(event.position.x - from.x) / self.zoom;
                card.y = at.y + f32::from(event.position.y - from.y) / self.zoom;
                cx.notify();
            }
            Some(Drag::Map) => {
                if let Some(map) = self.map() {
                    self.centre_on_map(event.position, &map, cx);
                }
            }
            None => {
                let spot = hovered.then(|| self.spot_at(event.position)).flatten();
                let pointer = hovered.then_some(event.position);
                let tip_moves =
                    pointer != self.pointer && spot.is_some_and(|spot| self.tells(spot));
                if spot != self.hover || tip_moves {
                    self.hover = spot;
                    self.pointer = pointer;
                    cx.notify();
                }
            }
        }
    }
}

impl Render for Diagram {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.measured {
            self.measure(window);
        }
        let size = self.viewport.get().size;
        let resized = !self.touched && size != self.fitted;
        if size.width > px(0.) {
            if self.fit_pending || resized && self.settling == Some(size) {
                self.fit(cx);
            } else if resized {
                self.settling = Some(size);
                cx.notify();
            }
        }
        if self.embedded {
            let width = f32::from(size.width);
            self.tallest = (width, self.height());
        }
        let t = Tokens::get(cx).clone();
        if self.schema.tables.is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(theme::ui_text(TEXT_SM))
                .text_color(t.text_tertiary)
                .child("No tables in this file yet")
                .into_any_element();
        }
        let frame = Frame {
            schema: self.schema.clone(),
            anchors: self.anchors.clone(),
            cards: self.cards.clone(),
            order: self.order.clone(),
            pan: self.pan,
            zoom: self.zoom,
            hover: self.hover.filter(|_| self.drag.is_none()),
            pointer: self.pointer,
            dragging: matches!(self.drag, Some(Drag::Card { .. } | Drag::Pan { .. })),
            waiting: self.fit_pending,
            hidden: self.loading,
            lanes: lanes(&self.schema, &self.anchors, &self.cards),
            map: !self.embedded,
            t: t.clone(),
        };
        let this = cx.entity().downgrade();
        let shared = self.viewport.clone();
        let summary = format!(
            "{} {} · {} {}",
            self.schema.tables.len(),
            if self.schema.tables.len() == 1 {
                "table"
            } else {
                "tables"
            },
            self.schema.refs.len(),
            if self.schema.refs.len() == 1 {
                "relationship"
            } else {
                "relationships"
            },
        );
        let percent = format!("{}%", (self.zoom * 100.).round());
        let height = self.embedded.then(|| self.height());
        div()
            .relative()
            .w_full()
            .map(|diagram| match height {
                Some(height) => diagram.h(px(height)),
                None => diagram.h_full(),
            })
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, _| {
                        shared.set(bounds);
                        window.insert_hitbox(bounds, HitboxBehavior::Normal)
                    },
                    move |bounds, hitbox, window, cx| {
                        listen(&this, &hitbox, window);
                        window.set_cursor_style(
                            if frame.dragging {
                                CursorStyle::ClosedHand
                            } else {
                                CursorStyle::OpenHand
                            },
                            &hitbox,
                        );
                        if frame.hidden {
                            return;
                        }
                        if frame.waiting {
                            let this = this.clone();
                            cx.defer(move |cx| {
                                let _ = this.update(cx, |_, cx| cx.notify());
                            });
                            return;
                        }
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            frame.paint(bounds, window, cx)
                        });
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top(px(10.))
                    .left(px(12.))
                    .text_size(theme::ui_text(TEXT_2XS))
                    .text_color(t.text_tertiary)
                    .child(summary),
            )
            .child(
                div()
                    .id("diagram-controls")
                    .occlude()
                    .absolute()
                    .bottom(px(MAP_MARGIN))
                    .right(px(MAP_MARGIN))
                    .flex()
                    .items_center()
                    .gap(SPACE_0)
                    .p(px(3.))
                    .rounded(theme::RADIUS_3)
                    .bg(t.surface_overlay)
                    .border_1()
                    .border_color(t.border)
                    .shadow(t.shadow_float())
                    .text_size(theme::ui_text(TEXT_2XS))
                    .text_color(t.text_secondary)
                    .child(
                        IconButton::new("diagram-zoom-out", Icon::Minus, "Zoom out")
                            .compact()
                            .on_click(
                                cx.listener(|diagram, _, _, cx| diagram.zoom_centre(1. / 1.25, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("diagram-zoom-reset")
                            .w(px(44.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme::RADIUS_2)
                            .cursor_pointer()
                            .hover(|reset| reset.bg(t.surface_hover))
                            .tooltip(crate::ui::tooltip("Actual size"))
                            .on_click(cx.listener(|diagram, _, _, cx| {
                                let factor = 1. / diagram.zoom;
                                diagram.zoom_centre(factor, cx)
                            }))
                            .child(percent),
                    )
                    .child(
                        IconButton::new("diagram-zoom-in", Icon::Plus, "Zoom in")
                            .compact()
                            .on_click(
                                cx.listener(|diagram, _, _, cx| diagram.zoom_centre(1.25, cx)),
                            ),
                    )
                    .child(div().w(px(1.)).h(px(14.)).mx(px(3.)).bg(t.border))
                    .child(
                        text_button("diagram-fit", "Fit", "Show every table", &t)
                            .on_click(cx.listener(|diagram, _, _, cx| diagram.fit(cx))),
                    )
                    .child(
                        text_button("diagram-arrange", "Arrange", "Lay the tables out again", &t)
                            .on_click(cx.listener(|diagram, _, _, cx| diagram.rearrange(cx))),
                    ),
            )
            .into_any_element()
    }
}

fn text_button(
    id: &'static str,
    label: &'static str,
    tip: &'static str,
    t: &Tokens,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .px(SPACE_2)
        .h(px(24.))
        .flex()
        .items_center()
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .hover(|button| button.bg(t.surface_hover).text_color(t.text_primary))
        .tooltip(crate::ui::tooltip(tip))
        .child(label)
}

/// Hears the pointer over the canvas `hitbox`: dragging pans or moves a table, a scroll that
/// begins over it pans, and pinching or scrolling with ⌘ zooms around the pointer.
fn listen(this: &WeakEntity<Diagram>, hitbox: &Hitbox, window: &mut Window) {
    window.on_mouse_event({
        let (this, hitbox) = (this.clone(), hitbox.clone());
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            cx.stop_propagation();
            let line = this
                .update(cx, |diagram, cx| diagram.press(event, cx))
                .ok()
                .flatten();
            let open = this
                .upgrade()
                .and_then(|diagram| diagram.read(cx).open.clone());
            if let Some((line, open)) = line.zip(open) {
                open(line, window, cx);
            }
        }
    });
    window.on_mouse_event({
        let (this, hitbox) = (this.clone(), hitbox.clone());
        move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let hovered = hitbox.is_hovered(window);
            let _ = this.update(cx, |diagram, cx| diagram.moved(event, hovered, cx));
        }
    });
    window.on_mouse_event({
        let this = this.clone();
        move |event: &MouseUpEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            let _ = this.update(cx, |diagram, cx| {
                if diagram.drag.take().is_some() {
                    if diagram.touched {
                        diagram.remember(cx);
                    }
                    cx.notify();
                }
            });
        }
    });
    window.on_mouse_event({
        let (this, hitbox) = (this.clone(), hitbox.clone());
        move |event: &ScrollEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let over = hitbox.is_hovered(window);
            let delta = event.delta.pixel_delta(px(20.));
            let took = this.update(cx, |diagram, cx| {
                if !diagram.latch.takes(event, over, |_| true) {
                    return false;
                }
                if event.modifiers.platform || event.modifiers.control {
                    let factor = (-f32::from(delta.y) / 240.).exp();
                    diagram.zoom_at(event.position, factor, cx);
                } else {
                    diagram.touched = true;
                    diagram.pan.x += f32::from(delta.x);
                    diagram.pan.y += f32::from(delta.y);
                    diagram.remember(cx);
                    cx.notify();
                }
                true
            });
            if took.unwrap_or(false) {
                cx.stop_propagation();
            }
        }
    });
    window.on_mouse_event({
        let (this, hitbox) = (this.clone(), hitbox.clone());
        move |event: &PinchEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                return;
            }
            cx.stop_propagation();
            let _ = this.update(cx, |diagram, cx| {
                diagram.zoom_at(event.position, 1. + event.delta, cx)
            });
        }
    });
}

/// What one frame draws, taken from the diagram when it renders.
struct Frame {
    schema: Rc<Schema>,
    anchors: Rc<Vec<Anchors>>,
    cards: Vec<Card>,
    order: Vec<usize>,
    pan: Point<f32>,
    zoom: f32,
    hover: Option<Spot>,
    pointer: Option<Point<Pixels>>,
    dragging: bool,
    waiting: bool,
    /// Its kept arrangement has not come yet, so it draws nothing rather than a layout that
    /// would jump.
    hidden: bool,
    /// Where each reference crossing a gap between tables turns, so lines sharing a gap
    /// take lanes of their own.
    lanes: HashMap<usize, f32>,
    /// Whether it shows the picture of the whole diagram when it does not fit.
    map: bool,
    t: Tokens,
}

impl Frame {
    fn screen(&self, bounds: Bounds<Pixels>, at: Point<f32>) -> Point<Pixels> {
        point(
            bounds.origin.x + px(self.pan.x + at.x * self.zoom),
            bounds.origin.y + px(self.pan.y + at.y * self.zoom),
        )
    }

    fn rect(&self, bounds: Bounds<Pixels>, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: self.screen(bounds, point(x, y)),
            size: gpui_kit::size(px(w * self.zoom), px(h * self.zoom)),
        }
    }

    /// Whether the reference touches what the pointer is over: the column when it names
    /// one that a reference starts or ends at, otherwise the table.
    fn lit(&self, reference: &Ref) -> bool {
        let Some(spot) = self.hover else {
            return false;
        };
        let touches = |end: &dbml::End| {
            end.table == spot.table
                && match spot.column {
                    Some(column) if self.column_linked(spot.table, column) => {
                        end.columns.contains(&column)
                    }
                    _ => true,
                }
        };
        touches(&reference.from) || touches(&reference.to)
    }

    fn column_linked(&self, table: usize, column: usize) -> bool {
        self.schema.refs.iter().any(|reference| {
            [&reference.from, &reference.to]
                .iter()
                .any(|end| end.table == table && end.columns.contains(&column))
        })
    }

    /// Paints what falls inside the window's view of the canvas; a diagram scrolled out of a
    /// chat paints nothing.
    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let seen = window.content_mask().bounds;
        if seen.size.width <= px(0.) || seen.size.height <= px(0.) {
            return;
        }
        self.paint_dots(bounds, seen, window);
        self.paint_groups(bounds, window, cx);
        let lit: Vec<bool> = self.schema.refs.iter().map(|r| self.lit(r)).collect();
        let any_lit = lit.iter().any(|lit| *lit);
        let mut related: HashSet<(usize, Option<usize>)> = HashSet::new();
        for (reference, lit) in self.schema.refs.iter().zip(&lit) {
            if *lit {
                for end in [&reference.from, &reference.to] {
                    related.insert((end.table, None));
                    for column in &end.columns {
                        related.insert((end.table, Some(*column)));
                    }
                }
            }
        }
        let quiet = Hsla {
            a: if any_lit { 0.16 } else { 0.5 },
            ..self.t.text_tertiary
        };
        for (index, lit) in lit.iter().enumerate() {
            if !lit {
                self.paint_wire(bounds, seen, index, quiet, 1.2, window);
            }
        }
        for (index, lit) in lit.iter().enumerate() {
            if *lit {
                self.paint_wire(bounds, seen, index, self.t.accent, 1.8, window);
            }
        }
        let linked: HashSet<(usize, usize)> = (self.schema.refs.iter())
            .flat_map(|reference| {
                let end = &reference.from;
                end.columns.iter().map(move |column| (end.table, *column))
            })
            .collect();
        for &table in &self.order {
            self.paint_card(bounds, seen, table, &related, &linked, window, cx);
        }
        self.paint_tip(bounds, window, cx);
        let map = (self.map).then(|| minimap(&self.cards, bounds, self.pan, self.zoom));
        if let Some(map) = map.flatten() {
            self.paint_map(&map, window);
        }
    }

    fn paint_dots(&self, bounds: Bounds<Pixels>, seen: Bounds<Pixels>, window: &mut Window) {
        let mut step = 24. * self.zoom;
        while step < 12. {
            step *= 2.;
        }
        let dot = Hsla {
            a: self.t.line_soft.a.max(0.08) * 1.6,
            ..self.t.line_soft
        };
        let Some(tile) = dot_tile(dot) else {
            return;
        };
        let span = step * TILE_DOTS as f32;
        let origin = bounds.origin + point(px(self.pan.x - step / 2.), px(self.pan.y - step / 2.));
        let from = seen.origin - origin;
        let first = |offset: Pixels| (f32::from(offset) / span).floor();
        let (columns, rows) = (first(from.x), first(from.y));
        let (right, bottom) = (seen.right(), seen.bottom());
        let mut row = rows;
        loop {
            let y = origin.y + px(row * span);
            if y >= bottom {
                break;
            }
            let mut column = columns;
            loop {
                let x = origin.x + px(column * span);
                if x >= right {
                    break;
                }
                let at = Bounds {
                    origin: point(x, y),
                    size: gpui_kit::size(px(span), px(span)),
                };
                let _ = window.paint_image(at, at, Corners::default(), tile.clone(), 0, false);
                column += 1.;
            }
            row += 1.;
        }
    }

    fn paint_groups(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        for group in &self.schema.groups {
            let Some((left, top, right, bottom)) =
                extent_of(group.tables.iter().map(|&table| &self.cards[table]))
            else {
                continue;
            };
            let tint = group.color.map(rgb).unwrap_or(self.t.text_tertiary);
            let frame = self.rect(
                bounds,
                left - GROUP_PAD,
                top - GROUP_PAD - GROUP_LABEL,
                right - left + GROUP_PAD * 2.,
                bottom - top + GROUP_PAD * 2. + GROUP_LABEL,
            );
            window.paint_quad(quad(
                frame,
                px(14. * self.zoom),
                Hsla { a: 0.06, ..tint },
                px(1.),
                Hsla { a: 0.28, ..tint },
                BorderStyle::Dashed,
            ));
            if self.zoom >= 0.2 {
                let size = (12. * self.zoom).max(10.);
                let origin = frame.origin + point(px(14. * self.zoom), px(10. * self.zoom));
                text(
                    &group.name,
                    name_font(),
                    size,
                    Hsla { a: 0.9, ..tint },
                    origin,
                    size * 1.3,
                    window,
                    cx,
                );
            }
        }
    }

    fn anchor(&self, end: &dbml::End, row: Option<usize>) -> f32 {
        let card = &self.cards[end.table];
        match row {
            Some(row) => card.y + HEADER + (row as f32 + 0.5) * ROW,
            None => card.y + HEADER / 2.,
        }
    }

    /// The corners a reference's line turns at, in the diagram's units, and which way it
    /// leaves each table: 1 to the right, -1 to the left.
    fn route(&self, index: usize) -> (Vec<Point<f32>>, f32, f32) {
        let reference = &self.schema.refs[index];
        let (a, b) = (
            &self.cards[reference.from.table],
            &self.cards[reference.to.table],
        );
        let (from, to) = self.anchors[index];
        let (ya, yb) = (
            self.anchor(&reference.from, from),
            self.anchor(&reference.to, to),
        );
        if reference.from.table == reference.to.table {
            let out = a.right() + STUB * 1.4;
            let points = vec![
                point(a.right(), ya),
                point(out, ya),
                point(out, yb),
                point(a.right(), yb),
            ];
            return (points, 1., 1.);
        }
        let (xa, da, xb, db) = if a.right() + STUB * 2. <= b.x {
            (a.right(), 1., b.x, -1.)
        } else if b.right() + STUB * 2. <= a.x {
            (a.x, -1., b.right(), 1.)
        } else {
            let right = a.right().max(b.right()) + STUB * 1.4;
            let left = a.x.min(b.x) - STUB * 1.4;
            let via_right = (right - a.right()) + (right - b.right());
            let via_left = (a.x - left) + (b.x - left);
            let (xa, xb, out, side) = if via_right <= via_left {
                (a.right(), b.right(), right, 1.)
            } else {
                (a.x, b.x, left, -1.)
            };
            let points = vec![point(xa, ya), point(out, ya), point(out, yb), point(xb, yb)];
            return (points, side, side);
        };
        if (ya - yb).abs() < 0.5 {
            return (vec![point(xa, ya), point(xb, yb)], da, db);
        }
        let mid = self.lanes.get(&index).copied().unwrap_or((xa + xb) / 2.);
        let points = vec![point(xa, ya), point(mid, ya), point(mid, yb), point(xb, yb)];
        (points, da, db)
    }

    fn paint_wire(
        &self,
        bounds: Bounds<Pixels>,
        seen: Bounds<Pixels>,
        index: usize,
        ink: Hsla,
        width: f32,
        window: &mut Window,
    ) {
        let reference = &self.schema.refs[index];
        let (corners, from_side, to_side) = self.route(index);
        let points: Vec<Point<Pixels>> =
            corners.iter().map(|at| self.screen(bounds, *at)).collect();
        let reach = px(16. * self.zoom);
        let (mut low, mut high) = (points[0], points[0]);
        for at in &points {
            low = point(low.x.min(at.x), low.y.min(at.y));
            high = point(high.x.max(at.x), high.y.max(at.y));
        }
        let around = Bounds::from_corners(low - point(reach, reach), high + point(reach, reach));
        if !around.intersects(&seen) {
            return;
        }
        let mut path = PathBuilder::stroke(px(width));
        path.move_to(points[0]);
        for index in 1..points.len() - 1 {
            let (before, at, after) = (points[index - 1], points[index], points[index + 1]);
            let reach = |from: Point<Pixels>| {
                let d = from - at;
                (f32::from(d.x).powi(2) + f32::from(d.y).powi(2)).sqrt()
            };
            let radius = (10. * self.zoom)
                .min(reach(before) / 2.)
                .min(reach(after) / 2.);
            let toward = |from: Point<Pixels>, length: f32| {
                let d = from - at;
                let span = reach(from).max(0.001);
                at + point(d.x * (length / span), d.y * (length / span))
            };
            path.line_to(toward(before, radius));
            path.curve_to(toward(after, radius), at);
        }
        path.line_to(points[points.len() - 1]);
        if let Ok(path) = path.build() {
            window.paint_path(path, ink);
        }
        self.paint_end(
            points[0],
            from_side,
            reference.from.many,
            ink,
            width,
            window,
        );
        let last = points[points.len() - 1];
        self.paint_end(last, to_side, reference.to.many, ink, width, window);
    }

    /// A crow's foot where many rows meet the line, a bar where one does.
    fn paint_end(
        &self,
        at: Point<Pixels>,
        side: f32,
        many: bool,
        ink: Hsla,
        width: f32,
        window: &mut Window,
    ) {
        let z = self.zoom;
        let mut path = PathBuilder::stroke(px(width));
        if many {
            let tip = at + point(px(side * 13. * z), px(0.));
            for spread in [-6., 0., 6.] {
                path.move_to(tip);
                path.line_to(at + point(px(0.), px(spread * z)));
            }
        } else {
            let x = at.x + px(side * 9. * z);
            path.move_to(point(x, at.y - px(6. * z)));
            path.line_to(point(x, at.y + px(6. * z)));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, ink);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_card(
        &self,
        bounds: Bounds<Pixels>,
        seen: Bounds<Pixels>,
        index: usize,
        related: &HashSet<(usize, Option<usize>)>,
        linked: &HashSet<(usize, usize)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (t, z) = (&self.t, self.zoom);
        let card = self.cards[index];
        let table = &self.schema.tables[index];
        let frame = self.rect(bounds, card.x, card.y, card.w, card.h);
        if !frame.intersects(&seen) {
            return;
        }
        let hovered = self.hover.is_some_and(|spot| spot.table == index);
        let is_related = related.contains(&(index, None));
        let radius = px(RADIUS * z);
        if hovered || self.dragging && self.order.last() == Some(&index) {
            window.paint_drop_shadows(frame, Corners::all(radius), 0., &t.shadow_raised());
        }
        let edge = match (hovered, is_related) {
            (true, _) => t.border_strong,
            (false, true) => Hsla {
                a: 0.55,
                ..t.accent
            },
            _ => t.border,
        };
        window.paint_quad(quad(
            frame,
            radius,
            t.surface_raised,
            px(1.),
            edge,
            BorderStyle::Solid,
        ));
        let tint = table.color.map(rgb);
        let header = self.rect(bounds, card.x, card.y, card.w, HEADER);
        window.paint_quad(quad(
            header,
            Corners {
                top_left: radius,
                top_right: radius,
                bottom_left: px(0.),
                bottom_right: px(0.),
            },
            match tint {
                Some(tint) => Hsla { a: 0.16, ..tint },
                None => t.surface_hover,
            },
            px(0.),
            Hsla::transparent_black(),
            BorderStyle::Solid,
        ));
        if z < DETAIL_ZOOM {
            let rows = if z < BARS_ZOOM {
                0
            } else {
                table.columns.len()
            };
            for row in 0..rows {
                let bar = self.rect(
                    bounds,
                    card.x + PAD,
                    card.y + HEADER + row as f32 * ROW + ROW * 0.36,
                    (card.w - PAD * 2.) * if row % 3 == 2 { 0.55 } else { 0.8 },
                    ROW * 0.28,
                );
                window.paint_quad(quad(
                    bar,
                    px(2.),
                    Hsla {
                        a: 0.12,
                        ..t.text_tertiary
                    },
                    px(0.),
                    Hsla::transparent_black(),
                    BorderStyle::Solid,
                ));
            }
            let room = (card.w - PAD * 2.) * z;
            let mut size = (NAME_SIZE * z).max(11.);
            if card.name_w > 0. {
                size *= (room / (card.name_w * size / NAME_SIZE)).min(1.);
            }
            let origin =
                header.origin + point(px(PAD * z), (header.size.height - px(size * 1.3)) / 2.);
            window.with_content_mask(Some(ContentMask { bounds: frame }), |window| {
                text(
                    &table.name,
                    name_font(),
                    size,
                    t.text_primary,
                    origin,
                    size * 1.3,
                    window,
                    cx,
                );
            });
            return;
        }
        window.with_content_mask(Some(ContentMask { bounds: frame }), |window| {
            let mut x = card.x + PAD;
            if let Some(tint) = tint {
                let dot = self.rect(bounds, x, card.y + HEADER / 2. - 4., 8., 8.);
                window.paint_quad(quad(
                    dot,
                    px(4. * z),
                    tint,
                    px(0.),
                    Hsla::transparent_black(),
                    BorderStyle::Solid,
                ));
                x += 14.;
            }
            let line = NAME_SIZE * 1.3;
            text(
                &table.name,
                name_font(),
                NAME_SIZE * z,
                t.text_primary,
                self.screen(bounds, point(x, card.y + (HEADER - line) / 2.)),
                line * z,
                window,
                cx,
            );
            let count = table.columns.len().to_string();
            text_right(
                &count,
                type_font(),
                TYPE_SIZE * z,
                t.text_disabled,
                self.screen(
                    bounds,
                    point(card.right() - PAD, card.y + (HEADER - line) / 2.),
                ),
                line * z,
                window,
                cx,
            );
            let rule = self.rect(bounds, card.x, card.y + HEADER - 1. / z, card.w, 1. / z);
            window.paint_quad(gpui_kit::fill(rule, t.border));
            for (row, column) in table.columns.iter().enumerate() {
                let top = card.y + HEADER + row as f32 * ROW;
                let over = hovered && self.hover.and_then(|spot| spot.column) == Some(row);
                let lit = related.contains(&(index, Some(row)));
                if over || lit {
                    let band = self.rect(bounds, card.x + 1. / z, top, card.w - 2. / z, ROW);
                    let ground = match lit {
                        true => Hsla {
                            a: 0.12,
                            ..t.accent
                        },
                        false => t.surface_hover,
                    };
                    window.paint_quad(gpui_kit::fill(band, ground));
                }
                let line = COLUMN_SIZE * 1.3;
                let baseline = top + (ROW - line) / 2.;
                let mark = self.rect(bounds, card.x + PAD, top + (ROW - 12.) / 2., 12., 12.);
                if column.pk {
                    paint_key(mark, t.warn, window);
                } else if linked.contains(&(index, row)) {
                    let _ = window.paint_svg(
                        mark,
                        Icon::Link.path().into(),
                        None,
                        gpui_kit::TransformationMatrix::unit(),
                        t.text_tertiary,
                        cx,
                    );
                }
                text(
                    &column.name,
                    column_font(column.pk),
                    COLUMN_SIZE * z,
                    if lit {
                        t.text_primary
                    } else {
                        t.text_secondary
                    },
                    self.screen(bounds, point(card.x + PAD + KEY_SPACE, baseline)),
                    line * z,
                    window,
                    cx,
                );
                text_right(
                    &column.kind,
                    type_font(),
                    TYPE_SIZE * z,
                    t.text_tertiary,
                    self.screen(bounds, point(card.right() - PAD, baseline + 0.5)),
                    line * z,
                    window,
                    cx,
                );
            }
        });
    }

    /// What the pointer's column or table says beyond its name: its type and settings,
    /// default and note.
    fn paint_tip(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (Some(spot), Some(pointer)) = (self.hover, self.pointer) else {
            return;
        };
        let table = &self.schema.tables[spot.table];
        let mut lines: Vec<(String, bool)> = Vec::new();
        match spot.column.and_then(|column| table.columns.get(column)) {
            Some(column) => {
                let flags: Vec<&str> = [
                    (column.pk, "primary key"),
                    (column.not_null, "not null"),
                    (column.unique, "unique"),
                    (column.increment, "increment"),
                ]
                .into_iter()
                .filter_map(|(on, said)| on.then_some(said))
                .collect();
                if !flags.is_empty() {
                    lines.push((flags.join(" · "), false));
                }
                if let Some(default) = &column.default {
                    lines.push((format!("default {default}"), false));
                }
                if let Some(note) = &column.note {
                    lines.extend(wrap(note, 56).into_iter().map(|line| (line, true)));
                }
                if lines.is_empty() {
                    return;
                }
                lines.insert(0, (format!("{}  {}", column.name, column.kind), true));
            }
            None => {
                let Some(note) = &table.note else {
                    return;
                };
                lines.push((table.name.clone(), true));
                lines.extend(wrap(note, 56).into_iter().map(|line| (line, true)));
            }
        }
        let t = &self.t;
        let (size, line) = (12., 17.);
        let width = lines
            .iter()
            .map(|(said, _)| said.chars().count() as f32 * 6.6)
            .fold(0., f32::max)
            .min(400.)
            + 20.;
        let height = lines.len() as f32 * line + 14.;
        let mut origin = pointer + point(px(14.), px(18.));
        if origin.x + px(width) > bounds.right() {
            origin.x = pointer.x - px(width + 10.);
        }
        if origin.y + px(height) > bounds.bottom() {
            origin.y = pointer.y - px(height + 10.);
        }
        let tip = Bounds {
            origin,
            size: gpui_kit::size(px(width), px(height)),
        };
        window.paint_drop_shadows(tip, Corners::all(px(6.)), 0., &t.shadow_float());
        window.paint_quad(quad(
            tip,
            px(6.),
            t.surface_tooltip,
            px(1.),
            t.border,
            BorderStyle::Solid,
        ));
        for (row, (said, strong)) in lines.iter().enumerate() {
            let at = origin + point(px(10.), px(7. + row as f32 * line));
            let (font, ink) = match (row, strong) {
                (0, _) => (name_font(), t.text_primary),
                (_, true) => (gpui_kit::font(theme::UI), t.text_secondary),
                (_, false) => (type_font(), t.text_tertiary),
            };
            text(said, font, size, ink, at, line, window, cx);
        }
    }

    fn paint_map(&self, map: &Map, window: &mut Window) {
        let t = &self.t;
        window.paint_quad(quad(
            map.frame,
            px(8.),
            t.surface_overlay,
            px(1.),
            t.border,
            BorderStyle::Solid,
        ));
        for card in &self.cards {
            let rect = Bounds {
                origin: map.to_screen(point(card.x, card.y)),
                size: gpui_kit::size(px(card.w * map.scale), px(card.h * map.scale)),
            };
            window.paint_quad(quad(
                rect,
                px(1.5),
                Hsla {
                    a: 0.35,
                    ..t.text_tertiary
                },
                px(0.),
                Hsla::transparent_black(),
                BorderStyle::Solid,
            ));
        }
        let seen = map.seen;
        let top_left = map.to_screen(point(seen.0, seen.1));
        let bottom_right = map.to_screen(point(seen.2, seen.3));
        let view = Bounds::from_corners(
            point(
                top_left.x.max(map.frame.left()),
                top_left.y.max(map.frame.top()),
            ),
            point(
                bottom_right.x.min(map.frame.right()),
                bottom_right.y.min(map.frame.bottom()),
            ),
        );
        window.paint_quad(quad(
            view,
            px(3.),
            Hsla {
                a: 0.06,
                ..t.text_primary
            },
            px(1.),
            Hsla {
                a: 0.5,
                ..t.text_secondary
            },
            BorderStyle::Solid,
        ));
    }
}

/// The small picture of the whole diagram in the bottom left corner, shown when the
/// diagram does not fit the canvas.
struct Map {
    frame: Bounds<Pixels>,
    scale: f32,
    /// The diagram's point drawn at the frame's top left corner.
    origin: Point<f32>,
    /// What the canvas shows, in the diagram's units.
    seen: (f32, f32, f32, f32),
}

impl Map {
    fn to_screen(&self, at: Point<f32>) -> Point<Pixels> {
        self.frame.origin
            + point(
                px((at.x - self.origin.x) * self.scale),
                px((at.y - self.origin.y) * self.scale),
            )
    }

    fn to_world(&self, at: Point<Pixels>) -> Point<f32> {
        let d = at - self.frame.origin;
        point(
            f32::from(d.x) / self.scale + self.origin.x,
            f32::from(d.y) / self.scale + self.origin.y,
        )
    }
}

fn minimap(cards: &[Card], bounds: Bounds<Pixels>, pan: Point<f32>, zoom: f32) -> Option<Map> {
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let seen = (
        -pan.x / zoom,
        -pan.y / zoom,
        (width - pan.x) / zoom,
        (height - pan.y) / zoom,
    );
    let all = extent(cards)?;
    if all.0 >= seen.0 && all.1 >= seen.1 && all.2 <= seen.2 && all.3 <= seen.3 {
        return None;
    }
    if width < MAP_WIDTH * 2.5 || height < MAP_HEIGHT * 2.5 {
        return None;
    }
    let pad = 40.;
    let (left, top, right, bottom) = (all.0 - pad, all.1 - pad, all.2 + pad, all.3 + pad);
    let scale = ((MAP_WIDTH - 12.) / (right - left)).min((MAP_HEIGHT - 12.) / (bottom - top));
    let frame = Bounds {
        origin: point(
            bounds.left() + px(MAP_MARGIN),
            bounds.bottom() - px(MAP_MARGIN + MAP_HEIGHT),
        ),
        size: gpui_kit::size(px(MAP_WIDTH), px(MAP_HEIGHT)),
    };
    let drawn = ((right - left) * scale, (bottom - top) * scale);
    let origin = point(
        left - (MAP_WIDTH - drawn.0) / 2. / scale,
        top - (MAP_HEIGHT - drawn.1) / 2. / scale,
    );
    Some(Map {
        frame,
        scale,
        origin,
        seen,
    })
}

/// The row a reference meets at its start and at its end.
type Anchors = (Option<usize>, Option<usize>);

/// The row each reference meets at either end: its column's, or for a reference over several
/// columns, the one fewest other references share, so references that share a column, such as
/// `(org_id, repo_id)` and `(org_id, team_id)`, leave from rows of their own.
fn anchors(schema: &Schema) -> Vec<Anchors> {
    let mut uses: HashMap<(usize, usize), usize> = HashMap::new();
    for reference in &schema.refs {
        for end in [&reference.from, &reference.to] {
            for &column in &end.columns {
                *uses.entry((end.table, column)).or_default() += 1;
            }
        }
    }
    let row = |end: &dbml::End| {
        (end.columns.iter().enumerate())
            .min_by_key(|(at, column)| (uses[&(end.table, **column)], usize::MAX - at))
            .map(|(_, column)| *column)
    };
    (schema.refs.iter())
        .map(|reference| (row(&reference.from), row(&reference.to)))
        .collect()
}

/// Spreads the turns of lines that cross the same gap between two tables evenly across it,
/// in the order of their height so they do not cross each other there.
fn lanes(schema: &Schema, anchors: &[Anchors], cards: &[Card]) -> HashMap<usize, f32> {
    let mut gaps: HashMap<(i32, i32), Vec<(f32, usize)>> = HashMap::new();
    for (index, reference) in schema.refs.iter().enumerate() {
        let (a, b) = (&cards[reference.from.table], &cards[reference.to.table]);
        let gap = if a.right() + STUB * 2. <= b.x {
            (a.right(), b.x)
        } else if b.right() + STUB * 2. <= a.x {
            (b.right(), a.x)
        } else {
            continue;
        };
        let y = |end: &dbml::End, row: Option<usize>| {
            cards[end.table].y + HEADER + row.map_or(0., |row| row as f32 * ROW)
        };
        let (from, to) = anchors[index];
        let height = y(&reference.from, from).min(y(&reference.to, to));
        let key = (gap.0.round() as i32, gap.1.round() as i32);
        gaps.entry(key).or_default().push((height, index));
    }
    let mut lanes = HashMap::new();
    for ((left, right), mut lines) in gaps {
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (left, right) = (left as f32 + STUB, right as f32 - STUB);
        let count = lines.len() as f32;
        let step = ((right - left) / (count + 1.)).min(14.);
        let first = (left + right) / 2. - step * (count - 1.) / 2.;
        for (lane, (_, index)) in lines.into_iter().enumerate() {
            lanes.insert(index, first + step * lane as f32);
        }
    }
    lanes
}

fn extent(cards: &[Card]) -> Option<(f32, f32, f32, f32)> {
    extent_of(cards.iter())
}

fn extent_of<'a>(cards: impl Iterator<Item = &'a Card>) -> Option<(f32, f32, f32, f32)> {
    cards.fold(None, |extent, card| {
        let (l, t, r, b) = extent.unwrap_or((f32::MAX, f32::MAX, f32::MIN, f32::MIN));
        Some((
            l.min(card.x),
            t.min(card.y),
            r.max(card.right()),
            b.max(card.bottom()),
        ))
    })
}

/// Lays the tables out: each table group as a block of its own, laid out by the references
/// inside it, then the blocks and the tables in no group, each to the right of what it refers
/// to, so a group's tables sit together and groups never overlap.
fn arrange(schema: &Schema, cards: &mut [Card]) {
    let n = cards.len();
    let keyed = |end: &dbml::End| {
        let columns = &schema.tables[end.table].columns;
        end.columns.iter().all(|&column| columns[column].pk)
    };
    let mut links: Vec<(usize, usize)> = Vec::new();
    for reference in &schema.refs {
        let (from, to) = (&reference.from, &reference.to);
        let link = match (from.many, to.many) {
            (false, true) => (to.table, from.table),
            (false, false) if keyed(from) && !keyed(to) => (to.table, from.table),
            _ => (from.table, to.table),
        };
        if link.0 != link.1 && !links.contains(&link) {
            links.push(link);
        }
    }
    let mut cluster_of = vec![usize::MAX; n];
    let mut clusters: Vec<(Vec<usize>, bool)> = Vec::new();
    for group in &schema.groups {
        let members: Vec<usize> = (group.tables.iter().copied())
            .filter(|&table| table < n && cluster_of[table] == usize::MAX)
            .collect();
        if members.is_empty() {
            continue;
        }
        for &table in &members {
            cluster_of[table] = clusters.len();
        }
        clusters.push((members, true));
    }
    for (table, cluster) in cluster_of.iter_mut().enumerate() {
        if *cluster == usize::MAX {
            *cluster = clusters.len();
            clusters.push((vec![table], false));
        }
    }
    let mut boxes = Vec::with_capacity(clusters.len());
    for (members, grouped) in &clusters {
        let local = |table: usize| members.iter().position(|&member| member == table);
        let inner: Vec<(usize, usize)> = (links.iter())
            .filter_map(|&(child, parent)| Some((local(child)?, local(parent)?)))
            .collect();
        let sizes: Vec<(f32, f32)> = members.iter().map(|&t| (cards[t].w, cards[t].h)).collect();
        let placed = layered(&sizes, &inner);
        for (&table, &(x, y)) in members.iter().zip(&placed) {
            cards[table].x = x;
            cards[table].y = y;
        }
        let (_, _, right, bottom) =
            extent_of(members.iter().map(|&t| &cards[t])).unwrap_or_default();
        let (left, top, below) = match grouped {
            true => (GROUP_PAD, GROUP_PAD + GROUP_LABEL, GROUP_PAD),
            false => (0., 0., 0.),
        };
        boxes.push(((right + left * 2., bottom + top + below), (left, top)));
    }
    let mut between: Vec<(usize, usize)> = Vec::new();
    for &(child, parent) in &links {
        let link = (cluster_of[child], cluster_of[parent]);
        if link.0 != link.1 && !between.contains(&link) {
            between.push(link);
        }
    }
    let sizes: Vec<(f32, f32)> = boxes.iter().map(|(size, _)| *size).collect();
    let placed = layered(&sizes, &between);
    for (cluster, (members, _)) in clusters.iter().enumerate() {
        let ((x, y), (_, (left, top))) = (placed[cluster], boxes[cluster]);
        for &table in members {
            cards[table].x += x + left;
            cards[table].y += y + top;
        }
    }
    for card in cards.iter_mut() {
        card.placed = true;
    }
}

/// Places boxes of `sizes` left to right in columns, each to the right of the boxes it links
/// to as `(child, parent)`, lined up with its neighbours; boxes with no links sit in rows
/// underneath. Answers each box's top left corner, the whole starting at the origin.
fn layered(sizes: &[(f32, f32)], links: &[(usize, usize)]) -> Vec<(f32, f32)> {
    let n = sizes.len();
    let mut at = vec![(0f32, 0f32); n];
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(child, parent) in links {
        if child != parent && !parents[child].contains(&parent) {
            parents[child].push(parent);
            children[parent].push(child);
        }
    }
    drop_cycles(&mut parents, &mut children);
    let linked: Vec<bool> = (0..n)
        .map(|node| !parents[node].is_empty() || !children[node].is_empty())
        .collect();
    let mut rank = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for child in 0..n {
            for &parent in &parents[child] {
                if rank[child] < rank[parent] + 1 && rank[parent] + 1 < n {
                    rank[child] = rank[parent] + 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    for node in 0..n {
        if parents[node].is_empty() && !children[node].is_empty() {
            let nearest = children[node].iter().map(|&c| rank[c]).min().unwrap_or(1);
            rank[node] = nearest.saturating_sub(1);
        }
    }
    let top = rank.iter().copied().max().unwrap_or(0);
    let mut columns: Vec<Vec<usize>> = Vec::new();
    for r in 0..=top {
        let stack: Vec<usize> = (0..n).filter(|&t| linked[t] && rank[t] == r).collect();
        if stack.is_empty() {
            continue;
        }
        let mut column: Vec<usize> = Vec::new();
        let mut height = 0.;
        for node in stack {
            if !column.is_empty() && height + sizes[node].1 > TALLEST_STACK {
                columns.push(std::mem::take(&mut column));
                height = 0.;
            }
            height += sizes[node].1 + GAP_Y;
            column.push(node);
        }
        columns.push(column);
    }
    let neighbours: Vec<Vec<usize>> = (0..n)
        .map(|node| {
            parents[node]
                .iter()
                .chain(&children[node])
                .copied()
                .collect()
        })
        .collect();
    let mut x = 0.;
    for column in &columns {
        let mut y = 0.;
        for &node in column {
            at[node] = (x, y);
            y += sizes[node].1 + GAP_Y;
        }
        x += column.iter().map(|&t| sizes[t].0).fold(0., f32::max) + GAP_X;
    }
    for sweep in 0..8 {
        let order: Vec<usize> = match sweep % 2 {
            0 => (0..columns.len()).collect(),
            _ => (0..columns.len()).rev().collect(),
        };
        for index in order {
            let column = &columns[index];
            let mut wanted: Vec<(f32, usize)> = column
                .iter()
                .map(|&node| {
                    let height = sizes[node].1;
                    let others: Vec<f32> = neighbours[node]
                        .iter()
                        .filter(|other| !column.contains(other))
                        .map(|&other| at[other].1 + sizes[other].1 / 2.)
                        .collect();
                    let centre = match others.is_empty() {
                        true => at[node].1 + height / 2.,
                        false => others.iter().sum::<f32>() / others.len() as f32,
                    };
                    (centre - height / 2., node)
                })
                .collect();
            wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut placed = Vec::with_capacity(wanted.len());
            let mut floor = f32::MIN;
            for &(want, node) in &wanted {
                let y = want.max(floor);
                placed.push(y);
                floor = y + sizes[node].1 + GAP_Y;
            }
            let shift = wanted
                .iter()
                .zip(&placed)
                .map(|((want, _), y)| want - y)
                .sum::<f32>()
                / wanted.len().max(1) as f32;
            for ((_, node), y) in wanted.iter().zip(&placed) {
                at[*node].1 = y + shift;
            }
            columns[index] = wanted.iter().map(|(_, node)| *node).collect();
        }
    }
    let lowest = (0..n)
        .filter(|&t| linked[t])
        .map(|t| at[t].1)
        .fold(f32::MAX, f32::min);
    if lowest < f32::MAX {
        for node in (0..n).filter(|&t| linked[t]) {
            at[node].1 -= lowest;
        }
    }
    let (mut right, mut bottom) = (0f32, f32::MIN);
    for node in (0..n).filter(|&t| linked[t]) {
        right = right.max(at[node].0 + sizes[node].0);
        bottom = bottom.max(at[node].1 + sizes[node].1);
    }
    let (width, mut y) = match bottom > f32::MIN {
        true => (right.max(1100.), bottom + GAP_Y * 2.),
        false => (1100., 0.),
    };
    let mut x = 0.;
    let mut tallest: f32 = 0.;
    for node in (0..n).filter(|&t| !linked[t]) {
        if x > 0. && x + sizes[node].0 > width {
            x = 0.;
            y += tallest + GAP_Y;
            tallest = 0.;
        }
        at[node] = (x, y);
        x += sizes[node].0 + GAP_Y;
        tallest = tallest.max(sizes[node].1);
    }
    at
}

/// Drops the references that lead back to a table that refers on to them, so every table
/// can sit to the right of all it refers to.
fn drop_cycles(parents: &mut [Vec<usize>], children: &mut [Vec<usize>]) {
    let n = parents.len();
    let mut state = vec![0u8; n];
    let mut back = Vec::new();
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        state[root] = 1;
        while let Some((table, next)) = stack.pop() {
            if let Some(&parent) = parents[table].get(next) {
                stack.push((table, next + 1));
                match state[parent] {
                    0 => {
                        state[parent] = 1;
                        stack.push((parent, 0));
                    }
                    1 => back.push((table, parent)),
                    _ => {}
                }
            } else {
                state[table] = 2;
            }
        }
    }
    for (child, parent) in back {
        parents[child].retain(|p| *p != parent);
        children[parent].retain(|c| *c != child);
    }
}

/// Stacks tables that are new to a diagram already laid out to the right of the rest.
fn place_new(cards: &mut [Card]) {
    let Some((_, top, right, _)) = extent_of(cards.iter().filter(|card| card.placed)) else {
        return;
    };
    let mut y = top;
    for card in cards.iter_mut().filter(|card| !card.placed) {
        card.x = right + GAP_X;
        card.y = y;
        y += card.h + GAP_Y;
        card.placed = true;
    }
}

/// A key: a ring with a shaft and two teeth, filling `bounds`.
fn paint_key(bounds: Bounds<Pixels>, ink: Hsla, window: &mut Window) {
    let unit = f32::from(bounds.size.width) / 12.;
    let at = |x: f32, y: f32| bounds.origin + point(px(x * unit), px(y * unit));
    let mut path = PathBuilder::stroke(px((1.4 * unit).max(0.6)));
    let radius = point(px(2.6 * unit), px(2.6 * unit));
    path.move_to(at(6.4, 6.));
    path.arc_to(radius, px(0.), false, true, at(1.2, 6.));
    path.arc_to(radius, px(0.), false, true, at(6.4, 6.));
    path.line_to(at(11.2, 6.));
    path.move_to(at(9.4, 6.));
    path.line_to(at(9.4, 8.4));
    path.move_to(at(11.2, 6.));
    path.line_to(at(11.2, 8.));
    if let Ok(path) = path.build() {
        window.paint_path(path, ink);
    }
}

/// How many dots a side of the background's tile holds, and how far apart they are in it.
const TILE_DOTS: u32 = 8;
const TILE_STEP: u32 = 48;

/// The canvas's background dots as one tile of them in `ink`, painted a few dozen times
/// rather than a dot at a time; made once for each colour.
fn dot_tile(ink: Hsla) -> Option<Arc<RenderImage>> {
    thread_local! {
        static TILE: RefCell<Option<(Hsla, Arc<RenderImage>)>> = const { RefCell::new(None) };
    }
    TILE.with(|tile| {
        if let Some((kept, image)) = &*tile.borrow() {
            if *kept == ink {
                return Some(image.clone());
            }
        }
        let side = TILE_DOTS * TILE_STEP;
        let rgba = gpui_kit::Rgba::from(ink);
        let radius = 1.9f32;
        let pixels = image::RgbaImage::from_fn(side, side, |x, y| {
            let centre = TILE_STEP as f32 / 2.;
            let dx = (x % TILE_STEP) as f32 + 0.5 - centre;
            let dy = (y % TILE_STEP) as f32 + 0.5 - centre;
            let cover = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0., 1.) * rgba.a;
            let channel = |value: f32| (value * cover * 255.).round() as u8;
            image::Rgba([
                channel(rgba.b),
                channel(rgba.g),
                channel(rgba.r),
                (cover * 255.).round() as u8,
            ])
        });
        let image = Arc::new(RenderImage::new([image::Frame::new(pixels)]));
        *tile.borrow_mut() = Some((ink, image.clone()));
        Some(image)
    })
}

/// A hash of `text` that stays the same from one run and build to the next, unlike std's.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

fn name_font() -> Font {
    Font {
        weight: FontWeight::SEMIBOLD,
        ..gpui_kit::font(theme::UI)
    }
}

fn column_font(primary: bool) -> Font {
    Font {
        weight: if primary {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        },
        ..gpui_kit::font(theme::UI)
    }
}

fn type_font() -> Font {
    gpui_kit::font(theme::MONO)
}

fn rgb(colour: u32) -> Hsla {
    gpui_kit::rgb(colour).into()
}

#[allow(clippy::too_many_arguments)]
fn text(
    said: &str,
    font: Font,
    size: f32,
    ink: Hsla,
    origin: Point<Pixels>,
    line: f32,
    window: &mut Window,
    cx: &mut App,
) -> Pixels {
    shaped(said, font, size, ink, window).map_or(px(0.), |shaped| {
        let _ = shaped.paint(origin, px(line), TextAlign::Left, None, window, cx);
        shaped.width
    })
}

/// Text ending at `end`'s x.
#[allow(clippy::too_many_arguments)]
fn text_right(
    said: &str,
    font: Font,
    size: f32,
    ink: Hsla,
    end: Point<Pixels>,
    line: f32,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(shaped) = shaped(said, font, size, ink, window) {
        let origin = point(end.x - shaped.width, end.y);
        let _ = shaped.paint(origin, px(line), TextAlign::Left, None, window, cx);
    }
}

fn shaped(
    said: &str,
    font: Font,
    size: f32,
    ink: Hsla,
    window: &mut Window,
) -> Option<gpui_kit::ShapedLine> {
    if said.is_empty() || size < 2. {
        return None;
    }
    // Zooming would otherwise shape every line and draw every glyph again at a new size
    // each frame; half-point steps let consecutive frames reuse them.
    let size = (size * 2.).round() / 2.;
    let run = TextRun {
        len: said.len(),
        font,
        color: ink,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    Some(window.text_system().shape_line(
        SharedString::from(said.to_string()),
        px(size),
        &[run],
        None,
    ))
}

/// `said` broken into lines of at most about `width` characters, at spaces.
fn wrap(said: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in said.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + word.chars().count() + 1 > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}
