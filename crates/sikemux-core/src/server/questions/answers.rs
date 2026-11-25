use super::*;

/// The lead answered the question `id` its agent asked it, which settles it: the answer goes on
/// the record, with who decided and why. An answer the question hides goes on it only as answered
/// privately, without the lead's why, which may repeat it.
pub(crate) fn lead_answered(
    core: &Core,
    lead: &str,
    agent_id: &str,
    id: &str,
    answer: &str,
    why: Option<String>,
) {
    let Some(question) = core.harness().questions.take_answered(lead, agent_id, id) else {
        return;
    };
    let (asked, text, why) = match question.secret {
        true => (
            private_title(&question),
            format!(
                "@{} answered @{}'s question privately",
                core.names().name_or_id(lead),
                question.name
            ),
            None,
        ),
        false => (Some(question.asked.clone()), answer.to_owned(), why),
    };
    crate::server::board::goal::keep_settled(
        core,
        crate::server::board::goal::Settled {
            project: question.project,
            by: lead.to_owned(),
            holder: question.agent_id,
            asked,
            text,
            why,
        },
    );
}

/// What the record names a question whose answer is hidden by: its words, or else its header,
/// whichever may not hold a secret itself.
pub(super) fn private_title(question: &Question) -> Option<String> {
    (std::iter::once(&question.asked).chain(&question.header))
        .find(|title| !may_hold_secret(title))
        .cloned()
}

/// The open questions and the answers their askers have not read, for the
/// app, the notch and phones.
pub(crate) fn views(core: &Core) -> Vec<AgentQuestion> {
    let (open, answered) = {
        let state = core.harness().questions.state.locked();
        (state.open.clone(), state.answered.clone())
    };
    views_of(core, open, answered)
}

/// Each question's view, with the ticket of the card its asker holds.
pub(super) fn views_of(
    core: &Core,
    open: Vec<Question>,
    answered: Vec<Answered>,
) -> Vec<AgentQuestion> {
    let open = (open.iter())
        .filter(|question| !(question.relayed.is_some() && question.relay_taken))
        .map(Question::view);
    let answered = answered.iter().map(|answered| {
        let mut view = answered.question.view();
        view.state = QuestionState::Answered;
        view.answer = answered.answer.clone();
        view
    });
    open.chain(answered)
        .map(|mut view| {
            view.ticket = crate::server::board::ticket_of_agent(core, &view.agent_id);
            view
        })
        .collect()
}

/// The person's answer: the first one wins, and reaches the asker as a
/// message.
pub(crate) async fn answer(core: &Arc<Core>, params: AnswerParams) -> Result<(), String> {
    answer_all(
        core,
        AnswersParams {
            answers: vec![params],
        },
    )
    .await
}

pub(crate) trait AnswerParamsExt: Sized {
    /// Writes the files and context into `answer`: what was typed, the files, then the context,
    /// so the agent reads the same text whichever client answered. The files stay, for a chat to
    /// take as it takes the composer's.
    fn written_out(&mut self);
}

impl AnswerParamsExt for AnswerParams {
    fn written_out(&mut self) {
        let context = std::mem::take(&mut self.context);
        if self.paths.is_empty() && context.is_empty() {
            return;
        }
        let attached = (!self.paths.is_empty()).then(|| attached(&self.paths));
        let context = (context.iter()).map(|item| match item.text.is_empty() {
            true => item.title.clone(),
            false => format!("{}:\n{}", item.title, item.text),
        });
        let parts = (self.answer.take().into_iter())
            .chain(attached)
            .chain(context)
            .filter(|part| !part.is_empty());
        self.answer = Some(parts.collect::<Vec<_>>().join("\n\n"));
    }
}

