use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::{Button as BaseButton, Selectable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};
use smallvec::SmallVec;

use super::controls::OnClick;
use super::{caps, icon, mark_box, meta, text};
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    Tokens, RADIUS_2, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_SM, TEXT_XS,
};

/// The hover group a row puts itself in, for a glyph in it that follows its text.
pub const ROW: &str = "row";

/// What sits at the end of a row or tab and makes way for the buttons that show there
/// while the pointer is over `group`.
pub fn gives_way(group: &'static str, element: impl IntoElement) -> gpui_kit::Div {
    div()
        .flex_none()
        .group_hover(group, |hidden| hidden.opacity(0.))
        .child(element)
}

/// The buttons at the trailing edge of a row or tab, shown while the pointer is over `group`
/// in place of what gives way to them.
pub fn hover_actions(group: &'static str) -> gpui_kit::Div {
    div()
        .absolute()
        .right(px(4.))
        .flex()
        .items_center()
        .opacity(0.)
        .group_hover(group, |actions| actions.opacity(1.))
}

/// The rails' row: a mark in its slot, a title, a line under it when it has one,
/// then whatever it adds as children, and a status mark pinned at the right.
#[derive(IntoElement)]
pub struct Row {
    id: ElementId,
    mark: Option<AnyElement>,
    slot: f32,
    title: SharedString,
    /// What shows in place of the title, such as a field while it is edited.
    title_view: Option<AnyElement>,
    quiet: bool,
    bold: bool,
    chosen: bool,
    /// The chosen surface is a copy gliding onto the row, which draws none of its own yet.
    glided: bool,
    compact: bool,
    centered: bool,
    subtitle: Option<AnyElement>,
    detail: Option<SharedString>,
    status: Option<AnyElement>,
    right: f32,
    /// The trailing room the row keeps while the pointer is over the hover group named, for
    /// the buttons that show there then.
    hover_right: Option<(&'static str, f32)>,
    trailing: SmallVec<[AnyElement; 2]>,
    on_click: Option<OnClick>,
}

impl Row {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            mark: None,
            slot: 22.,
            title: title.into(),
            title_view: None,
            quiet: false,
            bold: false,
            chosen: false,
            glided: false,
            compact: false,
            centered: false,
            subtitle: None,
            detail: None,
            status: None,
            right: 8.,
            hover_right: None,
            trailing: SmallVec::new(),
            on_click: None,
        }
    }

    /// The mark and the slot it sits in; rows line their marks up on a 20px
    /// column 4px in, whatever the slot.
    pub fn mark(mut self, slot: f32, mark: impl IntoElement) -> Self {
        self.slot = slot;
        self.mark = Some(mark.into_any_element());
        self
    }

    /// A note after the title that gives way first when the row is narrow.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A title in the body ink rather than the full one, as the rail's actions.
    pub fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }

    pub fn bold(mut self, bold: bool) -> Self {
        self.bold = bold;
        self
    }

    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }

    pub fn glided(mut self, glided: bool) -> Self {
        self.glided = glided;
        self
    }

    /// Centres a two-line row's mark on both lines rather than beside the title, for a
    /// picture as tall as the two.
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    pub fn subtitle(mut self, subtitle: impl IntoElement) -> Self {
        self.subtitle = Some(subtitle.into_any_element());
        self
    }

    pub fn title_view(mut self, view: Option<impl IntoElement>) -> Self {
        self.title_view = view.map(IntoElement::into_any_element);
        self
    }

    pub fn status(mut self, status: Option<impl IntoElement>) -> Self {
        self.status = status.map(IntoElement::into_any_element);
        self
    }

    /// A pixel shorter on one line, as the rail's Compact density draws it.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// Keeps `right` free at the trailing edge while the pointer is over `group`, so what
    /// shows there on hover never covers the row's text, and hides the status under it.
    pub fn hover_padding(mut self, group: &'static str, right: f32) -> Self {
        self.hover_right = Some((group, right));
        self
    }

    pub fn padding_right(mut self, right: f32) -> Self {
        self.right = right;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Selectable for Row {
    fn selected(self, selected: bool) -> Self {
        self.chosen(selected)
    }

    fn is_selected(&self) -> bool {
        self.chosen
    }
}

impl ParentElement for Row {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.trailing.extend(elements);
    }
}

