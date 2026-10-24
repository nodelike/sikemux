//! A tab's views side by side, as the core's layout splits them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, px, AnyElement, App, AppContext, Bounds, Context, CursorStyle, DragMoveEvent,
    EmptyView, Focusable, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Pixels, Point, SharedString, StatefulInteractiveElement, Styled, Window,
};
use serde_json::json;
use sikemux_wire::protocol::{Pane, PaneKind, SessionId, SplitDir, Tab};

use crate::drag::DraggedTab;
use crate::workbench::{Strip, TakesKeys, Workbench};
use crate::{desk, home, terminal, ui};

/// The room between two panes, which is also where a divider is dragged.
pub const GAP: f32 = 10.;
/// How tall the strip of tabs over a stack is.
const STACK_STRIP: f32 = 32.;
/// The least a pane is squeezed to by a divider.
const MIN_PANE: f32 = 160.;
/// How far one press of a resize key moves a divider, and the least it leaves the pane beside,
/// in thousandths of the split.
const RESIZE_STEP: u16 = 40;
const LEAST_SHARE: u16 = 50;

/// Which way from the view in focus a key moves the focus or a divider.
#[derive(Clone, Copy, PartialEq)]
pub enum Towards {
    Left,
    Right,
    Up,
    Down,
}

/// What only this window decides about a tab's views.
#[derive(Default)]
pub struct Panes {
    /// The view the person last clicked into, by id.
    pub focused: Option<String>,
    resizing: Option<Resizing>,
    /// Where a tab dragged over a view would go beside it.
    aimed: Option<Aimed>,
    /// The view that fills its tab for now, by id.
    pub zoomed: Option<String>,
    /// Where each view of the tab in front was last drawn, by id.
    drawn: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
}

/// A tab dragged over a view: the side of the view it would take, which the view lights.
#[derive(Clone, PartialEq)]
struct Aimed {
    tab: String,
    pane: String,
    side: Side,
    from: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn word(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
        }
    }
}

/// A split tab holds at most three views side by side, or two stacked.
const MOST_SIDE_BY_SIDE: usize = 3;

/// Where one more view can go: any side of a lone view, then only across while a row of
/// views has room.
fn open_sides(root: &Pane) -> Vec<Side> {
    match root {
        Pane::View { .. } => vec![Side::Left, Side::Right, Side::Top, Side::Bottom],
        Pane::Split { dir, panes, .. } => {
            let flat = panes.iter().all(|pane| matches!(pane, Pane::View { .. }));
            match *dir == SplitDir::Row && flat && panes.len() < MOST_SIDE_BY_SIDE {
                true => vec![Side::Left, Side::Right],
                false => Vec::new(),
            }
        }
    }
}

/// A divider being dragged, and the sizes it gives its split until the core keeps them.
struct Resizing {
    tab: String,
    split: String,
    /// The split's sizes as the core had them when the drag began.
    from: Vec<u16>,
    sizes: Vec<u16>,
}

#[derive(Clone)]
struct Divider {
    tab: String,
    split: String,
    at: usize,
}

impl Panes {
    /// The sizes to draw `split` at: the core's, or a drag's while the core has not answered it.
    fn sizes<'a>(&'a self, split: &str, kept: &'a [u16]) -> &'a [u16] {
        match &self.resizing {
            Some(resizing) if resizing.split == split && resizing.from == kept => &resizing.sizes,
            _ => kept,
        }
    }
}

pub fn tab(
    bench: &Workbench,
    tab: &Tab,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    bench.panes.drawn.borrow_mut().clear();
    let zoomed = bench
        .panes
        .zoomed
        .as_deref()
        .and_then(|id| view_in(&tab.pane, id));
    pane(bench, &tab.id, zoomed.unwrap_or(&tab.pane), window, cx)
}

fn view_in<'a>(pane: &'a Pane, view: &str) -> Option<&'a Pane> {
    match pane {
        Pane::View { id, .. } => (id == view).then_some(pane),
        Pane::Split { panes, .. } => panes.iter().find_map(|child| view_in(child, view)),
    }
}

fn first_view(pane: &Pane) -> Option<&str> {
    match pane {
        Pane::View { id, .. } => Some(id),
        Pane::Split { panes, .. } => panes.first().and_then(first_view),
    }
}

