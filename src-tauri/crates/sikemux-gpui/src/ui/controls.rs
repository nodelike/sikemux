use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::{Button as BaseButton, Selectable};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClickEvent, ElementId, Entity, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Radians, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, Transformation, Window,
};

use super::{icon, kbd, text};
use crate::icons::Icon;
use crate::motion;
use crate::theme::{Tokens, RADIUS_2};

pub type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type OnSelect = Rc<dyn Fn(&usize, &mut Window, &mut App)>;

/// How much of its colour an icon button keeps while it can't be pressed, low enough to tell
/// at a glyph's size from one that can.
const DISABLED_STRENGTH: f32 = 0.4;

/// A glyph you press: the rails' header actions at 26px, their add buttons at 18px.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: Icon,
    size: f32,
    width: Option<f32>,
    height: Option<f32>,
    glyph: f32,
    quiet: bool,
    round: bool,
    pressed: bool,
    disabled: bool,
    turned: f32,
    tint: Option<Hsla>,
    label: SharedString,
    on_click: Option<OnClick>,
}

impl IconButton {
    /// `label` names it for the accessibility tree and its tooltip.
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            size: 26.,
            height: None,
            width: None,
            glyph: 15.,
            quiet: true,
            round: false,
            pressed: false,
            disabled: false,
            turned: 0.,
            tint: None,
            label: label.into(),
            on_click: None,
        }
    }

    /// Drawn in `tint` on a wash of it, as the mic while it listens.
    pub fn tint(mut self, tint: Hsla) -> Self {
        self.tint = Some(tint);
        self
    }

    /// Shown, but with nothing to do now, as Back on a page with no history.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Turns the glyph by `radians`, as a chevron pointing back.
    pub fn turned(mut self, radians: f32) -> Self {
        self.turned = radians;
        self
    }

    /// The 18px add and edit buttons beside a group's header.
    pub fn small(mut self) -> Self {
        self.size = 18.;
        self.glyph = 11.;
        self.quiet = false;
        self
    }

    /// A 24px circle in a pane's header pill.
    pub fn round(mut self) -> Self {
        self.size = 24.;
        self.glyph = 13.;
        self.quiet = false;
        self.round = true;
        self
    }

    /// Wider than tall, as the rail's page switches.
    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn glyph(mut self, size: f32) -> Self {
        self.glyph = size;
        self
    }

    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
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

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx);
        let enabled = if self.quiet {
            t.icon_quiet
        } else {
            t.text_tertiary
        };
        let rest = match self.disabled {
            true => Hsla {
                a: enabled.a * DISABLED_STRENGTH,
                ..enabled
            },
            false => enabled,
        };
        let (hover, primary, chosen) = (t.surface_hover, t.text_primary, t.surface_chosen);
        let (hover, primary) = if self.disabled {
            (Hsla::transparent_black(), rest)
        } else {
            (hover, primary)
        };
        let (rest, hover, primary, chosen) = match self.tint {
            Some(tint) => {
                let wash = Hsla { a: 0.14, ..tint };
                (tint, wash, tint, wash)
            }
            None => (rest, hover, primary, chosen),
        };
        let on_click = self.on_click.filter(|_| !self.disabled);
        let (over, keep_over) = motion::hovered(&self.id, window, cx);
        let wash = match (over, self.pressed) {
            (true, _) => hover,
            (false, true) => chosen,
            (false, false) => hover.opacity(0.),
        };
        let wash = motion::fade(&self.id, "wash", wash, motion::CONTROL_TINT, window, cx);
        let ink = if over || self.pressed { primary } else { rest };
        let ink = motion::fade(&self.id, "ink", ink, motion::CONTROL_TINT, window, cx);
        let glyph = icon(self.icon, self.glyph).text_color(ink);
        BaseButton::new(self.id.clone())
            .group(GROUP)
            .tooltip(super::tooltip(self.label.clone()))
            .accessibility_label(self.label)
            .size(px(self.size))
            .when_some(self.width, |button, width| button.w(px(width)))
            .when_some(self.height, |button, height| button.h(px(height)))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .map(|button| {
                if self.round {
                    button.rounded_full()
                } else {
                    button.rounded(RADIUS_2)
                }
            })
            .text_color(ink)
            .bg(wash)
            .on_hover(keep_over)
            .when_some(on_click, |button, handler| {
                button.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .child(crate::motion::animated(
                ElementId::NamedChild(Arc::new(self.id), "turned".into()),
                self.turned,
                crate::motion::LANE_FOLD,
                move |turned| {
                    glyph
                        .with_transformation(Transformation::rotate(Radians(turned)))
                        .into_any_element()
                },
            ))
    }
}