impl RenderOnce for Row {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let (over, keep_over) = motion::hovered(&self.id, window, cx);
        let hovered = ElementId::NamedChild(Arc::new(self.id.clone()), "hover".into());
        let lit = motion::follow(
            hovered,
            if over && !self.chosen { 1. } else { 0. },
            motion::ROW_TINT,
            window,
            cx,
        );
        let (rest, rest_ink) = match (self.chosen, self.glided) {
            (true, false) => (t.surface_chosen, t.text_primary),
            (true, true) => (t.surface_chosen.opacity(0.), t.text_primary),
            (false, _) => (t.surface_hover.opacity(0.), t.text_secondary),
        };
        let tint = motion::blend(rest, t.surface_hover, lit);
        let ink = motion::blend(rest_ink, t.text_primary, lit);
        let two_lines = self.subtitle.is_some();
        let top_aligned = two_lines && !self.centered;
        let right = if self.status.is_some() {
            28.
        } else {
            self.right
        };
        let on_click = self.on_click;
        BaseButton::new(self.id)
            .group(ROW)
            .selected(self.chosen)
            .accessibility_label(self.title.clone())
            .relative()
            .w_full()
            .h(px(match (two_lines, self.compact) {
                (true, _) => 40.,
                (false, true) => 27.,
                (false, false) => 28.,
            }))
            .pl(px(4. + (20. - self.slot) / 2.))
            .pr(px(right))
            .when_some(self.hover_right, |row, (group, room)| {
                row.group_hover(group, move |style| style.pr(px(room.max(right))))
            })
            .flex()
            .justify_start()
            .gap(SPACE_2)
            .map(|row| {
                if top_aligned {
                    row.items_start().pt(SPACE_1)
                } else {
                    row.items_center()
                }
            })
            .rounded(RADIUS_2)
            .text_color(ink)
            .bg(tint)
            .on_hover(keep_over)
            .when_some(on_click, |row, handler| {
                row.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .children(
                self.mark.map(|mark| {
                    mark_box(self.slot, mark).when(top_aligned, |slot| slot.mt(px(-2.)))
                }),
            )
            .child(
                div()
                    .flex_1()
                    .when(self.detail.is_some(), |title| title.flex_auto())
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div().flex().items_center().gap(SPACE_2).child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .line_height(crate::theme::ui_text(18.))
                                .font_weight(if self.bold {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .when(!self.quiet, |title| title.text_color(t.text_primary))
                                .map(|title| match self.title_view {
                                    Some(view) => title.flex_1().child(view),
                                    None => title.child(self.title),
                                }),
                        ),
                    )
                    .children(self.subtitle),
            )
            .children(self.detail.map(|detail| {
                let mut detail = meta(detail, cx)
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis();
                detail.style().flex_shrink = Some(100.);
                detail
            }))
            .children(self.trailing)
            .children(self.status.map(|status| {
                let status = match self.hover_right {
                    Some((group, _)) => gives_way(group, status).into_any_element(),
                    None => status,
                };
                div()
                    .absolute()
                    .right(px(4.))
                    .top_0()
                    .map(|slot| match top_aligned {
                        true => slot.h(px(28.)),
                        false => slot.bottom_0(),
                    })
                    .flex()
                    .items_center()
                    .child(mark_box(20., status))
            }))
    }
}

/// A rail's group: its spaced capitals, a rule and its actions, then its rows
/// or what it says when it has none.
#[derive(IntoElement)]
pub struct Group {
    label: &'static str,
    actions: Option<AnyElement>,
    toolbar: Option<AnyElement>,
    empty: SharedString,
    first: bool,
    rows: SmallVec<[AnyElement; 8]>,
}

impl Group {
    pub fn new(label: &'static str, empty: impl Into<SharedString>) -> Self {
        Self {
            label,
            actions: None,
            toolbar: None,
            empty: empty.into(),
            first: false,
            rows: SmallVec::new(),
        }
    }

    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }

