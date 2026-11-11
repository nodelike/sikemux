//! A field that shows one of a few choices and opens a menu to pick another.

use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, Anchor, AnyElement, App, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, Transformation, Window,
};

use super::{icon, menu_button, MenuRow};
use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_2, SPACE_1H, SPACE_2, TEXT_XS};

/// One choice: what picking it hands back, and its words.
pub struct Pick {
    pub key: SharedString,
    pub label: SharedString,
    /// Words at the end of its row, such as what the choice does.
    pub hint: Option<SharedString>,
}

impl Pick {
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// The field: `shown` while closed, `picks` in its menu with `chosen` ticked, and
/// `on_pick` told which key was picked. Disabled, it shows `shown` and opens nothing.
pub fn select(
    id: impl Into<SharedString>,
    shown: impl Into<SharedString>,
    picks: Vec<Pick>,
    chosen: Option<SharedString>,
    disabled: bool,
    on_pick: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    select_led(id, None, shown, picks, chosen, disabled, on_pick, cx)
}

/// What a select shows while closed: `shown` in a bordered field, `lead` before it, with a
/// chevron after. Other pickers that open their own menus use it to look the same.
pub fn select_field(
    id: impl Into<SharedString>,
    lead: Option<AnyElement>,
    shown: impl Into<SharedString>,
    disabled: bool,
    cx: &App,
) -> BaseButton {
    let t = Tokens::get(cx).clone();
    let shown: SharedString = shown.into();
    BaseButton::new(id.into())
        .accessibility_label(shown.clone())
        .h(px(26.))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .text_color(if disabled {
            t.text_tertiary
        } else {
            t.text_primary
        })
        .when(!disabled, |field| {
            field.hover(move |style| style.border_color(t.border_strong))
        })
        .children(lead)
        .child(div().truncate().child(shown))
        .child(
            icon(Icon::Chevron, 9.)
                .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                .text_color(t.text_tertiary),
        )
}

/// [`select`] with `lead` before its words, such as an agent's mark.
#[allow(clippy::too_many_arguments)]
fn select_led(
    id: impl Into<SharedString>,
    lead: Option<AnyElement>,
    shown: impl Into<SharedString>,
    picks: Vec<Pick>,
    chosen: Option<SharedString>,
    disabled: bool,
    on_pick: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let id: SharedString = id.into();
    let trigger = select_field(
        SharedString::from(format!("{id}-field")),
        lead,
        shown,
        disabled,
        cx,
    );
    if disabled {
        return trigger.into_any_element();
    }
    let on_pick = Rc::new(on_pick);
    menu_button(id, Anchor::TopLeft, trigger, move |mut popup, _, cx| {
        popup = popup.min_w(px(260.));
        for pick in &picks {
            let (on_pick, key) = (on_pick.clone(), pick.key.clone());
            popup = popup.item(
                MenuRow::new(pick.label.clone())
                    .when_some(pick.hint.clone(), MenuRow::hint)
                    .item(cx)
                    .checked(chosen.as_ref() == Some(&pick.key))
                    .on_click(move |_, window, cx| on_pick(&key, window, cx)),
            );
        }
        popup
    })
    .into_any_element()
}
