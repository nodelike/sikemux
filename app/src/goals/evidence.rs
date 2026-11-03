//! Evidence drawn large, as the gate waiting leads with it, or as a thumbnail in the strip
//! under it. Each kind of proof is drawn one way.

use chrono::TimeZone;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage,
};
use sikemux_wire::slice::{Evidence, Proof, Verdict};

use super::GoalsPane;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_SM,
    TEXT_XS,
};
use crate::ui;

const LARGE: f32 = 340.;
const THUMB: (f32, f32) = (92., 56.);

/// What the strip calls a piece of evidence.
pub(super) fn label(evidence: &Evidence) -> &'static str {
    match &evidence.proof {
        Proof::Picture { seconds: None, .. } => "Picture",
        Proof::Picture { .. } => "Recording",
        Proof::Diff { .. } => "Change",
        Proof::Numbers { .. } => "Numbers",
        Proof::Run { .. } => "Run",
        Proof::Timeline { .. } => "Timeline",
    }
}

pub(super) fn is_picture(evidence: &Evidence) -> bool {
    matches!(evidence.proof, Proof::Picture { .. })
}

/// How a check went, as a run's mark; none for evidence the core did not rule on.
pub(super) fn verdict_mark(verdict: Verdict, size: f32, cx: &gpui_kit::App) -> AnyElement {
    let outcome = match verdict {
        Verdict::Passed => "success",
        Verdict::Failed => "failure",
    };
    ui::status_of_run(outcome, size, cx)
}

/// The evidence in front, as large as the pane lets it be.
pub(super) fn large(evidence: &Evidence, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let frame = div()
        .id(SharedString::from(format!("evidence-{}", evidence.id)))
        .w_full()
        .rounded(theme::RADIUS_3)
        .border_1()
        .border_color(t.border)
        .overflow_hidden();
    match &evidence.proof {
        Proof::Picture {
            path,
            seconds,
            opened: page,
            ..
        } => {
            let (opened, still) = (path.clone(), seconds.is_none());
            let frame = frame
                .h(px(LARGE))
                .bg(t.surface_sunken)
                .cursor_pointer()
                .on_click(cx.listener(move |pane, _, window, cx| match still {
                    true => pane.show_picture(&opened, window, cx),
                    false => pane.open_file(&opened, cx),
                }));
            match seconds {
                None => div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .child(
                        frame.child(
                            img(std::path::PathBuf::from(path))
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        ),
                    )
                    .when_some(page.clone(), |column, page| {
                        column.child(ui::meta(
                            format!("Sikemux opened {page} after the check"),
                            cx,
                        ))
                    })
                    .into_any_element(),
                Some(seconds) => frame
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(SPACE_2)
                    .child(play(44., cx))
                    .child(ui::meta(
                        format!("{} recording · opens in the viewer", length(*seconds)),
                        cx,
                    ))
                    .into_any_element(),
            }
        }
        Proof::Diff {
            base,
            head,
            files,
            added,
            removed,
        } => frame
            .child(
                head_row(cx)
                    .child(ui::mono(format!("+{added}"), TEXT_XS, cx).text_color(t.git_added))
                    .child(ui::mono(format!("−{removed}"), TEXT_XS, cx).text_color(t.git_deleted))
                    .child(div().flex_1())
                    .child(ui::mono(
                        format!("{}..{}", short(base), short(head)),
                        TEXT_2XS,
                        cx,
                    )),
            )
            .children(files.iter().map(|file| {
                body_row()
                    .child(crate::file_icons::glyph(file, 14., cx))
                    .child(
                        ui::text(file.clone(), TEXT_XS, FontWeight::NORMAL)
                            .text_color(t.text_secondary),
                    )
            }))
            .into_any_element(),
        Proof::Numbers { measures } => frame
            .children(measures.iter().map(|measure| {
                body_row()
                    .child(
                        ui::text(measure.label.clone(), TEXT_SM, FontWeight::NORMAL)
                            .text_color(t.text_secondary),
                    )
                    .child(div().flex_1())
                    .when_some(measure.before.clone(), |row, before| {
                        row.child(ui::mono(before, TEXT_SM, cx))
                            .child(ui::meta("→", cx))
                    })
                    .child(ui::mono(measure.after.clone(), TEXT_SM, cx).text_color(t.text_primary))
            }))
            .into_any_element(),
        Proof::Run {
            command,
            sha,
            output,
        } => frame
            .child(
                head_row(cx)
                    .children(
                        evidence
                            .verdict
                            .map(|verdict| verdict_mark(verdict, 14., cx)),
                    )
                    .child(ui::mono(command.clone(), TEXT_XS, cx).text_color(t.text_primary))
                    .child(div().flex_1())
                    .children(sha.as_deref().map(|sha| ui::mono(short(sha), TEXT_2XS, cx))),
            )
            .child(
                div()
                    .px(SPACE_3)
                    .py(SPACE_2H)
                    .max_h(px(LARGE))
                    .overflow_hidden()
                    .bg(t.surface_sunken)
                    .font_family(theme::MONO)
                    .text_size(theme::ui_text(TEXT_2XS))
                    .text_color(t.text_secondary)
                    .child(output.clone()),
            )
            .into_any_element(),
        Proof::Timeline { moments } => frame
            .py(SPACE_1)
            .children(moments.iter().map(|moment| {
                div()
                    .px(SPACE_3)
                    .py(SPACE_0)
                    .flex()
                    .gap(SPACE_3)
                    .child(ui::mono(clock(moment.at), TEXT_2XS, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme::ui_text(TEXT_XS))
                            .text_color(t.text_secondary)
                            .child(moment.text.clone()),
                    )
            }))
            .into_any_element(),
    }
}

