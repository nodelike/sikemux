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
    Tokens, RADIUS_2, RADIUS_3, RADIUS_4, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS_H, TEXT_3XS_H,
    TEXT_LG,
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
/// words at its end, and the danger colour for a choice that deletes or discards. A row can
/// also say what its choice means on a quieter line under its name, with its icon in a tile.
pub struct MenuRow {
    label: SharedString,
    icon: Option<Icon>,
    mark: Option<Rc<dyn Fn() -> AnyElement>>,
    title: Option<Rc<dyn Fn() -> AnyElement>>,
    about: Option<SharedString>,
    tiled: bool,
    hint: Option<SharedString>,
    danger: bool,
    chosen: bool,
}

/// How tall a row with a line under its name is, and the tile its icon sits in.
const ABOUT_ROW_HEIGHT: gpui_kit::Pixels = px(40.);
const TILE: f32 = 26.;

impl FluentBuilder for MenuRow {}

impl MenuRow {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            mark: None,
            title: None,
            about: None,
            tiled: false,
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

    /// Draws the name as this element, such as a wordmark, rather than as plain words.
    pub fn title<E: IntoElement>(mut self, title: impl Fn() -> E + 'static) -> Self {
        self.title = Some(Rc::new(move || title().into_any_element()));
        self
    }

    /// What the choice means, on a quieter line under its name.
    pub fn about(mut self, about: impl Into<SharedString>) -> Self {
        self.about = Some(about.into());
        self
    }

    /// Draws the icon in a tile, tinted in the accent while the row is chosen, in place of a tick.
    pub fn tiled(mut self) -> Self {
        self.tiled = true;
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
        if self.tiled || self.about.is_some() || self.title.is_some() {
            return self.described();
        }
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
                                .rounded(RADIUS_2)
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

impl MenuRow {
    /// A row with its icon in a tile, its name, and what it means under it.
    fn described(self) -> PopupMenuItem {
        let Self {
            label,
            icon,
            mark,
            title,
            about,
            tiled,
            hint,
            chosen,
            ..
        } = self;
        PopupMenuItem::element(move |_, cx| {
            let t = Tokens::get(cx);
            let lead = icon.map(|glyph| {
                let tint = if chosen { t.accent } else { t.text_secondary };
                let glyph = super::icon(glyph, 14.).text_color(tint);
                match tiled {
                    true => div()
                        .flex_none()
                        .size(px(TILE))
                        .rounded(RADIUS_2)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(match chosen {
                            true => Hsla {
                                a: 0.16,
                                ..t.accent
                            },
                            false => t.surface_hover,
                        })
                        .child(glyph)
                        .into_any_element(),
                    false => glyph.into_any_element(),
                }
            });
            let ground = t.surface_chosen;
            let side = ROW_PADDING;
            div()
                .id(ElementId::Name(label.clone()))
                .aria_label(label.clone())
                .test_support()
                .relative()
                .flex_1()
                .min_w_0()
                .h(match about {
                    Some(_) => ABOUT_ROW_HEIGHT,
                    None => ROW_HEIGHT,
                })
                .flex()
                .items_center()
                .gap(SPACE_2H)
                .when(chosen, |row| {
                    row.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(-side)
                            .right(-side)
                            .rounded(RADIUS_2)
                            .bg(ground),
                    )
                })
                .children(lead)
                .children(mark.as_ref().map(|mark| mark()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(match &title {
                            Some(title) => title(),
                            None => div()
                                .truncate()
                                .text_color(t.text_primary)
                                .child(label.clone())
                                .into_any_element(),
                        })
                        .children(about.clone().map(|about| {
                            text(about, TEXT_3XS_H, FontWeight::NORMAL)
                                .truncate()
                                .text_color(t.text_tertiary)
                        })),
                )
                .children(hint.clone().map(|hint| {
                    text(hint, TEXT_3XS_H, FontWeight::NORMAL).text_color(t.text_tertiary)
                }))
        })
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
