use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClickEvent, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};
use smallvec::SmallVec;

use super::{mark_box, meta, text};
use crate::theme::{Tokens, RADIUS_3};

/// How far an agent pane's contents sit inside its frame, as on the Mac.
pub const AGENT_INSET: f32 = 6.;

/// A pane's frame. Panes paint no ground of their own: the shell's shows through.
pub fn pane(cx: &App) -> Div {
    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .border_1()
        .border_color(Tokens::get(cx).border)
        .rounded(RADIUS_3)
        .children(super::grain::field(cx))
}

/// The blurred pill a pane's header is made of.
pub fn pill(cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .h(px(32.))
        .flex()
        .items_center()
        .border_1()
        .border_color(t.border)
        .rounded_full()
        .bg(Hsla {
            a: 0.62,
            ..t.surface_sunken
        })
        .backdrop_blur(px(14.))
}

/// An agent pane's header, two pills floating over what the pane shows: what it
/// is with its actions, then its controls and anything beside them. It sits as far
/// in from the frame's top as from its sides, as on the Mac.
#[derive(IntoElement)]
pub struct PaneHeader {
    mark: Option<AnyElement>,
    title: SharedString,
    /// What shows in the title's place, such as a field editing it.
    title_view: Option<AnyElement>,
    on_title_double_click: Option<super::controls::OnClick>,
    name: Option<SharedString>,
    actions: SmallVec<[AnyElement; 2]>,
    controls: Option<AnyElement>,
    side: Option<AnyElement>,
}

impl PaneHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            mark: None,
            title: title.into(),
            title_view: None,
            on_title_double_click: None,
            name: None,
            actions: SmallVec::new(),
            controls: None,
            side: None,
        }
    }

    pub fn title_view(mut self, view: Option<impl IntoElement>) -> Self {
        self.title_view = view.map(IntoElement::into_any_element);
        self
    }

    pub fn on_title_double_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_title_double_click = Some(Rc::new(handler));
        self
    }

    pub fn mark(mut self, mark: impl IntoElement) -> Self {
        self.mark = Some(mark.into_any_element());
        self
    }

    pub fn name(mut self, name: Option<SharedString>) -> Self {
        self.name = name;
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    pub fn controls(mut self, controls: impl IntoElement) -> Self {
        self.controls = Some(controls.into_any_element());
        self
    }

    /// What sits beside the controls in a circle of its own, as the desk toggle.
    pub fn side(mut self, side: impl IntoElement) -> Self {
        self.side = Some(side.into_any_element());
        self
    }
}

impl RenderOnce for PaneHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let primary = Tokens::get(cx).text_primary;
        div()
            .absolute()
            .top(px(AGENT_INSET + 2.))
            .left(px(AGENT_INSET + 6.))
            .right(px(AGENT_INSET + 4.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                pill(cx)
                    .min_w_0()
                    .overflow_hidden()
                    .pl(px(8.))
                    .pr(px(4.))
                    .gap(px(8.))
                    .children(self.mark.map(|mark| mark_box(18., mark).flex_none()))
                    .children(self.title_view.or_else(|| {
                        let double_click = self.on_title_double_click;
                        (!self.title.is_empty()).then(|| {
                            text(self.title, 13., FontWeight::MEDIUM)
                                .id("pane-title")
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(primary)
                                .when_some(double_click, |title, handler| {
                                    title.on_click(move |event, window, cx| {
                                        if event.click_count() == 2 {
                                            handler(event, window, cx)
                                        }
                                    })
                                })
                                .into_any_element()
                        })
                    }))
                    .children(self.name.map(|name| {
                        meta(name, cx)
                            .text_size(crate::theme::ui_text(12.))
                            .min_w_0()
                            .truncate()
                    }))
                    .children(
                        self.actions
                            .into_iter()
                            .map(|action| div().flex_none().child(action)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .children(
                        self.controls
                            .map(|controls| pill(cx).px(px(3.)).gap(px(6.)).child(controls)),
                    )
                    .when_some(self.side, |end, side| {
                        end.child(pill(cx).size(px(32.)).justify_center().child(side))
                    }),
            )
    }
}
