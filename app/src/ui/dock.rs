//! A list docked over the top edge of what it serves, such as the composer's menu of
//! agents, files and commands: rows with a mark, a name and what it is, one of them
//! picked, chosen by click or by the keys its owner binds.

use std::rc::Rc;

use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, Div, InteractiveElement, IntoElement, MouseButton,
    ParentElement, RenderOnce, SharedString, Styled, Window,
};

use crate::theme::{self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, TEXT_3XS_H};

type Choose = Rc<dyn Fn(&mut Window, &mut App)>;

/// One row: a mark, a name, what it is, and what goes at its far end.
pub struct DockRow {
    name: SharedString,
    mark: Option<AnyElement>,
    detail: Option<SharedString>,
    /// How the thing stands, such as an agent at work, drawn after its detail.
    state: Option<AnyElement>,
    aside: Option<SharedString>,
    /// The heading a run of rows starts under.
    group: Option<SharedString>,
    choose: Choose,
}

impl DockRow {
    pub fn new(name: impl Into<SharedString>, choose: Choose) -> Self {
        Self {
            name: name.into(),
            mark: None,
            detail: None,
            state: None,
            aside: None,
            group: None,
            choose,
        }
    }

    pub fn mark(mut self, mark: impl IntoElement) -> Self {
        self.mark = Some(mark.into_any_element());
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn state(mut self, state: Option<AnyElement>) -> Self {
        self.state = state;
        self
    }

    pub fn aside(mut self, aside: Option<SharedString>) -> Self {
        self.aside = aside;
        self
    }

    pub fn group(mut self, group: Option<SharedString>) -> Self {
        self.group = group;
        self
    }
}

/// One of the kinds of thing a menu can list, in its heading: the key that asks for it and its
/// name. A click switches the menu to it.
pub struct DockScope {
    pub key: SharedString,
    pub label: SharedString,
    pub on: bool,
    pub choose: Choose,
}

#[derive(IntoElement)]
pub struct DockMenu {
    rows: Vec<DockRow>,
    selected: usize,
    /// What the menu says while it has no rows.
    empty: Option<SharedString>,
    /// What it lists is on its way, so rows the size of real ones hold its height.
    loading: bool,
    /// What it lists, as the kinds it can switch between or a title, and what taking a row
    /// does, over its rows.
    heading: Option<(Lead, SharedString)>,
    /// What was typed to narrow it, whose letters its rows mark.
    query: SharedString,
}

enum Lead {
    Scopes(Vec<DockScope>),
    Title(SharedString),
}

/// How wide each loading row's last bar is, as the Mac draws them.
const LOADING_ROWS: [f32; 3] = [0.72, 0.54, 0.63];

impl DockMenu {
    pub fn new(rows: Vec<DockRow>, selected: usize) -> Self {
        Self {
            rows,
            selected,
            empty: None,
            loading: false,
            heading: None,
            query: SharedString::default(),
        }
    }

    /// Marks in each row the letters `query` found there.
    pub fn found(mut self, query: impl Into<SharedString>) -> Self {
        self.query = query.into();
        self
    }

    /// A menu whose rows are on their way.
    pub fn loading() -> Self {
        Self {
            loading: true,
            ..Self::new(Vec::new(), 0)
        }
    }

    /// A menu with nothing to offer, saying why.
    pub fn empty(text: impl Into<SharedString>) -> Self {
        Self {
            empty: Some(text.into()),
            ..Self::new(Vec::new(), 0)
        }
    }

    pub fn heading(mut self, scopes: Vec<DockScope>, aside: impl Into<SharedString>) -> Self {
        self.heading = Some((Lead::Scopes(scopes), aside.into()));
        self
    }

