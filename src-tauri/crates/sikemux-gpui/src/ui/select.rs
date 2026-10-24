//! A field that shows one of a few choices and opens a menu to pick another.

use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, Popover};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, Anchor, AnyElement, App, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, Transformation, Window,
};

use super::{icon, Menu, MenuItem};
use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_2};

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
    let t = Tokens::get(cx).clone();
    let id: SharedString = id.into();
    let shown: SharedString = shown.into();
    let trigger = BaseButton::new(SharedString::from(format!("{id}-field")))
        .accessibility_label(shown.clone())
        .h(px(26.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(12.))
        .text_color(if disabled {
            t.text_tertiary
        } else {
            t.text_primary
        })
        .when(!disabled, |field| {
            field.hover(move |style| style.border_color(t.border_strong))
        })
        .child(div().truncate().child(shown))
        .child(
            icon(Icon::Chevron, 9.)
                .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                .text_color(t.text_tertiary),
        );
    if disabled {
        return trigger.into_any_element();
    }
    let (picks, on_pick) = (Rc::new(picks), Rc::new(on_pick));
    Popover::new(id.clone())
        .anchor(Anchor::TopLeft)
        .offset(px(4.))
        .trigger(trigger)
        .content(move |_, _, cx| {
            let popover = cx.entity();
            div()
                .w(px(260.))
                .child(Menu::new().children(picks.iter().map(|pick| {
                    let (on_pick, popover, key) =
                        (on_pick.clone(), popover.clone(), pick.key.clone());
                    MenuItem::new(
                        SharedString::from(format!("pick-{key}")),
                        pick.label.clone(),
                    )
                    .checked(chosen.as_ref() == Some(&pick.key))
                    .when_some(pick.hint.clone(), |item, hint| item.hint(hint))
                    .on_click(move |_, window, cx| {
                        on_pick(&key, window, cx);
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                    })
                })))
        })
        .into_any_element()
}
