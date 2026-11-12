//! What agents ask the person in a chat: the questions on show, their answers, and the keys
//! that move through them.

use super::*;

use std::future::Future;
use std::pin::Pin;

use sikemux_core::server::call;
use sikemux_core::server::questions::AnswerParams;
use sikemux_core::server::questions::AnswersParams;

use crate::theme::{
    SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_3XS, TEXT_MD, TEXT_SM, TEXT_XS,
    TEXT_XS_H,
};

/// The person's answers on their way to the core, which the questions they answer wait on.
pub enum Answers {
    One(AnswerParams),
    All(AnswersParams),
}

impl Answers {
    fn ids(&self) -> Vec<String> {
        match self {
            Answers::One(answer) => vec![answer.id.clone()],
            Answers::All(all) => all.answers.iter().map(|answer| answer.id.clone()).collect(),
        }
    }

    fn send(self, target: &Target) -> Pin<Box<dyn Future<Output = Result<(), String>>>> {
        match self {
            Answers::One(answer) => Box::pin(target.request_typed::<call::AnswerQuestion>(answer)),
            Answers::All(all) => Box::pin(target.request_typed::<call::AnswerQuestions>(all)),
        }
    }
}

/// What a change to a chat's questions asks of the core: the person's answers, or another ask.
pub enum Answering {
    Answers(Answers),
    Ask(Ask),
}

impl From<Answers> for Answering {
    fn from(answers: Answers) -> Self {
        Answering::Answers(answers)
    }
}

impl From<Ask> for Answering {
    fn from(ask: Ask) -> Self {
        Answering::Ask(ask)
    }
}

/// A question an agent put to the person, as the core's `questions` call lists it.
#[derive(Clone)]
pub struct Question {
    pub asked: AgentQuestion,
}

impl std::ops::Deref for Question {
    type Target = AgentQuestion;

    fn deref(&self) -> &AgentQuestion {
        &self.asked
    }
}

impl Question {
    pub fn of(&self, agent: &str, state: QuestionState) -> bool {
        self.agent_id == agent && self.state == state
    }

    /// Whether the person can answer it in `agent`'s chat: it waits on them, or the agent went
    /// on assuming an answer they may still change.
    pub fn asks(&self, agent: &str) -> bool {
        self.of(agent, QuestionState::Open) || self.of(agent, QuestionState::Assumed)
    }

    pub(super) fn title(&self, ix: usize, count: usize) -> String {
        match &self.header {
            Some(header) => header.clone(),
            None if count > 1 => t!("Question {number}", number = ix + 1),
            None => t!("Question").into(),
        }
    }
}

pub(super) enum Answer {
    Choice(usize),
    Choices(Vec<usize>),
    Text {
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    },
}

/// An answer picked for an open question, and what the dock shows was picked; empty for a secret.
pub(super) struct Pick {
    pub(super) answer: Answer,
    pub(super) shown: String,
}

/// Hands a key's action to the question on show; one it does not take goes on to the draft.
/// The keys the open question on show takes: its choices by number and the arrows, and with
/// `all`, space to tick and return to pick, which only the chat itself takes. They are bound
/// under `QuestionDock`, which `keyed` must name in its key context.
pub fn question_keys<E: InteractiveElement + FluentBuilder>(
    keyed: E,
    agent: &str,
    all: bool,
    bench: &WeakEntity<Workbench>,
) -> E {
    keyed
        .on_action(pressed(
            agent,
            |choose: &Choose| Press::Choose(choose.0),
            bench,
        ))
        .on_action(pressed(agent, |_: &PreviousChoice| Press::Up, bench))
        .on_action(pressed(agent, |_: &NextChoice| Press::Down, bench))
        .on_action(pressed(agent, |_: &PreviousQuestion| Press::Left, bench))
        .on_action(pressed(agent, |_: &NextQuestion| Press::Right, bench))
        .when(all, |keyed| {
            keyed
                .on_action(pressed(agent, |_: &Tick| Press::Tick, bench))
                .on_action(pressed(agent, |_: &Confirm| Press::Pick, bench))
        })
}