/// A piece of evidence in the strip, lifted while it is the one in front.
/// `serves` names the assertions the evidence was collected for, by number.
pub(super) fn thumb(
    evidence: &Evidence,
    chosen: bool,
    serves: Option<String>,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = evidence.id;
    let face = div()
        .w(px(THUMB.0))
        .h(px(THUMB.1))
        .flex()
        .items_center()
        .justify_center()
        .gap(SPACE_1)
        .rounded(theme::RADIUS_2)
        .overflow_hidden()
        .border_1()
        .border_color(if chosen { t.text_secondary } else { t.border })
        .bg(t.surface_sunken);
    let face = match &evidence.proof {
        Proof::Picture {
            path,
            seconds: None,
            ..
        } => face.child(
            img(std::path::PathBuf::from(path))
                .size_full()
                .object_fit(ObjectFit::Cover),
        ),
        Proof::Picture {
            seconds: Some(seconds),
            ..
        } => face
            .child(play(20., cx))
            .child(ui::mono(length(*seconds), TEXT_2XS, cx)),
        Proof::Diff { added, removed, .. } => face
            .child(ui::mono(format!("+{added}"), TEXT_2XS, cx).text_color(t.git_added))
            .child(ui::mono(format!("−{removed}"), TEXT_2XS, cx).text_color(t.git_deleted)),
        Proof::Numbers { measures } => face.children(measures.first().map(|measure| {
            ui::mono(measure.after.clone(), TEXT_2XS, cx).text_color(t.text_primary)
        })),
        Proof::Run { command, .. } => face.child(
            ui::mono(
                command
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                TEXT_2XS,
                cx,
            )
            .text_color(t.text_secondary),
        ),
        Proof::Timeline { moments } => face
            .child(ui::icon(Icon::LogLines, 14.).text_color(t.text_tertiary))
            .child(ui::mono(moments.len().to_string(), TEXT_2XS, cx)),
    };
    div()
        .id(SharedString::from(format!("thumb-{id}")))
        .relative()
        .flex()
        .flex_col()
        .items_center()
        .gap(SPACE_1)
        .cursor_pointer()
        .on_click(cx.listener(move |pane, _, _, cx| {
            pane.picked = Some(id);
            cx.notify();
        }))
        .child(face)
        .when_some(evidence.verdict, |thumb, verdict| {
            thumb.child(
                div()
                    .absolute()
                    .top(px(3.))
                    .right(px(3.))
                    .child(verdict_mark(verdict, 12., cx)),
            )
        })
        .child(
            ui::text(label(evidence), TEXT_2XS, FontWeight::NORMAL).text_color(if chosen {
                t.text_primary
            } else {
                t.text_tertiary
            }),
        )
        .children(serves.map(|serves| ui::meta(format!("for {serves}"), cx)))
        .into_any_element()
}

fn play(size: f32, cx: &gpui_kit::App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(t.surface_raised)
        .border_1()
        .border_color(t.border)
        .child(ui::icon(Icon::Run, size * 0.45).text_color(t.text_primary))
        .into_any_element()
}

fn head_row(cx: &gpui_kit::App) -> gpui_kit::Div {
    let t = Tokens::get(cx);
    div()
        .h(px(36.))
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .border_b_1()
        .border_color(t.border)
}

fn body_row() -> gpui_kit::Div {
    div()
        .h(px(30.))
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_1H)
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn length(seconds: f64) -> String {
    ui::time::elapsed(seconds.round() as u64)
}

/// A moment as the clock read then, here, to the second.
fn clock(at: u64) -> String {
    local(at, "%H:%M:%S")
}

/// A moment as the clock read then, here, to the minute.
pub(super) fn minute(at: u64) -> String {
    local(at, "%H:%M")
}

fn local(at: u64, format: &str) -> String {
    chrono::Local
        .timestamp_millis_opt(at as i64)
        .single()
        .map_or_else(String::new, |at| at.format(format).to_string())
}
