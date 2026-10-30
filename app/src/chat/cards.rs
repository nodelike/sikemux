//! The cards a turn waits on the person with: a permission to grant and a secret to give.

use super::*;

use crate::theme::{SPACE_0, SPACE_1, SPACE_1H, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_3XS_H, TEXT_XS};

/// What stands where the composer would, in a chat that can only be read here.
pub(super) fn permission(
    agent: &str,
    ask: &PermissionAsk,
    busy: bool,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let mut answers: Vec<(String, Option<String>, bool)> = ask
        .options
        .iter()
        .map(|option| {
            (
                option.name.clone(),
                Some(option.option_id.clone()),
                !option.kind.is_reject(),
            )
        })
        .collect();
    if !ask.options.iter().any(|option| option.kind.is_reject()) {
        answers.push(("Cancel".into(), None, false));
    }
    let buttons: Vec<_> = answers
        .into_iter()
        .map(|(name, option, allow)| {
            let (agent, request) = (agent.to_owned(), ask.request_id.clone());
            let id = SharedString::from(format!(
                "permission-{request}-{}",
                option.as_deref().unwrap_or("")
            ));
            BaseButton::new(id)
                .h(px(28.))
                .px(SPACE_2H)
                .flex()
                .items_center()
                .border_1()
                .border_color(if allow {
                    Hsla { a: 0.45, ..t.warn }
                } else {
                    t.line
                })
                .rounded(theme::RADIUS_2)
                .bg(t.line_soft)
                .when(!busy, |button| {
                    button.hover(move |style| style.bg(t.surface_chosen))
                })
                .text_size(crate::theme::ui_text(TEXT_3XS_H))
                .text_color(if allow {
                    t.text_primary
                } else {
                    t.text_tertiary
                })
                .disabled(busy)
                .on_click(cx.listener(move |bench, _, _, cx| {
                    if let Some(chat) = bench.chats.open.get_mut(&agent) {
                        chat.replying = Some(request.clone());
                    }
                    bench.parts.redraw_stage(cx);
                    bench.ask_chat(ChatAsk::Permission {
                        agent: agent.clone(),
                        request: request.clone(),
                        option: option.clone(),
                    })
                }))
                .child(name)
        })
        .collect();
    let copy = div()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .child(
            ui::text(ask.tool_call.title.clone(), TEXT_XS, FontWeight::SEMIBOLD)
                .text_color(t.text_primary),
        )
        .child(
            ui::text(
                "Agent needs permission before this tool can continue.",
                TEXT_2XS,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        );
    column().child(
        div()
            .w_full()
            .max_w(px(756.))
            .mt(SPACE_3)
            .mb(SPACE_3)
            .p(SPACE_3)
            .border_1()
            .border_color(Hsla { a: 0.44, ..t.warn })
            .rounded(theme::RADIUS_3)
            .bg(Hsla { a: 0.07, ..t.warn })
            .child(
                div()
                    .flex()
                    .gap(px(9.))
                    .child(
                        ui::icon(Icon::ShieldBolt, 16.)
                            .flex_none()
                            .text_color(t.warn),
                    )
                    .child(copy),
            )
            .child(
                div()
                    .mt(px(11.))
                    .flex()
                    .justify_end()
                    .gap(SPACE_1H)
                    .children(buttons),
            ),
    )
}

pub(super) fn kept(t: &Tokens) -> impl IntoElement {
    div()
        .px(SPACE_3)
        .pt(SPACE_1)
        .pb(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary)
        .child(ui::icon(Icon::Shield, 12.).text_color(t.text_tertiary))
        .child("Answer kept, sent with the rest")
}

/// The masked field a secret answer is typed into, which keeps nothing once it is picked.
pub(super) fn secret(
    agent: &str,
    question: &Question,
    chat: &Chat,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Option<impl IntoElement> {
    let field = chat.composer.as_ref()?.secret.clone();
    let agent = agent.to_owned();
    let done = Button::new("secret-done", "Done", Variant::Outline)
        .disabled(chat.sending_answers())
        .on_click(cx.listener(move |bench, _, window, cx| bench.keep_secret(&agent, window, cx)));
    Some(
        div()
            .px(SPACE_3)
            .pt(SPACE_1)
            .pb(SPACE_2H)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(
                        div()
                            .flex_1()
                            .h(px(30.))
                            .px(SPACE_2H)
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(t.line)
                            .rounded(theme::RADIUS_2)
                            .bg(t.surface_sunken)
                            .text_size(crate::theme::ui_text(TEXT_XS))
                            .child(Input::new(&field).appearance(false).bordered(false)),
                    )
                    .child(done),
            )
            .children(question.secret_note.clone().map(|note| {
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_tertiary)
                    .child(ui::icon(Icon::Shield, 12.).text_color(t.text_tertiary))
                    .child(note)
            })),
    )
}

impl Chat {
    pub fn keep_secret(&mut self, text: String, open: &[&Question]) -> Option<Answers> {
        let question = self
            .current(open)
            .filter(|question| question.secret && !self.is_folded(open))?;
        let id = question.id.clone();
        self.pick(
            &id,
            Pick {
                answer: Answer::Text(text),
                shown: String::new(),
            },
            open,
        )
    }
}