/// Answers the agent has not read yet because it is busy, until the core hands them over.
pub(super) fn waiting_answers(
    agent: &str,
    answered: &[&Question],
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    if answered.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let label = live::Label {
        key: "answered".into(),
        word: t!("answered").into(),
        count: answered.len(),
        folds: false,
        folded: false,
        icon: div().into_any_element(),
        peek: Vec::new(),
        action: None,
    }
    .draw(agent, cx);
    let rows = answered.iter().map(|question| {
        let answer = match question.secret {
            true => t!("Kept private").to_owned(),
            false => question.answer.clone().unwrap_or_default(),
        };
        div()
            .px(px(11.))
            .pt(px(3.))
            .pb(SPACE_1H)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_tertiary)
                    .child(question.text.clone()),
            )
            .child(
                div()
                    .max_w(relative(0.45))
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_primary)
                    .child(answer),
            )
    });
    Some(
        div()
            .flex()
            .flex_col()
            .child(label)
            .children(rows)
            .into_any_element(),
    )
}

/// The questions the agent waits on, at the top of the live work over the composer: a tab
/// for each, what the one on show asks, and its choices.
pub(super) fn questions(
    agent: &str,
    chat: &Chat,
    open: &[&Question],
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let current = chat.current(open)?;
    let t = Tokens::get(cx).clone();
    let count = open.len();
    let body = if chat.is_folded(open) {
        let agent = agent.to_owned();
        let peek = open
            .iter()
            .enumerate()
            .map(|(ix, question)| question.title(ix, count))
            .collect::<Vec<_>>()
            .join(" · ");
        BaseButton::new("questions-unfold")
            .w_full()
            .px(px(11.))
            .py(SPACE_1H)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .text_color(t.text_disabled)
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.answer(
                    &agent,
                    |chat, open| {
                        chat.fold(open, false);
                        None
                    },
                    cx,
                );
                cx.notify();
            }))
            .child(
                match (
                    count,
                    open.iter().all(|question| question.assumption.is_some()),
                ) {
                    (_, true) => t!("ASSUMED"),
                    (1, false) => t!("QUESTION"),
                    _ => t!("QUESTIONS"),
                },
            )
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_color(t.text_tertiary)
                    .child(count.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .child(peek),
            )
            .child(ui::icon(Icon::Chevron, 10.).text_color(t.text_disabled))
            .into_any_element()
    } else {
        asking(agent, chat, open, current, &t, cx).into_any_element()
    };
    Some(body)
}

/// A tab for each open question, the one on show chosen; a click shows another.
fn question_tabs(
    agent: &str,
    chat: &Chat,
    open: &[&Question],
    current: &Question,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let count = open.len();
    let edge = Hsla {
        a: 0.06,
        ..t.text_primary
    };
    open.iter()
        .enumerate()
        .map(|(ix, question)| {
            let on = question.id == current.id;
            let (agent, id) = (agent.to_owned(), question.id.clone());
            let mark = if chat.picks.contains_key(&question.id) {
                ui::icon(Icon::Check, 10.)
                    .text_color(t.text_disabled)
                    .into_any_element()
            } else {
                div()
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(TEXT_3XS))
                    .text_color(t.text_disabled)
                    .child((ix + 1).to_string())
                    .into_any_element()
            };
            let hover = t.surface_hover;
            BaseButton::new(SharedString::from(format!("question-tab-{id}")))
                .h(px(24.))
                .px(px(9.))
                .min_w_0()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .rounded(theme::RADIUS_2)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .map(|tab| {
                    if on {
                        tab.bg(t.surface_chosen).text_color(t.text_primary)
                    } else {
                        tab.border_1()
                            .border_color(edge)
                            .text_color(t.text_secondary)
                    }
                })
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |bench, _, window, cx| {
                    bench.answer(
                        &agent,
                        |chat, _| {
                            chat.show_question(&id);
                            None
                        },
                        cx,
                    );
                    bench.focus_answer(&agent, window, cx);
                    cx.notify();
                }))
                .child(mark)
                .child(div().min_w_0().truncate().child(question.title(ix, count)))
                .into_any_element()
        })
        .collect()
}

