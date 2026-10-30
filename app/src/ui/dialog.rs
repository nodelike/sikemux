use std::rc::Rc;

use gpui_kit::base::{AlertDialog, Button as BaseButton};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    div, px, AnyElement, App, Div, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, RenderOnce, Role, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

use super::{icon, text};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, RADIUS_4, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3,
    SPACE_3H, SPACE_4, SPACE_5, TEXT_2XS, TEXT_2XS_H, TEXT_3XS, TEXT_3XS_H, TEXT_MD, TEXT_SM,
    TEXT_XL, TEXT_XS,
};

pub(crate) type Answer = Rc<dyn Fn(&mut Window, &mut App)>;
type Typed = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// The sheet every dialog is drawn on: a title, what it says, then its buttons on a
/// sunken foot. Escape calls `no`, Enter `yes`.
pub(crate) fn sheet(
    title: SharedString,
    body: AnyElement,
    foot: Div,
    yes: Answer,
    no: Answer,
    cx: &mut App,
) -> AlertDialog {
    sheet_of(px(440.), title, body, foot, yes, no, cx)
}

/// The keyboard a dialog holds while it shows, and where it goes back to once answered.
struct Held {
    focus: FocusHandle,
    before: Option<FocusHandle>,
}

impl Held {
    /// `answer`, then the keyboard back where it was before the dialog took it.
    fn back(&self, answer: Answer) -> Answer {
        let before = self.before.clone();
        Rc::new(move |window, cx| {
            answer(window, cx);
            if let Some(before) = &before {
                window.focus(before, cx);
            }
        })
    }
}

/// Takes the keyboard into a dialog the first time it draws, so Enter and Escape answer it
/// and not what is behind it.
fn hold_focus(window: &mut Window, cx: &mut App) -> Held {
    let held = window.use_keyed_state("dialog-focus", cx, |window, cx| {
        let before = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        (focus, before)
    });
    let (focus, before) = held.read(cx).clone();
    Held { focus, before }
}

pub(crate) fn sheet_of(
    width: Pixels,
    title: SharedString,
    body: AnyElement,
    foot: Div,
    yes: Answer,
    no: Answer,
    cx: &mut App,
) -> AlertDialog {
    let head = head(None, title, cx);
    framed(width, head, body, foot, yes, no, cx)
}

/// The mark beside a sheet's title, in the danger colour for a sheet whose yes cannot be undone.
pub(crate) struct Tile {
    pub icon: Icon,
    pub danger: bool,
}

impl From<Icon> for Tile {
    fn from(icon: Icon) -> Self {
        Tile {
            icon,
            danger: false,
        }
    }
}

/// A sheet with `tile` beside its title, as the Mac's dialogs have it; its body clears the
/// tile, at `GLYPH_INSET`, and its foot starts from [`hinted_foot`].
pub(crate) fn marked_sheet(
    tile: impl Into<Tile>,
    title: SharedString,
    body: AnyElement,
    foot: Div,
    yes: Answer,
    no: Answer,
    cx: &mut App,
) -> AlertDialog {
    let head = head(Some(tile.into()), title, cx);
    framed(px(440.), head, body, foot, yes, no, cx)
}

/// A sheet's foot before its buttons, with `note` at its left.
pub(crate) fn hinted_foot(note: impl IntoElement, cx: &App) -> Div {
    div().child(
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(Tokens::get(cx).text_disabled)
            .child(note),
    )
}

/// A one-line field of a sheet's body, under its name, in the mono face for what the machine
/// reads, such as a command or a URL.
pub(crate) fn field(name: &'static str, input: &Entity<InputState>, mono: bool, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(text(name, TEXT_XS, FontWeight::MEDIUM).text_color(t.text_secondary))
        .child(
            div()
                .h(px(32.))
                .px(SPACE_2H)
                .flex()
                .items_center()
                .border_1()
                .border_color(t.border_strong)
                .rounded(RADIUS_2)
                .bg(t.surface_sunken)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .when(mono, |field| field.font_family(theme::MONO))
                .child(Input::new(input).appearance(false)),
        )
}

/// What Escape does, as a foot's note says it.
pub(crate) fn escape(word: &'static str, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(super::kbd("esc", cx))
        .child(word)
}

