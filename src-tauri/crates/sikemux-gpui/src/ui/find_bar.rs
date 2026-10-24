//! A find bar over the top right corner of what it searches: what to find, how many
//! places it was found, whether case and whole words count, and the way between them.
//! Enter, ⇧Enter and Esc are actions in its key context.

use std::rc::Rc;

use gpui_kit::component::input::InputState;
use gpui_kit::{
    actions, div, px, App, Entity, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, Pixels, RenderOnce, SharedString, Styled, Window,
};

use super::{mono, Field, IconButton, Toggle};
use crate::icons::Icon;
use crate::theme::{self, Tokens};

actions!(find_bar, [FindNext, FindPrevious, CloseFind]);

const CONTEXT: &str = "FindBar";

pub fn install(cx: &mut App) {
    let input = Some("FindBar > Input");
    cx.bind_keys([
        KeyBinding::new("enter", FindNext, input),
        KeyBinding::new("shift-enter", FindPrevious, input),
        KeyBinding::new("escape", CloseFind, input),
    ]);
}

type Act = Rc<dyn Fn(&mut Window, &mut App)>;
type Step = Rc<dyn Fn(bool, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct FindBar {
    input: Entity<InputState>,
    found: SharedString,
    match_case: bool,
    whole_word: bool,
    regex: Option<(bool, Act)>,
    on_case: Act,
    on_word: Act,
    on_step: Step,
    on_close: Act,
    top: Pixels,
}

impl FindBar {
    pub fn new(input: &Entity<InputState>, found: impl Into<SharedString>) -> Self {
        let none: Act = Rc::new(|_, _| {});
        Self {
            input: input.clone(),
            found: found.into(),
            match_case: false,
            whole_word: false,
            regex: None,
            on_case: none.clone(),
            on_word: none.clone(),
            on_step: Rc::new(|_, _, _| {}),
            on_close: none,
            top: px(7.),
        }
    }

    /// How far below the top edge it sits, to clear a header drawn over what it searches.
    pub fn below(mut self, top: Pixels) -> Self {
        self.top = top;
        self
    }

    /// Whether case counts, and what flips it.
    pub fn match_case(mut self, on: bool, flip: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        (self.match_case, self.on_case) = (on, Rc::new(flip));
        self
    }

    /// Whether only whole words count, and what flips it.
    pub fn whole_word(mut self, on: bool, flip: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        (self.whole_word, self.on_word) = (on, Rc::new(flip));
        self
    }

    /// Offers matching by regular expression: whether it is on, and what flips it.
    pub fn regex(mut self, on: bool, flip: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.regex = Some((on, Rc::new(flip)));
        self
    }

    /// Moves to the next place found, or with `false` the one before.
    pub fn on_step(mut self, step: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_step = Rc::new(step);
        self
    }

    pub fn on_close(mut self, close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Rc::new(close);
        self
    }
}

impl RenderOnce for FindBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx);
        let (step, close) = (self.on_step, self.on_close);
        let stepping = |forward: bool| {
            let step = step.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                step(forward, window, cx)
            }
        };
        let (next, previous, closing) = (step.clone(), step.clone(), close.clone());
        let (case, word) = (self.on_case, self.on_word);
        div()
            .key_context(CONTEXT)
            .on_action(move |_: &FindNext, window, cx| next(true, window, cx))
            .on_action(move |_: &FindPrevious, window, cx| previous(false, window, cx))
            .on_action(move |_: &CloseFind, window, cx| closing(window, cx))
            .absolute()
            .top(self.top)
            .right(px(12.))
            .flex()
            .items_center()
            .gap(px(3.))
            .p(px(5.))
            .border_1()
            .border_color(t.line)
            .rounded(theme::RADIUS_3)
            .bg(t.surface_overlay)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().w(px(250.)).child(Field::new(&self.input)))
            .child(
                div()
                    .min_w(px(52.))
                    .flex()
                    .justify_center()
                    .child(mono(self.found, 10., cx)),
            )
            .child(
                Toggle::new("find-case", "Aa", self.match_case)
                    .on_click(move |_, window, cx| case(window, cx)),
            )
            .child(
                Toggle::new("find-word", "W", self.whole_word)
                    .on_click(move |_, window, cx| word(window, cx)),
            )
            .children(self.regex.map(|(on, flip)| {
                Toggle::new("find-regex", ".*", on).on_click(move |_, window, cx| flip(window, cx))
            }))
            .child(
                IconButton::new("find-previous", Icon::ArrowUp, "Previous match")
                    .small()
                    .on_click(stepping(false)),
            )
            .child(
                IconButton::new("find-next", Icon::ArrowDown, "Next match")
                    .small()
                    .on_click(stepping(true)),
            )
            .child(
                IconButton::new("find-close", Icon::Close, "Close")
                    .small()
                    .on_click(move |_, window, cx| close(window, cx)),
            )
    }
}
