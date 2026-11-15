//! The background tasks that ended in one turn: one line with a dot for each, which opens onto a
//! quiet list of them.

use super::rows::{fold_toggle, tucked, Drawn};
use super::*;
use sikemux_view::chat::{EndedTask, EndedTasks};

use crate::theme::{SPACE_1, SPACE_2H, TEXT_3XS, TEXT_XS_H};

/// More dots than this would run past the line; the count already says how many.
const MOST_DOTS: usize = 24;

pub(super) fn tasks(
    row: &str,
    part: &str,
    tasks: &EndedTasks,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = &Tokens::get(cx).clone();
    let key = format!("tasks:{row}:{part}");
    let open = looks.reading.opened.contains(&key);
    let count: u32 = tasks.tasks.iter().map(|task| task.times).sum();
    let (mark, mark_ink) = match tasks.failed {
        0 => (Icon::CheckCircle, t.live),
        _ => (Icon::AlertCircle, t.danger),
    };
    let named = match count {
        1 => t!("1 background task").to_owned(),
        count => t!("{count} background tasks", count = count),
    };
    let dots = tasks
        .tasks
        .iter()
        .flat_map(|task| std::iter::repeat_n(state_ink(task, looks, t), task.times as usize))
        .take(MOST_DOTS)
        .map(|ink| div().size(px(6.)).flex_none().rounded_full().bg(ink));
    let says = vec![
        ui::icon(mark, 13.).text_color(mark_ink).into_any_element(),
        div()
            .flex_none()
            .text_color(looks.ink)
            .child(named)
            .into_any_element(),
        div()
            .flex()
            .flex_none()
            .gap(px(3.))
            .children(dots)
            .into_any_element(),
        div()
            .min_w_0()
            .truncate()
            .font_weight(FontWeight::NORMAL)
            .text_color(looks.quiet)
            .child(how_they_ended(tasks))
            .into_any_element(),
    ];
    let line = div()
        .flex()
        .child(fold_toggle(&key, open, says, looks).h(px(26.)));
    let shown = motion::follow(
        SharedString::from(format!("{key}:shown")),
        if open { 1. } else { 0. },
        if open { motion::UNFOLD } else { motion::FOLD_AWAY },
        window,
        cx,
    );
    let list = (shown > 0.).then(|| {
        let rows = tasks.tasks.iter().map(|task| ended(task, looks, t));
        let list = div()
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element();
        tucked(format!("{key}:list"), shown, !open, vec![list], window, cx)
    });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(line)
        .children(list)
        .into_any_element()
}

/// How many ended each way, as "5 done · 2 watches stopped".
fn how_they_ended(tasks: &EndedTasks) -> String {
    let mut said = Vec::new();
    if tasks.done > 0 {
        said.push(t!("{count} done", count = tasks.done));
    }
    if tasks.failed > 0 {
        said.push(t!("{count} failed", count = tasks.failed));
    }
    match (tasks.stopped, tasks.watches_stopped) {
        (0, _) => {}
        (1, 1) => said.push(t!("1 watch stopped").to_owned()),
        (stopped, watches) if stopped == watches => {
            said.push(t!("{count} watches stopped", count = stopped))
        }
        (stopped, _) => said.push(t!("{count} stopped", count = stopped)),
    }
    said.join(" · ")
}

fn state_ink(task: &EndedTask, looks: &Drawn, t: &Tokens) -> Hsla {
    match task.state {
        NoticeState::Completed => t.live.opacity(0.85),
        NoticeState::Failed => t.danger,
        NoticeState::Stopped => looks.faint,
    }
}

/// One task in the opened list: how it ended, its name in sentence case, and how many times it
/// ran when it ran more than once.
fn ended(task: &EndedTask, looks: &Drawn, t: &Tokens) -> AnyElement {
    let watch = task.task_type.as_deref() == Some("monitor");
    let (icon, icon_ink, ink, word) = match task.state {
        NoticeState::Completed => (Icon::Check, t.live, t.text_secondary, None),
        NoticeState::Failed => (Icon::Close, t.danger, t.text_secondary, Some((t!("failed"), t.danger))),
        NoticeState::Stopped => (
            if watch { Icon::Eye } else { Icon::Run },
            looks.faint,
            looks.quiet,
            Some((t!("stopped"), looks.quiet)),
        ),
    };
    let hover = looks.hover;
    div()
        .h(px(28.))
        .ml(px(-8.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(10.))
        .min_w_0()
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .text_color(ink)
        .hover(move |row| row.bg(hover))
        .child(ui::icon(icon, 13.).flex_none().text_color(icon_ink))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(SPACE_2H)
                .child(div().min_w_0().truncate().child(sentence_case(&task.name)))
                .children((task.times > 1).then(|| {
                    div()
                        .flex_none()
                        .px(px(4.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(looks.rule)
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_3XS))
                        .text_color(looks.quiet)
                        .child(format!("×{}", task.times))
                })),
        )
        .children(word.map(|(word, ink)| {
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_3XS))
                .text_color(ink)
                .child(word)
        }))
        .into_any_element()
}

fn sentence_case(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