/// How far a marked sheet's body sits in, so its text lines up with the title, not the tile.
pub(crate) const GLYPH_INSET: f32 = 54.;

/// A marked sheet's body: in from the tile, and clear of the foot by the Mac's margin.
pub(crate) fn marked_body() -> Div {
    div()
        .pl(px(GLYPH_INSET))
        .pr(SPACE_5)
        .pt(SPACE_2)
        .pb(SPACE_3H)
        .flex()
        .flex_col()
}

/// A paragraph of a marked sheet's body.
pub(crate) fn prose(words: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .text_size(crate::theme::ui_text(TEXT_SM))
        .line_height(crate::theme::ui_text(TEXT_XL))
        .text_color(t.text_tertiary)
        .child(words.into())
}

fn head(tile: Option<Tile>, title: SharedString, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .px(SPACE_5)
        .pt(SPACE_4)
        .children(tile.map(|tile| {
            let (ink, ground) = match tile.danger {
                true => (t.danger, t.danger.opacity(0.14)),
                false => (t.text_primary, t.surface_chosen),
            };
            div()
                .size(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_2)
                .bg(ground)
                .child(icon(tile.icon, 15.).text_color(ink))
        }))
        .child(text(title, TEXT_MD, FontWeight::SEMIBOLD).text_color(t.text_primary))
}

fn framed(
    width: Pixels,
    head: Div,
    body: AnyElement,
    foot: Div,
    yes: Answer,
    no: Answer,
    cx: &mut App,
) -> AlertDialog {
    let t = Tokens::get(cx).clone();
    let content = div().flex().flex_col().child(head).child(body).child(
        foot.flex()
            .items_center()
            .justify_end()
            .gap(SPACE_2)
            .mt(SPACE_1H)
            .pt(SPACE_3)
            .pr(SPACE_3)
            .pb(SPACE_3)
            .pl(SPACE_4)
            .border_t_1()
            .border_color(t.border)
            .bg(t.surface_sunken),
    );
    floating(width, content, yes, no, cx)
}

/// A dialog drawn wholly by what it shows, with no title or foot of the sheet's: the scrim,
/// the float, and Escape and Enter answering it as a sheet's do.
pub(crate) fn floating(
    width: Pixels,
    content: impl IntoElement,
    yes: Answer,
    no: Answer,
    cx: &mut App,
) -> AlertDialog {
    let t = Tokens::get(cx).clone();
    AlertDialog::new(cx)
        .open(true)
        .close_on_escape(true)
        .backdrop(div().size_full().bg(Hsla {
            h: 0.,
            s: 0.,
            l: 0.,
            a: 0.55,
        }))
        .on_cancel(move |_, window, cx| {
            no(window, cx);
            true
        })
        .on_ok(move |_, window, cx| {
            yes(window, cx);
            true
        })
        .popup(
            div()
                .id("dialog")
                .w(width)
                .flex()
                .flex_col()
                .overflow_hidden()
                .bg(t.surface_overlay)
                .border_1()
                .border_color(t.border)
                .rounded(RADIUS_4)
                .shadow(t.shadow_float())
                .child(content),
        )
}

/// A dialog's button: the plain one, or the filled one that answers yes.
pub(crate) fn button(
    id: &'static str,
    label: SharedString,
    fill: Option<Hsla>,
    answer: Answer,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    BaseButton::new(id)
        .accessibility_label(label.clone())
        .h(px(30.))
        .px(SPACE_3)
        .rounded(RADIUS_2)
        .border_1()
        .text_size(crate::theme::ui_text(TEXT_SM))
        .font_weight(FontWeight::MEDIUM)
        .on_click(move |_, window, cx| answer(window, cx))
        .map(|button| match fill {
            Some(fill) => button
                .border_color(gpui_kit::transparent_black())
                .bg(fill)
                .text_color(t.on_accent)
                .font_weight(FontWeight::SEMIBOLD)
                .hover(|style| style.opacity(0.92)),
            None => button
                .border_color(t.border)
                .bg(t.surface_raised)
                .text_color(t.text_secondary)
                .hover(move |style| {
                    style
                        .text_color(t.text_primary)
                        .border_color(t.border_strong)
                }),
        })
        .child(label)
}

