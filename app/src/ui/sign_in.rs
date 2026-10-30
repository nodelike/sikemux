//! The screen every sign-in is drawn on, an agent's account or a plugin's service alike: one
//! column in the middle, the service's mark raised in a tile, what signing in gives, the form,
//! and the quieter ways round below it.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, linear_color_stop, linear_gradient, px, Animation, AnimationExt, AnyElement, App, Div,
    ElementId, Entity, Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Window,
};

use super::text;
use crate::theme::{
    self, Tokens, RADIUS_2, RADIUS_3, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, SPACE_6,
    TEXT_2XS, TEXT_LG, TEXT_SM, TEXT_XS,
};

/// Where a service's sign-in says the token it takes is kept.
#[cfg(target_os = "macos")]
pub const KEPT: &str = "Sikemux keeps your sign-in in the macOS Keychain.";
#[cfg(not(target_os = "macos"))]
pub const KEPT: &str = "Sikemux keeps your sign-in where only Sikemux can read it.";

const COLUMN: f32 = 340.;
const TILE: f32 = 56.;
/// The height of the form's buttons and of the waiting row that stands in for one.
const ACTION: f32 = 34.;
const SPINNER: f32 = 12.;

/// What the form's boxes are filled with: the ink at 3%, half a hovered row's.
fn tint(t: &Tokens) -> Hsla {
    t.surface_hover.opacity(0.5)
}

#[derive(IntoElement)]
pub struct Screen {
    id: ElementId,
    mark: AnyElement,
    title: SharedString,
    lede: Option<SharedString>,
    foot: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl Screen {
    pub fn new(
        id: impl Into<ElementId>,
        mark: impl IntoElement,
        title: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            mark: mark.into_any_element(),
            title: title.into(),
            lede: None,
            foot: None,
            children: Vec::new(),
        }
    }

    /// What signing in gives, under the title.
    pub fn lede(mut self, lede: impl Into<SharedString>) -> Self {
        self.lede = Some(lede.into());
        self
    }

    /// A last quiet line, such as where the sign-in is kept.
    pub fn foot(mut self, foot: impl Into<SharedString>) -> Self {
        self.foot = Some(foot.into());
        self
    }
}

impl ParentElement for Screen {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Screen {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let tile = div()
            .size(px(TILE))
            .flex_none()
            .mb(SPACE_1)
            .flex()
            .items_center()
            .justify_center()
            .rounded(RADIUS_3)
            .border_1()
            .border_color(t.border)
            .bg(linear_gradient(
                180.,
                linear_color_stop(t.surface_raised, 0.3),
                linear_color_stop(t.surface_sunken, 1.),
            ))
            .shadow(t.shadow_raised())
            .text_color(t.text_primary)
            .child(self.mark);
        let column = div()
            .w_full()
            .max_w(px(COLUMN))
            .my_auto()
            .flex()
            .flex_col()
            .items_center()
            .gap(SPACE_3)
            .text_center()
            .child(tile)
            .child(
                div()
                    .text_size(theme::ui_text(TEXT_LG))
                    .font_weight(FontWeight::SEMIBOLD)
                    .letter_spacing(-0.015)
                    .text_color(t.text_primary)
                    .child(self.title),
            )
            .children(self.lede.map(|lede| {
                div()
                    .mb(SPACE_2)
                    .text_size(theme::ui_text(TEXT_SM))
                    .line_height(theme::ui_text(19.5))
                    .text_color(t.text_secondary)
                    .child(lede)
            }))
            .children(self.children)
            .children(self.foot.map(|foot| {
                div()
                    .mt(SPACE_2)
                    .text_size(theme::ui_text(TEXT_2XS))
                    .line_height(theme::ui_text(16.5))
                    .text_color(t.text_disabled)
                    .child(foot)
            }));
        div()
            .id(self.id)
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px(SPACE_4)
            .py(SPACE_6)
            .child(column)
    }
}

