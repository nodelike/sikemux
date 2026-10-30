use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, Selectable};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::Side;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    div, px, Anchor, AnimationExt, AnyElement, AnyView, App, AppContext, ClickEvent, Context, Div,
    ElementId, FontWeight, Hsla, InteractiveElement, Interactivity, IntoElement, ParentElement,
    Render, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled, Transformation, Window,
};

use super::{icon, text};
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    Tokens, RADIUS_2, RADIUS_3, RADIUS_4, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS_H,
    TEXT_3XS_H, TEXT_LG, TEXT_SM,
};

/// How far rows sit inside a float, so a chosen row's corners run parallel to the float's:
/// the float's corner less its border and a row's corner.
pub const FLOAT_INSET: f32 = 14. - 1. - 5.;
/// The same inside a panel with the smaller [`RADIUS_3`] corners.
pub const PANEL_INSET: f32 = 10. - 1. - 5.;

/// What floats over everything else: menus, pickers, palettes.
pub fn float(cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .bg(Hsla {
            a: 0.94,
            ..t.surface_overlay
        })
        .border_1()
        .border_color(t.border)
        .rounded(RADIUS_4)
        .shadow(t.shadow_float())
        .backdrop_blur(px(20.))
}

/// Kit's menu with the Mac's measure: at least 196 wide, a choice's tick at the end of its row.
pub fn menu(
    items: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |popup, window, cx| items(popup.min_w(px(196.)).check_side(Side::Right), window, cx)
}

/// Opens Kit's menu of `items` from one of our own buttons, as Kit's `dropdown_menu` does
/// from its own, below the button and lined up on `anchor`'s side.
pub fn menu_button<T: Selectable + IntoElement + 'static>(
    id: impl Into<ElementId>,
    anchor: Anchor,
    trigger: T,
    items: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> impl IntoElement {
    MenuTrigger {
        frame: div().id(id),
        trigger,
    }
    .dropdown_menu_with_anchor(anchor, menu(items))
}

#[derive(IntoElement)]
struct MenuTrigger<T: Selectable + IntoElement + 'static> {
    frame: Stateful<Div>,
    trigger: T,
}

impl<T: Selectable + IntoElement + 'static> Styled for MenuTrigger<T> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.frame.style()
    }
}

impl<T: Selectable + IntoElement + 'static> InteractiveElement for MenuTrigger<T> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.frame.interactivity()
    }
}

impl<T: Selectable + IntoElement + 'static> Selectable for MenuTrigger<T> {
    fn selected(mut self, selected: bool) -> Self {
        self.trigger = self.trigger.selected(selected);
        self
    }

    fn is_selected(&self) -> bool {
        self.trigger.is_selected()
    }
}

impl<T: Selectable + IntoElement + 'static> RenderOnce for MenuTrigger<T> {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.frame.child(self.trigger)
    }
}

impl<T: Selectable + IntoElement + 'static> DropdownMenu for MenuTrigger<T> {}

/// A menu row with what Kit's own rows lack: a mark in its own colours where the icon goes,
/// words at its end, and the danger colour for a choice that deletes or discards.
pub struct MenuRow {
    label: SharedString,
    icon: Option<Icon>,
    mark: Option<Rc<dyn Fn() -> AnyElement>>,
    hint: Option<SharedString>,
    danger: bool,
}

impl FluentBuilder for MenuRow {}

impl MenuRow {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            mark: None,
            hint: None,
            danger: false,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn mark<E: IntoElement>(mut self, mark: impl Fn() -> E + 'static) -> Self {
        self.mark = Some(Rc::new(move || mark().into_any_element()));
        self
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    pub fn item(self, cx: &App) -> PopupMenuItem {
        let t = Tokens::get(cx);
        let ink = self
            .danger
            .then(|| crate::theme::mix(t.danger, t.text_secondary, 0.85));
        let icon = self.icon.map(|glyph| {
            let icon = gpui_kit::component::Icon::from(glyph);
            match ink {
                Some(ink) => icon.text_color(ink),
                None => icon,
            }
        });
        let Self {
            label, mark, hint, ..
        } = self;
        let item = if mark.is_none() && hint.is_none() && ink.is_none() {
            PopupMenuItem::new(label)
        } else {
            PopupMenuItem::element(move |_, cx| {
                let hint_ink = Tokens::get(cx).text_tertiary;
                div()
                    .id(ElementId::Name(label.clone()))
                    .aria_label(label.clone())
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .children(mark.as_ref().map(|mark| mark()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when_some(ink, |label, ink| label.text_color(ink))
                            .child(label.clone()),
                    )
                    .children(hint.clone().map(|hint| {
                        text(hint, TEXT_3XS_H, FontWeight::NORMAL).text_color(hint_ink)
                    }))
            })
        };
        match icon {
            Some(icon) => item.icon(icon),
            None => item,
        }
    }
}

/// A short label that appears after a pointer rests on its anchor.
pub struct TooltipView {
    text: SharedString,
}

impl Render for TooltipView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx);
        div()
            .max_w(px(360.))
            .px(px(9.))
            .py(px(5.))
            .rounded(RADIUS_2)
            .bg(t.surface_tooltip)
            .border_1()
            .border_color(Hsla {
                a: 0.72,
                ..t.border_strong
            })
            .text_size(crate::theme::ui_text(TEXT_2XS_H))
            .line_height(crate::theme::ui_text(TEXT_LG))
            .text_color(t.text_primary)
            .child(self.text.clone())
            .with_animation(
                "tooltip-in",
                motion::entrance(motion::TOOLTIP_IN),
                |tip, shown| tip.opacity(shown),
            )
    }
}

