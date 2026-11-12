//! The cards a turn waits on the person with: a permission to grant and a secret to give.

use super::*;

use gpui_kit::component::input::{Copy, Cut, Textarea};
use gpui_kit::component::Sizable;

use crate::theme::{
    SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_3XS_H, TEXT_XS,
};

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
        answers.push((t!("Cancel").into(), None, false));
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
                t!("Agent needs permission before this tool can continue."),
                TEXT_2XS,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        );
    column().child(
        div()
            .w_full()
            .max_w(px(sikemux_browser::REPLY_COLUMN_WIDTH))
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

/// A gate the agent waits at for the person to pass, over the composer, with the evidence it
/// leads with.
pub(super) fn approval(
    workbench: &Workbench,
    agent: &str,
    item: &sikemux_view::inbox::InboxItem,
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
    let body = div()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .children(item.evidence.as_ref().map(|evidence| {
            let face = crate::ui::evidence::thumb_face(evidence, false, cx);
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
        .children(crate::approvals::pass_button(item, busy, target, bench));
    div().p(SPACE_3).child(body).into_any_element()
}

/// A thumbnail that opens what it shows: a picture full size, a recording in its viewer.
fn evidence_opens(
    evidence: &sikemux_view::slice::Evidence,
    face: Div,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let sikemux_view::slice::Proof::Picture { path, seconds, .. } = &evidence.proof else {
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
        .child(t!("Answer kept, sent with the rest"))
}

/// How much a hidden answer holds, so the person can see their typing and paste landed.
fn hidden_size(text: &str) -> String {
    let characters = text.chars().count();
    let size = match characters {
        1 => t!("1 character").to_owned(),
        _ => t!("{characters} characters", characters = characters),
    };
    match text.lines().count() {
        0 | 1 => size,
        lines => t!("{lines} lines · {size}", lines = lines, size = size),
    }
}

fn dots_for(text: &str) -> String {
    text.lines()
        .map(|line| "•".repeat(line.chars().count()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The field a secret answer is typed or pasted into, which keeps nothing once it is picked.
/// Only a single-line field can mask, and it drops a paste's line breaks, so this one draws its
/// text invisibly under a row of dots and refuses to copy it. Both are fixed-width, which keeps
/// the caret on the dots.
pub(super) fn secret(
    agent: &str,
    question: &Question,
    chat: &Chat,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Option<impl IntoElement> {
    let composer = chat.composer.as_ref()?;
    let (field, focused) = (composer.secret.clone(), composer.secret_focused);
    let held = field.read(cx).value();
    let size = (!held.is_empty()).then(|| hidden_size(&held));
    let dots = (!held.is_empty()).then(|| dots_for(&held));
    let text_size = crate::theme::ui_text(TEXT_XS);
    let agent = agent.to_owned();
    let variant = if held.is_empty() {
        Variant::Outline
    } else {
        Variant::Primary
    };
    let done = Button::new("secret-done", t!("Done"), variant)
        .disabled(held.is_empty() || chat.sending_answers())
        .on_click(cx.listener(move |bench, _, window, cx| bench.keep_secret(&agent, window, cx)));
    Some(
        div()
            .px(SPACE_3)
            .pt(SPACE_1)
            .pb(SPACE_2H)
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h(px(30.))
                            .px(SPACE_1)
                            .py(SPACE_1)
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .border_1()
                            .border_color(if focused { t.accent } else { t.line })
                            .rounded(theme::RADIUS_2)
                            .bg(t.surface_sunken)
                            .capture_action(|_: &Copy, _, cx| cx.stop_propagation())
                            .capture_action(|_: &Cut, _, cx| cx.stop_propagation())
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        Textarea::new(&field)
                                            .xsmall()
                                            .appearance(false)
                                            .font_family(theme::MONO)
                                            .text_size(text_size)
                                            .text_color(gpui_kit::transparent_black()),
                                    )
                                    .children(dots.map(|dots| {
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .px(SPACE_1)
                                            .overflow_hidden()
                                            .font_family(theme::MONO)
                                            .text_size(text_size)
                                            .text_color(t.text_primary)
                                            .child(dots)
                                    })),
                            )
                            .children(size.map(|size| {
                                div()
                                    .flex_none()
                                    .pr(SPACE_1)
                                    .text_size(crate::theme::ui_text(TEXT_2XS))
                                    .text_color(t.text_tertiary)
                                    .child(size)
                            })),
                    )
                    .child(done),
            )
            .children(question.secret_note.clone().map(|note| {
                div()
                    .flex()
                    .items_start()
                    .gap(SPACE_1H)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .line_height(relative(1.4))
                    .text_color(t.text_tertiary)
                    .child(
                        div()
                            .flex_none()
                            .pt(px(2.))
                            .child(ui::icon(Icon::Shield, 12.).text_color(t.text_tertiary)),
                    )
                    .child(div().flex_1().min_w_0().child(note))
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
                answer: Answer::Text {
                    text,
                    paths: Vec::new(),
                    context: Vec::new(),
                },
                shown: String::new(),
            },
            open,
        )
    }
}