/// The splits from the tab's top down to `view`, each with which of its panes holds it.
fn path_to<'a>(pane: &'a Pane, view: &str, path: &mut Vec<(&'a Pane, usize)>) -> bool {
    match pane {
        Pane::View { id, .. } => id == view,
        Pane::Split { panes, .. } => panes.iter().enumerate().any(|(at, child)| {
            path.push((pane, at));
            let found = path_to(child, view, path);
            if !found {
                path.pop();
            }
            found
        }),
    }
}

/// The view nearest `from` that lies wholly `towards` it and shares some of its span across,
/// weighing how far off centre it sits half as much as how far away it is.
fn neighbour(
    drawn: &HashMap<String, Bounds<Pixels>>,
    from: &str,
    towards: Towards,
) -> Option<String> {
    let a = drawn.get(from)?;
    let centre = a.center();
    let mut best: Option<(f32, &String)> = None;
    for (id, b) in drawn {
        if id == from {
            continue;
        }
        let rows_meet = b.top() < a.bottom() && b.bottom() > a.top();
        let columns_meet = b.left() < a.right() && b.right() > a.left();
        let (beyond, distance, off) = match towards {
            Towards::Left => (
                rows_meet && b.right() <= a.left() + px(1.),
                a.left() - b.right(),
                b.center().y - centre.y,
            ),
            Towards::Right => (
                rows_meet && b.left() >= a.right() - px(1.),
                b.left() - a.right(),
                b.center().y - centre.y,
            ),
            Towards::Up => (
                columns_meet && b.bottom() <= a.top() + px(1.),
                a.top() - b.bottom(),
                b.center().x - centre.x,
            ),
            Towards::Down => (
                columns_meet && b.top() >= a.bottom() - px(1.),
                b.top() - a.bottom(),
                b.center().x - centre.x,
            ),
        };
        let score = f32::from(distance) + f32::from(off).abs() * 0.5;
        if beyond && best.is_none_or(|(least, _)| score < least) {
            best = Some((score, id));
        }
    }
    best.map(|(_, id)| id.clone())
}