/// A question asked before something goes ahead: what it is, what it does, and the word
/// that goes ahead, filled in the danger colour when it cannot be undone. Escape or the
/// other button answers no, Enter yes.
#[derive(IntoElement)]
pub struct Confirm {
    title: SharedString,
    body: SharedString,
    yes: SharedString,
    destructive: bool,
    marked: bool,
    on_yes: Answer,
    on_no: Answer,
    other: Option<(SharedString, Answer)>,
}

impl Confirm {
    pub fn new(
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            yes: yes.into(),
            destructive: false,
            marked: false,
            on_yes: Rc::new(|_, _| {}),
            on_no: Rc::new(|_, _| {}),
            other: None,
        }
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// A second way to go ahead, between Cancel and `yes`.
    pub fn other(
        mut self,
        label: impl Into<SharedString>,
        answer: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.other = Some((label.into(), Rc::new(answer)));
        self
    }

    /// Drawn as the Mac's own confirm is, with its mark beside the title and Escape's word
    /// at its foot; the Git pane's questions stay plain, as the Mac's Git modals are.
    pub fn marked(mut self) -> Self {
        self.marked = true;
        self
    }

    pub fn on_yes(mut self, answer: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_yes = Rc::new(answer);
        self
    }

    pub fn on_no(mut self, answer: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_no = Rc::new(answer);
        self
    }
}

impl RenderOnce for Confirm {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let fill = if self.destructive { t.danger } else { t.accent };
        let held = hold_focus(window, cx);
        let (on_yes, on_no) = (held.back(self.on_yes), held.back(self.on_no));
        let other = self
            .other
            .map(|(label, answer)| button("confirm-other", label, None, held.back(answer), cx));
        if self.marked {
            let lines = self.body.split('\n').filter(|line| !line.trim().is_empty());
            let body = marked_body()
                .gap(SPACE_2)
                .children(lines.map(|line| prose(line.to_owned(), &t)))
                .into_any_element();
            let foot = hinted_foot(escape("cancel", cx), cx)
                .child(button(
                    "confirm-no",
                    "Cancel".into(),
                    None,
                    on_no.clone(),
                    cx,
                ))
                .children(other)
                .child(button(
                    "confirm-yes",
                    self.yes,
                    Some(fill),
                    on_yes.clone(),
                    cx,
                ));
            let tile = Tile {
                icon: if self.destructive {
                    Icon::Warning
                } else {
                    Icon::InfoCircle
                },
                danger: self.destructive,
            };
            return marked_sheet(tile, self.title, body, foot, on_yes, on_no, cx)
                .focus_handle(held.focus);
        }
        let foot = div()
            .child(button(
                "confirm-no",
                "cancel".into(),
                None,
                on_no.clone(),
                cx,
            ))
            .children(other)
            .child(button(
                "confirm-yes",
                self.yes,
                Some(fill),
                on_yes.clone(),
                cx,
            ));
        sheet(
            self.title,
            div()
                .px(SPACE_4)
                .py(SPACE_3H)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .line_height(crate::theme::ui_text(18.))
                .text_color(t.text_secondary)
                .child(self.body)
                .into_any_element(),
            foot,
            on_yes,
            on_no,
            cx,
        )
        .focus_handle(held.focus)
    }
}

/// A question answered with a line of text, such as a new branch's name. Enter gives
/// what was typed, Escape gives up.
#[derive(IntoElement)]
pub struct Prompt {
    title: SharedString,
    field: Entity<InputState>,
    marked: bool,
    on_yes: Typed,
    on_no: Answer,
}

impl Prompt {
    pub fn new(title: impl Into<SharedString>, field: &Entity<InputState>) -> Self {
        Self {
            title: title.into(),
            field: field.clone(),
            marked: false,
            on_yes: Rc::new(|_, _, _| {}),
            on_no: Rc::new(|_, _| {}),
        }
    }

    /// Drawn as the Mac's own prompt is; see [`Confirm::marked`].
    pub fn marked(mut self) -> Self {
        self.marked = true;
        self
    }

