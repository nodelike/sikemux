use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    div, px, AnimationExt, AnyElement, AnyView, App, AppContext, ClickEvent, Context, Div,
    ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window,
};
use smallvec::SmallVec;

use super::controls::OnClick;
use super::{icon, text};
use crate::icons::Icon;
use crate::motion;
use crate::theme::{Tokens, RADIUS_2, RADIUS_4};

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

/// A float of choices.
#[derive(IntoElement)]
pub struct Menu {
    items: SmallVec<[AnyElement; 8]>,
}

impl Menu {
    pub fn new() -> Self {
        Self {
            items: SmallVec::new(),
        }
    }

    /// A hairline between groups of items. Like Kit's, it is an item with no label to anything
    /// reading the window, so menus of both kinds read the same.
    pub fn separator(mut self, cx: &App) -> Self {
        let id = ElementId::NamedInteger("menu-separator".into(), self.items.len() as u64);
        self.items.push(
            div()
                .id(id)
                .test_support()
                .h(px(1.))
                .mx(px(2.))
                .my(px(4.))
                .bg(Tokens::get(cx).border)
                .into_any_element(),
        );
        self
    }
}

impl ParentElement for Menu {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.items.extend(elements);
    }
}

impl RenderOnce for Menu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        float(cx)
            .id("menu")
            .test_support()
            .role(Role::Menu)
            .min_w(px(196.))
            .max_w(px(340.))
            .p(px(4.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .children(self.items)
    }
}

/// One of a menu's choices: an icon, its label, an optional line under it and
/// a hint at the end, ticked when it is the current choice.
#[derive(IntoElement)]
pub struct MenuItem {
    id: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    mark: Option<AnyElement>,
    hint: Option<SharedString>,
    checked: bool,
    disabled: bool,
    danger: bool,
    on_click: Option<OnClick>,
}

impl MenuItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            mark: None,
            hint: None,
            checked: false,
            disabled: false,
            danger: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A mark drawn in its own colours where the icon goes, such as an agent's.
    pub fn mark(mut self, mark: impl IntoElement) -> Self {
        self.mark = Some(mark.into_any_element());
        self
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Drawn in the danger colour, for a choice that deletes or discards.
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for MenuItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let (rest, hover, wash) = if self.danger {
            (
                crate::theme::mix(t.danger, t.text_secondary, 0.85),
                t.danger,
                t.danger.opacity(0.12),
            )
        } else {
            (t.text_secondary, t.text_primary, t.surface_hover)
        };
        let on_click = self.on_click.filter(|_| !self.disabled);
        let (over, keep_over) = motion::hovered(&self.id, window, cx);
        let lit = over && !self.disabled;
        let fade =
            |channel, lit_colour: Hsla, rest_colour: Hsla, window: &mut Window, cx: &mut App| {
                let target = if lit { lit_colour } else { rest_colour };
                motion::fade(&self.id, channel, target, motion::MENU_ITEM, window, cx)
            };
        let tint = fade("tint", wash, wash.opacity(0.), window, cx);
        let ink = fade(
            "ink",
            hover,
            if self.disabled { t.text_tertiary } else { rest },
            window,
            cx,
        );
        let glyph_rest = if self.danger { rest } else { t.text_tertiary };
        let glyph_ink = fade("glyph", hover, glyph_rest, window, cx);
        BaseButton::new(self.id.clone())
            .group(GROUP)
            .disabled(self.disabled)
            .role(Role::MenuItem)
            .accessibility_label(self.label.clone())
            .w_full()
            .px(px(10.))
            .py(px(5.))
            .flex()
            .items_center()
            .justify_start()
            .gap(px(10.))
            .rounded(RADIUS_2)
            .text_size(crate::theme::ui_text(12.))
            .line_height(crate::theme::ui_text(17.))
            .text_color(ink)
            .bg(tint)
            .on_hover(keep_over)
            .when_some(on_click, |item, handler| {
                item.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .children(self.icon.map(|glyph| {
                div()
                    .min_w(px(14.))
                    .mr(px(-2.))
                    .child(icon(glyph, 13.).text_color(glyph_ink))
            }))
            .children(
                self.mark
                    .map(|mark| div().min_w(px(14.)).mr(px(-2.)).child(mark)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(self.label),
            )
            .children(
                self.hint
                    .map(|hint| text(hint, 10.5, FontWeight::NORMAL).text_color(t.text_tertiary)),
            )
            .when(self.checked, |item| {
                item.child(icon(Icon::Check, 11.).text_color(t.accent))
            })
    }
}

/// The hover group a menu item puts its icon in, so the icon follows its text.
const GROUP: &str = "menu-item";

/// A right-click menu's choice in the danger colour, for one that deletes or discards, as
/// `MenuItem::danger` draws a float's. GPUI Kit's own items have no such colour, so its label
/// is drawn here, named for accessibility, and keeps the danger colour while the item is hovered.
pub fn danger_popup_item(
    label: impl Into<SharedString>,
    icon: Option<Icon>,
    cx: &App,
) -> PopupMenuItem {
    let t = Tokens::get(cx);
    let ink = crate::theme::mix(t.danger, t.text_secondary, 0.85);
    let label = label.into();
    let item = PopupMenuItem::element(move |_, _| {
        div()
            .id(ElementId::Name(label.clone()))
            .aria_label(label.clone())
            .text_color(ink)
            .child(label.clone())
            .test_support()
    });
    match icon {
        Some(glyph) => item.icon(gpui_kit::component::Icon::from(glyph).text_color(ink)),
        None => item,
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
            .text_size(crate::theme::ui_text(11.5))
            .line_height(crate::theme::ui_text(16.))
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