    /// What sits between the group's heading and its rows, such as a filter over them.
    pub fn toolbar(mut self, toolbar: Option<impl IntoElement>) -> Self {
        self.toolbar = toolbar.map(IntoElement::into_any_element);
        self
    }

    /// The first group in its rail, which sits on the rail's own padding.
    pub fn first(mut self) -> Self {
        self.first = true;
        self
    }
}

impl ParentElement for Group {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.rows.extend(elements);
    }
}

impl RenderOnce for Group {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = Tokens::get(cx).border;
        let empty = self.rows.is_empty();
        div()
            .flex()
            .flex_col()
            .when(!self.first, |group| group.mt(SPACE_4))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .pt(SPACE_1)
                    .pr(SPACE_1H)
                    .pb(SPACE_1H)
                    .pl(SPACE_2)
                    .child(caps(self.label, cx).flex_none())
                    .child(div().flex_1().h(px(1.)).bg(border))
                    .children(self.actions),
            )
            .children(self.toolbar)
            .when(empty, |group| {
                group.child(
                    div().p(SPACE_3).child(
                        text(self.empty, TEXT_XS, FontWeight::NORMAL)
                            .text_color(Tokens::get(cx).text_tertiary),
                    ),
                )
            })
            .children(self.rows)
    }
}

/// A tab in the strip over the stage.
#[derive(IntoElement)]
pub struct Tab {
    id: ElementId,
    mark: Option<AnyElement>,
    label: SharedString,
    note: Option<SharedString>,
    counts: Option<AnyElement>,
    tail: Option<AnyElement>,
    active: bool,
    /// The chosen surface is a copy gliding onto the tab, which draws none of its own yet.
    glided: bool,
    /// A preview tab, which the next file opened replaces, draws its label slanted.
    italic: bool,
    on_click: Option<OnClick>,
    on_close: Option<OnClick>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            mark: None,
            label: label.into(),
            note: None,
            counts: None,
            tail: None,
            active: false,
            glided: false,
            italic: false,
            on_click: None,
            on_close: None,
        }
    }

    pub fn mark(mut self, mark: impl IntoElement) -> Self {
        self.mark = Some(mark.into_any_element());
        self
    }

    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    /// What follows the label, such as an agent's @name.
    pub fn note(mut self, note: Option<SharedString>) -> Self {
        self.note = note;
        self
    }

    /// What an agent has going, as its rail row counts it: subagents, shells, a timer.
    pub fn counts(mut self, counts: Option<impl IntoElement>) -> Self {
        self.counts = counts.map(IntoElement::into_any_element);
        self
    }

    /// The 20px status slot at the end.
    pub fn tail(mut self, tail: Option<impl IntoElement>) -> Self {
        self.tail = tail.map(IntoElement::into_any_element);
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn glided(mut self, glided: bool) -> Self {
        self.glided = glided;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Tab {
    /// Closes the tab from a button that shows while the pointer is over it, in place of its status.
    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let (over, keep_over) = motion::hovered(&self.id, window, cx);
        let closing = self.on_close.clone().filter(|_| over);
        let (rest, rest_ink) = match (self.active, self.glided) {
            (true, false) => (t.surface_chosen, t.text_primary),
            (true, true) => (t.surface_chosen.opacity(0.), t.text_primary),
            (false, _) => (crate::see_through::pill(t.surface, cx), t.text_tertiary),
        };
        let hovered = ElementId::NamedChild(Arc::new(self.id.clone()), "hover".into());
        let lit = motion::follow(
            hovered,
            if over { 1. } else { 0. },
            motion::PILL_TINT,
            window,
            cx,
        );
        let tint = motion::blend(rest, t.surface_hover, lit);
        let ink = motion::blend(rest_ink, t.text_secondary, lit);
        let close_id = ElementId::NamedChild(Arc::new(self.id.clone()), "close".into());
        let label = self.label.clone();
        let edge = gpui_kit::Hsla {
            a: 0.06,
            ..t.text_primary
        };
        let on_click = self.on_click;
        let wrap_id = ElementId::NamedChild(Arc::new(self.id.clone()), "wrap".into());
        let hidden = closing.is_some();
        let makes_way = move |element: AnyElement| {
            div()
                .flex_none()
                .when(hidden, |way| way.opacity(0.))
                .child(element)
        };
        let room = self.note.is_none()
            && self.counts.is_none()
            && self.tail.is_none()
            && self.on_close.is_some();
        div()
            .id(wrap_id)
            .relative()
            .flex_none()
            .h(px(26.))
            .min_w(px(120.))
            .flex()
            .items_center()
            .gap(SPACE_1)
            .rounded(RADIUS_2)
            .bg(tint)
            .text_color(ink)
            .when(!self.active, |tab| tab.border_1().border_color(edge))
            .on_hover(keep_over)
            .child(
                BaseButton::new(self.id)
                    .selected(self.active)
                    .accessibility_label(self.label.clone())
                    .flex_1()
                    .h_full()
                    .pl(SPACE_2)
                    .pr(px(match (self.tail.is_some(), room) {
                        (true, _) => 3.,
                        (false, true) => 26.,
                        (false, false) => 8.,
                    }))
                    .flex()
                    .items_center()
                    .justify_start()
                    .gap(SPACE_2)
                    .when_some(on_click, |tab, handler| {
                        tab.on_click(move |event, window, cx| handler(event, window, cx))
                    })
                    .children(self.mark.map(|mark| mark_box(22., mark)))
                    .child(
                        text(self.label, TEXT_XS, FontWeight::MEDIUM)
                            .max_w(px(160.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .when(self.italic, |label| label.italic()),
                    )
                    .children(
                        self.note
                            .map(|note| makes_way(meta(note, cx).pr(SPACE_1).into_any_element())),
                    )
                    .children(self.counts.map(makes_way))
                    .children(
                        self.tail
                            .map(|tail| makes_way(mark_box(20., tail).into_any_element())),
                    ),
            )
            .children(closing.map(|close| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(3.))
                    .flex()
                    .items_center()
                    .child(close_button(
                        close_id,
                        format!("Close {label}"),
                        move |event, window, cx| close(event, window, cx),
                        cx,
                    ))
            }))
    }
}

/// The hover group a close button puts its glyph in, so the glyph reddens with it.
const CLOSE: &str = "tab-close";

/// The × that closes a tab or a rail row: no box until the pointer is on it, then red on a
/// red wash, as the Mac's tab and row closes are.
pub fn close_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    BaseButton::new(id)
        .group(CLOSE)
        .accessibility_label(label.into())
        .size(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_2)
        .text_color(t.text_tertiary)
        .hover(move |style| {
            style.text_color(t.danger).bg(gpui_kit::Hsla {
                a: 0.18,
                ..t.danger
            })
        })
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx)
        })
        .child(
            icon(Icon::Close, 11.)
                .text_color(t.text_tertiary)
                .group_hover(CLOSE, move |style| style.text_color(t.danger)),
        )
}

