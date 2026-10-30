//! What agents ask the person in a chat: the questions on show, their answers, and the keys
//! that move through them.

use super::*;

use crate::theme::{
    SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_3XS, TEXT_SM, TEXT_XS, TEXT_XS_H,
};

/// A question an agent put to the person, as the core's `questions` call lists it.
#[derive(Clone)]
pub struct Question {
    pub asked: AgentQuestion,
    /// The account host whose agent asks, by its key; none for this host's own.
    pub(crate) host: Option<String>,
}

impl std::ops::Deref for Question {
    type Target = AgentQuestion;

    fn deref(&self) -> &AgentQuestion {
        &self.asked
    }
}

impl Chats {
    /// Keeps the questions the core listed: this host's, or with `host_read` those of the
    /// account host `host` in front.
    pub fn questions_read(&mut self, value: Value, host_read: bool, host: Option<String>) {
        match serde_json::from_value::<Vec<AgentQuestion>>(value) {
            Ok(asked) => {
                let questions = asked.into_iter().map(|asked| Question {
                    asked,
                    host: host.clone(),
                });
                if host_read {
                    self.host_questions = questions.collect();
                } else {
                    self.questions = questions.collect();
                }
            }
            Err(error) => eprintln!("sikemux: the core's questions do not read: {error}"),
        }
        self.questions.retain(|question| question.host.is_none());
        self.questions.extend(self.host_questions.iter().cloned());
    }
}

impl Question {
    /// Who asked and what, while `agent_id` waits on the person's answer.
    pub(crate) fn open_from(&self, agent_id: &str) -> Option<(&str, &str)> {
        self.of(agent_id, QuestionState::Open)
            .then_some((&self.name, &self.text))
    }

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
            None if count > 1 => format!("Question {}", ix + 1),
            None => "Question".into(),
        }
    }
}

pub(super) enum Answer {
    Choice(usize),
    Choices(Vec<usize>),
    Text(String),
}

/// An answer picked for an open question, and what the dock shows was picked; empty for a secret.
pub(super) struct Pick {
    pub(super) answer: Answer,
    pub(super) shown: String,
}