fn pane(
    bench: &Workbench,
    tab: &str,
    pane: &Pane,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    match pane {
        Pane::View { id, kind, target } => {
            let focused = id.clone();
            let (aiming, dropping) = ((tab.to_owned(), id.clone()), id.clone());
            let aimed = bench
                .panes
                .aimed
                .as_ref()
                .filter(|aimed| aimed.pane == *id && cx.has_active_drag())
                .map(|aimed| aimed.side);
            div()
                .id(SharedString::from(id.clone()))
                .relative()
                .flex()
                .size_full()
                .min_w_0()
                .min_h_0()
                .capture_any_mouse_down(
                    cx.listener(move |bench, _, _, _| bench.panes.focused = Some(focused.clone())),
                )
                .children(ui::spot(|| format!("pane-{kind:?}-{target}")))
                .on_drag_move(cx.listener(
                    move |bench, event: &DragMoveEvent<DraggedTab>, _, cx| {
                        let (tab, pane) = &aiming;
                        bench.aim_split(tab, pane, event, cx);
                    },
                ))
                .on_drop(cx.listener(move |bench, dropped: &DraggedTab, _, cx| {
                    bench.drop_split(&dropping, dropped, cx);
                }))
                .child(view(bench, tab, id, *kind, target, window, cx))
                .child({
                    let (drawn, view) = (Rc::clone(&bench.panes.drawn), id.clone());
                    canvas(
                        move |bounds, _, _| {
                            drawn.borrow_mut().insert(view, bounds);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full()
                })
                .children(aimed.map(|side| preview(side, cx)))
                .into_any_element()
        }
        Pane::Split {
            dir: SplitDir::Stack,
            panes,
            ..
        } => stack(bench, tab, panes, window, cx),
        Pane::Split {
            id,
            dir,
            panes,
            sizes,
        } => split(bench, tab, id, *dir, panes, sizes, window, cx),
    }
}

/// Views in one place: a strip of tabs over the one in front, which holds the view in focus,
/// else the first. The views behind are not drawn, so a terminal keeps the size it last had.
fn stack(
    bench: &Workbench,
    tab: &str,
    panes: &[Pane],
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = crate::theme::Tokens::get(cx).clone();
    let focused = bench.panes.focused.as_deref();
    let front = panes
        .iter()
        .position(|child| focused.is_some_and(|view| view_in(child, view).is_some()))
        .unwrap_or(0);
    let pills = panes.iter().enumerate().filter_map(|(at, child)| {
        let lead = first_view(child)?.to_owned();
        let (kind, target) = crate::tabs::first_view(child)?;
        let mark = ui::picture(
            crate::tabs::view_icon(kind, target),
            13.,
            match kind {
                PaneKind::Terminal => t.live,
                _ => t.text_tertiary,
            },
        );
        Some(
            ui::Tab::new(
                SharedString::from(format!("stack-{lead}")),
                crate::tabs::view_label(bench, child),
            )
            .active(at == front)
            .mark(mark)
            .on_click(cx.listener(move |bench, _, window, cx| {
                bench.panes.focused = Some(lead.clone());
                bench.focus_view(&lead, window, cx);
                bench.parts.redraw(&bench.parts.stage, cx);
            })),
        )
    });
    div()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .size_full()
        .min_w_0()
        .min_h_0()
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .h(px(STACK_STRIP))
                .px(px(4.))
                .overflow_hidden()
                .children(pills),
        )
        .children(panes.get(front).map(|child| {
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(pane(bench, tab, child, window, cx))
        }))
        .into_any_element()
}

/// The part of a view a dropped tab would take, lit while the tab is over it.
fn preview(side: Side, cx: &App) -> impl IntoElement {
    let t = crate::theme::Tokens::get(cx);
    let across = matches!(side, Side::Left | Side::Right);
    div()
        .absolute()
        .when(side != Side::Right, |half| half.left_0())
        .when(side == Side::Right, |half| half.right_0())
        .when(side != Side::Bottom, |half| half.top_0())
        .when(side == Side::Bottom, |half| half.bottom_0())
        .when(across, |half| half.w(gpui_kit::relative(0.5)).h_full())
        .when(!across, |half| half.h(gpui_kit::relative(0.5)).w_full())
        .rounded(crate::theme::RADIUS_3)
        .bg(Hsla {
            a: 0.14,
            ..t.accent
        })
        .border_2()
        .border_color(Hsla {
            a: 0.55,
            ..t.accent
        })
}

/// A desk of another host, which this app does not show yet.
fn elsewhere(bench: &Workbench, host: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let name = bench.host_name(host);
    ui::pane(cx)
        .flex()
        .items_center()
        .justify_center()
        .child(ui::meta(format!("This runs on {name}"), cx))
        .into_any_element()
}

fn view(
    bench: &Workbench,
    tab: &str,
    id: &str,
    kind: PaneKind,
    target: &str,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    if let (crate::hosts::Place::Account(host), PaneKind::Desk) = (bench.place(), kind) {
        return elsewhere(bench, &host, cx);
    }
    match kind {
        PaneKind::Terminal => ui::pane(cx)
            .child(terminal::pane(
                bench,
                &cx.entity(),
                &bench.place(),
                target,
                cx,
            ))
            .into_any_element(),
        PaneKind::Files => ui::pane(cx)
            .child(crate::editor::pane(bench, &cx.entity(), target, window, cx))
            .into_any_element(),
        PaneKind::Git if bench.area != crate::codehost::Area::Local && bench.host.is_some() => {
            match &bench.host {
                Some(host) => ui::pane(cx).child(host.clone()).into_any_element(),
                None => ui::pane(cx).into_any_element(),
            }
        }
        PaneKind::Git if bench.git_repo().is_none() && bench.git_repos().is_some() => ui::pane(cx)
            .child(crate::git::repos::picker(bench, cx))
            .into_any_element(),
        PaneKind::Git => match &bench.git {
            Some(pane) => {
                if bench.claims_keys(TakesKeys::Git) {
                    let focused = pane.clone();
                    cx.defer_in(window, move |_, window, cx| {
                        focused.update(cx, |pane, cx| pane.take_keys(window, cx));
                    });
                }
                pane.clone().into_any_element()
            }
            None => ui::pane(cx).into_any_element(),
        },
        PaneKind::Search => match &bench.search {
            Some(pane) => ui::pane(cx).child(pane.clone()).into_any_element(),
            None => ui::pane(cx).into_any_element(),
        },
        PaneKind::Board => match &bench.board {
            Some(pane) => {
                if bench.claims_keys(TakesKeys::Board) {
                    let focus = pane.read(cx).focus_handle(cx);
                    cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
                }
                ui::pane(cx).child(pane.clone()).into_any_element()
            }
            None => ui::pane(cx).into_any_element(),
        },
        PaneKind::Plugin => match bench.plugin_panes.get(id) {
            Some(pane) => ui::pane(cx).child(pane.clone()).into_any_element(),
            None => ui::pane(cx).into_any_element(),
        },
        PaneKind::Agent => home::agent_view(bench, tab, id, target, cx),
        PaneKind::Desk => desk::pane(bench, target, window, cx),
        PaneKind::ProjectSettings => crate::project_settings::pane(bench, target, window, cx),
        PaneKind::InstallSikemux => crate::install_pane::pane(bench, target, cx),
        PaneKind::Unknown => ui::pane(cx)
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::theme::ui_text(12.))
                    .text_color(crate::theme::Tokens::get(cx).text_tertiary)
                    .child("This tab needs a newer Sikemux."),
            )
            .into_any_element(),
        _ => home::agent_view(bench, tab, id, "", cx),
    }
}

