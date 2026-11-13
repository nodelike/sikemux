use std::rc::Rc;

use gpui_kit::base::Selectable;
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::Side;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    div, px, Anchor, AnimationExt, AnyElement, AnyView, App, AppContext, Context, Div, ElementId,
    FontWeight, Hsla, InteractiveElement, Interactivity, IntoElement, ParentElement, Render,
    RenderOnce, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement, Styled,
    Window,
};

use super::text;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    radius_2, radius_3, radius_4, Tokens, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS_H, TEXT_3XS_H,
    TEXT_LG,
};

/// How far rows sit inside a float, so a chosen row's corners run parallel to the float's:
/// the float's corner less its border and a row's corner.
pub const FLOAT_INSET: f32 = 14. - 1. - 5.;
/// The same inside a panel with the smaller [`radius_3()`] corners.
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
        .rounded(radius_4())
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
        frame: div().id(id).cursor_pointer(),
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

/// The side padding and height Kit gives a menu row.
const ROW_PADDING: gpui_kit::Pixels = px(8.);
const ROW_HEIGHT: gpui_kit::Pixels = px(26.);

/// A menu row with what Kit's own rows lack: a mark in its own colours where the icon goes,
/// words at its end, and the danger colour for a choice that deletes or discards.
pub struct MenuRow {
    label: SharedString,
    icon: Option<Icon>,
    mark: Option<Rc<dyn Fn() -> AnyElement>>,
    hint: Option<SharedString>,
    danger: bool,
    chosen: bool,
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
            chosen: false,
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

    /// Ticked, with the chosen surface across the whole row, for the choice in force.
    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
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
        let has_icon = self.icon.is_some();
        let Self {
            label,
            mark,
            hint,
            chosen,
            ..
        } = self;
        let item = if mark.is_none() && hint.is_none() && ink.is_none() && !chosen {
            PopupMenuItem::new(label)
        } else {
            PopupMenuItem::element(move |window, cx| {
                let t = Tokens::get(cx);
                let hint_ink = t.text_tertiary;
                // Kit lays a row out as its padding, the icon and a gap, this element, then the
                // tick and its margin; the chosen ground reaches across all of them.
                let rem = window.rem_size();
                let before = ROW_PADDING + if has_icon { rem } else { px(0.) };
                let after = ROW_PADDING + rem * 1.5;
                let ground = t.surface_chosen;
                div()
                    .id(ElementId::Name(label.clone()))
                    .aria_label(label.clone())
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .when(chosen, |row| {
                        row.relative().h(ROW_HEIGHT).child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(-before)
                                .right(-after)
                                .rounded(radius_2())
                                .bg(ground),
                        )
                    })
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
        let item = match icon {
            Some(icon) => item.icon(icon),
            None => item,
        };
        if chosen {
            item.checked(true)
        } else {
            item
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
            .rounded(radius_2())
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
            .rounded(radius_3())
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