/// Hands a key's action to the question on show; one it does not take goes on to the draft.
/// The keys the open question on show takes: its choices by number and the arrows, and with
/// `all`, space to tick and return to pick, which only the chat itself takes.
pub fn question_keys<E: InteractiveElement + FluentBuilder>(
    keyed: E,
    agent: &str,
    all: bool,
    bench: &WeakEntity<Workbench>,
) -> E {
    keyed
        .key_context("QuestionDock")
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
        word: "answered".into(),
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
            true => "Kept private".to_owned(),
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
                    (_, true) => "ASSUMED",
                    (1, false) => "QUESTION",
                    _ => "QUESTIONS",
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

pub(super) fn asking(
    agent: &str,
    chat: &Chat,
    open: &[&Question],
    current: &Question,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let count = open.len();
    let edge = Hsla {
        a: 0.06,
        ..t.text_primary
    };
    let tabs: Vec<_> = open
        .iter()
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
                .on_click(cx.listener(move |bench, _, _, cx| {
                    bench.answer(
                        &agent,
                        |chat, _| {
                            chat.show_question(&id);
                            None
                        },
                        cx,
                    );
                    cx.notify();
                }))
                .child(mark)
                .child(div().min_w_0().truncate().child(question.title(ix, count)))
        })
        .collect();
    let ready = open
        .iter()
        .all(|question| chat.picks.contains_key(&question.id));
    let sending = chat.sending_answers();
    let send = (count > 1 || current.multi).then(|| {
        let agent = agent.to_owned();
        BaseButton::new("questions-send")
            .disabled(!ready || sending)
            .h(px(22.))
            .px(SPACE_2)
            .flex()
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
                bench.answer(&agent, |chat, open| chat.send_answers(open), cx);
                cx.notify();
            }))
            .child("Send")
            .child(
                div()
                    .pl(SPACE_1H)
                    .border_l_1()
                    .border_color(t.line)
                    .text_size(crate::theme::ui_text(9.5))
                    .text_color(t.text_disabled)
                    .child("⌘↵"),
            )
    });
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
                let ask = Ask::Call {
                    method: "answerQuestion",
                    params: json!({ "id": id, "fine": true }),
                };
                bench.ask_answering(&agent, ask, cx);
            }))
            .child("Fine")
    });
    let fold = {
        let agent = agent.to_owned();
        let hover = t.surface_hover;
        BaseButton::new("questions-fold")
            .accessibility_label("Fold the questions away")
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
        asked.push_str(" Pick any.");
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
        let lead = format!("{} went on assuming ", current.name);
        let said = format!("{lead}{assumption}");
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
                        ui::text("recommended", TEXT_2XS, FontWeight::NORMAL)
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
                bench.answer(&agent, |chat, open| chat.choose(&id, ix, open), cx);
                cx.notify();
            }))
            .into_any_element()
        })
        .collect();
    if let Some(Pick {
        answer: Answer::Text(_),
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
pub(super) fn radio(on: bool, t: &Tokens) -> AnyElement {
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

/// An answer to a question as one text: what was typed, the files attached to it, then any context.
pub(super) fn answer_text(text: &str, paths: &[String], context: &[ChatContext]) -> String {
    let attached = (!paths.is_empty()).then(|| {
        let listed: Vec<String> = paths.iter().map(|path| format!("- {path}")).collect();
        format!("Attached:\n{}", listed.join("\n"))
    });
    let context = context
        .iter()
        .map(|item| format!("{}:\n{}", item.title, item.text));
    std::iter::once(text.to_owned())
        .chain(attached)
        .chain(context)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A question as it reads once answered: who asked, when not the chat's own agent, its words,
/// and the answer, or only that it was answered when the answer is a secret.
pub(super) fn answered_question(
    asker: Option<(&str, &str)>,
    question: &str,
    answer: &str,
    secret: bool,
    t: &Tokens,
) -> gpui_kit::Div {
    let bold = HighlightStyle {
        color: Some(t.text_primary),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    let asked = match asker {
        Some((cli, name)) => div()
            .flex()
            .items_center()
            .gap(px(7.))
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(t.text_tertiary)
            .child(ui::mark(cli, 14.))
            .child(
                StyledText::new(format!("{name} asked you"))
                    .with_highlights([(0..name.len(), bold)]),
            ),
        None => div()
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .text_color(t.text_disabled)
            .child("QUESTION"),
    };
    let (icon, answered) = if secret {
        (
            Icon::Shield,
            div().text_color(t.text_tertiary).child("Kept private"),
        )
    } else {
        (
            Icon::Check,
            div().text_color(t.text_primary).child(answer.to_owned()),
        )
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_3)
        .bg(t.surface_composer)
        .child(div().px(SPACE_3).pt(px(9.)).child(asked))
        .child(
            div()
                .px(SPACE_3)
                .pt(SPACE_1)
                .pb(SPACE_2H)
                .text_size(crate::theme::ui_text(13.5))
                .line_height(relative(1.5))
                .text_color(t.text_primary)
                .child(question.to_owned()),
        )
        .child(
            div()
                .px(SPACE_3)
                .py(SPACE_2)
                .flex()
                .items_start()
                .gap(SPACE_2)
                .border_t_1()
                .border_color(t.border)
                .text_size(crate::theme::ui_text(TEXT_SM))
                .line_height(relative(1.45))
                .child(
                    ui::icon(icon, 12.)
                        .flex_none()
                        .mt(px(3.))
                        .text_color(t.text_tertiary),
                )
                .child(answered.min_w_0()),
        )
}

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
    pub(super) fn pick(&mut self, id: &str, pick: Pick, open: &[&Question]) -> Option<Ask> {
        self.picks.insert(id.to_owned(), pick);
        self.move_on(id, open)
    }

    /// Moves on from a question with its answer: to the next still without one, or sends a lone question.
    pub(super) fn move_on(&mut self, id: &str, open: &[&Question]) -> Option<Ask> {
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
    pub fn press(&mut self, press: Press, open: &[&Question]) -> (bool, Option<Ask>) {
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
    pub fn choose(&mut self, id: &str, choice: usize, open: &[&Question]) -> Option<Ask> {
        let question = open.iter().find(|question| question.id == id)?;
        let label = |ix: &usize| {
            question
                .choices
                .get(*ix)
                .map_or("", |choice| choice.label.as_str())
        };
        if !question.multi {
            return self.pick(
                id,
                Pick {
                    answer: Answer::Choice(choice),
                    shown: label(&choice).to_owned(),
                },
                open,
            );
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
    pub fn send_answers(&mut self, open: &[&Question]) -> Option<Ask> {
        if open.is_empty()
            || !open
                .iter()
                .all(|question| self.picks.contains_key(&question.id))
        {
            return None;
        }
        let answers: Vec<Value> = open
            .iter()
            .filter_map(|question| {
                let id = &question.id;
                Some(match &self.picks.get(id)?.answer {
                    Answer::Choice(choice) => json!({ "id": id, "choice": choice }),
                    Answer::Choices(choices) => json!({ "id": id, "choices": choices }),
                    Answer::Text(answer) => json!({ "id": id, "answer": answer }),
                })
            })
            .collect();
        Some(Ask::Call {
            method: "answerQuestions",
            params: json!({ "answers": answers }),
        })
    }
}

impl Workbench {
    /// The chat in front while its question asks and nothing else holds the keys, as when the
    /// person clicked away from every field, so the question takes its keys from anywhere.
    pub fn asking_freely(&self, window: &Window) -> Option<String> {
        if !self.keys.focus.is_focused(window) {
            return None;
        }
        self.front_asks()
    }

    /// The chat in front, while it shows a question its keys answer.
    pub fn front_asks(&self) -> Option<String> {
        let agent = self.agent()?;
        let chat = self.chats.open.get(&agent.id)?;
        let open: Vec<&Question> = self
            .chats
            .questions
            .iter()
            .filter(|question| question.asks(&agent.id))
            .collect();
        let asking = chat.current(&open).is_some() && !chat.is_folded(&open);
        (asking && !chat.sending_answers()).then_some(agent.id)
    }

    /// Asks what a change to `agent`'s questions asks. Answers wait on the core, the questions
    /// they answer held as sending, then leave the open ones without waiting for its next list.
    pub fn ask_answering(&mut self, agent: &str, ask: Ask, cx: &mut Context<Self>) {
        let (method, params) = match ask {
            Ask::Call {
                method: method @ ("answerQuestion" | "answerQuestions"),
                params,
            } => (method, params),
            ask => return self.chat_target(agent).ask(ask),
        };
        let ids: Vec<String> = params["id"]
            .as_str()
            .into_iter()
            .chain(
                params["answers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|answer| answer["id"].as_str()),
            )
            .map(str::to_owned)
            .collect();
        if let Some(chat) = self.chats.open.get_mut(agent) {
            chat.answering.extend(ids.iter().cloned());
            if let Some(composing) = chat.composer.as_mut() {
                composing.refusal = None;
            }
        }
        self.parts.redraw_stage(cx);
        let answered = self.chat_target(agent).request(method, params);
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