/// Sends every open question's answer, once each has one.
fn send_button(
    agent: &str,
    ready: bool,
    sending: bool,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let edge = Hsla {
        a: 0.06,
        ..t.text_primary
    };
    let agent = agent.to_owned();
    BaseButton::new("questions-send")
        .disabled(!ready || sending)
        .h(px(22.))
        .px(SPACE_2)
        .flex()
        .flex_none()
        .items_center()
        .gap(SPACE_1H)
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .map(|send| {
            if ready {
                send.bg(t.surface_chosen).text_color(t.text_primary)
            } else {
                send.border_1()
                    .border_color(edge)
                    .text_color(t.text_disabled)
            }
        })
        .on_click(cx.listener(move |bench, _, _, cx| {
            bench.answer(
                &agent,
                |chat, open| chat.send_answers(open).map(Into::into),
                cx,
            );
            cx.notify();
        }))
        .child(t!("Send"))
        .child(
            div()
                .pl(SPACE_1H)
                .border_l_1()
                .border_color(t.line)
                .text_size(crate::theme::ui_text(9.5))
                .text_color(t.text_disabled)
                .child("⌘↵"),
        )
}

pub(super) fn asking(
    agent: &str,
    chat: &Chat,
    open: &[&Question],
    current: &Question,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let count = open.len();
    let tabs = question_tabs(agent, chat, open, current, t, cx);
    let ready = open
        .iter()
        .all(|question| chat.picks.contains_key(&question.id));
    let sending = chat.sending_answers();
    let send = (count > 1 || current.multi).then(|| send_button(agent, ready, sending, t, cx));
    let fine = current.assumption.is_some().then(|| {
        let (agent, id) = (agent.to_owned(), current.id.clone());
        BaseButton::new("question-fine")
            .disabled(sending)
            .h(px(22.))
            .px(SPACE_2)
            .flex()
            .items_center()
            .rounded(theme::RADIUS_2)
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .bg(t.surface_chosen)
            .text_color(t.text_primary)
            .on_click(cx.listener(move |bench, _, _, cx| {
                let fine = AnswerParams {
                    id: id.clone(),
                    fine: true,
                    ..AnswerParams::default()
                };
                bench.ask_answering(&agent, Answers::One(fine), cx);
            }))
            .child(t!("Fine"))
    });
    let fold = {
        let agent = agent.to_owned();
        let hover = t.surface_hover;
        BaseButton::new("questions-fold")
            .accessibility_label(t!("Fold the questions away"))
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS_2)
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.answer(
                    &agent,
                    |chat, open| {
                        chat.fold(open, true);
                        None
                    },
                    cx,
                );
                cx.notify();
            }))
            .child(
                ui::icon(Icon::Chevron, 10.)
                    .text_color(t.text_disabled)
                    .with_transformation(Transformation::rotate(Radians(FRAC_PI_2))),
            )
    };
    let head = div()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .px(SPACE_1H)
        .pt(SPACE_1H)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .gap(SPACE_1)
                .overflow_hidden()
                .children(tabs),
        )
        .children(fine)
        .children(send)
        .child(fold);
    let mut asked = current.text.clone();
    let plain = asked.len();
    if current.multi {
        asked.push(' ');
        asked.push_str(t!("Pick any."));
    }
    if let Some(via) = &current.via_note {
        asked.push_str(&format!(" · {via}"));
    }
    let faint = HighlightStyle {
        color: Some(t.text_disabled),
        ..Default::default()
    };
    let text = div()
        .px(SPACE_3)
        .pt(SPACE_2H)
        .pb(SPACE_1)
        .text_size(crate::theme::ui_text(13.5))
        .line_height(relative(1.5))
        .text_color(t.text_primary)
        .child(crate::ui::selectable::selectable(
            SharedString::from(format!("question-text-{}", current.id)),
            asked.clone(),
            [(plain..asked.len(), faint)],
        ));
    let assumed = current.assumption.as_ref().map(|assumption| {
        let lead = t!("{name} went on assuming", name = current.name);
        let said = format!("{lead} {assumption}");
        div()
            .px(SPACE_3)
            .pb(SPACE_1)
            .text_size(crate::theme::ui_text(TEXT_XS_H))
            .line_height(relative(1.5))
            .text_color(t.text_secondary)
            .child(crate::ui::selectable::selectable(
                SharedString::from(format!("question-assumed-{}", current.id)),
                said,
                [(0..lead.len(), faint)],
            ))
    });
    let picked = chat.picks.get(&current.id);
    let cursor = chat
        .cursor
        .as_ref()
        .filter(|(id, _)| *id == current.id)
        .map(|(_, at)| *at);
    let chosen = |ix: usize| match picked.map(|pick| &pick.answer) {
        Some(Answer::Choice(choice)) => *choice == ix,
        Some(Answer::Choices(choices)) => choices.contains(&ix),
        _ => false,
    };
    let mut choices: Vec<AnyElement> = current
        .choices
        .iter()
        .enumerate()
        .map(|(ix, choice)| {
            let (agent, id) = (agent.to_owned(), current.id.clone());
            let key = match current.multi {
                true => tick(chosen(ix), t),
                false => radio(chosen(ix), t),
            };
            let number = ui::kbd((ix + 1).to_string(), cx)
                .line_height(crate::theme::ui_text(19.))
                .into_any_element();
            let label = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(SPACE_2)
                .child(
                    ui::text(choice.label.clone(), TEXT_SM, FontWeight::MEDIUM)
                        .line_height(crate::theme::ui_text(19.)),
                )
                .when(choice.recommended, |label| {
                    label.child(
                        ui::text(t!("recommended"), TEXT_2XS, FontWeight::NORMAL)
                            .text_color(t.text_disabled),
                    )
                });
            let detail = choice.detail.clone().map(|detail| {
                div()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .line_height(relative(1.45))
                    .text_color(t.text_tertiary)
                    .child(detail)
            });
            answer_row(
                SharedString::from(format!("question-{id}-{ix}")),
                chosen(ix),
                cursor == Some(ix),
                key,
                div().child(label).children(detail),
                Some(number),
                t,
            )
            .disabled(chat.sending_answers())
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.answer(
                    &agent,
                    |chat, open| chat.choose(&id, ix, open).map(Into::into),
                    cx,
                );
                cx.notify();
            }))
            .into_any_element()
        })
        .collect();
    if let Some(Pick {
        answer: Answer::Text { .. },
        shown,
    }) = picked.filter(|_| !current.secret)
    {
        let key = ui::icon(Icon::Pencil, 11.).mt(SPACE_1).into_any_element();
        let typed = ui::text(shown.clone(), TEXT_SM, FontWeight::MEDIUM)
            .line_height(crate::theme::ui_text(19.));
        choices.push(
            answer_row(
                SharedString::from(format!("question-{}-typed", current.id)),
                true,
                false,
                key,
                typed,
                None,
                t,
            )
            .into_any_element(),
        );
    }
    let secret = match (current.secret, picked) {
        (false, _) => None,
        (true, Some(_)) => Some(kept(t).into_any_element()),
        (true, None) => secret(agent, current, chat, t, cx).map(IntoElement::into_any_element),
    };
    div()
        .flex()
        .flex_col()
        .child(head)
        .child(text)
        .children(assumed)
        .when(!choices.is_empty(), |body| {
            body.child(
                div()
                    .px(SPACE_3)
                    .pt(SPACE_1H)
                    .pb(SPACE_3)
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .children(choices),
            )
        })
        .children(secret)
}