impl Selectable for IconButton {
    fn selected(mut self, selected: bool) -> Self {
        self.pressed = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.pressed
    }
}

/// The hover group every control here puts its glyph in, so the glyph follows the text.
const GROUP: &str = "control";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// A hairline box, as the empty stage's actions.
    Outline,
    /// Filled with the accent, for the one thing a place asks you to do.
    Primary,
    /// A dashed box for adding something that is not there yet.
    Dashed,
}

/// A labelled button, optionally with a leading icon and a trailing shortcut.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    variant: Variant,
    keys: Option<SharedString>,
    on_click: Option<OnClick>,
    disabled: bool,
    pressed: bool,
    lead: Option<AnyElement>,
    trail: Option<AnyElement>,
    tooltip: Option<SharedString>,
}

/// A button can open a popover; it looks the same open or closed.
impl Selectable for Button {
    fn selected(self, _: bool) -> Self {
        self
    }

    fn is_selected(&self) -> bool {
        false
    }
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, variant: Variant) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            variant,
            keys: None,
            on_click: None,
            disabled: false,
            pressed: false,
            lead: None,
            trail: None,
            tooltip: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Dimmed, and does nothing when clicked.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Lifted as a chosen item is, while what it opens is open.
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// Something drawn before the label in place of an icon, such as a status dot.
    pub fn lead(mut self, lead: impl IntoElement) -> Self {
        self.lead = Some(lead.into_any_element());
        self
    }

    /// Something drawn after the label, such as a count.
    pub fn trail(mut self, trail: impl IntoElement) -> Self {
        self.trail = Some(trail.into_any_element());
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// The shortcut that does the same, shown after the label.
    pub fn keys(mut self, keys: Option<SharedString>) -> Self {
        self.keys = keys;
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

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let on_click = self.on_click.filter(|_| !self.disabled);
        let (over, keep_over) = motion::hovered(&self.id, window, cx);
        let lit = over && !self.disabled;
        let mut fade = |channel, target| {
            motion::fade(&self.id, channel, target, motion::CONTROL_TINT, window, cx)
        };
        let base = BaseButton::new(self.id.clone())
            .on_hover(keep_over)
            .when(self.disabled, |button| button.opacity(0.5))
            .group(GROUP)
            .accessibility_label(self.label.clone())
            .flex()
            .items_center()
            .justify_start()
            .gap(px(8.))
            .rounded(RADIUS_2)
            .whitespace_nowrap();
        let base = match self.variant {
            Variant::Outline => base
                .h(px(28.))
                .px(px(12.))
                .border_1()
                .border_color(fade("edge", if lit { t.border_strong } else { t.border }))
                .text_size(crate::theme::ui_text(11.5))
                .text_color(fade(
                    "ink",
                    if lit {
                        t.text_primary
                    } else {
                        t.text_secondary
                    },
                ))
                .bg(fade(
                    "tint",
                    if lit {
                        t.surface_hover
                    } else {
                        t.surface_hover.opacity(0.)
                    },
                )),
            Variant::Primary => base
                .h(px(28.))
                .px(px(12.))
                .justify_center()
                .gap(px(6.))
                .bg(t.accent)
                .text_size(crate::theme::ui_text(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.on_accent),
            Variant::Dashed => base
                .h(px(22.))
                .px(px(8.))
                .gap(px(5.))
                .border_1()
                .border_dashed()
                .border_color(t.border)
                .text_size(crate::theme::ui_text(11.))
                .text_color(fade(
                    "ink",
                    if lit { t.text_primary } else { t.text_tertiary },
                )),
        };
        let (glyph, rest) = match self.variant {
            Variant::Primary => (13., t.on_accent),
            Variant::Outline => (11., t.text_secondary),
            Variant::Dashed => (11., t.text_tertiary),
        };
        let hover = if self.variant == Variant::Primary {
            t.on_accent
        } else {
            t.text_primary
        };
        let base = base
            .when(self.pressed, |button| {
                button.bg(t.surface_chosen).text_color(t.text_primary)
            })
            .when_some(self.tooltip, |button, tip| {
                button.tooltip(super::tooltip(tip))
            });
        base.when_some(on_click, |button, handler| {
            button.on_click(move |event, window, cx| handler(event, window, cx))
        })
        .children(self.lead)
        .children(self.icon.map(|icon_name| {
            icon(icon_name, glyph).text_color(fade("glyph", if lit { hover } else { rest }))
        }))
        .child(self.label)
        .children(self.trail)
        .children(
            self.keys
                .map(|keys| kbd(keys, cx).font_family(crate::theme::MONO)),
        )
    }
}

/// A track of choices with the chosen one lifted, as the agent rail's scope.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    /// Each choice's icon, its label, and how many it holds when that is known.
    options: Vec<(Option<Icon>, SharedString, Option<u64>)>,
    selected: usize,
    pill: bool,
    fill: bool,
    on_select: Option<OnSelect>,
    /// Why it cannot be changed now, shown over it; none while it can.
    disabled: Option<SharedString>,
}