    pub fn on_yes(mut self, answer: impl Fn(String, &mut Window, &mut App) + 'static) -> Self {
        self.on_yes = Rc::new(answer);
        self
    }

    pub fn on_no(mut self, answer: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_no = Rc::new(answer);
        self
    }
}

impl RenderOnce for Prompt {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let (field, on_yes) = (self.field.clone(), self.on_yes);
        let yes: Answer = Rc::new(move |window, cx| {
            let typed = field.read(cx).value().to_string();
            on_yes(typed, window, cx)
        });
        if self.marked {
            let field = div()
                .h(px(32.))
                .px(SPACE_2)
                .flex()
                .items_center()
                .border_1()
                .border_color(t.border)
                .rounded(RADIUS_2)
                .bg(t.surface_sunken)
                .text_size(crate::theme::ui_text(TEXT_SM))
                .text_color(t.text_primary)
                .child(Input::new(&self.field).appearance(false));
            let foot = hinted_foot(escape("cancel", cx), cx)
                .child(button(
                    "prompt-no",
                    "Cancel".into(),
                    None,
                    self.on_no.clone(),
                    cx,
                ))
                .child(button(
                    "prompt-yes",
                    "Save".into(),
                    Some(t.accent),
                    yes.clone(),
                    cx,
                ));
            let body = marked_body().pt(SPACE_3).child(field).into_any_element();
            return marked_sheet(
                Icon::InfoCircle,
                self.title,
                body,
                foot,
                yes,
                self.on_no,
                cx,
            );
        }
        let foot = div()
            .child(button(
                "prompt-no",
                "cancel".into(),
                None,
                self.on_no.clone(),
                cx,
            ))
            .child(button(
                "prompt-yes",
                "ok".into(),
                Some(t.accent),
                yes.clone(),
                cx,
            ));
        sheet(
            self.title,
            div()
                .px(SPACE_4)
                .py(SPACE_3H)
                .child(
                    div()
                        .h(px(32.))
                        .px(SPACE_2H)
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(t.border_strong)
                        .rounded(RADIUS_2)
                        .bg(t.surface_sunken)
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .child(Input::new(&self.field).appearance(false)),
                )
                .into_any_element(),
            foot,
            yes,
            self.on_no,
            cx,
        )
    }
}

/// One of a keyed menu's choices: the key that picks it, what it does, and a hint.
pub struct Choice {
    key: Option<char>,
    label: SharedString,
    hint: Option<SharedString>,
    destructive: bool,
    disabled: bool,
    pick: Answer,
}

impl Choice {
    pub fn new(
        key: impl Into<Option<char>>,
        label: impl Into<SharedString>,
        pick: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            hint: None,
            destructive: false,
            disabled: false,
            pick: Rc::new(pick),
        }
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A menu of choices each picked by a key, as the Git pane's menus are. Escape gives up.
#[derive(IntoElement)]
pub struct Choices {
    title: SharedString,
    choices: Vec<Choice>,
    on_no: Answer,
}

impl Choices {
    pub fn new(title: impl Into<SharedString>, choices: Vec<Choice>) -> Self {
        Self {
            title: title.into(),
            choices,
            on_no: Rc::new(|_, _| {}),
        }
    }

