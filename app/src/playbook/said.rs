//! The line under each of the Playbook's rows: who said it and where, with Open, which jumps to
//! that moment in the chat, and Undo.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, App, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window,
};
use sikemux_view::memory::Learned;

use crate::core::ChatAsk;
use crate::theme::{self, Tokens, SPACE_1, TEXT_2XS_H, TEXT_LG};
use crate::workbench::Workbench;

/// The group a row joins so its quiet actions show while the pointer is over it.
pub(super) const ROW: &str = "playbook-row";

/// A moment in an agent's chat, which Open jumps to.
#[derive(Clone)]
pub(crate) struct Moment {
    agent: String,
    feed: Option<String>,
    turn: Option<u64>,
}

impl Moment {
    pub(crate) fn new(agent: String, feed: Option<String>, turn: Option<u64>) -> Self {
        Self { agent, feed, turn }
    }

    /// Where `learned` was said, when it was said in a chat.
    pub(super) fn of(learned: &Learned) -> Option<Self> {
        let agent = (learned.agent.clone())
            .or_else(|| learned.with.as_ref().map(|with| with.agent.clone()))?;
        Some(Self {
            agent,
            feed: learned.feed.clone(),
            turn: learned.turn,
        })
    }
}

/// Who said it, as the line opens: "You, in chat with @Ivo", "You" or "@Ada".
pub(super) fn who(learned: &Learned) -> String {
    match (&learned.agent, &learned.with) {
        (Some(_), _) => at_name(learned.name.as_deref()),
        (None, Some(with)) => t!(
            "You, in chat with {name}",
            name = at_name(with.name.as_deref())
        ),
        (None, None) => t!("You").to_owned(),
    }
}

pub(super) fn at_name(name: Option<&str>) -> String {
    name.map_or_else(|| t!("An agent").to_owned(), |name| format!("@{name}"))
}

/// The day something was said, as "9 Nov".
pub(super) fn day(at: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(at as i64)
        .single()
        .map(|at| at.format("%-d %b").to_string())
        .unwrap_or_default()
}

type Act = Rc<dyn Fn(&mut Workbench, &mut Context<Workbench>)>;

/// A quiet action at the end of the line.
pub(super) struct Action {
    label: &'static str,
    act: Act,
    /// It shows only while the pointer is over its row.
    on_hover: bool,
}

impl Action {
    pub(super) fn new(
        label: &'static str,
        act: impl Fn(&mut Workbench, &mut Context<Workbench>) + 'static,
    ) -> Self {
        Self {
            label,
            act: Rc::new(act),
            on_hover: false,
        }
    }

    pub(super) fn on_hover(mut self) -> Self {
        self.on_hover = true;
        self
    }
}

/// The line: `words`, then Open when `moment` is known, then `actions`, all parted by dots.
pub(super) fn line(
    id: impl Into<SharedString>,
    words: impl Into<SharedString>,
    moment: Option<Moment>,
    actions: Vec<Action>,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id: SharedString = id.into();
    let open = moment.map(|moment| {
        Action::new(t!("Open"), move |bench, cx| {
            open_moment(bench, moment.clone(), cx)
        })
    });
    let link = |at: usize, action: Action| {
        let (this, act) = (this.clone(), action.act.clone());
        div()
            .flex()
            .gap(SPACE_1)
            .when(action.on_hover, |part| {
                part.opacity(0.).group_hover(ROW, |part| part.opacity(1.))
            })
            .child("·")
            .child(
                div()
                    .id(SharedString::from(format!("{id}-{at}")))
                    .cursor_pointer()
                    .text_color(t.text_secondary)
                    .hover(|word| word.text_color(t.text_primary))
                    .child(action.label)
                    .on_click(move |_, _: &mut Window, cx| {
                        this.update(cx, |bench, cx| act(bench, cx));
                    }),
            )
    };
    div()
        .flex()
        .flex_wrap()
        .gap(SPACE_1)
        .text_size(theme::ui_text(TEXT_2XS_H))
        .line_height(theme::ui_text(TEXT_LG))
        .text_color(t.text_tertiary)
        .child(words.into())
        .children(
            open.into_iter()
                .chain(actions)
                .enumerate()
                .map(|(at, action)| link(at, action)),
        )
        .into_any_element()
}

/// Brings the agent's chat forward and scrolls it to the turn, opening a closed chat again.
pub(crate) fn open_moment(bench: &mut Workbench, moment: Moment, cx: &mut Context<Workbench>) {
    let opening = bench.bring_agent(&moment.agent, cx);
    cx.spawn(async move |bench, cx| {
        let Some(agent) = opening.await else {
            return;
        };
        let (Some(feed), Some(around)) = (moment.feed, moment.turn) else {
            return;
        };
        let _ = bench.update(cx, |bench, _| {
            bench.ask_chat(ChatAsk::Around {
                agent,
                feed,
                around,
            })
        });
    })
    .detach();
}
