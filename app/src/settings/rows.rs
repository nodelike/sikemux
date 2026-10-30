//! What every settings page is made of: sections under a heading, and rows
//! with a name, a line about them, and the control that changes them.

use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::IndexPath;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Div, Entity, FontWeight, Global, Hsla,
    IntoElement, ParentElement, SharedString, Styled, Subscription, Window,
};

use crate::theme::{
    self, Tokens, SPACE_0, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4, TEXT_2XS_H, TEXT_LG,
    TEXT_SM, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

/// The name of the page being drawn. A section named as its page leaves its name to the page's
/// header and shows only what it says beside and under it.
pub struct Shown(pub &'static str);

impl Global for Shown {}

/// The fill under a section's card: the theme's ground, held back enough that the
/// pane's picture still shows around the cards while their words stay readable over it.
pub fn card_fill(cx: &App) -> Hsla {
    let t = Tokens::get(cx);
    t.surface.opacity(if t.dark { 0.84 } else { 0.9 })
}

/// A section of a page: a card with its heading, a count or state beside it, a line
/// under it, and its rows. One a search led to is lit for a moment.
pub fn section(
    title: impl Into<SharedString>,
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
    title: impl Into<SharedString>,
    meta: Option<SharedString>,
    action: Option<AnyElement>,
    sub: Option<SharedString>,
    flash: bool,
    body: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let title: SharedString = title.into();
    let named = cx
        .try_global::<Shown>()
        .is_none_or(|shown| shown.0 != title.as_ref());
    let headed = named || meta.is_some() || action.is_some() || sub.is_some();
    let heading = div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .px(SPACE_4)
        .pt(SPACE_3H)
        .pb(SPACE_2H)
        .when(named || meta.is_some() || action.is_some(), |heading| {
            heading.child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .min_h(px(22.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(SPACE_2)
                            .when(named, |line| {
                                line.child(
                                    ui::text(title, TEXT_SM, FontWeight::SEMIBOLD)
                                        .text_color(t.text_primary),
                                )
                            })
                            .children(meta.map(|meta| {
                                div()
                                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                                    .text_color(t.text_tertiary)
                                    .child(meta)
                            })),
                    )
                    .children(action.map(|action| div().flex_none().child(action))),
            )
        })
        .children(sub.map(|sub| {
            div()
                .max_w(px(560.))
                .text_size(crate::theme::ui_text(TEXT_XS))
                .line_height(crate::theme::ui_text(17.))
                .text_color(t.text_secondary)
                .child(sub)
        }));
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(if flash {
            t.surface_chosen
        } else {
            card_fill(cx)
        })
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_3)
        .when(headed, |card| card.child(heading))
        .child(
            div()
                .when(headed, |body| body.border_t_1().border_color(t.line_soft))
                .child(div().mb(px(-1.)).flex().flex_col().child(body)),
        )
        .into_any_element()
}

/// A card with no heading, for a section that is one picture of itself.
pub fn card(flash: bool, body: impl IntoElement, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .overflow_hidden()
        .bg(if flash {
            t.surface_chosen
        } else {
            card_fill(cx)
        })
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_3)
        .child(body)
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
        .gap(SPACE_4)
        .px(SPACE_4)
        .py(px(11.))
        .border_b_1()
        .border_color(t.line_soft)
        .children(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child(label.into()),
                )
                .children(about.map(|about| {
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
                        .line_height(crate::theme::ui_text(TEXT_LG))
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
        .px(SPACE_4)
        .py(SPACE_3)
        .text_size(crate::theme::ui_text(TEXT_XS))
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