/// The person's answers to questions asked together. Each asker gets all of
/// its answers in one message, so the first does not start a turn before the
/// rest arrive.
pub(crate) async fn answer_all(core: &Arc<Core>, mut params: AnswersParams) -> Result<(), String> {
    attach_picked_pictures(core, &mut params.answers);
    params
        .answers
        .iter_mut()
        .for_each(AnswerParams::written_out);
    let taken = {
        let mut state = core.harness().questions.state.locked();
        let mut taken = Vec::with_capacity(params.answers.len());
        for given in params.answers {
            let question = state
                .open
                .iter()
                .find(|question| question.id == given.id)
                .ok_or("That question has been answered already")?;
            let text = answer_text(question, &given)?;
            let held = (state.open.iter())
                .filter(|open| question.holds.contains(&open.id))
                .map(|open| {
                    let given = AnswerParams {
                        id: open.id.clone(),
                        answer: Some(text.clone()),
                        paths: given.paths.clone(),
                        ..AnswerParams::default()
                    };
                    (open.clone(), given, text.clone())
                })
                .collect::<Vec<_>>();
            taken.push((question.clone(), given, text));
            taken.extend(held);
        }
        state
            .open
            .retain(|question| !taken.iter().any(|(taken, ..)| taken.id == question.id));
        for (question, given, text) in &taken {
            if let (None, None) = (&question.from_host, &question.dialog) {
                let text = match given.choice.is_some() {
                    true => without_attached(text, &given.paths),
                    false => text.clone(),
                };
                state.answered.push(Answered {
                    question: question.clone(),
                    answer: (!question.secret).then_some(text),
                });
            }
        }
        let over = state.answered.len().saturating_sub(MAX_ANSWERED);
        state.answered.drain(..over);
        core.harness().questions.changed(&mut state);
        taken
    };
    let reopen = |questions: Vec<Question>| {
        let mut state = core.harness().questions.state.locked();
        state.answered.retain(|answered| {
            !questions
                .iter()
                .any(|question| question.id == answered.question.id)
        });
        state.open.extend(questions);
        core.harness().questions.changed(&mut state);
    };
    // Each answer as the asker gets it, with what was done about it when that is not the answer itself.
    let mut letters: Vec<Letter> = Vec::new();
    let mut remaining = taken.into_iter();
    while let Some((question, given, text)) = remaining.next() {
        let reopen_all =
            |letters: Vec<Letter>,
             question: Question,
             remaining: std::vec::IntoIter<(Question, AnswerParams, String)>| {
                reopen(
                    letters
                        .into_iter()
                        .map(|letter| letter.question)
                        .chain(std::iter::once(question))
                        .chain(remaining.map(|(question, ..)| question))
                        .collect(),
                );
            };
        if let Some(host) = &question.from_host {
            let params = match serde_json::to_value(&given) {
                Ok(params) => params,
                Err(error) => {
                    reopen_all(letters, question, remaining);
                    return Err(error.to_string());
                }
            };
            let asked = match serde_json::to_value(&question) {
                Ok(asked) => asked,
                Err(error) => {
                    reopen_all(letters, question, remaining);
                    return Err(error.to_string());
                }
            };
            crate::server::hosts::route::relay_answer(core, host, params, asked);
            continue;
        }
        if let Some(dialog) = &question.dialog {
            if let Some(chat) = core.chats().get(&question.agent_id) {
                let _ = chat.send(ChatCommand::AnswerDialog {
                    id: dialog.clone(),
                    answer: text,
                    choice: given.choice,
                });
            }
            continue;
        }
        let typed = (given.answer.clone())
            .map(|typed| without_attached(&typed, &given.paths))
            .filter(|typed| !typed.trim().is_empty());
        let told = match (&question.approval, question.secret, typed) {
            (Some(approval), ..) => crate::server::approvals::settle(core, approval, given.choice)
                .await
                .map(|done| (text, Some(done), false)),
            (None, true, Some(typed)) => kept_privately(core, &question, typed).await.map(|note| {
                match picked_labels(&question, &given) {
                    Some(picked) => (format!("{picked}\n\n{note}"), None, true),
                    None => (note, None, true),
                }
            }),
            (None, ..) => Ok((text, None, false)),
        };
        match told {
            Ok((text, done, private)) => letters.push(Letter {
                question,
                text,
                done,
                paths: given.paths,
                private,
            }),
            Err(error) => {
                reopen_all(letters, question, remaining);
                return Err(error);
            }
        }
    }
    let mut askers: Vec<&str> = letters
        .iter()
        .map(|letter| letter.question.agent_id.as_str())
        .collect();
    askers.sort_unstable();
    askers.dedup();
    let mut failed = None;
    for asker in askers {
        let own: Vec<&Letter> = letters
            .iter()
            .filter(|letter| letter.question.agent_id == asker)
            .collect();
        let sent = own
            .iter()
            .map(|letter| {
                let Letter {
                    question,
                    text,
                    done,
                    paths,
                    private,
                } = letter;
                let note = done.clone().unwrap_or_else(|| match &question.about {
                    Some(about) if !question.holds.is_empty() => format!(
                        "The person answered the question you asked; @{} has the answer to its own question too. Go on with it.",
                        about.name
                    ),
                    _ => "The person answered the question you asked; go on with it.".to_owned(),
                });
                (
                    answered(question, &question.asked, *private),
                    text.clone(),
                    note,
                    paths.clone(),
                )
            })
            .collect();
        match messages::answer(core, asker.to_owned(), sent).await {
            Ok(delivered)
                if delivered.get("delivery").and_then(Value::as_str) == Some("waiting") => {}
            Ok(_) => core.harness().questions.delivered(asker),
            Err(error) => {
                reopen(
                    own.into_iter()
                        .map(|letter| letter.question.clone())
                        .collect(),
                );
                failed = Some(error);
                continue;
            }
        }
        for letter in own.into_iter().filter(|letter| !letter.private) {
            copy_to_lead(core, &letter.question, &letter.text).await;
        }
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// An answer as its asker is told it, and whether what the person typed went to a private file.
struct Letter {
    question: Question,
    text: String,
    done: Option<String>,
    paths: Vec<String>,
    private: bool,
}

/// The labels of the choices picked in `given`, when it picked any.
fn picked_labels(question: &Question, given: &AnswerParams) -> Option<String> {
    let labels: Vec<&str> = (given.choice.iter())
        .filter_map(|&choice| Some(question.choices.get(choice)?.label.as_str()))
        .collect();
    (!labels.is_empty()).then(|| labels.join(", "))
}

/// The person's answer, copied to the lead a question went to first.
async fn copy_to_lead(core: &Arc<Core>, question: &Question, text: &str) {
    if let Stage::Person {
        lead: Some((lead_id, _)),
        ..
    } = &question.stage
    {
        let note = format!(
            "The person answered {}'s question, which went to you first. {} has the answer too.",
            question.name, question.name
        );
        let _ = messages::answer(
            core,
            lead_id.clone(),
            vec![(
                answered(question, &question.text, false),
                text.to_owned(),
                note,
                Vec::new(),
            )],
        )
        .await;
    }
}

pub(super) fn answered(
    question: &Question,
    question_text: &str,
    private: bool,
) -> messages::Answered {
    messages::Answered {
        question: question_text.to_owned(),
        asker: question.name.clone(),
        asker_id: question.agent_id.clone(),
        cli: question.cli.clone(),
        secret: private,
    }
}

/// What the person's answer says: the choice they picked, with any words, or their words alone.
fn answer_text(question: &Question, params: &AnswerParams) -> Result<String, String> {
    match params.choice {
        Some(choice) => question
            .choices
            .get(choice)
            .map(|choice| with_picked(choice.label.clone(), params))
            .ok_or_else(|| "That choice is not one of the question's".into()),
        None => reply_text(params),
    }
}

/// A picked choice's label and, when its question showed pictures, the pictures picked.
fn with_picked(label: String, params: &AnswerParams) -> String {
    let note = (params.answer.as_deref())
        .map(|note| without_attached(note, &params.paths))
        .filter(|note| !note.is_empty());
    let parts = [
        Some(label),
        note,
        (!params.paths.is_empty()).then(|| attached(&params.paths)),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join("\n\n")
}

/// The words the person wrote as their answer.
fn reply_text(params: &AnswerParams) -> Result<String, String> {
    match params.answer.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() && text.len() <= MAX_ANSWER_BYTES => Ok(text.to_owned()),
        Some(text) if !text.is_empty() => Err(format!(
            "An answer holds at most {} KB",
            MAX_ANSWER_BYTES / 1024
        )),
        _ => Err("The answer is empty".into()),
    }
}