impl Segmented {
    pub fn new(
        id: impl Into<ElementId>,
        options: Vec<(Icon, SharedString)>,
        selected: usize,
    ) -> Self {
        let options = options
            .into_iter()
            .map(|(glyph, label)| (Some(glyph), label, None))
            .collect();
        Self {
            id: id.into(),
            options,
            selected,
            pill: false,
            fill: false,
            on_select: None,
            disabled: None,
        }
    }

    /// Choices in words alone, each with how many it holds when that is known.
    pub fn words(
        id: impl Into<ElementId>,
        words: Vec<(SharedString, Option<u64>)>,
        selected: usize,
    ) -> Self {
        let options = words
            .into_iter()
            .map(|(label, count)| (None, label, count))
            .collect();
        Self {
            id: id.into(),
            options,
            selected,
            pill: false,
            fill: false,
            on_select: None,
            disabled: None,
        }
    }

    /// Spans the width it is given, its choices sharing it, as a side panel's segment does.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// The round, borderless switch in a pane's header, as GUI and TUI.
    pub fn pill(mut self) -> Self {
        self.pill = true;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn disabled_because(mut self, reason: Option<impl Into<SharedString>>) -> Self {
        self.disabled = reason.map(Into::into);
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let on_select = self.on_select.filter(|_| self.disabled.is_none());
        let (pill, fill) = (self.pill, self.fill);
        div()
            .id(self.id)
            .flex()
            .when(fill, |track| track.w_full())
            .when_some(self.disabled, |track, reason| {
                track.opacity(0.5).tooltip(super::tooltip(reason))
            })
            .map(|track| {
                if pill {
                    track.h(px(24.)).p(px(2.)).rounded_full()
                } else {
                    track
                        .h(px(30.))
                        .p(px(3.))
                        .rounded(RADIUS_2)
                        .border_1()
                        .border_color(t.border)
                }
            })
            .children(
                self.options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (glyph, label, count))| {
                        let chosen = index == self.selected;
                        let on_select = on_select.clone();
                        BaseButton::new(("segment", index))
                            .group(GROUP)
                            .selected(chosen)
                            .accessibility_label(label.clone())
                            .when(fill, |segment| segment.flex_1().min_w_0())
                            .when(!fill && !pill && glyph.is_some(), |segment| {
                                segment.flex_1()
                            })
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(if pill { 5. } else { 7. }))
                            .map(|segment| {
                                if pill {
                                    segment.rounded_full()
                                } else {
                                    segment.rounded(px(3.))
                                }
                            })
                            .text_color(if chosen {
                                t.text_primary
                            } else {
                                t.text_tertiary
                            })
                            .when(chosen, |segment| segment.bg(t.surface_active))
                            .hover(move |style| style.text_color(t.text_primary))
                            .when_some(on_select, |segment, handler| {
                                segment.on_click(move |_, window, cx| handler(&index, window, cx))
                            })
                            .children(glyph.map(|glyph| {
                                icon(glyph, 13.)
                                    .text_color(if chosen {
                                        t.text_primary
                                    } else {
                                        t.text_tertiary
                                    })
                                    .group_hover(GROUP, move |style| {
                                        style.text_color(t.text_primary)
                                    })
                            }))
                            .child(
                                text(label, if pill { 10.5 } else { 11. }, FontWeight::NORMAL)
                                    .overflow_hidden()
                                    .text_ellipsis(),
                            )
                            .children(count.map(|count| {
                                text(count.to_string(), 10., FontWeight::NORMAL)
                                    .font_features(crate::theme::tabular())
                                    .text_color(t.text_tertiary)
                            }))
                    }),
            )
    }
}

