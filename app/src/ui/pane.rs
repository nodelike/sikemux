use crate::ui::Clicks;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window,
};
use smallvec::SmallVec;

use super::{mark_box, meta, text};
use crate::theme::{
    Tokens, RADIUS_3, RADIUS_4, SPACE_0, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, TEXT_SM,
    TEXT_XS,
};

/// How far an agent pane's contents sit inside its frame, as on the Mac.
pub const AGENT_INSET: f32 = 6.;

/// A pane's frame over the pane field. Panes paint no ground of their own: the shell's shows
/// through.
pub fn pane(cx: &App) -> Div {
    pane_frame(cx).children(super::grain::field(cx))
}

/// A pane's frame with nothing behind what it shows.
pub fn pane_frame(cx: &App) -> Div {
    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .border_1()
        .border_color(Tokens::get(cx).border)
        .rounded(RADIUS_3)
}

/// A bar across the top of a pane saying something about what it shows, in `tone`, with the
/// actions it offers at the end.
pub fn notice_bar(
    text: impl Into<SharedString>,
    tone: Hsla,
    actions: impl IntoIterator<Item = impl IntoElement>,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .px(SPACE_3)
        .py(SPACE_1H)
        .border_b_1()
        .border_color(t.border)
        .bg(t.surface_sunken)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(tone)
                .font_weight(FontWeight::MEDIUM)
                .child(text.into()),
        )
        .children(actions)
}

/// A detail panel that floats over the right of a page, which stays in view behind it.
/// The page's frame must be `relative`.
pub fn floating_panel(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let t = Tokens::get(cx);
    div()
        .id(id)
        .absolute()
        .top(SPACE_3)
        .right(SPACE_3)
        .bottom(SPACE_3)
        .w(relative(0.42))
        .min_w(px(360.))
        .max_w(px(540.))
        .occlude()
        .overflow_y_scroll()
        .rounded(RADIUS_4)
        .border_1()
        .border_color(t.border_strong)
        .bg(t.surface_sunken)
        .shadow(t.shadow_float())
}

/// An agent pane's header: a bar across the top of the frame, what the pane is on the left
/// and its controls and actions on the right. What the pane shows starts below it, so
/// nothing passes underneath.
#[derive(IntoElement)]
pub struct PaneHeader {
    back: Option<AnyElement>,
    mark: Option<AnyElement>,
    title: SharedString,
    /// What shows in the title's place, such as a field editing it.
    title_view: Option<AnyElement>,
    on_title_double_click: Option<super::controls::OnClick>,
    name: Option<SharedString>,
    fact: Option<AnyElement>,
    actions: SmallVec<[AnyElement; 2]>,
    controls: Option<AnyElement>,
    side: Option<AnyElement>,
}

impl PaneHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            back: None,
            mark: None,
            title: title.into(),
            title_view: None,
            on_title_double_click: None,
            name: None,
            fact: None,
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

    /// What leads the header before the mark, such as the chain down to a worker.
    pub fn back(mut self, back: impl IntoElement) -> Self {
        self.back = Some(back.into_any_element());
        self
    }

    pub fn name(mut self, name: Option<SharedString>) -> Self {
        self.name = name;
        self
    }

    /// What stands after the name, such as how a terminal agent's turn stands.
    pub fn fact(mut self, fact: impl IntoElement) -> Self {
        self.fact = Some(fact.into_any_element());
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

    /// What sits last, after the controls, as the desk toggle.
    pub fn side(mut self, side: impl IntoElement) -> Self {
        self.side = Some(side.into_any_element());
        self
    }
}

impl RenderOnce for PaneHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx);
        let (primary, border) = (t.text_primary, t.border);
        let rule = || {
            div()
                .flex_none()
                .w(px(1.))
                .h(px(14.))
                .mx(SPACE_0)
                .bg(border)
        };
        let has_actions = !self.actions.is_empty();
        div()
            .flex_none()
            .w_full()
            .h(px(44.))
            .flex()
            .items_center()
            .gap(SPACE_2)
            .pl(px(18.))
            .pr(SPACE_3H)
            .children(self.back)
            .children(self.mark.map(|mark| mark_box(18., mark).flex_none()))
            .children(self.title_view.or_else(|| {
                let double_click = self.on_title_double_click;
                (!self.title.is_empty()).then(|| {
                    text(self.title, TEXT_SM, FontWeight::MEDIUM)
                        .id("pane-title")
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(primary)
                        .when_some(double_click, |title, handler| {
                            title.clicks_keep_cursor(move |event, window, cx| {
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
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .min_w_0()
                    .truncate()
            }))
            .children(
                self.fact
                    .map(|fact| div().min_w_0().ml(crate::theme::SPACE_1).child(fact)),
            )
            .child(div().flex_1())
            .when_some(self.controls, |bar, controls| {
                bar.child(div().flex_none().child(controls))
                    .when(has_actions, |bar| bar.child(rule()))
            })
            .children(
                self.actions
                    .into_iter()
                    .map(|action| div().flex_none().child(action)),
            )
            .children(self.side.map(|side| div().flex_none().child(side)))
    }
}