/// What `.tooltip()` takes, for a tooltip that says `text`.
pub fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text = text.into();
    move |_, cx| cx.new(|_| TooltipView { text: text.clone() }).into()
}

/// A tooltip laid out as a small card rather than a line of text.
pub struct CardTooltipView {
    width: f32,
    body: Rc<dyn Fn(&App) -> AnyElement>,
}

impl Render for CardTooltipView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx);
        div()
            .w(px(self.width))
            .px(SPACE_3)
            .pt(SPACE_2H)
            .pb(px(11.))
            .rounded(RADIUS_3)
            .bg(t.surface_overlay)
            .border_1()
            .border_color(t.border)
            .shadow(t.shadow_float())
            .text_color(t.text_primary)
            .child((self.body)(cx))
            .with_animation(
                "tooltip-in",
                motion::entrance(motion::TOOLTIP_IN),
                |tip, shown| tip.opacity(shown),
            )
    }
}

/// What `.tooltip()` takes, for a card `width` wide that `body` fills.
pub fn card_tooltip(
    width: f32,
    body: impl Fn(&App) -> AnyElement + 'static,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let body: Rc<dyn Fn(&App) -> AnyElement> = Rc::new(body);
    move |_, cx| {
        let body = body.clone();
        cx.new(|_| CardTooltipView { width, body }).into()
    }
}

/// The menu a split button's chevron opens.
pub type SplitMenu = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// A primary button with a chevron beside it, as GitHub's merge button: its face does the
/// chosen thing, and the chevron opens `menu` to pick another. It dims and stops while `off`;
/// `opens_up` lifts the menu above it, for a button at the foot of a page.
#[allow(clippy::too_many_arguments)]
pub fn split_button(
    id: &str,
    label: impl Into<SharedString>,
    glyph: Option<Icon>,
    off: bool,
    opens_up: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    menu: Option<SplitMenu>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let label: SharedString = label.into();
    let face = BaseButton::new(SharedString::from(id.to_owned()))
        .accessibility_label(label.clone())
        .px(SPACE_3)
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .gap(SPACE_1H)
        .text_size(crate::theme::ui_text(TEXT_SM))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.on_accent)
        .when(!off, |button| button.on_click(on_click))
        .children(glyph.map(|glyph| icon(glyph, 13.).text_color(t.on_accent)))
        .child(label);
    let picker = menu.map(|menu| {
        let chevron = BaseButton::new(SharedString::from(format!("{id}-pill")))
            .accessibility_label("More")
            .w(px(28.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .border_l_1()
            .border_color(t.on_accent.opacity(0.25))
            .child(
                icon(Icon::Chevron, 11.)
                    .with_transformation(Transformation::rotate(gpui_kit::radians(
                        std::f32::consts::FRAC_PI_2,
                    )))
                    .text_color(t.on_accent),
            );
        let anchor = if opens_up {
            Anchor::BottomRight
        } else {
            Anchor::TopRight
        };
        menu_button(
            format!("{id}-menu"),
            anchor,
            chevron,
            move |popup, window, cx| menu(popup, window, cx),
        )
        .into_any_element()
    });
    div()
        .flex()
        .flex_none()
        .rounded(RADIUS_2)
        .overflow_hidden()
        .bg(t.accent)
        .when(off, |button| button.opacity(0.5))
        .child(face)
        .children(picker)
        .into_any_element()
}
