//! What the chat's project listens for, at the end of the line under the composer: the marks of
//! its timed and webhook triggers that are on, how many, and on hover each one.

use gpui_kit::base::Button as BaseButton;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled,
};
use sikemux_view::chat::{ChatTrigger, TriggerWhen};
use sikemux_view::playbook::RunsAlone;

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1, SPACE_1H, TEXT_SM, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

struct Heard {
    mark: Icon,
    title: String,
    when: String,
}

/// The item for the project `id` in `folder`, leaving out the chat's `own` triggers; none
/// while the project listens for nothing.
pub(super) fn item(
    bench: &Workbench,
    id: &str,
    folder: &str,
    own: &[ChatTrigger],
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    if !bench.playbooks.contains_key(folder) {
        let (this, folder) = (cx.entity(), folder.to_owned());
        cx.defer(move |cx| this.update(cx, |bench, cx| bench.want_playbook(&folder, cx)));
    }
    let playbook = bench.playbook_of(folder)?;
    let heard: Vec<Heard> = (playbook.automation.iter())
        .filter_map(|line| heard(line, own))
        .collect();
    if heard.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let mut marks: Vec<Icon> = Vec::new();
    for one in &heard {
        if !marks.contains(&one.mark) {
            marks.push(one.mark);
        }
    }
    let tint = move |mark: Icon, t: &Tokens| match mark {
        Icon::Timer => t.warn,
        _ => t.text_secondary,
    };
    let count = heard.len();
    let card = move |cx: &gpui_kit::App| {
        let t = Tokens::get(cx);
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .children(heard.iter().map(|one| {
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .min_w_0()
                    .child(
                        ui::icon(one.mark, 12.)
                            .flex_none()
                            .text_color(match one.mark {
                                Icon::Timer => t.warn,
                                _ => t.text_primary,
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(t.text_primary)
                            .child(one.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(t.text_tertiary)
                            .child(one.when.clone()),
                    )
            }))
            .child(
                div()
                    .mt(SPACE_1H)
                    .pt(SPACE_1H)
                    .border_t_1()
                    .border_color(t.border)
                    .text_color(t.text_tertiary)
                    .child(t!("Opens Automation")),
            )
            .into_any_element()
    };
    let open = {
        let (id, folder) = (id.to_owned(), folder.to_owned());
        cx.listener(move |bench, _, window, cx| bench.open_automation(&id, &folder, window, cx))
    };
    let item = BaseButton::new("composer-listening")
        .accessibility_label(t!("Listening for {count}", count = count))
        .flex_none()
        .h(px(24.))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded_full()
        .hover(move |style| style.bg(t.surface_hover))
        .tooltip(ui::card_tooltip(330., card))
        .on_click(open)
        .child(ui::text("·", TEXT_SM, FontWeight::NORMAL).text_color(t.border_strong))
        .child(
            div().flex().items_center().gap(px(3.)).children(
                marks
                    .iter()
                    .map(|mark| ui::icon(*mark, 11.).text_color(tint(*mark, &t))),
            ),
        )
        .child(
            ui::text(
                t!("listening for {count}", count = count),
                TEXT_XS,
                FontWeight::NORMAL,
            )
            .text_color(t.text_secondary),
        );
    Some(item.into_any_element())
}

/// A trigger the project listens for: timed or a webhook, on, and not the chat's own.
fn heard(line: &RunsAlone, own: &[ChatTrigger]) -> Option<Heard> {
    let RunsAlone::Trigger {
        id,
        title,
        when,
        next_at,
        paused: None,
        from_github,
        ..
    } = line
    else {
        return None;
    };
    if own.iter().any(|trigger| trigger.id == *id) {
        return None;
    }
    let (mark, when) = match when {
        TriggerWhen::Webhook { .. } => (
            if *from_github {
                Icon::Github
            } else {
                Icon::Bolt
            },
            t!("webhook").to_owned(),
        ),
        TriggerWhen::Once { .. } | TriggerWhen::Every { .. } | TriggerWhen::Daily { .. } => {
            (Icon::Timer, next_at.map(next).unwrap_or_default())
        }
        TriggerWhen::Ci { .. } | TriggerWhen::Mod { .. } => return None,
    };
    Some(Heard {
        mark,
        title: title.clone(),
        when,
    })
}

fn next(at: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(at as i64)
        .single()
        .map(|at| t!("next {at}", at = at.format("%-d %b %-H:%M").to_string()))
        .unwrap_or_default()
}
