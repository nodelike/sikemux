//! A thumbnail of a piece of evidence, drawn one way for each kind of proof.

use gpui_kit::{
    div, img, px, AnyElement, App, Div, IntoElement, ObjectFit, ParentElement, Styled, StyledImage,
};
use sikemux_view::slice::{Evidence, Proof};

use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1, TEXT_2XS};

const THUMB: (f32, f32) = (92., 56.);

pub(crate) fn thumb_face(evidence: &Evidence, chosen: bool, cx: &App) -> Div {
    let t = Tokens::get(cx).clone();
    let face = div()
        .w(px(THUMB.0))
        .h(px(THUMB.1))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(SPACE_1)
        .rounded(theme::radius_2())
        .overflow_hidden()
        .border_1()
        .border_color(if chosen { t.text_secondary } else { t.border })
        .bg(t.surface_sunken);
    match &evidence.proof {
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
        } => face.child(play(20., cx)).child(super::mono(
            super::time::elapsed(seconds.round() as u64),
            TEXT_2XS,
            cx,
        )),
        Proof::Diff { added, removed, .. } => face
            .child(super::mono(format!("+{added}"), TEXT_2XS, cx).text_color(t.git_added))
            .child(super::mono(format!("−{removed}"), TEXT_2XS, cx).text_color(t.git_deleted)),
        Proof::Numbers { measures } => face.children(measures.first().map(|measure| {
            super::mono(measure.after.clone(), TEXT_2XS, cx).text_color(t.text_primary)
        })),
        Proof::Run { command, .. } => face.child(
            super::mono(
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
            .child(super::icon(Icon::LogLines, 14.).text_color(t.text_tertiary))
            .child(super::mono(moments.len().to_string(), TEXT_2XS, cx)),
    }
}

fn play(size: f32, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(crate::theme::radius_round())
        .bg(t.surface_raised)
        .border_1()
        .border_color(t.border)
        .child(super::icon(Icon::Run, size * 0.45).text_color(t.text_primary))
        .into_any_element()
}