#[allow(clippy::too_many_arguments)]
fn split(
    bench: &Workbench,
    tab: &str,
    id: &str,
    dir: SplitDir,
    panes: &[Pane],
    kept: &[u16],
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let across = dir == SplitDir::Row;
    let sizes = bench.panes.sizes(id, kept).to_vec();
    let mut row = div()
        .id(SharedString::from(id.to_owned()))
        .flex()
        .when(!across, |row| row.flex_col())
        .size_full()
        .min_w_0()
        .min_h_0();
    {
        let split = id.to_owned();
        row = row.on_drag_move(
            cx.listener(move |bench, event: &DragMoveEvent<Divider>, _, cx| {
                let divider = event.drag(cx).clone();
                if divider.split != split {
                    return;
                }
                let kept = bench
                    .tab()
                    .and_then(|tab| split_sizes(&tab.pane, &split))
                    .unwrap_or_default();
                bench
                    .panes
                    .drag(&divider, kept, event.bounds, event.event.position, across);
                bench.parts.redraw(&bench.parts.stage, cx);
            }),
        );
        row = row
            .on_drop(cx.listener(|bench, _: &Divider, _, _| bench.finish_resize()))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|bench, _, _, _| bench.finish_resize()),
            );
    }
    for (at, (child, share)) in panes.iter().zip(sizes.iter().copied()).enumerate() {
        if at > 0 {
            row = row.child(divider(tab, id, at - 1, across));
        }
        let mut cell = div().flex().min_w_0().min_h_0().flex_basis(px(0.));
        cell.style().flex_grow = Some(f32::from(share.max(1)));
        row = row.child(
            cell.when(across, |cell| cell.h_full())
                .when(!across, |cell| cell.w_full())
                .child(pane(bench, tab, child, window, cx)),
        );
    }
    row.into_any_element()
}

fn divider(tab: &str, split: &str, at: usize, across: bool) -> impl IntoElement {
    let value = Divider {
        tab: tab.to_owned(),
        split: split.to_owned(),
        at,
    };
    handle(format!("{split}-divider-{at}"), value, across)
}

/// The gap between two panes that a drag resizes, carrying `value` to whatever hears the
/// drag move: side by side when `across`, else one over the other.
pub(crate) fn handle<T: Clone + 'static>(
    id: impl Into<SharedString>,
    value: T,
    across: bool,
) -> impl IntoElement {
    div()
        .id(id.into())
        .flex_none()
        .when(across, |divider| {
            divider
                .w(px(GAP))
                .h_full()
                .cursor(CursorStyle::ResizeLeftRight)
        })
        .when(!across, |divider| {
            divider
                .h(px(GAP))
                .w_full()
                .cursor(CursorStyle::ResizeUpDown)
        })
        .on_drag(value, |_, _, _, cx| cx.new(|_| EmptyView))
}

fn split_sizes(pane: &Pane, split: &str) -> Option<Vec<u16>> {
    match pane {
        Pane::Split { id, sizes, .. } if id == split => Some(sizes.clone()),
        Pane::Split { panes, .. } => panes.iter().find_map(|child| split_sizes(child, split)),
        Pane::View { .. } => None,
    }
}