/// A labelled box to type in, across the column.
#[derive(IntoElement)]
pub struct Field {
    label: SharedString,
    state: Entity<InputState>,
    mono: bool,
    hint: Option<SharedString>,
}

impl Field {
    pub fn new(label: impl Into<SharedString>, state: &Entity<InputState>) -> Self {
        Self {
            label: label.into(),
            state: state.clone(),
            mono: false,
            hint: None,
        }
    }

    /// For a key, a token or an address, typed as the machine reads it.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Where to find what goes in it, under the box.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl RenderOnce for Field {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let focused = self.state.read(cx).focus_handle(cx).is_focused(window);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .text_left()
            .child(label(self.label, cx))
            .child(
                div()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .rounded(RADIUS_2)
                    .border_1()
                    .border_color(match focused {
                        true => t.border_selected,
                        false => t.border,
                    })
                    .bg(tint(&t))
                    .text_color(t.text_primary)
                    .map(|field| match self.mono {
                        true => field
                            .font_family(theme::MONO)
                            .text_size(theme::ui_text(TEXT_XS)),
                        false => field.text_size(theme::ui_text(TEXT_SM)),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.state).appearance(false).bordered(false)),
                    ),
            )
            .children(self.hint.map(|hint| {
                div()
                    .text_size(theme::ui_text(TEXT_2XS))
                    .line_height(theme::ui_text(16.5))
                    .text_color(t.text_tertiary)
                    .child(hint)
            }))
    }
}

/// What a box in the form is for, over it.
pub fn label(words: impl Into<SharedString>, cx: &App) -> Div {
    text(words, TEXT_2XS, FontWeight::MEDIUM).text_color(Tokens::get(cx).text_tertiary)
}

/// `control` under its label, across the column, for a pick that is not typed.
pub fn labelled(words: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .text_left()
        .child(label(words, cx))
        .child(control)
}

/// An address already given, as a box with what answered there and the way to change it.
pub fn address(
    shown: impl Into<SharedString>,
    meta: Option<SharedString>,
    change: Stateful<Div>,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx);
    div()
        .w_full()
        .h(px(30.))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .bg(tint(t))
        .text_left()
        .child(
            text(shown, TEXT_XS, FontWeight::NORMAL)
                .min_w_0()
                .truncate()
                .font_family(theme::MONO)
                .text_color(t.text_primary),
        )
        .children(meta.map(|meta| {
            text(meta, TEXT_2XS, FontWeight::NORMAL)
                .flex_none()
                .font_family(theme::MONO)
                .text_color(t.live)
        }))
        .child(div().flex_1())
        .child(change)
}

/// A quiet way round the form, as words that underline under the pointer.
pub fn link(id: impl Into<ElementId>, label: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let t = Tokens::get(cx).clone();
    div()
        .id(id)
        .flex_none()
        .cursor_pointer()
        .text_size(theme::ui_text(TEXT_XS))
        .text_color(t.text_secondary)
        .hover(move |link| link.text_color(t.text_primary).underline())
        .child(label.into())
}

/// The links under a form, wrapping onto a second line when they do not fit.
pub fn links() -> Div {
    div()
        .flex()
        .flex_wrap()
        .justify_center()
        .gap_x(SPACE_4)
        .gap_y(SPACE_2)
}

/// Stands in for the main button while the person finishes signing in somewhere else.
pub fn waiting(
    id: &'static str,
    words: impl Into<SharedString>,
    cancel: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx);
    div()
        .w_full()
        .h(px(ACTION))
        .flex()
        .items_center()
        .justify_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .text_size(theme::ui_text(TEXT_SM))
        .text_color(t.text_secondary)
        .child(spinner(id, cx))
        .child(words.into())
        .child(
            link(SharedString::from(format!("{id}-cancel")), "Cancel", cx)
                .ml(SPACE_1)
                .on_click(cancel),
        )
}