pub(super) fn answer_row(
    id: SharedString,
    chosen: bool,
    cursor: bool,
    key: AnyElement,
    body: impl IntoElement,
    number: Option<AnyElement>,
    t: &Tokens,
) -> BaseButton {
    let hover = t.surface_hover;
    BaseButton::new(id)
        .w_full()
        .pt(px(7.))
        .pb(SPACE_2)
        .pl(SPACE_1)
        .pr(SPACE_2)
        .flex()
        .items_start()
        .rounded(theme::RADIUS_2)
        .when(chosen, |row| row.bg(t.surface_chosen))
        .when(!chosen && cursor, |row| row.bg(t.surface_hover))
        .text_color(if chosen {
            t.text_primary
        } else {
            t.text_secondary
        })
        .when(!chosen && !cursor, |row| {
            row.hover(move |style| style.bg(hover))
        })
        .child(
            div()
                .w(px(22.))
                .flex_none()
                .flex()
                .justify_center()
                .line_height(crate::theme::ui_text(19.))
                .text_color(t.text_disabled)
                .child(key),
        )
        .child(div().flex_1().min_w_0().flex().flex_col().child(body))
        .children(number.map(|number| div().flex_none().pl(SPACE_2).child(number)))
}

/// The ring of a choice on a question that takes one.
pub(crate) fn radio(on: bool, t: &Tokens) -> AnyElement {
    div()
        .mt(px(3.5))
        .size(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(match on {
            true => t.text_primary,
            false => Hsla {
                a: 0.3,
                ..t.text_primary
            },
        })
        .when(on, |ring| {
            ring.child(div().size(px(6.)).rounded_full().bg(t.text_primary))
        })
        .into_any_element()
}

/// The box of a choice on a question that takes several.
pub(super) fn tick(on: bool, t: &Tokens) -> AnyElement {
    let tick = div()
        .mt(px(3.5))
        .size(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.));
    match on {
        true => tick
            .bg(t.text_primary)
            .child(ui::icon(Icon::Check, 9.).text_color(t.surface_sunken)),
        false => tick.border_1().border_color(Hsla {
            a: 0.3,
            ..t.text_primary
        }),
    }
    .into_any_element()
}