    pub fn on_no(mut self, answer: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_no = Rc::new(answer);
        self
    }
}

impl RenderOnce for Choices {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let held = hold_focus(window, cx);
        let keyed: Vec<(char, Answer)> = self
            .choices
            .iter()
            .filter(|choice| !choice.disabled)
            .filter_map(|choice| Some((choice.key?, held.back(choice.pick.clone()))))
            .collect();
        let rows = self.choices.into_iter().enumerate().map(|(index, choice)| {
            let ink = if choice.disabled {
                t.text_tertiary
            } else if choice.destructive {
                t.danger
            } else {
                t.text_secondary
            };
            let (wash, lit) = if choice.destructive {
                (t.danger.opacity(0.12), t.danger)
            } else {
                (t.surface_hover, t.text_primary)
            };
            let pick = held.back(choice.pick.clone());
            BaseButton::new(("choice", index))
                .role(Role::MenuItem)
                .accessibility_label(choice.label.clone())
                .disabled(choice.disabled)
                .w_full()
                .px(SPACE_4)
                .py(SPACE_1H)
                .flex()
                .items_center()
                .justify_start()
                .gap(SPACE_2H)
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(ink)
                .when(choice.disabled, |row| row.opacity(0.6))
                .when(!choice.disabled, |row| {
                    row.hover(move |style| style.bg(wash).text_color(lit))
                })
                .on_click(move |_, window, cx| pick(window, cx))
                .children(choice.key.map(|key| {
                    div()
                        .w(px(18.))
                        .flex_none()
                        .text_center()
                        .border_1()
                        .border_color(if choice.destructive {
                            t.danger.opacity(0.3)
                        } else {
                            t.border
                        })
                        .rounded(px(4.))
                        .bg(t.surface)
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if choice.destructive {
                            t.danger
                        } else {
                            t.text_primary
                        })
                        .child(key.to_string())
                }))
                .child(div().flex_1().min_w_0().child(choice.label))
                .children(choice.hint.map(|hint| {
                    div()
                        .flex_none()
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .child(hint)
                }))
        });
        let no = held.back(self.on_no.clone());
        sheet(
            self.title,
            div()
                .id("choices")
                .test_support()
                .role(Role::Menu)
                .py(SPACE_2)
                .flex()
                .flex_col()
                .on_key_down(move |event: &KeyDownEvent, window, cx| {
                    let typed = event.keystroke.key_char.as_deref().unwrap_or_default();
                    let mut chars = typed.chars();
                    if let (Some(typed), None) = (chars.next(), chars.next()) {
                        if let Some((_, pick)) = keyed.iter().find(|(key, _)| *key == typed) {
                            cx.stop_propagation();
                            pick(window, cx);
                        }
                    }
                })
                .children(rows)
                .into_any_element(),
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child("esc  cancel"),
            no.clone(),
            no,
            cx,
        )
        .focus_handle(held.focus)
    }
}

/// The keys a place answers to, in sections, as a wide sheet Escape closes.
#[derive(IntoElement)]
pub struct Cheatsheet {
    title: SharedString,
    sections: &'static [Section],
    on_close: Answer,
}

/// A cheatsheet's section: its name, then each key with what it does.
pub type Section = (&'static str, &'static [(&'static str, &'static str)]);

impl Cheatsheet {
    pub fn new(title: impl Into<SharedString>, sections: &'static [Section]) -> Self {
        Self {
            title: title.into(),
            sections,
            on_close: Rc::new(|_, _| {}),
        }
    }

    pub fn on_close(mut self, close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Rc::new(close);
        self
    }
}

impl RenderOnce for Cheatsheet {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let held = hold_focus(window, cx);
        let t = Tokens::get(cx).clone();
        let count = self.sections.len();
        let sections = self
            .sections
            .iter()
            .enumerate()
            .map(|(index, (title, rows))| {
                div()
                    .px(SPACE_4)
                    .pt(SPACE_2)
                    .pb(SPACE_3)
                    .when(index + 1 < count, |section| {
                        section.border_b_1().border_color(t.border)
                    })
                    .child(
                        div()
                            .mb(SPACE_1H)
                            .text_size(crate::theme::ui_text(TEXT_3XS))
                            .text_color(t.text_tertiary)
                            .child(title.to_uppercase()),
                    )
                    .children(rows.iter().map(|(keys, label)| {
                        div()
                            .flex()
                            .gap(SPACE_3)
                            .py(SPACE_0)
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .child(
                                div()
                                    .w(px(180.))
                                    .flex_none()
                                    .text_color(t.accent)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(*keys),
                            )
                            .child(div().text_color(t.text_secondary).child(*label))
                    }))
            });
        let close = held.back(self.on_close.clone());
        sheet_of(
            px(560.),
            self.title,
            div()
                .id("cheatsheet")
                .py(SPACE_1)
                .max_h(px(560.))
                .overflow_y_scroll()
                .font_family(theme::MONO)
                .children(sections)
                .into_any_element(),
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child("esc  close"),
            close.clone(),
            close,
            cx,
        )
        .focus_handle(held.focus)
    }
}