/// A small ring with its head running round in the accent; still for a person who asked for
/// less motion.
pub fn spinner(id: &'static str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let (track, head) = (t.border_strong, t.accent);
    let ring = move |turn: f32| super::arcs(Some((turn, turn + 0.25, head)), SPINNER, 1.5, track);
    if cx.reduce_motion() {
        return ring(0.).into_any_element();
    }
    div()
        .size(px(SPINNER))
        .flex_none()
        .with_animation(
            SharedString::from(format!("{id}-spinner")),
            Animation::new(Duration::from_millis(800))
                .repeat()
                .with_max_fps(30.),
            move |spinner, turn| spinner.child(ring(turn)),
        )
        .into_any_element()
}

/// Why the sign-in failed, in the danger colour; a long one scrolls rather than pushing the
/// form off the screen.
pub fn error(words: impl Into<SharedString>, cx: &App) -> AnyElement {
    let tone = Tokens::get(cx).danger;
    callout(tone)
        .id("sign-in-error")
        .max_h(px(120.))
        .overflow_y_scroll()
        .child(words.into())
        .into_any_element()
}

/// Something to know before signing in, in the warning colour.
pub fn warning(words: impl Into<SharedString>, cx: &App) -> Div {
    callout(Tokens::get(cx).warn).child(words.into())
}

fn callout(tone: Hsla) -> Div {
    div()
        .w_full()
        .px(SPACE_3)
        .py(SPACE_2)
        .rounded(RADIUS_2)
        .border_1()
        .border_color(tone.opacity(0.34))
        .bg(tone.opacity(0.08))
        .text_left()
        .text_size(theme::ui_text(TEXT_XS))
        .line_height(theme::ui_text(18.))
        .text_color(tone)
}

/// A hairline across the column with `words` in its middle, before another way to sign in.
pub fn or(words: impl Into<SharedString>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let line = || div().flex_1().h(px(1.)).bg(t.border);
    div()
        .w_full()
        .my(SPACE_1)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary)
        .child(line())
        .child(words.into())
        .child(line())
}

/// A line of small print in the column, such as what a button above it does.
pub fn note(words: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(theme::ui_text(TEXT_2XS))
        .line_height(theme::ui_text(16.5))
        .text_color(Tokens::get(cx).text_tertiary)
        .child(words.into())
}

/// One of several accounts to pick between: its mark, its name, and a word on how it stands.
pub fn choice(
    id: impl Into<ElementId>,
    mark: impl IntoElement,
    name: impl Into<SharedString>,
    meta: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = Tokens::get(cx);
    choice_box(id, t.text_primary, t)
        .flex_1()
        .min_w_0()
        .px(SPACE_3)
        .gap(SPACE_2)
        .text_left()
        .text_size(theme::ui_text(TEXT_SM))
        .child(mark)
        .child(div().flex_1().min_w_0().truncate().child(name.into()))
        .child(text(meta, TEXT_2XS, FontWeight::NORMAL).text_color(t.text_tertiary))
}

/// A square beside a choice for what it holds that is not picking it, such as removing it.
pub fn choice_beside(
    id: impl Into<ElementId>,
    glyph: crate::icons::Icon,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = Tokens::get(cx);
    choice_box(id, t.danger, t)
        .w(px(40.))
        .flex_none()
        .justify_center()
        .text_color(t.text_tertiary)
        .tooltip(super::tooltip(label.into()))
        .child(super::icon(glyph, 13.))
}

/// A choice's box, taking `ink` under the pointer.
fn choice_box(id: impl Into<ElementId>, ink: Hsla, t: &Tokens) -> Stateful<Div> {
    let (hover, strong) = (t.surface_hover, t.border_strong);
    div()
        .id(id)
        .h(px(40.))
        .flex()
        .items_center()
        .cursor_pointer()
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .bg(tint(t))
        .text_color(t.text_primary)
        .hover(move |choice| choice.bg(hover).border_color(strong).text_color(ink))
}

/// Choices one under another, across the column.
pub fn choices() -> Div {
    div().w_full().flex().flex_col().gap(SPACE_1H)
}
