//! What every settings page is made of: sections under a heading, and rows
//! with a name, a line about them, and the control that changes them.

use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::IndexPath;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Div, Entity, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Window,
};

use crate::theme::{self, Tokens};
use crate::workbench::Workbench;

/// A section of a page: its heading, a count or state beside it, a line
/// under it, and its rows. One a search led to is lit for a moment.
pub fn section(
    title: &'static str,
    meta: Option<SharedString>,
    sub: Option<SharedString>,
    flash: bool,
    body: impl IntoElement,
    cx: &App,
) -> AnyElement {
    section_with(title, meta, None, sub, flash, body, cx)
}

/// A section as [`section`] draws it, with `action` at the far end of its heading.
pub fn section_with(
    title: &'static str,
    meta: Option<SharedString>,
    action: Option<AnyElement>,
    sub: Option<SharedString>,
    flash: bool,
    body: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(4.))
        .rounded(theme::RADIUS_3)
        .when(flash, |section| section.bg(t.surface_chosen))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(crate::theme::ui_text(13.))
                                .text_color(t.text_primary)
                                .child(title),
                        )
                        .children(meta.map(|meta| {
                            div()
                                .text_size(crate::theme::ui_text(11.5))
                                .text_color(t.text_tertiary)
                                .child(meta)
                        })),
                )
                .children(action.map(|action| div().flex_1().flex().justify_end().child(action))),
        )
        .children(sub.map(|sub| {
            div()
                .text_size(crate::theme::ui_text(12.))
                .text_color(t.text_tertiary)
                .child(sub)
        }))
        .child(
            div()
                .flex()
                .flex_col()
                .border_1()
                .border_color(t.border)
                .rounded(theme::RADIUS_3)
                .child(body),
        )
        .into_any_element()
}

/// A row in a section: what it is, a line about it, and its control.
pub fn row(
    label: impl Into<SharedString>,
    about: Option<&'static str>,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    row_with(None, label, about.map(SharedString::from), control, cx)
}

/// A row with a mark before its name, and a line about it made as it is drawn.
pub fn row_with(
    mark: Option<AnyElement>,
    label: impl Into<SharedString>,
    about: Option<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .border_b_1()
        .border_color(t.line_soft)
        .children(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(crate::theme::ui_text(13.))
                        .text_color(t.text_primary)
                        .child(label.into()),
                )
                .children(about.map(|about| {
                    div()
                        .text_size(crate::theme::ui_text(11.5))
                        .text_color(t.text_tertiary)
                        .child(about)
                })),
        )
        .child(div().flex_none().child(control))
}

/// A switch that sets the boolean preference `key`.
pub fn switch(id: &'static str, key: &'static str, on: bool, this: &Entity<Workbench>) -> Switch {
    let this = this.clone();
    Switch::new(id)
        .checked(on)
        .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
            let on = *on;
            this.update(cx, |bench, _| super::set(bench, key, on))
        })
}

/// A line of quiet text in place of a section's rows.
pub fn note(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(px(14.))
        .py(px(12.))
        .text_size(crate::theme::ui_text(12.))
        .text_color(t.text_tertiary)
        .child(text.into())
        .into_any_element()
}

/// A dropdown of fixed choices, which a page keeps between frames.
pub struct Choice {
    state: Entity<SelectState<Vec<SharedString>>>,
    _picked: Subscription,
}

impl Choice {
    /// Offers `labels` with the one at `chosen` showing; `pick` hears the place of the one chosen.
    pub fn new(
        labels: Vec<impl Into<SharedString>>,
        chosen: usize,
        pick: impl Fn(&mut Workbench, usize, &mut Context<Workbench>) + 'static,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) -> Self {
        let labels: Vec<SharedString> = labels.into_iter().map(Into::into).collect();
        let offered = labels.clone();
        let state = cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(chosen)), window, cx));
        let picked = cx.subscribe(
            &state,
            move |bench, _, event: &SelectEvent<Vec<SharedString>>, cx| {
                let SelectEvent::Confirm(Some(label)) = event else {
                    return;
                };
                if let Some(at) = offered.iter().position(|offered| offered == label) {
                    pick(bench, at, cx);
                }
            },
        );
        Self {
            state,
            _picked: picked,
        }
    }

    pub fn view(&self) -> Select<Vec<SharedString>> {
        Select::new(&self.state).w(px(220.))
    }
}