/// A one-line text field in the rails' box, on GPUI Kit's input.
#[derive(IntoElement)]
pub struct Field {
    state: Entity<InputState>,
    icon: Option<Icon>,
    flush: bool,
    mono: bool,
    hint: Option<SharedString>,
}

impl Field {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            icon: None,
            flush: false,
            mono: false,
            hint: None,
        }
    }

    /// For what is typed as code: a path, a command, a colour.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// The keys that bring the field into focus, shown at its end.
    pub fn hint(mut self, keys: Option<SharedString>) -> Self {
        self.hint = keys;
        self
    }

    /// A row across the top of a dropdown, over a hairline, with no box or ring.
    pub fn flush(mut self) -> Self {
        self.flush = true;
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl RenderOnce for Field {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx);
        let field = div().flex().items_center().gap(px(7.));
        match self.flush {
            true => field
                .h(px(32.))
                .px(px(12.))
                .border_b_1()
                .border_color(t.border),
            false => field
                .h(px(34.))
                .px(px(10.))
                .border_1()
                .border_color(t.line)
                .rounded(RADIUS_2)
                .bg(Hsla {
                    a: 0.62,
                    ..t.line_soft
                }),
        }
        .text_color(t.text_tertiary)
        .children(
            self.icon
                .map(|glyph| icon(glyph, 12.).text_color(t.text_tertiary)),
        )
        .child(
            div()
                .flex_1()
                .map(|text| match self.mono {
                    true => text
                        .font_family(crate::theme::MONO)
                        .text_size(crate::theme::ui_text(12.)),
                    false => text.text_size(crate::theme::ui_text(13.)),
                })
                .text_color(t.text_primary)
                .child(Input::new(&self.state).appearance(false).bordered(false)),
        )
        .children(self.hint.map(|keys| kbd(keys, cx).flex_none()))
    }
}

/// A small text button that stays pressed while its choice holds, as the
/// calendar's metrics.
#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    label: SharedString,
    pressed: bool,
    on_click: Option<OnClick>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, pressed: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            pressed,
            on_click: None,
        }
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let on_click = self.on_click;
        BaseButton::new(self.id)
            .selected(self.pressed)
            .accessibility_label(self.label.clone())
            .h(px(22.))
            .px(px(8.))
            .flex()
            .items_center()
            .border_1()
            .border_color(if self.pressed {
                t.border
            } else {
                Hsla::transparent_black()
            })
            .when(self.pressed, |toggle| toggle.bg(t.surface_chosen))
            .text_size(crate::theme::ui_text(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if self.pressed {
                t.text_primary
            } else {
                t.text_tertiary
            })
            .hover(move |style| style.bg(t.surface_hover).text_color(t.text_primary))
            .when_some(on_click, |toggle, handler| {
                toggle.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .child(self.label)
    }
}
