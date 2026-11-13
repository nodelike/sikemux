//! The questions the shell puts to the person over the window, and what their answers do.

use crate::ui::Clicks;
use std::rc::Rc;

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, Styled, Window,
};

use crate::theme::Tokens;
use crate::workbench::Workbench;

/// How the person answered a confirm: its main button, its second one, or Cancel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Yes,
    Other,
    No,
}

type Confirmed = Rc<dyn Fn(&mut Workbench, Choice, &mut Window, &mut Context<Workbench>)>;

type Typed = Rc<dyn Fn(&mut Workbench, String, &mut Window, &mut Context<Workbench>)>;

/// A question the shell puts to the person over the window, and what its answer does.
pub enum Asking {
    Confirm {
        title: SharedString,
        body: SharedString,
        yes: SharedString,
        other: Option<SharedString>,
        destructive: bool,
        then: Confirmed,
    },
    Prompt {
        title: SharedString,
        initial: String,
        field: Option<Entity<InputState>>,
        then: Typed,
    },
    Review(Entity<crate::agents::review::AgentReview>),
    PairPhone(Entity<crate::sim::pair::PairPhone>),
}

impl Workbench {
    /// Asks the person whether something goes ahead; `then` hears yes or no.
    pub fn confirm(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        destructive: bool,
        then: impl Fn(&mut Workbench, bool, &mut Window, &mut Context<Workbench>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.confirm_or(
            title,
            body,
            yes,
            None,
            destructive,
            move |bench, choice, window, cx| then(bench, choice == Choice::Yes, window, cx),
            cx,
        );
    }

    /// Asks as [`Self::confirm`] does, with a second way to go ahead named `other`.
    #[allow(clippy::too_many_arguments)]
    pub fn confirm_or(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        other: Option<SharedString>,
        destructive: bool,
        then: impl Fn(&mut Workbench, Choice, &mut Window, &mut Context<Workbench>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.overlays.asking = Some(Asking::Confirm {
            title: title.into(),
            body: body.into(),
            yes: yes.into(),
            other,
            destructive,
            then: Rc::new(then),
        });
        cx.notify();
    }

    /// Asks the person for a line of text, starting from `initial`; `then` hears what they
    /// typed, and nothing is done when they give up.
    pub fn prompt(
        &mut self,
        title: impl Into<SharedString>,
        initial: impl Into<String>,
        then: impl Fn(&mut Workbench, String, &mut Window, &mut Context<Workbench>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.overlays.asking = Some(Asking::Prompt {
            title: title.into(),
            initial: initial.into(),
            field: None,
            then: Rc::new(then),
        });
        cx.notify();
    }

    pub(crate) fn show_review(
        &mut self,
        review: Entity<crate::agents::review::AgentReview>,
        cx: &mut Context<Self>,
    ) {
        self.overlays.asking = Some(Asking::Review(review));
        cx.notify();
    }

    pub(crate) fn close_review(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlays.asking, Some(Asking::Review(_))) {
            self.take_asking(cx);
            cx.notify();
        }
    }

    pub(crate) fn show_pairing(
        &mut self,
        pairing: Entity<crate::sim::pair::PairPhone>,
        cx: &mut Context<Self>,
    ) {
        self.overlays.asking = Some(Asking::PairPhone(pairing));
        cx.notify();
    }

    pub(crate) fn close_pairing(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlays.asking, Some(Asking::PairPhone(_))) {
            self.take_asking(cx);
            cx.notify();
        }
    }

    /// Takes the question being asked away, however it was answered; a trust question that
    /// came in behind it is asked next.
    pub(crate) fn take_asking(&mut self, cx: &mut Context<Self>) -> Option<Asking> {
        let bench = cx.entity();
        cx.defer(move |cx| bench.update(cx, |bench, cx| bench.ask_trust(cx)));
        self.overlays.asking.take()
    }

    /// The question being asked, as a dialog, making a prompt's field the first time.
    pub(crate) fn asked(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let bench = cx.entity();
        let answered = move |choice: Choice| {
            let bench = bench.clone();
            move |window: &mut Window, cx: &mut App| {
                bench.update(cx, |bench, cx| {
                    if let Some(Asking::Confirm { then, .. }) = bench.take_asking(cx) {
                        then(bench, choice, window, cx);
                    }
                    cx.notify();
                })
            }
        };
        Some(match self.overlays.asking.as_mut()? {
            Asking::Review(review) => {
                let leaving = cx.entity();
                panel("agent-review", review.clone().into(), cx, move |cx| {
                    leaving.update(cx, |bench, cx| bench.close_review(cx));
                })
            }
            Asking::PairPhone(pairing) => {
                let leaving = cx.entity();
                panel("pair-phone", pairing.clone().into(), cx, move |cx| {
                    leaving.update(cx, |bench, cx| bench.close_pairing(cx));
                })
            }
            Asking::Confirm {
                title,
                body,
                yes,
                other,
                destructive,
                ..
            } => crate::ui::Confirm::new(title.clone(), body.clone(), yes.clone())
                .marked()
                .destructive(*destructive)
                .on_yes(answered(Choice::Yes))
                .on_no(answered(Choice::No))
                .when_some(other.clone(), |confirm, other| {
                    confirm.other(other, answered(Choice::Other))
                })
                .into_any_element(),
            Asking::Prompt {
                title,
                initial,
                field,
                ..
            } => {
                let field = field
                    .get_or_insert_with(|| {
                        let initial = initial.clone();
                        cx.new(|cx| {
                            let mut state = InputState::new(window, cx);
                            state.set_value(initial, window, cx);
                            state.focus(window, cx);
                            state.select_all(window, cx);
                            state
                        })
                    })
                    .clone();
                let (typing, giving_up) = (cx.entity(), cx.entity());
                crate::ui::Prompt::new(title.clone(), &field)
                    .marked()
                    .on_yes(move |typed, window, cx| {
                        typing.update(cx, |bench, cx| {
                            if let Some(Asking::Prompt { then, .. }) = bench.take_asking(cx) {
                                then(bench, typed.trim().to_owned(), window, cx);
                            }
                            cx.notify();
                        })
                    })
                    .on_no(move |_, cx| {
                        giving_up.update(cx, |bench, cx| {
                            bench.take_asking(cx);
                            cx.notify();
                        })
                    })
                    .into_any_element()
            }
        })
    }
}

/// A panel floating over the dimmed window, which a click beside it closes.
fn panel(
    id: &'static str,
    view: gpui_kit::AnyView,
    cx: &App,
    close: impl Fn(&mut App) + 'static,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    div()
        .id(SharedString::from(format!("{id}-backdrop")))
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui_kit::black().opacity(0.4))
        .occlude()
        .clicks_keep_cursor(move |_, _, cx| close(cx))
        .child(
            div()
                .id(id)
                .occlude()
                .clicks_keep_cursor(|_, _, cx| cx.stop_propagation())
                .bg(t.surface_overlay)
                .border_1()
                .border_color(t.border)
                .rounded(crate::theme::radius_3())
                .shadow(t.shadow_float())
                .child(view),
        )
        .into_any_element()
}