/// One answer in a thread of answered questions: what was typed, or that it was kept private,
/// and the files sent with it.
pub(super) struct Answered<'a> {
    pub question: &'a str,
    pub answer: &'a str,
    pub secret: bool,
    pub files: Vec<AnyElement>,
}

/// The questions an agent asked and the person's answers, as the conversation they were: each
/// question in the asker's bubble at the left, its mark on the first, and the answer in the
/// person's bubble at the right. A teammate's questions say who asked above them.
pub(super) fn answered_questions(
    mark: AnyElement,
    asker: Option<&str>,
    answers: Vec<Answered>,
    looks: &Looks,
) -> gpui_kit::Div {
    let asked_by = asker.map(|name| {
        div()
            .pl(px(ASKER_MARK + 10.))
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(looks.quiet)
            .child(format!("@{name}"))
    });
    let mut mark = Some(mark);
    let pairs = answers.into_iter().map(|answered| {
        let face = match mark.take() {
            Some(mark) => div().mt(px(13.)).child(mark),
            None => div().w(px(ASKER_MARK)),
        };
        let question = div()
            .min_w_0()
            .px(px(14.))
            .py(SPACE_2H)
            .rounded(rows::BUBBLE_RADIUS)
            .rounded_bl(SPACE_1H)
            .border_1()
            .border_color(looks.line)
            .bg(looks.asked)
            .text_color(looks.ink)
            .child(answered.question.to_owned());
        let said = if answered.secret {
            Some(div().text_color(looks.quiet).child(t!("Kept private")))
        } else {
            (!answered.answer.is_empty()).then(|| {
                div()
                    .text_color(looks.ink)
                    .child(answered.answer.to_owned())
            })
        };
        let files = (!answered.files.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .justify_end()
                .items_end()
                .gap(SPACE_2)
                .children(answered.files)
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .child(
                div()
                    .max_w(relative(0.78))
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(face.flex_none())
                    .child(question),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(SPACE_1H)
                    .children(files)
                    .children(said.map(|said| {
                        div()
                            .max_w(relative(0.78))
                            .child(rows::bubble(looks).child(said))
                    })),
            )
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .text_size(crate::theme::ui_text(TEXT_MD))
        .line_height(relative(1.6))
        .children(asked_by)
        .children(pairs)
}

/// How wide the asker's mark is beside its first question.
pub(super) const ASKER_MARK: f32 = 16.;

/// The questions the agent put to the person, answered from its chat.
impl Chat {
    /// The core took the answers to `ids`, or says why not, leaving them picked for another go.
    pub(super) fn took_answers(&mut self, ids: &[String], error: Option<String>) {
        for id in ids {
            self.answering.remove(id);
        }
        if error.is_none() {
            self.picks.retain(|id, _| !ids.contains(id));
            if self.shown.as_ref().is_some_and(|shown| ids.contains(shown)) {
                self.shown = None;
            }
            self.cursor = None;
        }
        self.failed(error);
    }

    /// Answers on their way to the core keep the questions from being answered again.
    pub fn sending_answers(&self) -> bool {
        !self.answering.is_empty()
    }

    /// The open question on show: the one the person chose, else the first still unanswered.
    pub(super) fn current<'a>(&self, open: &[&'a Question]) -> Option<&'a Question> {
        let chosen = self
            .shown
            .as_ref()
            .and_then(|id| open.iter().find(|question| &question.id == id));
        chosen
            .or_else(|| {
                open.iter()
                    .find(|question| !self.picks.contains_key(&question.id))
            })
            .or(open.first())
            .copied()
    }

    pub(super) fn is_folded(&self, open: &[&Question]) -> bool {
        open.first()
            .is_some_and(|first| self.folded.as_ref() == Some(&first.id))
    }

    pub fn fold(&mut self, open: &[&Question], fold: bool) {
        self.folded = open.first().filter(|_| fold).map(|first| first.id.clone());
    }

    pub fn toggle_group(&mut self, key: &str) {
        if !self.groups_folded.remove(key) {
            self.groups_folded.insert(key.to_owned());
        }
    }

    pub fn show_question(&mut self, id: &str) {
        self.shown = Some(id.to_owned());
    }

    /// Holds the answer, then moves on to the next question still without one, or sends a lone question.
    pub(super) fn pick(&mut self, id: &str, pick: Pick, open: &[&Question]) -> Option<Answers> {
        self.picks.insert(id.to_owned(), pick);
        self.move_on(id, open)
    }

    /// Moves on from a question with its answer: to the next still without one, or sends a lone question.
    pub(super) fn move_on(&mut self, id: &str, open: &[&Question]) -> Option<Answers> {
        self.cursor = None;
        if open.len() == 1 {
            return self.send_answers(open);
        }
        let after = open.iter().skip_while(|question| question.id != id).skip(1);
        if let Some(next) = after
            .chain(open)
            .find(|question| !self.picks.contains_key(&question.id))
        {
            self.shown = Some(next.id.clone());
        }
        None
    }

    /// A key the question on show takes while nothing is written: a number picks, the
    /// arrows move, Space ticks, Enter picks, moves on or sends. Says whether it took it.
    pub fn press(&mut self, press: Press, open: &[&Question]) -> (bool, Option<Answers>) {
        let Some(current) = self
            .current(open)
            .filter(|_| !self.is_folded(open) && !self.sending_answers())
        else {
            return (false, None);
        };
        let (id, count, multi, cursor) = (
            current.id.clone(),
            current.choices.len(),
            current.multi,
            self.cursor_at(current),
        );
        let ready = open
            .iter()
            .all(|question| self.picks.contains_key(&question.id));
        match press {
            Press::Up | Press::Down if count > 0 => {
                let step = if press == Press::Up { count - 1 } else { 1 };
                self.cursor = Some((id, (cursor + step) % count));
                (true, None)
            }
            Press::Left | Press::Right if open.len() > 1 => {
                let at = open
                    .iter()
                    .position(|question| question.id == id)
                    .unwrap_or(0);
                let step = if press == Press::Left {
                    open.len() - 1
                } else {
                    1
                };
                self.shown = Some(open[(at + step) % open.len()].id.clone());
                self.cursor = None;
                (true, None)
            }
            Press::Tick if multi && count > 0 => (true, self.choose(&id, cursor, open)),
            Press::Pick if multi => match self.picks.contains_key(&id) {
                false if count > 0 => (true, self.choose(&id, cursor, open)),
                false => (false, None),
                true if ready => (true, self.send_answers(open)),
                true => (true, self.move_on(&id, open)),
            },
            Press::Pick => {
                let already = matches!(self.picks.get(&id).map(|pick| &pick.answer), Some(Answer::Choice(choice)) if *choice == cursor);
                if ready && (already || count == 0) {
                    (true, self.send_answers(open))
                } else if count > 0 {
                    (true, self.choose(&id, cursor, open))
                } else {
                    (false, None)
                }
            }
            Press::Choose(ix) if ix < count => {
                self.cursor = Some((id.clone(), ix));
                (true, self.choose(&id, ix, open))
            }
            _ => (false, None),
        }
    }

    /// Picks a choice, or on a question that takes several, ticks or unticks it.
    pub fn choose(&mut self, id: &str, choice: usize, open: &[&Question]) -> Option<Answers> {
        let question = open.iter().find(|question| question.id == id)?;
        let label = |ix: &usize| {
            question
                .choices
                .get(*ix)
                .map_or("", |choice| choice.label.as_str())
        };
        if !question.multi {
            let pick = Pick {
                answer: Answer::Choice(choice),
                shown: label(&choice).to_owned(),
            };
            return self.pick(id, pick, open);
        }
        let mut ticked = match self.picks.remove(id) {
            Some(Pick {
                answer: Answer::Choices(ticked),
                ..
            }) => ticked,
            _ => Vec::new(),
        };
        match ticked.iter().position(|held| *held == choice) {
            Some(at) => {
                ticked.remove(at);
            }
            None => {
                ticked.push(choice);
                ticked.sort_unstable();
            }
        }
        if !ticked.is_empty() {
            let shown = ticked.iter().map(label).collect::<Vec<_>>().join(", ");
            self.picks.insert(
                id.to_owned(),
                Pick {
                    answer: Answer::Choices(ticked),
                    shown,
                },
            );
        }
        None
    }

    /// Sends every open question's answer together, once each has one.
    pub fn send_answers(&mut self, open: &[&Question]) -> Option<Answers> {
        if open.is_empty()
            || !open
                .iter()
                .all(|question| self.picks.contains_key(&question.id))
        {
            return None;
        }
        let answers = open
            .iter()
            .filter_map(|question| {
                let id = question.id.clone();
                Some(match &self.picks.get(&id)?.answer {
                    Answer::Choice(choice) => AnswerParams {
                        id,
                        choice: Some(*choice),
                        ..AnswerParams::default()
                    },
                    Answer::Choices(choices) => AnswerParams {
                        id,
                        choices: Some(choices.clone()),
                        ..AnswerParams::default()
                    },
                    Answer::Text {
                        text,
                        paths,
                        context,
                    } => AnswerParams {
                        id,
                        answer: Some(text.clone()),
                        paths: paths.clone(),
                        context: context.clone(),
                        ..AnswerParams::default()
                    },
                })
            })
            .collect();
        Some(Answers::All(AnswersParams { answers }))
    }
}

