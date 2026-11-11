//! What the core found wrong on the way to a host, drawn as a list of steps, and the output a
//! failed update or set-up left, drawn under its card.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, ClipboardItem, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, Styled,
};
use sikemux_view::host_stats::{DiagnosisStep, HostDiagnosis, ReachStep, StepState, StepWhy};

use super::model::Listed;
use crate::hosts::install_pane::Words;
use crate::theme::{Tokens, RADIUS_2, SPACE_1H, SPACE_2, SPACE_2H, TEXT_2XS, TEXT_SM};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// Whether the chain has a step that failed, which is when it is worth showing.
pub(super) fn failed(diagnosis: &HostDiagnosis) -> Option<&DiagnosisStep> {
    (diagnosis.steps.iter()).find(|step| step.state == StepState::Failed)
}

/// One line on the card under its title.
pub(super) fn summary(diagnosis: &HostDiagnosis, name: &str) -> String {
    let tried = match diagnosis.tries {
        0 => String::new(),
        1 => "Sikemux tried once. ".to_owned(),
        tries => format!("Sikemux tried {tries} times. "),
    };
    let failed = failed(diagnosis).map(|step| words(step, name).0);
    format!(
        "{tried}{}",
        failed.map_or(String::new(), |failed| format!("{failed}."))
    )
}

/// The steps, each with a tick, a cross or a dash, the failed one saying what may be behind it.
pub(super) fn steps(diagnosis: &HostDiagnosis, name: &str, t: &Tokens) -> AnyElement {
    div()
        .mt(SPACE_2H)
        .flex()
        .flex_col()
        .children(diagnosis.steps.iter().map(|step| row(step, name, t)))
        .into_any_element()
}