impl Panes {
    fn drag(
        &mut self,
        divider: &Divider,
        kept: Vec<u16>,
        bounds: Bounds<Pixels>,
        at: Point<Pixels>,
        across: bool,
    ) {
        let resizing = match &mut self.resizing {
            Some(resizing) if resizing.split == divider.split && resizing.from == kept => resizing,
            _ => self.resizing.insert(Resizing {
                tab: divider.tab.clone(),
                split: divider.split.clone(),
                sizes: kept.clone(),
                from: kept,
            }),
        };
        let sizes = &mut resizing.sizes;
        let (Some(&before), Some(&after)) = (sizes.get(divider.at), sizes.get(divider.at + 1))
        else {
            return;
        };
        let (start, length, pointer) = if across {
            (bounds.left(), bounds.size.width, at.x)
        } else {
            (bounds.top(), bounds.size.height, at.y)
        };
        let room = f32::from(length) - GAP * (sizes.len() - 1) as f32;
        if room <= 0. {
            return;
        }
        let total: f32 = sizes.iter().map(|&size| f32::from(size)).sum();
        let ahead: f32 = sizes[..divider.at]
            .iter()
            .map(|&size| f32::from(size))
            .sum();
        let offset = f32::from(pointer - start) - GAP * (divider.at as f32 + 0.5);
        let pair = f32::from(before) + f32::from(after);
        let least = (MIN_PANE / room * total).min(pair / 2.);
        let wanted = (offset / room * total - ahead).clamp(least, pair - least);
        sizes[divider.at] = wanted.round() as u16;
        sizes[divider.at + 1] = (pair - wanted).round() as u16;
    }
}

impl Workbench {
    fn finish_resize(&mut self) {
        let Some(resizing) = &self.panes.resizing else {
            return;
        };
        if resizing.sizes == resizing.from {
            self.panes.resizing = None;
            return;
        }
        let Some(strip) = self.strip() else {
            return;
        };
        let params = json!({
            "tab": resizing.tab,
            "split": resizing.split,
            "sizes": resizing.sizes,
        });
        self.call("resizeSplit", strip.with(params));
    }

    /// The view in focus in the tab in front, else its first.
    pub(crate) fn focused_view(&self) -> Option<String> {
        let tab = self.tab()?;
        self.panes
            .focused
            .clone()
            .filter(|id| view_in(&tab.pane, id).is_some())
            .or_else(|| first_view(&tab.pane).map(str::to_owned))
    }

    /// What the view in focus in the tab in front shows, else its first.
    pub fn focused_kind(&self) -> Option<PaneKind> {
        let tab = self.tab()?;
        match view_in(&tab.pane, &self.focused_view()?)? {
            Pane::View { kind, .. } => Some(*kind),
            Pane::Split { .. } => None,
        }
    }

    /// A text size key that no chat, editor or terminal in focus took sizes the text of the
    /// view in front, as on the Mac, and the terminals' when that is neither a chat nor an editor.
    pub(crate) fn resize_front_text(&self, grow: Option<bool>) {
        match self.focused_kind() {
            Some(PaneKind::Agent) => crate::chat::resize_text(self, grow),
            Some(PaneKind::Files) => crate::editor::resize_text(self, grow),
            _ => crate::terminal::resize_text(self, grow),
        }
    }

    /// A fresh shell beside the view in focus, which takes the focus: to its right for a row,
    /// below it for a column, or behind it for a stack, where it comes in front.
    /// The core picks the shell for the strip, taking one it started ahead when it can.
    pub fn split_front(&mut self, dir: SplitDir, cx: &mut Context<Self>) {
        let (Some(strip), Some(tab), Some(pane)) = (
            self.strip(),
            self.tab().map(|tab| tab.id.clone()),
            self.focused_view(),
        ) else {
            return;
        };
        let side = match dir {
            SplitDir::Row => "right",
            SplitDir::Column => "bottom",
            SplitDir::Stack => "stack",
        };
        let asks = self.asks.clone();
        let shell = crate::core::request(&self.asks, "stripShell", strip.with(json!({})));
        cx.spawn(async move |bench, cx| {
            let session = match shell.await.and_then(|shell| {
                serde_json::from_value::<SessionId>(shell).map_err(|error| error.to_string())
            }) {
                Ok(session) => session,
                Err(error) => {
                    eprintln!("sikemux: no shell for the split: {error}");
                    return;
                }
            };
            let params = json!({
                "tab": tab,
                "pane": pane,
                "side": side,
                "kind": PaneKind::Terminal,
                "target": session.to_string(),
            });
            let added = crate::core::request(&asks, "splitPane", strip.with(params)).await;
            let _ = bench.update(cx, |bench, cx| {
                bench.panes.zoomed = None;
                if let Some(added) = added
                    .ok()
                    .and_then(|added| added.as_str().map(str::to_owned))
                {
                    bench.panes.focused = Some(added);
                }
                bench.parts.redraw(&bench.parts.stage, cx);
            });
        })
        .detach();
    }