impl Workbench {
    /// Gives each question in the front chat its own draft: what was typed for the question
    /// left is kept, and the one now on show gets back its unsent words or its typed answer.
    pub fn follow_question_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agent().map(|agent| agent.id.clone()) else {
            return;
        };
        let open: Vec<&Question> = self
            .feeds
            .questions_outside_calls()
            .filter(|question| question.asks(&agent))
            .collect();
        let Some(chat) = self.chats.open.get_mut(&agent) else {
            return;
        };
        let now = chat
            .current(&open)
            .filter(|question| !question.secret && !chat.is_folded(&open))
            .map(|question| question.id.clone());
        if now == chat.drafted_for {
            return;
        }
        let was = std::mem::replace(&mut chat.drafted_for, now.clone());
        let (Some(was), Some(now)) = (was, now) else {
            return;
        };
        let Some(draft) = chat
            .composer
            .as_ref()
            .map(|composing| composing.draft.clone())
        else {
            return;
        };
        let typed = draft.read(cx).value().to_string();
        let answered = |chat: &Chat, id: &str| match chat.picks.get(id).map(|pick| &pick.answer) {
            Some(Answer::Text { text, .. }) => Some(text.clone()),
            _ => None,
        };
        if typed.trim().is_empty() || answered(chat, &was).as_deref() == Some(typed.as_str()) {
            chat.typed.remove(&was);
        } else {
            chat.typed.insert(was, typed.clone());
        }
        let shown = (chat.typed.remove(&now))
            .or_else(|| answered(chat, &now))
            .unwrap_or_default();
        if shown != typed {
            draft.update(cx, |draft, cx| draft.set_value(shown, window, cx));
            self.typed_draft(&agent, cx);
        }
    }
}