fn row(step: &DiagnosisStep, name: &str, t: &Tokens) -> AnyElement {
    let (said, hint) = words(step, name);
    let (glyph, tint, ground) = match step.state {
        StepState::Passed => ("✓", t.live, t.live.opacity(0.14)),
        StepState::Failed => ("✕", t.danger, t.danger.opacity(0.16)),
        _ => ("–", t.text_tertiary, gpui_kit::transparent_black()),
    };
    let unchecked = !matches!(step.state, StepState::Passed | StepState::Failed);
    div()
        .flex()
        .items_start()
        .gap(SPACE_2)
        .py(px(4.))
        .child(
            div()
                .mt(px(1.))
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(ground)
                .when(unchecked, |mark| {
                    mark.border_1().border_dashed().border_color(t.border)
                })
                .text_color(tint)
                .child(ui::text(glyph, TEXT_2XS, FontWeight::MEDIUM)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .text_size(crate::theme::ui_text(TEXT_SM))
                .child(
                    div()
                        .text_color(match step.state {
                            StepState::Passed => t.text_primary,
                            StepState::Failed => t.danger,
                            _ => t.text_tertiary,
                        })
                        .child(said),
                )
                .children(hint.map(|hint| div().text_color(t.text_tertiary).child(hint))),
        )
        .into_any_element()
}

/// What a step says, and for a failed one what may be behind it.
fn words(step: &DiagnosisStep, name: &str) -> (String, Option<String>) {
    let label = match step.step {
        ReachStep::RemoteAccess => "Remote access here".to_owned(),
        ReachStep::Online => "This computer online".to_owned(),
        ReachStep::Account => "Your account".to_owned(),
        ReachStep::Path => format!("A path to {name}"),
        ReachStep::Core => format!("{name}'s core"),
        ReachStep::Accepted => format!("{name} lets this computer in"),
        ReachStep::Unknown => "A step this version does not know".to_owned(),
    };
    match step.state {
        StepState::Passed => (
            match step.step {
                ReachStep::RemoteAccess => "Remote access is on here".to_owned(),
                ReachStep::Online => "This computer is online".to_owned(),
                ReachStep::Account => "Your account is connected".to_owned(),
                ReachStep::Path if step.relay == Some(true) => {
                    format!("A path to {name}, through a relay")
                }
                ReachStep::Path if step.relay == Some(false) => {
                    format!("A path to {name}, straight there")
                }
                ReachStep::Core => format!("{name}'s core answers"),
                _ => label,
            },
            None,
        ),
        StepState::Failed => failure(step.why, name, label),
        _ => (format!("{label}: not checked"), None),
    }
}

fn failure(why: Option<StepWhy>, name: &str, label: String) -> (String, Option<String>) {
    let (said, hint): (String, Option<&str>) = match why {
        Some(StepWhy::Off) => ("Remote access is off here".into(), None),
        Some(StepWhy::NoRelay) => (
            "This computer reaches no relay".into(),
            Some("It may be offline, or a firewall blocks it."),
        ),
        Some(StepWhy::SignedOut) => ("This computer is not signed in".into(), None),
        Some(StepWhy::RemovedFromAccount) => {
            ("This computer was taken off the account".into(), None)
        }
        Some(StepWhy::AccountOffline) => (
            "The account server is out of reach".into(),
            Some("Sikemux keeps trying."),
        ),
        Some(StepWhy::NoAddress) => (
            format!("No way to {name} is known"),
            Some("It has not been online since this computer last heard of it."),
        ),
        Some(StepWhy::NoAnswer) => (
            format!("Nothing answers on the way to {name}"),
            Some("It may be off, asleep or offline, or its core stopped."),
        ),
        Some(StepWhy::NotLinked) => (format!("{name} no longer lists this computer"), None),
        Some(StepWhy::Closed) => (format!("{name} closed the connection"), None),
        Some(StepWhy::Unknown) | None => (format!("{label} failed"), None),
    };
    (said, hint.map(str::to_owned))
}

/// What fixes the first failed step, as buttons, and the diagnosis to copy.
pub(super) fn fixes(
    bench: &Workbench,
    host: &Listed,
    diagnosis: &HostDiagnosis,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let mut fixes = Vec::new();
    let failed = failed(diagnosis).map(|step| (step.step, step.why));
    let primary = |id: &str, words: &str| {
        Button::new(
            SharedString::from(id.to_owned()),
            words.to_owned(),
            Variant::Primary,
        )
    };
    match failed {
        Some((ReachStep::RemoteAccess, _)) => {
            fixes.push(
                primary("diagnosis-remote", "Turn on remote access")
                    .on_click(cx.listener(|bench, _, _, _| {
                        let _ = bench.asks.send(crate::core::Ask::Remote(
                            crate::core::RemoteAsk::Enable(true),
                        ));
                    }))
                    .into_any_element(),
            )
        }
        Some((ReachStep::Account, Some(StepWhy::SignedOut | StepWhy::RemovedFromAccount))) => fixes
            .push(
                primary("diagnosis-sign-in", "Sign in")
                    .on_click(cx.listener(|bench, _, _, _| bench.sign_in()))
                    .into_any_element(),
            ),
        Some((ReachStep::Path | ReachStep::Core, _)) => {
            let alias = host.row.and_then(|row| row.ssh.clone());
            let offered = (alias.as_deref()).and_then(|alias| bench.set_up_offered(alias));
            if let Some((alias, set_up)) = alias.zip(offered) {
                fixes.push(
                    primary("diagnosis-set-up", set_up.menu_label())
                        .on_click(
                            cx.listener(move |bench, _, _, cx| {
                                bench.set_up_host(&alias, set_up, cx)
                            }),
                        )
                        .into_any_element(),
                );
            }
        }
        _ => {}
    }
    let copied = copy_text(diagnosis, &host.name);
    fixes.push(copy_button("diagnosis-copy", "Copy diagnosis", copied, cx));
    fixes
}

/// The chain as plain text, for a message or an issue.
fn copy_text(diagnosis: &HostDiagnosis, name: &str) -> String {
    let mut text = format!("Reaching {name}:\n");
    for step in &diagnosis.steps {
        let (said, hint) = words(step, name);
        let mark = match step.state {
            StepState::Passed => "ok",
            StepState::Failed => "FAILED",
            _ => "-",
        };
        text.push_str(&format!("  [{mark}] {said}"));
        if let Some(hint) = hint {
            text.push_str(&format!(" ({hint})"));
        }
        text.push('\n');
    }
    text
}

/// A button that copies `text` and says so.
pub(super) fn copy_button(
    id: &str,
    words: &str,
    text: String,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    Button::new(
        SharedString::from(id.to_owned()),
        words.to_owned(),
        Variant::Outline,
    )
    .on_click(cx.listener(move |bench, _, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        bench.notify(crate::toast::success("Copied"), cx);
    }))
    .into_any_element()
}

/// The last lines a failed tool wrote, as it wrote them.
pub(super) fn evidence(text: &str, t: &Tokens, cx: &mut Context<Workbench>) -> AnyElement {
    div()
        .id("diagnosis-evidence")
        .mt(SPACE_2H)
        .px(SPACE_2H)
        .py(SPACE_2)
        .rounded(RADIUS_2)
        .bg(t.surface_sunken)
        .border_1()
        .border_color(t.border)
        .gap(SPACE_1H)
        .child(ui::mono(text.to_owned(), TEXT_2XS, cx).text_color(t.text_secondary))
        .into_any_element()
}