/// A row of tabs over a page, the chosen one underlined in ink, with what acts on the page
/// at its end.
pub fn page_tabs(
    tabs: impl IntoIterator<Item = gpui_kit::Stateful<gpui_kit::Div>>,
    end: Option<AnyElement>,
) -> gpui_kit::Div {
    div()
        .flex_none()
        .px(SPACE_4)
        .flex()
        .items_center()
        .gap(px(18.))
        .children(tabs)
        .children(end.map(|end| div().ml_auto().child(end)))
}

/// One tab of a page's row: its glyph and name, then how many it holds or how it stands.
pub fn page_tab(
    id: &str,
    glyph: Icon,
    label: &'static str,
    mark: Option<AnyElement>,
    on: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    div()
        .id(SharedString::from(format!("{id}-{label}")))
        .h(px(34.))
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .border_b_2()
        .border_color(if on {
            t.text_primary
        } else {
            gpui_kit::transparent_black()
        })
        .text_size(crate::theme::ui_text(TEXT_SM))
        .text_color(if on { t.text_primary } else { t.text_secondary })
        .cursor_pointer()
        .when(!on, |tab| {
            tab.hover(move |tab| tab.text_color(t.text_primary))
        })
        .child(super::icon(glyph, 13.).text_color(if on {
            t.text_primary
        } else {
            t.text_tertiary
        }))
        .child(label)
        .children(mark)
}