    /// A heading that names what the menu lists, for one that switches to nothing else.
    pub fn titled(
        mut self,
        title: impl Into<SharedString>,
        aside: impl Into<SharedString>,
    ) -> Self {
        self.heading = Some((Lead::Title(title.into()), aside.into()));
        self
    }
}

impl RenderOnce for DockMenu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let (query, mark) = (self.query.clone(), super::found_mark(cx));
        let rows = self.rows.into_iter().enumerate().flat_map(|(index, row)| {
            let heading = row.group.map(|group| {
                div()
                    .px(SPACE_2H)
                    .pt(px(7.))
                    .pb(px(3.))
                    .border_t_1()
                    .border_color(t.line_soft)
                    .text_size(crate::theme::ui_text(9.5))
                    .text_color(t.text_disabled)
                    .child(group)
                    .into_any_element()
            });
            let choose = row.choose;
            let line = div()
                .id(("dock-row", index))
                .min_h(px(30.))
                .px(SPACE_2H)
                .py(px(3.))
                .flex()
                .items_center()
                .gap(SPACE_2)
                .cursor_pointer()
                .when(index == self.selected, |line| line.bg(t.surface_chosen))
                .hover(move |line| line.bg(t.surface_hover))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    choose(window, cx);
                })
                .child(
                    div()
                        .w(px(16.))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .children(row.mark),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_primary)
                        .child(super::found_with(row.name, &query, mark)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .children(
                            row.detail
                                .map(|detail| super::found_with(detail, &query, mark)),
                        ),
                )
                .children(row.state)
                .children(row.aside.map(|aside| {
                    div()
                        .flex_none()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(9.5))
                        .text_color(t.text_disabled)
                        .child(aside)
                }))
                .into_any_element();
            heading.into_iter().chain([line])
        });
        let loading = self.loading;
        let heading = self.heading.map(|(lead, aside)| {
            let (scopes, title) = match lead {
                Lead::Scopes(scopes) => (scopes, None),
                Lead::Title(title) => (Vec::new(), Some(title)),
            };
            div()
                .min_h(px(26.))
                .px(SPACE_1H)
                .pb(px(3.))
                .flex()
                .items_center()
                .gap(SPACE_0)
                .children(scopes.into_iter().enumerate().map(|(index, scope)| {
                    let choose = scope.choose;
                    div()
                        .id(("dock-scope", index))
                        .h(px(22.))
                        .px(SPACE_1H)
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .rounded(theme::RADIUS_2)
                        .cursor_pointer()
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(if scope.on {
                            t.text_primary
                        } else {
                            t.text_tertiary
                        })
                        .when(scope.on, |chip| chip.bg(t.surface_chosen))
                        .hover(move |chip| chip.text_color(t.text_primary))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            choose(window, cx);
                        })
                        .map(|chip| match loading && scope.on {
                            true => {
                                chip.child(Spinner::new().with_size(px(10.)).color(t.text_tertiary))
                            }
                            false => chip.child(
                                div()
                                    .font_family(theme::MONO)
                                    .text_size(crate::theme::ui_text(9.5))
                                    .child(scope.key),
                            ),
                        })
                        .child(scope.label)
                }))
                .children(title.map(|title| {
                    div()
                        .px(SPACE_1)
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .child(title)
                }))
                .child(div().flex_1())
                .child(
                    div()
                        .pr(SPACE_1)
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(9.5))
                        .text_color(t.text_disabled)
                        .child(aside),
                )
        });
        let bar = t.text_primary.opacity(0.08);
        let placeholders = LOADING_ROWS.iter().filter(|_| loading).map(|&width| {
            div()
                .min_h(px(30.))
                .px(SPACE_2H)
                .py(px(3.))
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .w(px(16.))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(Skeleton::new().size(px(12.)).rounded_full().bg(bar)),
                )
                .child(
                    Skeleton::new()
                        .w(px(34.))
                        .h(px(6.))
                        .rounded(theme::RADIUS_1)
                        .bg(bar),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Skeleton::new()
                            .w(relative(width))
                            .h(px(6.))
                            .rounded(theme::RADIUS_1)
                            .bg(bar),
                    ),
                )
        });
        let empty = self.empty.map(|text| {
            div()
                .min_h(px(30.))
                .px(SPACE_2H)
                .flex()
                .items_center()
                .text_size(crate::theme::ui_text(TEXT_3XS_H))
                .text_color(t.text_tertiary)
                .child(text)
        });
        shell(cx)
            .py(SPACE_1)
            .children(heading)
            .children(placeholders)
            .children(rows)
            .children(empty)
    }
}

/// The tab docked over the composer's top edge, open at the bottom where it meets it.
pub fn shell(cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .absolute()
        .left(px(19.))
        .right(px(19.))
        .bottom(relative(1.))
        .mb(px(1.))
        .flex()
        .flex_col()
        .border_t_1()
        .border_l_1()
        .border_r_1()
        .border_color(t.border)
        .rounded_t(theme::RADIUS_3)
        .bg(t.surface_composer)
        .occlude()
}