    /// Moves the focus to the view beside the one in focus, as the views were last drawn.
    pub fn focus_towards(&mut self, towards: Towards, window: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.focused_view() else {
            return;
        };
        let Some(to) = neighbour(&self.panes.drawn.borrow(), &from, towards) else {
            return;
        };
        self.panes.focused = Some(to.clone());
        self.focus_view(&to, window, cx);
        self.parts.redraw(&self.parts.stage, cx);
    }

    /// Puts the keys in the view `id` of the tab in front: a terminal's screen or a chat's draft.
    pub(crate) fn focus_view(&self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.place() != crate::hosts::Place::This {
            return self.focus.focus(window, cx);
        }
        let Some(Pane::View { kind, target, .. }) =
            self.tab().and_then(|tab| view_in(&tab.pane, id))
        else {
            return;
        };
        match kind {
            PaneKind::Terminal => {
                let focus = target.parse::<SessionId>().ok().and_then(|session| {
                    let key = self.terminals.key(&self.place(), session);
                    self.terminals.focus(key)
                });
                if let Some(focus) = focus {
                    window.focus(focus, cx);
                }
            }
            PaneKind::Agent => {
                if let Some(composing) = self
                    .chats
                    .get(target)
                    .and_then(|chat| chat.composer.as_ref())
                {
                    composing.draft.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
            _ => self.focus.focus(window, cx),
        }
    }

    /// Moves the divider beside the view in focus `towards`, growing the view: along the
    /// nearest split that runs that way, into the pane beyond it.
    pub fn resize_towards(&mut self, towards: Towards) {
        let (Some(strip), Some(tab), Some(view)) = (self.strip(), self.tab(), self.focused_view())
        else {
            return;
        };
        let across = matches!(towards, Towards::Left | Towards::Right);
        let mut path = Vec::new();
        if !path_to(&tab.pane, &view, &mut path) {
            return;
        }
        let along = path.iter().rev().find_map(|(split, at)| match split {
            Pane::Split { id, dir, sizes, .. }
                if *dir != SplitDir::Stack && (*dir == SplitDir::Row) == across =>
            {
                Some((id.clone(), sizes.clone(), *at))
            }
            _ => None,
        });
        let Some((split, mut sizes, at)) = along else {
            return;
        };
        let beyond = match towards {
            Towards::Right | Towards::Down => at + 1,
            Towards::Left | Towards::Up if at > 0 => at - 1,
            _ => return,
        };
        let Some(&spare) = sizes.get(beyond) else {
            return;
        };
        let total: u32 = sizes.iter().map(|&size| u32::from(size)).sum();
        let scale = |thousandths: u16| (u32::from(thousandths) * total / 1000) as u16;
        let give = scale(RESIZE_STEP).min(spare.saturating_sub(scale(LEAST_SHARE)));
        if give == 0 {
            return;
        }
        sizes[at] += give;
        sizes[beyond] -= give;
        let params = json!({ "tab": tab.id, "split": split, "sizes": sizes });
        self.call("resizeSplit", strip.with(params));
    }

    /// Lets the view in focus fill its tab, or puts the tab's other views back.
    pub fn toggle_zoom(&mut self, cx: &mut Context<Self>) {
        self.panes.zoomed = match self.panes.zoomed.take() {
            Some(_) => None,
            None => self
                .tab()
                .filter(|tab| matches!(tab.pane, Pane::Split { .. }))
                .and_then(|_| self.focused_view()),
        };
        self.parts.redraw(&self.parts.stage, cx);
    }

    /// Puts a view of `kind` showing `target` beside the view `pane` of `tab`.
    pub fn split_pane(
        &self,
        strip: &Strip,
        tab: &str,
        pane: &str,
        side: &str,
        kind: PaneKind,
        target: &str,
    ) {
        let params = json!({
            "tab": tab,
            "pane": pane,
            "side": side,
            "kind": kind,
            "target": target,
        });
        self.call("splitPane", strip.with(params));
    }

    /// Notes the side of the view `pane` of `tab` that a tab dragged over it would take: the
    /// edge the pointer is nearest that the layout still has room on.
    fn aim_split(
        &mut self,
        tab: &str,
        pane: &str,
        event: &DragMoveEvent<DraggedTab>,
        cx: &mut Context<Self>,
    ) {
        let over = event.bounds.contains(&event.event.position);
        let dragged = event.drag(cx).clone();
        let open = self
            .tab()
            .filter(|shown| shown.id == tab)
            .map(|shown| open_sides(&shown.pane))
            .unwrap_or_default();
        let aimable = over
            && dragged.tab != tab
            && self.strip().as_ref() == Some(&dragged.strip)
            && !open.is_empty();
        let next = aimable.then(|| {
            let (bounds, at) = (event.bounds, event.event.position);
            let distance = |side: Side| match side {
                Side::Left => f32::from(at.x - bounds.left()) / f32::from(bounds.size.width),
                Side::Right => f32::from(bounds.right() - at.x) / f32::from(bounds.size.width),
                Side::Top => f32::from(at.y - bounds.top()) / f32::from(bounds.size.height),
                Side::Bottom => f32::from(bounds.bottom() - at.y) / f32::from(bounds.size.height),
            };
            let side = open
                .iter()
                .copied()
                .min_by(|a, b| distance(*a).total_cmp(&distance(*b)))
                .unwrap_or(Side::Right);
            Aimed {
                tab: tab.to_owned(),
                pane: pane.to_owned(),
                side,
                from: dragged.tab.clone(),
            }
        });
        let leaving = !over
            && self
                .panes
                .aimed
                .as_ref()
                .is_some_and(|aimed| aimed.pane == pane);
        if (next.is_some() || leaving) && self.panes.aimed != next {
            self.panes.aimed = next;
            self.parts.redraw(&self.parts.stage, cx);
        }
    }

    /// Moves the dropped tab's view beside the view `pane`, on the side it was aimed at.
    fn drop_split(&mut self, pane: &str, dropped: &DraggedTab, cx: &mut Context<Self>) {
        let Some(aimed) = self
            .panes
            .aimed
            .take()
            .filter(|aimed| aimed.pane == pane && aimed.from == dropped.tab)
        else {
            return;
        };
        let params = json!({
            "tab": aimed.tab,
            "pane": aimed.pane,
            "side": aimed.side.word(),
            "fromTab": aimed.from,
        });
        self.call("splitPane", dropped.strip.with(params));
        self.parts.redraw(&self.parts.stage, cx);
    }

    pub fn close_pane(&self, strip: &Strip, tab: &str, pane: &str) {
        self.call("closePane", strip.with(json!({ "tab": tab, "pane": pane })));
    }

    /// The view of a split `tab` its menu can move out into a tab of its own, as the Mac's
    /// does: the view in focus, else the first, when it is a terminal, Git, a search or a
    /// saved file.
    pub(crate) fn separable(&self, tab: &Tab) -> Option<(String, PaneKind)> {
        if matches!(tab.pane, Pane::View { .. }) {
            return None;
        }
        let view = self
            .panes
            .focused
            .clone()
            .filter(|id| view_in(&tab.pane, id).is_some())
            .or_else(|| first_view(&tab.pane).map(str::to_owned))?;
        let Pane::View { kind, target, .. } = view_in(&tab.pane, &view)? else {
            return None;
        };
        let saved = || {
            !self
                .editors
                .is_dirty(&crate::editor::FileKey::new(self.place(), target))
        };
        match kind {
            PaneKind::Terminal | PaneKind::Git | PaneKind::Search => Some((view, *kind)),
            PaneKind::Files if saved() => Some((view, *kind)),
            _ => None,
        }
    }

    pub fn separate_pane(&self, strip: &Strip, tab: &str, pane: &str) {
        self.call(
            "separatePane",
            strip.with(json!({ "tab": tab, "pane": pane })),
        );
    }
}
