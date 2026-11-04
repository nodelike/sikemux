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

/// What the inbox holds for the agent to approve, over the composer: an action that leaves this
/// machine with its exact command, or a gate with the evidence it leads with.
pub(super) fn approval(
    workbench: &Workbench,
    agent: &str,
    item: &sikemux_wire::inbox::InboxItem,
    busy: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (target, bench) = (workbench.chat_target(agent), cx.weak_entity());
    let said = ui::text(item.what.clone(), TEXT_XS, FontWeight::SEMIBOLD)
        .flex_shrink_1()
        .min_w_0()
        .truncate()
        .text_color(t.text_primary);
    let row = div().flex().items_center().gap(SPACE_2H);
    let body = match &item.hold {
        Some(hold) => div()
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .child(
                row.child(ui::icon(Icon::Shield, 14.).flex_none().text_color(t.warn))
                    .child(said)
                    .child(div().flex_1())
                    .child(crate::approvals::ship_buttons(
                        &hold.id, busy, target, bench,
                    )),
            )
            .child(
                ui::mono(hold.what.clone(), TEXT_XS, cx)
                    .whitespace_normal()
                    .text_color(t.text_secondary),
            )
            .children(hold.why.clone().map(|why| {
                ui::text(why, TEXT_2XS, FontWeight::NORMAL).text_color(t.text_tertiary)
            })),
        None => row
            .children(item.evidence.as_ref().map(|evidence| {
                let face = crate::goals::thumb_face(evidence, false, cx);
                evidence_opens(evidence, face, cx)
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(SPACE_0)
                    .child(said)
                    .children(item.detail.clone().map(|detail| {
                        ui::text(detail, TEXT_2XS, FontWeight::NORMAL)
                            .truncate()
                            .text_color(t.text_tertiary)
                    })),
            )
            .children(crate::approvals::pass_button(item, busy, target, bench)),
    };
    div().p(SPACE_3).child(body).into_any_element()
}

/// A thumbnail that opens what it shows: a picture full size, a recording in its viewer.
fn evidence_opens(
    evidence: &sikemux_wire::slice::Evidence,
    face: Div,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let sikemux_wire::slice::Proof::Picture { path, seconds, .. } = &evidence.proof else {
        return face.into_any_element();
    };
    let (path, still) = (PathBuf::from(path), seconds.is_none());
    face.id(SharedString::from(format!(
        "approval-evidence-{}",
        evidence.id
    )))
    .cursor_pointer()
    .on_click(cx.listener(move |bench, _, window, cx| match still {
        true => bench.show_picture(path.clone(), window, cx),
        false => {
            bench.show_workbench(cx);
            bench.open_file(&path, cx);
        }
    }))
    .into_any_element()
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