impl Workbench {
    /// Hands the keys back to `agent`'s draft after a click on a question, so typing answers
    /// it and ⌘↵ sends.
    fn focus_answer(&self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let draft = (self.chats.open.get(agent))
            .and_then(|chat| Some(chat.composer.as_ref()?.draft.clone()));
        if let Some(draft) = draft {
            draft.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// The chat in front while its question asks and nothing else holds the keys, as when the
    /// person clicked away from every field, so the question takes its keys from anywhere.
    pub fn asking_freely(&self, window: &Window, cx: &App) -> Option<String> {
        if !self.keys.focus.is_focused(window) {
            return None;
        }
        self.front_asks(cx)
    }

    /// The chat in front, while it shows a question its keys answer.
    pub fn front_asks(&self, cx: &App) -> Option<String> {
        let agent = self.agent()?;
        let chat = self.chats.open.get(&agent.id)?;
        let open: Vec<&Question> = self
            .feeds
            .questions_outside_calls()
            .filter(|question| question.asks(&agent.id))
            .collect();
        let asking = chat.current(&open).is_some() && !chat.is_folded(&open)
            || self
                .keyed_call(chat.target.place(), &agent.id, cx)
                .is_some();
        (asking && !chat.sending_answers()).then_some(agent.id)
    }

    /// Asks what a change to `agent`'s questions asks. Answers wait on the core, the questions
    /// they answer held as sending, then leave the open ones without waiting for its next list.
    pub fn ask_answering(
        &mut self,
        agent: &str,
        asking: impl Into<Answering>,
        cx: &mut Context<Self>,
    ) {
        let answers = match asking.into() {
            Answering::Answers(answers) => answers,
            Answering::Ask(ask) => return self.chat_target(agent).ask(ask),
        };
        let ids = answers.ids();
        if let Some(chat) = self.chats.open.get_mut(agent) {
            chat.answering.extend(ids.iter().cloned());
            if let Some(composing) = chat.composer.as_mut() {
                composing.refusal = None;
            }
        }
        self.parts.redraw_stage(cx);
        let answered = answers.send(&self.chat_target(agent));
        let agent = agent.to_owned();
        cx.spawn(async move |bench, cx| {
            let error = answered.await.err();
            bench
                .update(cx, |bench, cx| {
                    if error.is_none() {
                        bench.keep_questions(|question| {
                            question.state == QuestionState::Answered || !ids.contains(&question.id)
                        });
                    }
                    if let Some(chat) = bench.chats.open.get_mut(&agent) {
                        chat.took_answers(&ids, error);
                    }
                    bench.parts.redraw_stage(cx);
                })
                .ok();
        })
        .detach();
    }
}
